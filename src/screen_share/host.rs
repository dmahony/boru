//! Host-side screen-share session driver: invitation, negotiation, and the
//! capture → encode → transport loop, plus consent-gated remote input.
//!
//! This is the app-facing counterpart of [`super::protocol::ScreenShareProtocol`].
//! The app talks to the driver through [`HostCommand`]s and receives lifecycle
//! events on the shared session-events channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use super::{
    channels::{
        ControlChannel, ControlOut, MediaChannel, DEFAULT_CONTROL_QUEUE_CAPACITY,
        DEFAULT_MEDIA_QUEUE_CAPACITY,
    },
    codec::{CodecConfig, OpenH264Encoder, VideoEncoder},
    permissions::Capability,
    platform::{capture_dimensions, create_capture_source, CAPTURE_FPS},
    protocol::{self, ControlMessage, ScreenShareMessage, SCREEN_SHARE_PROTOCOL_VERSION},
    reconnect::{retry_reconnect, ReconnectPolicy},
    remote_input::{self, create_platform_backend, InputEvent, NormalizedPointer},
    session::{ScreenShareSessionId, SessionEvent, SessionManager, SessionState},
    transport::{read_unit, QuicScreenTransport, ReadUnit},
    ScreenShareError, SCREEN_SHARE_ALPN,
};
use iroh::endpoint::Endpoint;
use iroh::PublicKey;

/// Demo capture geometry: 640x360 @ 15fps keeps encode cost and bandwidth
/// modest on the test machines while still exercising the full pipeline.
pub const DEMO_WIDTH: u32 = 640;
/// Height of the demo capture source.
pub const DEMO_HEIGHT: u32 = 360;
/// Frame rate of the demo capture source.
pub const DEMO_FPS: u32 = 15;

/// Commands the app sends into the host driver task.
#[derive(Debug, Clone)]
pub enum HostCommand {
    /// Host grants control capabilities to the viewer (emits GrantControl).
    GrantControl(Vec<Capability>),
    /// Host revokes control without ending view-only sharing (emits RevokeControl).
    RevokeControl,
}

/// Run a full host session: dial the viewer, negotiate consent, then stream
/// capture frames and apply consent-gated remote input. Emits session events
/// on `events` and honours `stop`/`commands` from the app.
#[allow(clippy::too_many_arguments)]
pub async fn run_host_session(
    endpoint: Endpoint,
    peer: PublicKey,
    local_public: PublicKey,
    conversation_id: u64,
    events: mpsc::Sender<SessionEvent>,
    stop: Arc<AtomicBool>,
    commands: mpsc::Receiver<HostCommand>,
) {
    let session_id = ScreenShareSessionId::generate();
    let mut manager = SessionManager::default();
    manager.start_invitation(session_id, local_public, peer, conversation_id);
    run_host_session_inner(
        endpoint,
        peer,
        session_id,
        &mut manager,
        &events,
        &stop,
        commands,
    )
    .await;
    // Every silent exit path (transport error, capture failure, peer drop)
    // otherwise leaves the host UI stuck on the "Screen sharing active"
    // indicator and blocks starting the next share. Emit Ended so the app
    // resets host state; it is a no-op when an EndSession/Reject already
    // ended the session.
    if !matches!(manager.state(session_id), Some(SessionState::Ended) | None) {
        let _ = events.send(SessionEvent::Ended { session_id }).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_host_session_inner(
    endpoint: Endpoint,
    peer: PublicKey,
    session_id: ScreenShareSessionId,
    manager: &mut SessionManager,
    events: &mpsc::Sender<SessionEvent>,
    stop: &Arc<AtomicBool>,
    mut commands: mpsc::Receiver<HostCommand>,
) {
    // Select the capture source up front so the Hello advertises the ACTIVE
    // geometry: a real portal/PipeWire capture when available, otherwise the
    // synthetic test pattern (demo/CI path).
    let mut capture = create_capture_source(false).await;
    tracing::info!(backend = capture.backend_name(), "screen-share capture backend selected");
    let (capture_width, capture_height) = capture_dimensions(&capture);
    let capture_fps = CAPTURE_FPS;
    let Some(hello) = manager.hello(session_id, vec!["h264".to_string()], capture_width.min(u16::MAX as u32) as u16, capture_height.min(u16::MAX as u32) as u16, capture_fps as u16) else { return };
    let addr = endpoint
        .remote_info(peer)
        .await
        .map(|info| iroh::EndpointAddr::from_parts(info.id(), info.into_addrs().map(|a| a.into_addr())))
        .unwrap_or_else(|| iroh::EndpointAddr::new(peer));
    // Use the default transport config: the earlier single_path() experiment
    // (multipath disabled) broke the QUIC handshake entirely — the offer
    // never arrived on the viewer. The media black hole it was meant to fix
    // turned out to be runtime starvation of the connection driver (see
    // start_screen_share in the app), fixed by running the host session on
    // a dedicated thread + runtime.
    let connection = match endpoint.connect(addr, SCREEN_SHARE_ALPN).await {
        Ok(connection) => connection,
        Err(error) => {
            let _ = events.send(SessionEvent::Rejected { session_id, reason: error.to_string() }).await;
            return;
        }
    };
    let remote_addrs = endpoint
        .remote_info(peer)
        .await
        .map(|info| {
            info.addrs()
                .map(|addr| format!("{:?}/{:?}", addr.addr(), addr.usage()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let conn_paths: Vec<String> = connection
        .paths()
        .iter()
        .map(|path| {
            let s = path.stats();
            format!(
                "{}/selected={}/status={}/cwnd={}/cong_events={}/stream_tx={}/udp_tx={}/lost={}",
                path.remote_addr(),
                path.is_selected(),
                path.status(),
                s.cwnd,
                s.congestion_events,
                s.frame_tx.stream,
                s.udp_tx.bytes,
                s.lost_packets,
            )
        })
        .collect();
    tracing::info!(remote_addrs = ?remote_addrs, paths = ?conn_paths, "screen-share: host connected to viewer");
    let transport = match QuicScreenTransport::new(connection.clone(), *session_id.as_bytes()) {
        Ok(transport) => transport,
        Err(error) => {
            let _ = events.send(SessionEvent::Rejected { session_id, reason: error.to_string() }).await;
            return;
        }
    };
    // Separate logical channels (PDF Task 3.2): a reliable control channel and
    // a dedicated bounded media channel. Chat traffic lives on a different
    // QUIC connection (gossip), so it cannot block screen-share frames; these
    // channels bound the queues inside the screen-share connection so stale
    // frames can never accumulate without limit.
    let control = match ControlChannel::new(transport.clone(), DEFAULT_CONTROL_QUEUE_CAPACITY) {
        Ok(channel) => channel,
        Err(error) => {
            let _ = events.send(SessionEvent::Rejected { session_id, reason: error.to_string() }).await;
            return;
        }
    };
    let media = match MediaChannel::new(transport.clone(), DEFAULT_MEDIA_QUEUE_CAPACITY) {
        Ok(channel) => channel,
        Err(error) => {
            let _ = events.send(SessionEvent::Rejected { session_id, reason: error.to_string() }).await;
            return;
        }
    };
    if let Err(error) = control.send(ControlOut::Legacy(ControlMessage::Hello(hello.clone()))).await {
        let _ = events.send(SessionEvent::Rejected { session_id, reason: error.to_string() }).await;
        return;
    }
    // Negotiation: wait for the viewer's explicit Accept/Reject, honouring
    // host commands (grants can be issued once streaming starts).
    while !stop.load(Ordering::Relaxed) {
        let accepted = tokio::select! {
            r = connection.accept_bi() => match r {
                Ok((mut send, recv)) => match read_unit(recv).await {
                    Ok(ReadUnit::Control(message)) => {
                        let response = manager.apply_remote(peer, message, events);
                        if let Some(response) = response {
                            let _ = write_control_response(&mut send, &response).await;
                        }
                        manager.state(session_id) == Some(SessionState::Streaming)
                    }
                    Ok(ReadUnit::Media(_, _)) => false,
                    Ok(ReadUnit::ScreenShare(message)) => {
                        // Versioned negotiation/lifecycle messages are the
                        // canonical protocol set (BORU-SS-08); the legacy
                        // host loop does not consume them yet.
                        tracing::debug!(?message, "screen-share: host ignored versioned message");
                        false
                    }
                    Err(_) => return,
                },
                Err(_) => return,
            },
            cmd = commands.recv() => match cmd {
                Some(HostCommand::GrantControl(capabilities)) => {
                    if let Some(message) = manager.grant_control(session_id, capabilities, events) {
                        let _ = control.send(ControlOut::Legacy(message)).await;
                    }
                    false
                }
                Some(HostCommand::RevokeControl) => {
                    if let Some(message) = manager.revoke_control(session_id, events) {
                        let _ = control.send(ControlOut::Legacy(message)).await;
                    }
                    false
                }
                None => return,
            },
            _ = tokio::time::sleep(Duration::from_millis(250)) => false,
        };
        if accepted { break; }
        if !matches!(manager.state(session_id), Some(SessionState::AwaitingAcceptance) | Some(SessionState::Connecting) | Some(SessionState::Streaming)) {
            tracing::warn!(session = ?session_id, state = ?manager.state(session_id), "screen-share: host negotiation exited without streaming");
            return;
        }
    }
    tracing::info!(session = ?session_id, "screen-share: host entering streaming");
    if stop.load(Ordering::Relaxed) {
        let _ = control.send(ControlOut::Legacy(ControlMessage::EndSession { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id })).await;
        return;
    }
    if manager.state(session_id) != Some(SessionState::Streaming) { return; }
    // Streaming: capture → encode → send, apply consent-gated input, honour
    // host commands and stop. The codec is configured from the ACTIVE
    // capture's geometry (the encoder requires even dimensions; real portal
    // sources are typically even, but round down defensively).
    let (capture_width, capture_height) = capture_dimensions(&capture);
    let encode_width = capture_width & !1;
    let encode_height = capture_height & !1;
    if encode_width == 0 || encode_height == 0 { return; }
    let mut config = CodecConfig { width: encode_width, height: encode_height, target_fps: capture_fps, ..CodecConfig::default() };
    let Ok(mut encoder) = OpenH264Encoder::new(config) else { return };
    tracing::info!("screen-share: host initializing remote-input backend");
    let backend_started = std::time::Instant::now();
    let mut backend = create_platform_backend((capture_width, capture_height)).await;
    tracing::info!(elapsed_ms = backend_started.elapsed().as_millis() as u64, "screen-share: host remote-input backend ready");
    let mut interval = tokio::time::interval(Duration::from_micros(1_000_000 / capture_fps as u64));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut media_drops: u64 = 0;
    let mut keyframe_requests: u64 = 0;
    // Reconnect-aware streaming loop (PDF Task 3.3): on a transient media
    // failure (media/control channel failed, connection dropped, stream
    // read error) the session does NOT end — it transitions to Reconnecting,
    // re-establishes the media path with bounded retries, forces a fresh
    // keyframe, and resumes. The chat/friend session is unaffected because
    // chat rides a separate QUIC connection; only this media path reconnects.
    let mut connection = connection;
    let mut control = control;
    let mut media = media;
    'streaming: loop {
        if stop.load(Ordering::Relaxed) {
            backend.shutdown().await;
            let _ = control.send(ControlOut::Legacy(ControlMessage::EndSession { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id })).await;
            return;
        }
        if media.failed() || control.failed() {
            match reconnect_media(&endpoint, peer, session_id, manager, &events, stop.as_ref(), hello.clone()).await {
                Some((new_connection, new_control, new_media)) => {
                    connection = new_connection;
                    control = new_control;
                    media = new_media;
                    // Fresh keyframe after reconnection (PDF Task 3.3 / REC-1):
                    // the next encoded frame is independently decodable, so the
                    // viewer resynchronises without waiting for the periodic
                    // keyframe.
                    encoder.force_keyframe();
                    media_drops = 0;
                    continue 'streaming;
                }
                None => {
                    backend.shutdown().await;
                    let _ = control.send(ControlOut::Legacy(ControlMessage::EndSession { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id })).await;
                    return;
                }
            }
        }
        let mut need_reconnect = false;
        tokio::select! {
            r = connection.accept_bi() => {
                match r {
                    Ok((mut send, recv)) => match read_unit(recv).await {
                        Ok(ReadUnit::Control(ControlMessage::Input { version: _, session_id: sid, capability, nonce, code, x, y, pressed })) => {
                            // Every input must carry the current grant nonce.
                            let authorized = manager.permissions(sid).is_some_and(|permissions| {
                                remote_input::authorize_nonce(permissions, sid, peer, capability, nonce).is_ok()
                            });
                            if !authorized { continue 'streaming; }
                            match capability {
                                Capability::ControlPointer => {
                                    if let Some((px, py)) = remote_input::normalize_to_capture(NormalizedPointer { x, y }, (capture_width, capture_height)) {
                                        let _ = backend.apply(InputEvent { code, capability, token: None, x: px as f32, y: py as f32, pressed }).await;
                                    }
                                }
                                Capability::ControlKeyboard => {
                                    let _ = backend.apply(InputEvent { code, capability, token: None, x: 0.0, y: 0.0, pressed }).await;
                                }
                                _ => {}
                            }
                        }
                        Ok(ReadUnit::Control(message)) => {
                            let response = manager.apply_remote(peer, message, events);
                            if let Some(response) = response { let _ = write_control_response(&mut send, &response).await; }
                            if manager.state(session_id) == Some(SessionState::Ended) { return; }
                        }
                        Ok(ReadUnit::ScreenShare(message)) => match message {
                            // Keyframe requests travel on the reliable control
                            // channel (PDF Task 3.2); force the encoder so the
                            // next unit is independently decodable.
                            ScreenShareMessage::KeyframeRequest { session_id: sid, .. } if sid == session_id => {
                                keyframe_requests += 1;
                                encoder.force_keyframe();
                            }
                            ScreenShareMessage::Error { session_id: sid, message: peer_error, .. } if sid == session_id => {
                                tracing::warn!(error = %peer_error, "screen-share: host received peer error");
                            }
                            other => {
                                // Other versioned lifecycle messages are not
                                // consumed by the legacy host loop yet.
                                tracing::debug!(?other, "screen-share: host ignored versioned message");
                            }
                        },
                        Ok(ReadUnit::Media(_, _)) => {}
                        Err(_) => { need_reconnect = true; }
                    },
                    Err(_) => { need_reconnect = true; }
                }
            }
            cmd = commands.recv() => match cmd {
                Some(HostCommand::GrantControl(capabilities)) => {
                    if let Some(message) = manager.grant_control(session_id, capabilities, events) {
                        let _ = control.send(ControlOut::Legacy(message)).await;
                    }
                }
                Some(HostCommand::RevokeControl) => {
                    backend.shutdown().await;
                    if let Some(message) = manager.revoke_control(session_id, events) {
                        let _ = control.send(ControlOut::Legacy(message)).await;
                    }
                }
                None => return,
            },
            _ = interval.tick() => {
                match capture.capture() {
                    Ok(Some(frame)) => {
                        // Real portal captures negotiate their geometry after
                        // streaming starts; reconfigure the encoder when the
                        // frame size differs from the initial config.
                        if frame.width != config.width || frame.height != config.height {
                            if frame.width == 0 || frame.height == 0 || frame.width % 2 != 0 || frame.height % 2 != 0 {
                                tracing::warn!(width = frame.width, height = frame.height, "screen-share: capture produced invalid geometry, ending session");
                                return;
                            }
                            let new_config = CodecConfig { width: frame.width, height: frame.height, ..config };
                            if encoder.reconfigure(new_config).is_err() { return; }
                            config = new_config;
                        }
                        match encoder.encode(&frame) {
                            Ok(encoded) => {
                                if encoded.sequence == 0 || encoded.sequence % 25 == 0 {
                                    let stats_paths: Vec<String> = connection
                                        .paths()
                                        .iter()
                                        .map(|path| {
                                            let s = path.stats();
                                            format!(
                                                "{}/selected={}/cwnd={}/cong_events={}/stream_tx={}/udp_tx={}/lost={}",
                                                path.remote_addr(),
                                                path.is_selected(),
                                                s.cwnd,
                                                s.congestion_events,
                                                s.frame_tx.stream,
                                                s.udp_tx.bytes,
                                                s.lost_packets,
                                            )
                                        })
                                        .collect();
                                    if encoded.sequence == 0 {
                                        tracing::info!(bytes = encoded.bytes.len(), "screen-share: host encoded first frame");
                                    }
                                    tracing::info!(sequence = encoded.sequence, paths = ?stats_paths, "screen-share: host streaming path stats");
                                }
                                // Hand the encoded frame to the bounded media
                                // channel. send_frame never blocks the capture
                                // loop: when the queue is full the oldest stale
                                // frame is dropped (bounded memory + latest-frame
                                // strategy) and the drop is counted.
                                let sequence = encoded.sequence;
                                let bytes_len = encoded.bytes.len();
                                let dropped = media.send_frame(encoded).await;
                                if dropped {
                                    media_drops += 1;
                                }
                                if sequence == 0 || sequence % 150 == 0 {
                                    tracing::info!(sequence, bytes = bytes_len, media_drops, keyframe_requests, "screen-share: host frame queued on media channel");
                                }
                                if media_drops > 0 && media_drops % 150 == 0 {
                                    tracing::warn!(media_drops, "screen-share: host dropping stale media frames (queue full)");
                                }
                            }
                            Err(error) => {
                                tracing::warn!(error = %error, "screen-share: host encode failed");
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(error = %error, "screen-share: capture failed, ending session");
                        return;
                    }
                }
            }
        }
        if need_reconnect {
            match reconnect_media(&endpoint, peer, session_id, manager, &events, stop.as_ref(), hello.clone()).await {
                Some((new_connection, new_control, new_media)) => {
                    connection = new_connection;
                    control = new_control;
                    media = new_media;
                    // Fresh keyframe after reconnection so the viewer can
                    // resynchronise immediately.
                    encoder.force_keyframe();
                    media_drops = 0;
                }
                None => {
                    backend.shutdown().await;
                    let _ = control.send(ControlOut::Legacy(ControlMessage::EndSession { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id })).await;
                    return;
                }
            }
        }
    }
}

/// Re-establish the media path after a transient failure (PDF Task 3.3).
///
/// Transitions the session Streaming → Reconnecting (resetting remote-control
/// permissions to view-only — REC-2), re-dials the viewer, re-sends the Hello,
/// waits for a fresh Accept, then transitions back to Streaming. Returns the
/// new connection/control/media channels on success; `None` when the reconnect
/// was abandoned (session is ended by the caller).
#[allow(clippy::too_many_arguments)]
async fn reconnect_media(
    endpoint: &Endpoint,
    peer: PublicKey,
    session_id: ScreenShareSessionId,
    manager: &mut SessionManager,
    events: &mpsc::Sender<SessionEvent>,
    stop: &AtomicBool,
    hello: protocol::Hello,
) -> Option<(iroh::endpoint::Connection, ControlChannel, MediaChannel)> {
    if !manager.begin_reconnect(session_id, events) {
        return None;
    }
    let addr = endpoint
        .remote_info(peer)
        .await
        .map(|info| iroh::EndpointAddr::from_parts(info.id(), info.into_addrs().map(|a| a.into_addr())))
        .unwrap_or_else(|| iroh::EndpointAddr::new(peer));
    let policy = ReconnectPolicy::default();
    let result = retry_reconnect(&policy, stop, |_attempt| {
        let addr = addr.clone();
        let hello = hello.clone();
        async move {
            let new_connection = endpoint
                .connect(addr, SCREEN_SHARE_ALPN)
                .await
                .map_err(|e| ScreenShareError::new(e.to_string()))?;
            let new_transport = QuicScreenTransport::new(new_connection.clone(), *session_id.as_bytes())?;
            let new_control = ControlChannel::new(new_transport.clone(), DEFAULT_CONTROL_QUEUE_CAPACITY)?;
            let new_media = MediaChannel::new(new_transport.clone(), DEFAULT_MEDIA_QUEUE_CAPACITY)?;
            new_control
                .send(ControlOut::Legacy(ControlMessage::Hello(hello)))
                .await?;
            // Wait for the viewer's fresh Accept on the new connection. This
            // only observes the wire message; the manager transition happens
            // in complete_reconnect after the attempt succeeds, so the retry
            // closure does not capture the manager mutably.
            wait_for_accept(&new_connection, session_id, stop).await?;
            Ok((new_connection, new_control, new_media))
        }
    })
    .await;
    match result {
        Ok(channels) => {
            manager.complete_reconnect(session_id, events);
            Some(channels)
        }
        Err(_) => {
            manager.fail_reconnect(session_id, events);
            None
        }
    }
}

/// Wait for a fresh Accept on a re-established connection. Returns once an
/// Accept for `session_id` is read on a stream; the caller completes the
/// manager transition afterwards.
async fn wait_for_accept(
    connection: &iroh::endpoint::Connection,
    session_id: ScreenShareSessionId,
    stop: &AtomicBool,
) -> Result<(), ScreenShareError> {
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(ScreenShareError::new("reconnect stopped"));
        }
        let (mut send, recv) = connection.accept_bi().await.map_err(|e| ScreenShareError::new(e.to_string()))?;
        match read_unit(recv).await {
            Ok(ReadUnit::Control(ControlMessage::Accept { session_id: id, .. })) if id == session_id => {
                return Ok(());
            }
            Ok(ReadUnit::Control(_)) | Ok(ReadUnit::ScreenShare(_)) | Ok(ReadUnit::Media(_, _)) => {
                let _ = send.reset(0u32.into());
            }
            Err(error) => return Err(error),
        }
    }
}

/// Write one control response on an accepted stream (mirrors the protocol
/// handler's framing; used for protocol-level rejections during negotiation).
async fn write_control_response(send: &mut iroh::endpoint::SendStream, message: &ControlMessage) -> Result<(), ScreenShareError> {
    use tokio::io::AsyncWriteExt;
    let bytes = protocol::encode(message).map_err(|e| ScreenShareError::new(e.to_string()))?;
    send.write_u8(0x01).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
    send.write_u32(bytes.len() as u32).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
    send.write_all(&bytes).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
    send.finish().map_err(|e| ScreenShareError::new(e.to_string()))?;
    Ok(())
}
