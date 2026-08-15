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
    adaptation::{AdaptiveQuality, PacingController, QualityDecision, ViewerQualityRequest},
    capture::{CaptureConfig, CaptureSource, CaptureSourceId, DirtyRegion},
    channels::{
        ControlChannel, ControlOut, MediaChannel, DEFAULT_CONTROL_QUEUE_CAPACITY,
        DEFAULT_MEDIA_QUEUE_CAPACITY,
    },
    codec::{CodecConfig, OpenH264Encoder, VideoEncoder},
    permissions::{Capability, SlidingWindowRateLimiter},
    platform::{capture_dimensions, create_capture_source, CAPTURE_FPS, ActiveCapture},
    protocol::{self, ControlMessage, InputEventKind, RedactedText, ScreenShareMessage, SCREEN_SHARE_PROTOCOL_VERSION},
    reconnect::{retry_reconnect, ReconnectPolicy},
    remote_input::{self, create_platform_backend, InputEvent, NormalizedPointer, RemoteInput},
    session::{ScreenShareSessionId, SessionEvent, SessionManager, SessionState},
    stats::{ScreenShareSessionMetrics, ScreenShareStats},
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
    /// Host pushes its local text clipboard to the viewer (PDF Task 9.3).
    /// Requires the explicitly granted `Clipboard` capability; never implied
    /// by remote control. The payload is wrapped in [`RedactedText`] so a
    /// stray Debug/log of the command can never leak clipboard contents.
    SendClipboard(RedactedText),
    /// Host user switches the shared monitor without ending the chat session
    /// (PDF Phase 10). The host sends a `SourceChanged` message BEFORE any
    /// frame with the new geometry, re-selects the capture source, forces a
    /// keyframe, and surfaces `SessionEvent::SourceChanged` to the app.
    SwitchSource(CaptureSourceId),
}

/// Reason a host screen-share session terminated, for structured logging
/// (PDF Phase 12: capture stop must record the reason for termination).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTermination {
    /// The host user stopped sharing (stop flag or explicit EndSession).
    UserStopped,
    /// The viewer ended the session (peer EndSession).
    PeerEnded,
    /// The initial QUIC connection to the viewer failed.
    ConnectFailed,
    /// Negotiation ended without streaming (rejected, timed out, protocol error).
    NegotiationFailed,
    /// The media/control transport could not be established.
    TransportFailed,
    /// The capture source had no valid (even, non-zero) geometry.
    InvalidGeometry,
    /// The encoder could not be initialised with the negotiated config.
    EncodeInitFailed,
    /// The pacing controller could not be initialised.
    PacingInitFailed,
    /// Re-establishing the media path after a transient failure was abandoned.
    ReconnectFailed,
    /// An internal pipeline error (e.g. pacing queue returned no frame).
    PipelineError,
    /// The app's command channel closed.
    HostCommandClosed,
}

impl std::fmt::Display for SessionTermination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::UserStopped => "user_stopped",
            Self::PeerEnded => "peer_ended",
            Self::ConnectFailed => "connect_failed",
            Self::NegotiationFailed => "negotiation_failed",
            Self::TransportFailed => "transport_failed",
            Self::InvalidGeometry => "invalid_geometry",
            Self::EncodeInitFailed => "encode_init_failed",
            Self::PacingInitFailed => "pacing_init_failed",
            Self::ReconnectFailed => "reconnect_failed",
            Self::PipelineError => "pipeline_error",
            Self::HostCommandClosed => "host_command_closed",
        };
        f.write_str(s)
    }
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
    let started = std::time::Instant::now();
    let mut manager = SessionManager::default();
    manager.start_invitation(session_id, local_public, peer, conversation_id);
    let termination = run_host_session_inner(
        endpoint,
        peer,
        session_id,
        &mut manager,
        &events,
        &stop,
        commands,
    )
    .await;
    // PDF Phase 12: every host exit records the reason for termination plus
    // the session duration in one structured line (no media data).
    tracing::info!(
        reason = %termination,
        duration_ms = started.elapsed().as_millis() as u64,
        "screen-share: capture stopped"
    );
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
) -> SessionTermination {
    // Select the capture source up front so the Hello advertises the ACTIVE
    // geometry: a real portal/PipeWire capture when available, otherwise the
    // synthetic test pattern (demo/CI path).
    let mut capture = create_capture_source(false).await;
    tracing::info!(backend = capture.backend_name(), "screen-share capture backend selected");
    let capture_fps = CAPTURE_FPS;
    let capture_config = CaptureConfig { target_fps: capture_fps, ..CaptureConfig::default() };
    // PDF Phase 10: enumerate available monitors before starting the share
    // and select the initial source (primary/first monitor). Monitor-based
    // backends (X11, Windows) enumerate real sources via list_sources; the
    // portal exposes a single pseudo-source (Wayland selection happens in
    // the portal dialog); the test-pattern backend exposes its one source.
    let mut current_source: Option<CaptureSource> = None;
    match capture.list_sources() {
        Ok(sources) => {
            let initial = sources.first().cloned();
            current_source = initial.clone();
            if let Some(source) = &initial {
                if let Err(error) = capture.start(source.id, &capture_config) {
                    tracing::warn!(error = %error, "screen-share: initial source start failed; continuing with backend fallback");
                    current_source = None;
                }
            }
            let _ = events.send(SessionEvent::SourcesEnumerated { session_id, sources }).await;
        }
        Err(error) => {
            tracing::warn!(error = %error, "screen-share: source enumeration failed; continuing without a monitor list");
        }
    }
    let (capture_width, capture_height) = capture_dimensions(&capture);
    let Some(hello) = manager.hello(session_id, vec!["h264".to_string()], capture_width.min(u16::MAX as u32) as u16, capture_height.min(u16::MAX as u32) as u16, capture_fps as u16) else { return SessionTermination::NegotiationFailed };
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
            return SessionTermination::ConnectFailed;
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
            return SessionTermination::TransportFailed;
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
            return SessionTermination::TransportFailed;
        }
    };
    let media = match MediaChannel::new(transport.clone(), DEFAULT_MEDIA_QUEUE_CAPACITY) {
        Ok(channel) => channel,
        Err(error) => {
            let _ = events.send(SessionEvent::Rejected { session_id, reason: error.to_string() }).await;
            return SessionTermination::TransportFailed;
        }
    };
    if let Err(error) = control.send(ControlOut::Legacy(ControlMessage::Hello(hello.clone()))).await {
        let _ = events.send(SessionEvent::Rejected { session_id, reason: error.to_string() }).await;
        return SessionTermination::TransportFailed;
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
                        // host loop does not consume them during negotiation.
                        // Log only the variant discriminant — clipboard
                        // payloads must never be logged (PDF guardrail).
                        tracing::debug!(variant = ?std::mem::discriminant(&message), "screen-share: host ignored versioned message during negotiation");
                        false
                    }
                    Err(_) => return SessionTermination::NegotiationFailed,
                },
                Err(_) => return SessionTermination::ConnectFailed,
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
                // Clipboard sync (PDF Task 9.3) is only meaningful once the
                // media path is streaming; during negotiation the payload is
                // dropped (never logged — PDF guardrail).
                Some(HostCommand::SendClipboard(_)) => false,
                // Source selection (PDF Phase 10/13): the sharer picks the
                // monitor BEFORE the viewer accepts, so the offer that leads
                // to streaming starts with the chosen source. Re-select the
                // capture backend now; the encoder is configured from the
                // ACTIVE source's dimensions when streaming begins. No wire
                // message is needed pre-acceptance — the viewer has not
                // started decoding and will initialize from the first frame.
                Some(HostCommand::SwitchSource(source_id)) => {
                    if let Ok(sources) = capture.list_sources() {
                        if sources.iter().any(|source| source.id == source_id) {
                            let _ = capture.switch_source(source_id, &capture_config);
                            current_source =
                                capture.current_source().or_else(|| current_source.clone());
                            tracing::info!(source = ?source_id, "screen-share: initial source selected");
                        } else {
                            tracing::warn!(source = ?source_id, "screen-share: selected source not in current enumeration");
                        }
                    }
                    false
                }
                None => return SessionTermination::HostCommandClosed,
            },
            _ = tokio::time::sleep(Duration::from_millis(250)) => false,
        };
        if accepted { break; }
        if !matches!(manager.state(session_id), Some(SessionState::AwaitingAcceptance) | Some(SessionState::Connecting) | Some(SessionState::Streaming)) {
            tracing::warn!(session = ?session_id, state = ?manager.state(session_id), "screen-share: host negotiation exited without streaming");
            return SessionTermination::NegotiationFailed;
        }
    }
    tracing::info!(session = ?session_id, "screen-share: host entering streaming");
    if stop.load(Ordering::Relaxed) {
        let _ = control.send(ControlOut::Legacy(ControlMessage::EndSession { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id })).await;
        return SessionTermination::UserStopped;
    }
    if manager.state(session_id) != Some(SessionState::Streaming) { return SessionTermination::NegotiationFailed; }
    // Streaming: capture → encode → send, apply consent-gated input, honour
    // host commands and stop. The codec is configured from the ACTIVE
    // capture's geometry (the encoder requires even dimensions; real portal
    // sources are typically even, but round down defensively) plus the
    // capture-session encode knobs — bitrate, keyframe interval and quality
    // profile ride the same CaptureConfig the capture backend uses, so the
    // whole pipeline is configured from one place (PDF Task 7.1).
    let (capture_width, capture_height) = capture_dimensions(&capture);
    let encode_width = capture_width & !1;
    let encode_height = capture_height & !1;
    if encode_width == 0 || encode_height == 0 { return SessionTermination::InvalidGeometry; }
    let mut config = CodecConfig::from_capture_config(&capture_config, encode_width, encode_height);
    let Ok(mut encoder) = OpenH264Encoder::new(config) else { return SessionTermination::EncodeInitFailed };
    // PDF Phase 12: one structured capture-start line with the negotiated
    // codec, dimensions, bitrate, frame rate and backend. Contains no media
    // data (never screen contents or raw frame bytes).
    tracing::info!(
        event = "capture_start",
        backend = capture.backend_name(),
        codec = "h264",
        width = encode_width,
        height = encode_height,
        bitrate_bps = config.target_bitrate_bps,
        frame_rate = capture_fps,
        "screen-share: capture started"
    );
    // PDF Phase 10: monitor unplug / laptop dock-undock handling. When no
    // source remains the stream PAUSES (no frames sent) instead of ending
    // the session; a periodic re-enumeration resumes with the first
    // available source (dock-in).
    let mut stream_paused = false;
    let mut paused_check = std::time::Instant::now();
    // The geometry last announced via SourceChanged, so a spontaneous
    // platform renegotiation (monitor resize / portal format change) that
    // the encoder cannot fully adopt does not re-announce on every frame.
    let mut announced_geometry: Option<(u32, u32)> = None;
    // The remote-input backend is created LAZILY, only when the host
    // explicitly grants control (PDF Task 9.1 / T5.3: "Remote control must be
    // separately offered and explicitly accepted by the sharer"). A view-only
    // share therefore never opens a RemoteDesktop portal session, never pops
    // the portal consent dialog, and works even when remote-input permission
    // is denied or the portal is absent — `create_platform_backend` fails
    // closed to `UnavailableInputBackend` and input simply does nothing.
    let mut backend: Option<Box<dyn RemoteInput>> = None;
    // Pathological input streams are rate-limited (PDF Task 9.2): a sliding
    // window bounds input events per second so a buggy or malicious viewer
    // cannot flood the platform injection backend. Drops are counted for
    // diagnostics; the share itself stays live.
    let mut input_limiter = SlidingWindowRateLimiter::default();
    let mut input_rate_drops: u64 = 0;
    let mut interval = tokio::time::interval(Duration::from_micros(1_000_000 / capture_fps as u64));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut media_drops: u64 = 0;
    let mut keyframe_requests: u64 = 0;
    // Damage-aware capture (BORU-SS-32): ticks the host chose not to encode
    // + transmit because nothing changed — either the backend returned no
    // frame (X11 damage skip; portal queue momentarily empty) or it attached
    // DirtyRegion::Rects(empty). Counted so the static-screen reduction is
    // measurable in the metrics/logs (capture/encode fps and bytes/sec drop).
    let mut skipped_frames: u64 = 0;
    // Frame pacing (PDF Task 7.2): bounded latest-frame queue between capture
    // and encode. When the encoder or network falls behind, obsolete frames
    // are dropped instead of building latency; queue length is capped at the
    // codec's max_queue_depth and drops are counted for BORU-SS-28 metrics.
    let Ok(mut pacing) = PacingController::new(config.max_queue_depth) else { return SessionTermination::PacingInitFailed };
    // Track the frame period so skipped interval ticks (MissedTickBehavior::Skip)
    // can be counted as dropped obsolete frames when the pipeline fell behind.
    let frame_period_us = 1_000_000 / capture_fps as u64;
    let mut last_tick = std::time::Instant::now();
    // Adaptive quality (PDF Task 7.3): a host-side stats collector feeds a
    // congestion controller that gradually reduces bitrate/fps/resolution
    // under sustained congestion and recovers conservatively. The viewer's
    // manual lower-quality request (QualityUpdate) clamps the controller to
    // the requested ceiling.
    let mut stats = ScreenShareStats::new();
    let mut adaptive = AdaptiveQuality::new(config);
    // Pacing counters are cumulative; track the last seen total so new drops
    // since the previous tick can be fed to the stats snapshot.
    let mut last_pacing_drops: u64 = 0;
    // Run the control loop every N encoded frames (≈ 1s at 25fps): often
    // enough to react to congestion without making every frame a decision.
    let adapt_interval_frames: u64 = 25;
    let mut frames_since_adapt: u64 = 0;
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
            if let Some(mut backend) = backend.take() { backend.shutdown().await; }
            let _ = control.send(ControlOut::Legacy(ControlMessage::EndSession { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id })).await;
            return SessionTermination::UserStopped;
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
                    if let Some(mut backend) = backend.take() { backend.shutdown().await; }
                    let _ = control.send(ControlOut::Legacy(ControlMessage::EndSession { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id })).await;
                    return SessionTermination::ReconnectFailed;
                }
            }
        }
        let mut need_reconnect = false;
        tokio::select! {
            r = connection.accept_bi() => {
                match r {
                    Ok((mut send, recv)) => match read_unit(recv).await {
                        Ok(ReadUnit::Control(ControlMessage::Input { version: _, session_id: sid, nonce, kind, code, x, y, pressed, modifiers })) => {
                            // Every input must carry the current grant nonce.
                            let capability = kind.capability();
                            let authorized = manager.permissions(sid).is_some_and(|permissions| {
                                remote_input::authorize_nonce(permissions, sid, peer, capability, nonce).is_ok()
                            });
                            if !authorized { continue 'streaming; }
                            // Rate-limit pathological input streams (PDF Task
                            // 9.2): a buggy or malicious viewer flooding the
                            // control channel must not pin the platform input
                            // backend. Drops are silent; the share stays live.
                            if !input_limiter.allow(std::time::Instant::now()) {
                                input_rate_drops += 1;
                                if input_rate_drops == 1 || input_rate_drops % 500 == 0 {
                                    tracing::warn!(input_rate_drops, "screen-share: host dropped pathological input stream (rate limited)");
                                }
                                continue 'streaming;
                            }
                            // The backend exists iff control was granted; when
                            // it was denied/unavailable (None) input is dropped
                            // and the share continues view-only.
                            let Some(backend) = backend.as_mut() else { continue 'streaming; };
                            if kind.is_pointer() {
                                if let Some((px, py)) = remote_input::normalize_to_capture(NormalizedPointer { x, y }, (capture_width, capture_height)) {
                                    let _ = backend.apply(InputEvent { kind, code, capability, token: None, x: px as f32, y: py as f32, pressed, modifiers }).await;
                                }
                            } else {
                                let _ = backend.apply(InputEvent { kind, code, capability, token: None, x: 0.0, y: 0.0, pressed, modifiers }).await;
                            }
                        }
                        Ok(ReadUnit::Control(message)) => {
                            let response = manager.apply_remote(peer, message, events);
                            if let Some(response) = response { let _ = write_control_response(&mut send, &response).await; }
                            if manager.state(session_id) == Some(SessionState::Ended) {
                                // Stop condition (PDF Task 9.1): sharing ended
                                // (peer EndSession) — shut the remote-input
                                // backend down immediately so no further input
                                // can be injected, then leave the loop.
                                if let Some(mut backend) = backend.take() { backend.shutdown().await; }
                                return SessionTermination::PeerEnded;
                            }
                        }
                        Ok(ReadUnit::ScreenShare(message)) => match message {
                            // Keyframe requests travel on the reliable control
                            // channel (PDF Task 3.2); force the encoder so the
                            // next unit is independently decodable.
                            ScreenShareMessage::KeyframeRequest { session_id: sid, .. } if sid == session_id => {
                                keyframe_requests += 1;
                                encoder.force_keyframe();
                            }
                            // Manual lower-quality request from the viewer
                            // (PDF Task 7.3 / QualityUpdate path): clamp the
                            // adaptive controller to the requested ceiling
                            // and apply the resulting config immediately.
                            ScreenShareMessage::QualityUpdate { session_id: sid, target_bitrate_bps, max_frame_rate, scale_factor, .. } if sid == session_id => {
                                let request = ViewerQualityRequest { target_bitrate_bps, max_frame_rate, scale_factor };
                                let decision = adaptive.apply_viewer_request(request);
                                if apply_quality_config(&mut encoder, &mut config, decision) {
                                    tracing::info!(target_bitrate_bps, max_frame_rate, scale_factor, level = adaptive.level(), "screen-share: host applied viewer quality request");
                                }
                            }
                            ScreenShareMessage::Error { session_id: sid, message: peer_error, .. } if sid == session_id => {
                                tracing::warn!(error = %peer_error, "screen-share: host received peer error");
                            }
                            // Text-only clipboard sync (PDF Task 9.3 /
                            // BORU-SS-25): the viewer pushes its local
                            // clipboard to the host. Clipboard is a SEPARATE
                            // optional capability — never implied by remote
                            // control — so the payload is authorized against
                            // the explicitly granted Clipboard capability
                            // (with the current grant nonce as the freshness
                            // gate, mirroring input). Unauthorized payloads
                            // are dropped without applying or logging them.
                            ScreenShareMessage::Clipboard { session_id: sid, nonce, text, .. } if sid == session_id => {
                                let authorized = manager.permissions(sid).is_some_and(|permissions| {
                                    remote_input::authorize_nonce(permissions, sid, peer, Capability::Clipboard, nonce).is_ok()
                                });
                                if !authorized { continue 'streaming; }
                                tracing::info!("screen-share: host applied peer clipboard payload (text)");
                                // The event carries the RedactedText wrapper so
                                // Debug can never leak the payload (PDF Phase
                                // 12); the app unwraps it for the clipboard.
                                let _ = events.send(SessionEvent::ClipboardReceived { session_id: sid, text }).await;
                            }
                            other => {
                                // Other versioned lifecycle messages are not
                                // consumed by the legacy host loop yet. Log
                                // only the variant discriminant — clipboard
                                // payloads must never be logged (PDF guardrail).
                                tracing::debug!(variant = ?std::mem::discriminant(&other), "screen-share: host ignored versioned message");
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
                    // Remote control is opt-in (PDF Task 9.1 / T5.3): the
                    // platform input backend (RemoteDesktop portal on
                    // Wayland/XWayland, XTest on native X11) is opened only
                    // when the host user explicitly grants CONTROL. A
                    // clipboard-only grant (PDF Task 9.3) must never open the
                    // input backend or pop the portal dialog — clipboard sync
                    // is a separate optional capability. If the platform is
                    // missing or the user denies the portal dialog,
                    // `create_platform_backend` fails closed to
                    // `UnavailableInputBackend` and the grant still proceeds
                    // at the protocol level while input does nothing — the
                    // share remains view-only and functional.
                    let grants_control = capabilities.iter().any(|capability| capability.is_control());
                    if grants_control && backend.is_none() {
                        tracing::info!("screen-share: host opening remote-input backend (explicit control grant)");
                        let backend_started = std::time::Instant::now();
                        backend = Some(create_platform_backend(
                            (capture_width, capture_height),
                            capture.input_origin(),
                            &capabilities,
                        )
                        .await);
                        tracing::info!(elapsed_ms = backend_started.elapsed().as_millis() as u64, "screen-share: host remote-input backend ready");
                    }
                    if let Some(message) = manager.grant_control(session_id, capabilities, events) {
                        let _ = control.send(ControlOut::Legacy(message)).await;
                    }
                }
                Some(HostCommand::RevokeControl) => {
                    if let Some(mut backend) = backend.take() { backend.shutdown().await; }
                    if let Some(message) = manager.revoke_control(session_id, events) {
                        let _ = control.send(ControlOut::Legacy(message)).await;
                    }
                }
                Some(HostCommand::SendClipboard(text)) => {
                    // Host pushes its text clipboard to the viewer (PDF
                    // Task 9.3). Clipboard is a separate optional capability:
                    // the payload rides the reliable control channel as a
                    // versioned message and the viewer authorizes it against
                    // the explicitly granted Clipboard capability. Bounded to
                    // MAX_CLIPBOARD_TEXT (validation would reject larger).
                    if text.as_str().len() > protocol::MAX_CLIPBOARD_TEXT { continue; }
                    let nonce = manager
                        .permissions(session_id)
                        .and_then(|permissions| permissions.token().map(|token| *token.nonce()));
                    let Some(nonce) = nonce else { continue; };
                    let _ = control
                        .send(ControlOut::Versioned(ScreenShareMessage::Clipboard {
                            version: SCREEN_SHARE_PROTOCOL_VERSION,
                            session_id,
                            nonce,
                            text,
                        }))
                        .await;
                }
                Some(HostCommand::SwitchSource(source_id)) => {
                    // PDF Phase 10: the sharer switches the shared monitor
                    // WITHOUT ending the Boru chat session. Sequencing
                    // contract: the SourceChanged message is sent BEFORE any
                    // frame with the new geometry, then the capture backend
                    // re-selects the source, the encoder reconfigures and a
                    // keyframe is forced so the viewer resynchronises
                    // immediately.
                    if let Some(geometry) = switch_capture_source(
                        &mut capture,
                        source_id,
                        &capture_config,
                        &mut config,
                        &mut encoder,
                        &mut adaptive,
                        &control,
                        session_id,
                        events,
                    )
                    .await
                    {
                        announced_geometry = Some(geometry);
                        current_source = capture.current_source().or_else(|| current_source.clone());
                        stream_paused = false;
                    }
                }
                None => return SessionTermination::HostCommandClosed,
            },
            _ = interval.tick() => {
                // Pacing (PDF Task 7.2): with MissedTickBehavior::Skip, when
                // the previous encode/send round exceeded one frame period the
                // interval coalesces the missed ticks into the next one. Those
                // frames were implicitly dropped rather than queued — the
                // latest-frame strategy. Count them as obsolete so drop
                // pressure is visible to metrics.
                let now = std::time::Instant::now();
                let elapsed_us = now.saturating_duration_since(last_tick).as_micros() as u64;
                last_tick = now;
                if elapsed_us > frame_period_us {
                    pacing.note_missed_frames(elapsed_us / frame_period_us - 1);
                }
                if stream_paused {
                    // PDF Phase 10: the shared source disappeared (monitor
                    // unplug / dock-undock) and no fallback remained, so the
                    // stream paused. Periodically re-enumerate: a dock-in may
                    // have restored a monitor. Resume with the first
                    // available source; the session (and the chat session it
                    // belongs to) survived the whole time.
                    if paused_check.elapsed() >= Duration::from_millis(1000) {
                        paused_check = std::time::Instant::now();
                        let sources = capture.list_sources().unwrap_or_default();
                        if let Some(source) = sources.first().cloned() {
                            if let Some(geometry) = switch_capture_source(
                                &mut capture,
                                source.id,
                                &capture_config,
                                &mut config,
                                &mut encoder,
                                &mut adaptive,
                                &control,
                                session_id,
                                events,
                            )
                            .await
                            {
                                announced_geometry = Some(geometry);
                                current_source = Some(source);
                                stream_paused = false;
                                tracing::info!(session = ?session_id, "screen-share: stream resumed after source reappeared");
                            }
                        }
                    }
                    continue 'streaming;
                }
                match capture.capture() {
                    Ok(Some(frame)) => {
                        // Damage-aware capture (BORU-SS-32): a backend that
                        // observed no pixel change attaches
                        // DirtyRegion::Rects(empty). Skip encode+transmit
                        // entirely — the viewer already holds an identical
                        // frame — and count the skip so the reduction is
                        // visible in the metrics (capture/encode fps and
                        // bytes/sec drop on a static screen).
                        if frame.dirty_region.as_ref().is_some_and(DirtyRegion::is_empty) {
                            skipped_frames += 1;
                            stats.observe_skip();
                            if skipped_frames == 1 || skipped_frames % 500 == 0 {
                                tracing::info!(skipped_frames, "screen-share: host skipped unchanged frame (empty dirty region)");
                            }
                            continue 'streaming;
                        }
                        // The pacing queue is bounded (max_queue_depth): if the
                        // encoder/network fell behind and the queue is full, the
                        // oldest stale frame is dropped (counted) and the newest
                        // survives — never build latency by queueing obsolete
                        // frames.
                        pacing.push(frame);
                        stats.observe_capture();
                        let Some(frame) = pacing.pop_latest() else {
                            if let Some(mut backend) = backend.take() { backend.shutdown().await; }
                            return SessionTermination::PipelineError;
                        };
                        // Feed the pacing drop counters into the stats collector
                        // (delta since the last tick).
                        let pacing_drops = pacing.counters().dropped_queue_full.saturating_add(pacing.counters().dropped_obsolete);
                        if pacing_drops > last_pacing_drops {
                            stats.observe_pacing_drop(pacing_drops - last_pacing_drops);
                        }
                        last_pacing_drops = pacing_drops;
                        // Real portal captures negotiate their geometry after
                        // streaming starts; reconfigure the encoder when the
                        // frame size differs from the initial config.
                        if frame.width != config.width || frame.height != config.height {
                            if frame.width == 0 || frame.height == 0 || frame.width % 2 != 0 || frame.height % 2 != 0 {
                                tracing::warn!(width = frame.width, height = frame.height, "screen-share: capture produced invalid geometry, ending session");
                                if let Some(mut backend) = backend.take() { backend.shutdown().await; }
                                return SessionTermination::InvalidGeometry;
                            }
                            // PDF Phase 10: send the explicit source-change /
                            // config-change message BEFORE the media
                            // dimensions change (platform renegotiation,
                            // monitor resize without an explicit switch),
                            // then force a keyframe so the viewer
                            // resynchronises. The announcement is made at
                            // most once per geometry to avoid flooding the
                            // control channel when the encoder cannot fully
                            // adopt the new size (adaptive clamping).
                            let geometry = (frame.width & !1, frame.height & !1);
                            if announced_geometry != Some(geometry) {
                                let announced = if let Some(source) = capture.current_source() {
                                    let message = source_changed_message(session_id, &source, capture_config.target_fps);
                                    control.send(ControlOut::Versioned(message)).await.is_ok()
                                } else {
                                    false
                                };
                                if !announced {
                                    // No tracked source identity (e.g. the
                                    // whole-root fallback): announce the
                                    // geometry change directly.
                                    let _ = control
                                        .send(ControlOut::Versioned(ScreenShareMessage::SourceChanged {
                                            version: SCREEN_SHARE_PROTOCOL_VERSION,
                                            session_id,
                                            source_id: 1,
                                            title: format!("Screen: {}x{}", frame.width, frame.height),
                                            width: geometry.0.min(u16::MAX as u32) as u16,
                                            height: geometry.1.min(u16::MAX as u32) as u16,
                                            frame_rate: capture_config.target_fps.min(u16::MAX as u32) as u16,
                                        }))
                                        .await;
                                }
                                announced_geometry = Some(geometry);
                                tracing::info!(session = ?session_id, width = frame.width, height = frame.height, "screen-share: announced capture geometry change before media");
                            }
                            encoder.force_keyframe();
                            // Track the new capture geometry in the adaptive
                            // controller (level preserved, viewer ceiling
                            // re-scaled) and apply its decision.
                            let decision = adaptive.set_capture_geometry(frame.width, frame.height);
                            let _ = apply_quality_config(&mut encoder, &mut config, decision);
                        }
                        let encode_started = std::time::Instant::now();
                        match encoder.encode(&frame) {
                            Ok(encoded) => {
                                stats.observe_encode(encode_started.elapsed());
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
                                let encode_age_us = encoded.encode_timestamp_us.saturating_sub(encoded.timestamp_us);
                                stats.observe_send(bytes_len);
                                let dropped = media.send_frame(encoded).await;
                                if dropped {
                                    media_drops += 1;
                                    stats.observe_media_drop();
                                }
                                if sequence == 0 || sequence % 150 == 0 {
                                    let pacing_counters = pacing.counters();
                                    tracing::info!(sequence, bytes = bytes_len, media_drops, keyframe_requests, encode_age_us,
                                        pacing_captured = pacing_counters.captured, pacing_encoded = pacing_counters.encoded,
                                        pacing_dropped_queue_full = pacing_counters.dropped_queue_full,
                                        pacing_dropped_obsolete = pacing_counters.dropped_obsolete,
                                        "screen-share: host frame queued on media channel");
                                }
                                if media_drops > 0 && media_drops % 150 == 0 {
                                    tracing::warn!(media_drops, "screen-share: host dropping stale media frames (queue full)");
                                }
                                // Adaptive quality (PDF Task 7.3): feed the
                                // congestion controller every N frames with a
                                // stats snapshot (queue depth, throughput,
                                // RTT, encode time, drops) and apply its
                                // decision to the encoder.
                                frames_since_adapt += 1;
                                if frames_since_adapt >= adapt_interval_frames {
                                    frames_since_adapt = 0;
                                    let queue_depth = media.len().await as u64;
                                    stats.set_send_queue_depth(queue_depth);
                                    stats.set_bytes_in_flight(queue_depth.saturating_mul(bytes_len as u64));
                                    // RTT from the selected QUIC path, when
                                    // available (0 when unknown).
                                    let rtt_us = connection
                                        .paths()
                                        .iter()
                                        .find(|path| path.is_selected())
                                        .map(|path| path.rtt().as_micros() as u64)
                                        .unwrap_or(0);
                                    stats.set_rtt_us(rtt_us);
                                    let snapshot = stats.snapshot();
                                    let decision = adaptive.update(snapshot);
                                    // PDF Phase 12: expose developer metrics to
                                    // the diagnostics overlay and logs — capture
                                    // FPS, encode FPS, average encode time,
                                    // bytes/sec, dropped frames, queue depth,
                                    // decode FPS, estimated end-to-end latency.
                                    // Reuses THIS snapshot (the same one the
                                    // adaptive controller just consumed) so the
                                    // extra publish never perturbs its interval
                                    // measurements. try_send so a full events
                                    // channel never stalls capture. No media
                                    // data is ever included.
                                    let metrics = ScreenShareSessionMetrics {
                                        codec: "h264".to_string(),
                                        width: config.width,
                                        height: config.height,
                                        fps: config.target_fps,
                                        bitrate_bps: config.target_bitrate_bps as u64,
                                        backend: capture.backend_name().to_string(),
                                        snapshot,
                                    };
                                    let _ = events.try_send(SessionEvent::Metrics { session_id, metrics });
                                    tracing::info!(
                                        capture_fps = snapshot.sender_fps,
                                        encode_fps = snapshot.encoded_fps,
                                        skipped_frames = snapshot.skipped_frames,
                                        encode_avg_us = snapshot.encode_time_avg_us,
                                        bytes_per_sec = snapshot.bitrate_bps / 8,
                                        dropped_frames = snapshot.dropped_frames,
                                        queue_depth = snapshot.send_queue_depth,
                                        decode_fps = snapshot.receiver_fps,
                                        latency_us = snapshot.frame_age_us,
                                        "screen-share: performance metrics"
                                    );
                                    if apply_quality_config(&mut encoder, &mut config, decision) {
                                        let pacing_counters = pacing.counters();
                                        tracing::info!(level = adaptive.level(), width = config.width, height = config.height,
                                            fps = config.target_fps, bitrate = config.target_bitrate_bps,
                                            queue_depth, rtt_us, measured_bps = snapshot.measured_throughput_bps,
                                            encode_avg_us = snapshot.encode_time_avg_us, dropped = snapshot.dropped_frames,
                                            pacing_captured = pacing_counters.captured, pacing_encoded = pacing_counters.encoded,
                                            pacing_dropped_queue_full = pacing_counters.dropped_queue_full,
                                            pacing_dropped_obsolete = pacing_counters.dropped_obsolete,
                                            "screen-share: adaptive quality changed");
                                    }
                                }
                            }
                            Err(error) => {
                                tracing::warn!(error = %error, "screen-share: host encode failed");
                            }
                        }
                    }
                    Ok(None) => {
                        // Damage-aware capture (BORU-SS-32): the X11 backend
                        // returns no frame when the DAMAGE extension reports
                        // nothing changed (the frame-level skip); portal
                        // backends return none when the frame queue is
                        // momentarily empty. Either way no encode happens
                        // this tick — count it so the reduction is visible.
                        skipped_frames += 1;
                        stats.observe_skip();
                        if skipped_frames == 1 || skipped_frames % 500 == 0 {
                            tracing::info!(skipped_frames, "screen-share: host tick produced no frame (unchanged screen or empty queue)");
                        }
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "screen-share: capture failed");
                        // PDF Phase 10: monitor unplug / laptop dock-undock.
                        // Recover gracefully instead of ending the session:
                        // re-enumerate and fall back to the first remaining
                        // source, or pause the stream when none remains. The
                        // chat session and the screen-share session both
                        // survive — no crash, no forced end.
                        if !recover_capture_source(
                            &mut capture,
                            &mut current_source,
                            &capture_config,
                            &mut config,
                            &mut encoder,
                            &mut adaptive,
                            &control,
                            session_id,
                            events,
                        )
                        .await
                        {
                            stream_paused = true;
                            paused_check = std::time::Instant::now();
                        }
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
                    if let Some(mut backend) = backend.take() { backend.shutdown().await; }
                    let _ = control.send(ControlOut::Legacy(ControlMessage::EndSession { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id })).await;
                    return SessionTermination::ReconnectFailed;
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

/// Apply one adaptive-quality decision to the live encoder.
///
/// Resolution/fps changes rebuild the encoder (config generation bump so the
/// viewer re-initialises its decoder); a pure bitrate change uses the cheaper
/// same-resolution rebuild (no generation bump, forced keyframe re-syncs the
/// stream). Returns `true` when a change was applied.
fn apply_quality_config(
    encoder: &mut OpenH264Encoder,
    config: &mut CodecConfig,
    decision: QualityDecision,
) -> bool {
    if !decision.changed { return false; }
    let next = decision.config;
    if next.width != config.width || next.height != config.height || next.target_fps != config.target_fps {
        if encoder.reconfigure(next).is_err() { return false; }
    } else if next.target_bitrate_bps != config.target_bitrate_bps {
        if encoder.reconfigure_bitrate(next.target_bitrate_bps).is_err() { return false; }
    } else {
        return false;
    }
    *config = next;
    true
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

/// Build the wire `SourceChanged` message that MUST be sent BEFORE the first
/// frame with the new geometry (PDF Phase 10: "send an explicit
/// source-change/config-change message before media dimensions change").
/// The dimensions are rounded down to even values to match what the encoder
/// will actually produce. Pure helper so the sequencing contract is
/// unit-testable without a live transport.
fn source_changed_message(
    session_id: ScreenShareSessionId,
    source: &CaptureSource,
    target_fps: u32,
) -> ScreenShareMessage {
    ScreenShareMessage::SourceChanged {
        version: SCREEN_SHARE_PROTOCOL_VERSION,
        session_id,
        source_id: source.id.0,
        title: source.title.clone(),
        width: (source.width & !1).min(u16::MAX as u32) as u16,
        height: (source.height & !1).min(u16::MAX as u32) as u16,
        frame_rate: target_fps.min(u16::MAX as u32) as u16,
    }
}

/// Select the fallback source after the current source disappears (monitor
/// unplug / laptop dock-undock, PDF Phase 10). Keeps the current source when
/// it is still enumerated (a transient error); otherwise falls back to the
/// first remaining source. Returns `None` when no source remains — the
/// stream pauses instead of ending the session.
fn select_fallback_source(
    sources: &[CaptureSource],
    current: Option<CaptureSourceId>,
) -> Option<CaptureSource> {
    if let Some(current) = current {
        if let Some(source) = sources.iter().find(|source| source.id == current) {
            return Some(source.clone());
        }
    }
    sources.first().cloned()
}

/// Plan a source switch (PDF Phase 10). Returns the wire `SourceChanged`
/// message — which MUST be sent before any frame with the new geometry —
/// and the encoder config the source requires, or `None` when `source_id`
/// is not in the current enumeration (e.g. a monitor that was unplugged) or
/// the source has no capturable even-sized geometry.
fn plan_source_switch(
    session_id: ScreenShareSessionId,
    sources: &[CaptureSource],
    source_id: CaptureSourceId,
    target_fps: u32,
    current: &CodecConfig,
) -> Option<(ScreenShareMessage, CodecConfig)> {
    let source = sources.iter().find(|source| source.id == source_id)?;
    let width = source.width & !1;
    let height = source.height & !1;
    if width == 0 || height == 0 {
        return None;
    }
    let mut config = *current;
    config.width = width;
    config.height = height;
    config.target_fps = target_fps;
    Some((source_changed_message(session_id, source, target_fps), config))
}

/// Execute a source switch (PDF Phase 10: the sharer switches the shared
/// monitor without ending the Boru chat session). Sequencing contract:
/// 1. the `SourceChanged` message is sent BEFORE any frame with the new
///    geometry;
/// 2. the capture backend re-selects the source (stop + start);
/// 3. the encoder reconfigures to the new geometry (config-generation bump
///    so the viewer re-initialises its decoder) and a keyframe is forced so
///    the viewer resynchronises immediately;
/// 4. `SessionEvent::SourceChanged` surfaces the change to the app UI.
/// Returns `Some((width, height))` — the announced even-sized geometry — on
/// success, `None` when the switch could not be applied (unknown source,
/// transport failure), in which case the previous source stays active.
#[allow(clippy::too_many_arguments)]
async fn switch_capture_source(
    capture: &mut ActiveCapture,
    source_id: CaptureSourceId,
    capture_config: &CaptureConfig,
    config: &mut CodecConfig,
    encoder: &mut OpenH264Encoder,
    adaptive: &mut AdaptiveQuality,
    control: &ControlChannel,
    session_id: ScreenShareSessionId,
    events: &mpsc::Sender<SessionEvent>,
) -> Option<(u32, u32)> {
    // Validate against the CURRENT enumeration (an unplugged monitor is no
    // longer a valid switch target).
    let sources = match capture.list_sources() {
        Ok(sources) => sources,
        Err(error) => {
            tracing::warn!(error = %error, ?source_id, "screen-share: switch source failed (enumeration error)");
            return None;
        }
    };
    let Some(source) = sources.iter().find(|source| source.id == source_id) else {
        tracing::warn!(?source_id, "screen-share: switch source failed (unknown source)");
        return None;
    };
    let width = source.width & !1;
    let height = source.height & !1;
    if width == 0 || height == 0 {
        tracing::warn!(?source_id, width, height, "screen-share: switch source failed (no capturable geometry)");
        return None;
    }
    // 1. Announce the change BEFORE any frame with the new geometry.
    let message = source_changed_message(session_id, source, capture_config.target_fps);
    if let Err(error) = control.send(ControlOut::Versioned(message)).await {
        tracing::warn!(error = %error, ?source_id, "screen-share: switch source failed (control channel)");
        return None;
    }
    // 2. Re-select the source on the capture backend.
    if let Err(error) = capture.switch_source(source.id, capture_config) {
        tracing::warn!(error = %error, ?source_id, "screen-share: switch source failed (capture backend)");
        return None;
    }
    // 3. Reconfigure the encoder for the new geometry. The adaptive
    // controller's base is updated FIRST so its next update tick cannot
    // revert the encoder to the OLD geometry; the encoder then adopts the
    // decision (or the full source geometry when the decision is a no-op,
    // e.g. switching between two same-sized monitors — the generation bump
    // still tells the viewer to re-initialise its decoder).
    let decision = adaptive.set_capture_geometry(width, height);
    let changed = apply_quality_config(encoder, config, decision);
    if !changed {
        let mut next = *config;
        next.width = width;
        next.height = height;
        next.target_fps = capture_config.target_fps;
        let _ = encoder.reconfigure(next);
        *config = next;
    }
    // Force a keyframe after the source/resolution change (PDF Phase 10).
    encoder.force_keyframe();
    // 4. Surface the change to the app UI.
    let _ = events.send(SessionEvent::SourceChanged {
        session_id,
        source_id: source.id.0,
        title: source.title.clone(),
        width: source.width,
        height: source.height,
    }).await;
    tracing::info!(session = ?session_id, ?source_id, title = %source.title, width, height, "screen-share: host switched source");
    Some((width, height))
}

/// Recover from a capture failure (PDF Phase 10: monitor unplug, laptop
/// dock/undock). Re-enumerates the backend; when the current source is gone
/// it falls back to the first remaining source (announcing the change and
/// forcing a keyframe), and when no source remains it pauses. Returns true
/// when streaming should continue, false to pause. The chat session and the
/// screen-share session survive either way — no crash, no forced end.
#[allow(clippy::too_many_arguments)]
async fn recover_capture_source(
    capture: &mut ActiveCapture,
    current_source: &mut Option<CaptureSource>,
    capture_config: &CaptureConfig,
    config: &mut CodecConfig,
    encoder: &mut OpenH264Encoder,
    adaptive: &mut AdaptiveQuality,
    control: &ControlChannel,
    session_id: ScreenShareSessionId,
    events: &mpsc::Sender<SessionEvent>,
) -> bool {
    let sources = match capture.list_sources() {
        Ok(sources) => sources,
        Err(error) => {
            tracing::warn!(error = %error, "screen-share: source re-enumeration failed during recovery; pausing");
            let _ = events.send(SessionEvent::SourceUnavailable { session_id, reason: error.to_string(), fallback: None }).await;
            return false;
        }
    };
    let current_id = current_source.as_ref().map(|source| source.id);
    let fallback = select_fallback_source(&sources, current_id);
    let Some(fallback) = fallback else {
        // No source remains — pause the stream (the session survives; a
        // periodic re-enumeration resumes when a monitor re-appears).
        let _ = events.send(SessionEvent::SourceUnavailable { session_id, reason: "no capture source remains (monitor unplugged?)".into(), fallback: None }).await;
        return false;
    };
    // The current source is still enumerated: a transient failure. Pause
    // briefly rather than re-arming the failing capture at frame rate; the
    // paused recovery re-checks within a second.
    if current_id == Some(fallback.id) {
        let _ = events.send(SessionEvent::SourceUnavailable { session_id, reason: "capture failed; pausing until the source is stable".into(), fallback: Some(fallback.title.clone()) }).await;
        return false;
    }
    // Switch to the first remaining source and continue streaming.
    if switch_capture_source(capture, fallback.id, capture_config, config, encoder, adaptive, control, session_id, events).await.is_some() {
        *current_source = Some(fallback.clone());
        let _ = events.send(SessionEvent::SourceUnavailable { session_id, reason: "capture source changed after unplug".into(), fallback: Some(fallback.title.clone()) }).await;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen_share::coords::MonitorGeometry;
    use crate::screen_share::capture::{CaptureSource, CaptureSourceId, CaptureSourceKind};

    fn source(id: u64, name: &str, width: u32, height: u32) -> CaptureSource {
        CaptureSource {
            id: CaptureSourceId(id),
            kind: CaptureSourceKind::Monitor,
            title: format!("{name}: {width}x{height}"),
            width,
            height,
            geometry: Some(MonitorGeometry::new(0, 0, width, height)),
        }
    }

    /// PDF Phase 10 sequencing contract: the `SourceChanged` message is
    /// produced together with the encoder config that follows it, carries
    /// the SAME new geometry, and is wire-valid BEFORE the dimensions
    /// change. A source switch can never reconfigure the encoder without
    /// first announcing the change.
    #[test]
    fn source_switch_plan_announces_before_dimensions_change() {
        let sid = ScreenShareSessionId::from_bytes([9; 16]);
        let sources = vec![
            source(1, "DP-1", 1920, 1080),
            source(2, "HDMI-A-0", 2560, 1440),
        ];
        let current = CodecConfig::default();
        let (message, config) = plan_source_switch(sid, &sources, CaptureSourceId(2), 15, &current)
            .expect("switch to the second monitor must plan");
        match &message {
            ScreenShareMessage::SourceChanged { source_id, title, width, height, frame_rate, .. } => {
                assert_eq!(*source_id, 2);
                assert_eq!(title, "HDMI-A-0: 2560x1440");
                // The announced dimensions are exactly what the encoder will
                // be reconfigured to (rounded to even).
                assert_eq!((*width as u32, *height as u32), (config.width, config.height));
                assert_eq!((config.width, config.height), (2560, 1440));
                assert_eq!(*frame_rate as u32, config.target_fps);
                assert_eq!(config.target_fps, 15);
            }
            other => panic!("expected SourceChanged plan, got {other:?}"),
        }
        // The message must be wire-valid before any frame with the new
        // geometry arrives.
        let bytes = message.encode().expect("announcement must be wire-valid");
        assert_eq!(ScreenShareMessage::decode(&bytes).unwrap(), message);
    }

    /// The announcement message rounds odd source dimensions down to even
    /// values so it always matches what the encoder will actually produce.
    #[test]
    fn source_changed_message_rounds_to_even_dimensions() {
        let sid = ScreenShareSessionId::from_bytes([9; 16]);
        let odd = source(7, "OddPanel", 1919, 1079);
        let message = source_changed_message(sid, &odd, 15);
        match &message {
            ScreenShareMessage::SourceChanged { width, height, .. } => {
                assert_eq!((*width, *height), (1918, 1078));
            }
            other => panic!("expected SourceChanged, got {other:?}"),
        }
        assert!(message.encode().is_ok(), "rounded announcement must validate");
    }

    /// A switch to a source that is NOT in the current enumeration (e.g. a
    /// monitor that was unplugged) must not plan — the caller keeps the
    /// previous source.
    #[test]
    fn plan_source_switch_rejects_unknown_or_absent_source() {
        let sid = ScreenShareSessionId::from_bytes([9; 16]);
        let sources = vec![source(1, "DP-1", 1920, 1080)];
        assert!(
            plan_source_switch(sid, &sources, CaptureSourceId(99), 15, &CodecConfig::default())
                .is_none(),
            "a monitor that is no longer enumerated must be rejected"
        );
        // A source with no capturable geometry (1x1 rounds to 0x0) is also
        // rejected — it cannot be encoded.
        let degenerate = vec![source(3, "Tiny", 1, 1)];
        assert!(
            plan_source_switch(sid, &degenerate, CaptureSourceId(3), 15, &CodecConfig::default())
                .is_none()
        );
    }

    /// PDF Phase 10 unplug handling: when the CURRENT source is still
    /// enumerated, a transient capture failure keeps it (no needless
    /// switch); the caller pauses briefly instead of looping.
    #[test]
    fn fallback_keeps_current_source_when_still_enumerated() {
        let sources = vec![source(1, "DP-1", 1920, 1080), source(2, "HDMI-A-0", 1280, 720)];
        let fallback = select_fallback_source(&sources, Some(CaptureSourceId(2))).expect("fallback");
        assert_eq!(fallback.id, CaptureSourceId(2), "current source is still present");
    }

    /// PDF Phase 10 unplug handling: when the CURRENT source disappears from
    /// the enumeration, the first remaining source is chosen as the
    /// fallback so the stream continues without ending the session.
    #[test]
    fn fallback_picks_first_remaining_source_after_unplug() {
        let remaining = vec![source(1, "DP-1", 1920, 1080)];
        let fallback = select_fallback_source(&remaining, Some(CaptureSourceId(2))).expect("fallback");
        assert_eq!(fallback.id, CaptureSourceId(1), "fall back to the remaining monitor");
        assert_eq!(fallback.title, "DP-1: 1920x1080");
    }

    /// PDF Phase 10 unplug handling: when NO source remains the fallback is
    /// None — the host pauses the stream (the session survives) instead of
    /// ending it.
    #[test]
    fn fallback_returns_none_when_no_source_remains() {
        assert!(select_fallback_source(&[], Some(CaptureSourceId(2))).is_none());
        assert!(select_fallback_source(&[], None).is_none());
    }

    /// BORU-SS-36 window handling: a minimized window is still enumerated, so
    /// recovery keeps it selected (the host pauses, then resumes when the
    /// window is restored); a CLOSED window disappears from the enumeration,
    /// so recovery falls back to the first remaining source (a monitor).
    #[test]
    fn window_source_fallback_keeps_then_switches() {
        let window = CaptureSource {
            id: CaptureSourceId(0x1234_5678),
            kind: CaptureSourceKind::Window,
            title: "Terminal: 800x600".to_string(),
            width: 800,
            height: 600,
            geometry: None,
        };
        let monitor = source(1, "DP-1", 1920, 1080);
        // Minimized but still enumerated → keep the window.
        let fallback =
            select_fallback_source(&[monitor.clone(), window.clone()], Some(window.id))
                .expect("fallback");
        assert_eq!(fallback.id, window.id);
        // Closed → gone from enumeration → fall back to the monitor.
        let fallback = select_fallback_source(&[monitor.clone()], Some(window.id)).expect("fallback");
        assert_eq!(fallback.id, monitor.id);
    }
}
