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
    codec::{CodecConfig, OpenH264Encoder, VideoEncoder},
    permissions::Capability,
    platform::{capture_dimensions, create_capture_source, CAPTURE_FPS},
    protocol::{self, ControlMessage, SCREEN_SHARE_PROTOCOL_VERSION},
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
                .map(|addr| format!("{:?}/{:?}", addr.addr(), addr.usage))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    tracing::info!(remote_addrs = ?remote_addrs, "screen-share: host connected to viewer");
    let transport = match QuicScreenTransport::new(connection.clone(), *session_id.as_bytes()) {
        Ok(transport) => transport,
        Err(error) => {
            let _ = events.send(SessionEvent::Rejected { session_id, reason: error.to_string() }).await;
            return;
        }
    };
    if let Err(error) = transport.send_control(&ControlMessage::Hello(hello)).await {
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
                    Err(_) => return,
                },
                Err(_) => return,
            },
            cmd = commands.recv() => match cmd {
                Some(HostCommand::GrantControl(capabilities)) => {
                    if let Some(message) = manager.grant_control(session_id, capabilities, events) {
                        let _ = transport.send_control(&message).await;
                    }
                    false
                }
                Some(HostCommand::RevokeControl) => {
                    if let Some(message) = manager.revoke_control(session_id, events) {
                        let _ = transport.send_control(&message).await;
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
        let _ = transport.send_control(&ControlMessage::EndSession { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id }).await;
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
    loop {
        if stop.load(Ordering::Relaxed) {
            backend.shutdown().await;
            let _ = transport.send_control(&ControlMessage::EndSession { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id }).await;
            return;
        }
        tokio::select! {
            r = connection.accept_bi() => {
                match r {
                    Ok((mut send, recv)) => match read_unit(recv).await {
                        Ok(ReadUnit::Control(ControlMessage::Input { version: _, session_id: sid, capability, nonce, code, x, y, pressed })) => {
                            // Every input must carry the current grant nonce.
                            let authorized = manager.permissions(sid).is_some_and(|permissions| {
                                remote_input::authorize_nonce(permissions, sid, peer, capability, nonce).is_ok()
                            });
                            if !authorized { continue; }
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
                        Ok(ReadUnit::Media(_, _)) => {}
                        Err(_) => return,
                    },
                    Err(_) => return,
                }
            }
            cmd = commands.recv() => match cmd {
                Some(HostCommand::GrantControl(capabilities)) => {
                    if let Some(message) = manager.grant_control(session_id, capabilities, events) {
                        let _ = transport.send_control(&message).await;
                    }
                }
                Some(HostCommand::RevokeControl) => {
                    backend.shutdown().await;
                    if let Some(message) = manager.revoke_control(session_id, events) {
                        let _ = transport.send_control(&message).await;
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
                                if encoded.sequence == 0 {
                                    tracing::info!(bytes = encoded.bytes.len(), "screen-share: host encoded first frame");
                                }
                                let send_started = std::time::Instant::now();
                                let sent = transport.send_frame(&encoded).await;
                                let send_elapsed = send_started.elapsed();
                                if encoded.sequence == 0 || encoded.sequence % 150 == 0 {
                                    tracing::info!(sequence = encoded.sequence, bytes = encoded.bytes.len(), elapsed_ms = send_elapsed.as_millis() as u64, "screen-share: host frame sent");
                                }
                                if send_elapsed > Duration::from_secs(2) {
                                    tracing::warn!(sequence = encoded.sequence, elapsed_ms = send_elapsed.as_millis() as u64, "screen-share: send_frame took abnormally long");
                                }
                                if sent.is_err() { return; }
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
