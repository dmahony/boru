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
    audio::{
        audio_sample_ring, create_system_audio_capture, OpusAudioEncoder, SystemAudioCapture,
        AUDIO_FRAME_MS, AUDIO_RING_SAMPLES, AUDIO_SAMPLES_PER_FRAME,
    },
    capture::{CaptureConfig, CaptureSource, CaptureSourceId, CaptureSourceKind, DirtyRegion},
    channels::{
        ControlChannel, ControlOut, MediaChannel, DEFAULT_CONTROL_QUEUE_CAPACITY,
        DEFAULT_MEDIA_QUEUE_CAPACITY,
    },
    codec::{available_encoder_codecs, create_encoder, CodecConfig, VideoEncoder},
    coords::{desktop_to_normalized, scale_sprite_to, CursorMeta, CursorSprite, MonitorGeometry},
    permissions::{Capability, SlidingWindowRateLimiter},
    platform::{capture_dimensions, create_capture_source, CAPTURE_FPS, ActiveCapture},
    presets::QualityPreset,
    protocol::{self, ControlMessage, InputEventKind, RedactedText, ScreenShareMessage, SourceMode, SCREEN_SHARE_PROTOCOL_VERSION},
    reconnect::{retry_reconnect, ReconnectPolicy},
    remote_input::{self, create_platform_backend, InputEvent, NormalizedPointer, RemoteInput},
    session::{ScreenShareSessionId, SessionEvent, SessionManager, SessionState},
    stats::{ScreenShareSessionMetrics, ScreenShareStats},
    transport::{read_unit, selected_path_kind, AudioHeader, PathKind, QuicScreenTransport, ReadUnit},
    ScreenShareError, ScreenShareErrorKind, SCREEN_SHARE_ALPN,
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
    /// Host user toggles system-audio sharing (BORU-SS-37). Audio is a
    /// SEPARATE optional capability: enabling it grants `Capability::Audio`
    /// (mirroring clipboard) and starts the platform capture backend; the
    /// capture thread pushes PCM into a bounded ring that the streaming loop
    /// drains every Opus frame. Disabling stops capture and audio packets.
    /// Emits `SessionEvent::AudioState` with the outcome.
    SetAudioEnabled(bool),
    /// Sharer overrides the quality preset (BORU-SS-39). The chosen preset's
    /// ceiling is applied immediately — the user's explicit choice wins over
    /// the path-derived auto preset until the session ends. `None` restores
    /// the path-derived preset (auto mode).
    SetQualityPreset(Option<QualityPreset>),
}

/// Host-side cursor delivery state for `Metadata` cursor mode (BORU-SS-33 /
/// PDF Task 5.3).
///
/// The capture backend attaches [`CursorMeta`] to frames instead of
/// compositing the cursor into the pixels. The host converts that metadata
/// into `CursorShape` (on shape change) + `CursorPosition` (per move) control
/// messages and — when only the cursor moved — skips the full-frame re-encode
/// entirely. This struct tracks what the viewer already knows so the host
/// never re-sends an unchanged shape or a duplicate position.
#[derive(Debug, Clone, Default)]
struct CursorTracker {
    /// Last scaled sprite actually sent (bytes + dims + hotspot).
    last_sprite: Option<CursorSprite>,
    /// Last normalized position actually sent.
    last_position: Option<(f32, f32)>,
    /// Last visibility actually sent.
    last_visible: Option<bool>,
    /// Monotonic counter for shape ids (never reused within a session).
    next_shape_id: u32,
}

/// Decide whether a captured frame can be SKIPPED because only the cursor
/// moved (BORU-SS-33 metadata mode).
///
/// Skipping is only allowed when ALL of:
/// - the backend delivered cursor metadata (`metadata_mode`) — in fallback
///   mode the cursor is composited into the pixels, so an unchanged frame
///   cannot be detected and skipping would drop real content;
/// - the encoder does NOT have a keyframe pending (reconnect / source
///   switch / viewer recovery request must always produce a frame);
/// - the frame pixels are byte-identical to the last frame actually encoded.
fn should_skip_unchanged_frame(
    metadata_mode: bool,
    keyframe_pending: bool,
    last_encoded_pixels: Option<&[u8]>,
    frame_pixels: &[u8],
) -> bool {
    metadata_mode && !keyframe_pending && last_encoded_pixels.is_some_and(|last| last == frame_pixels)
}

impl CursorTracker {
    /// Build a `CursorShape` message when the sprite changed, updating the
    /// tracker. The sprite is scaled from the source resolution to the
    /// encode resolution so the viewer composites it 1:1 into the frame it
    /// actually decodes (BORU-SS-33). Returns `None` when the sprite is
    /// byte-identical to the last shape sent.
    fn shape_message(
        &mut self,
        session_id: ScreenShareSessionId,
        meta: &CursorMeta,
        source: (u32, u32),
        encode: (u32, u32),
    ) -> Option<ScreenShareMessage> {
        let sprite = meta.sprite.as_ref()?;
        let scaled = scale_sprite_to(sprite, source.0, source.1, encode.0, encode.1);
        if self.last_sprite.as_ref() == Some(&scaled) {
            return None;
        }
        self.next_shape_id = self.next_shape_id.wrapping_add(1);
        self.last_sprite = Some(scaled.clone());
        Some(ScreenShareMessage::CursorShape {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id,
            shape_id: self.next_shape_id,
            width: scaled.width.min(u16::MAX as u32) as u16,
            height: scaled.height.min(u16::MAX as u32) as u16,
            hotspot_x: scaled.hotspot_x.min(u16::MAX as u32) as u16,
            hotspot_y: scaled.hotspot_y.min(u16::MAX as u32) as u16,
            pixels: scaled.pixels,
        })
    }

    /// Build a `CursorPosition` message when the normalized position or
    /// visibility changed, updating the tracker. Position is normalized
    /// against the shared source using the source geometry (or treated as
    /// source-relative when no geometry is known).
    fn position_message(
        &mut self,
        session_id: ScreenShareSessionId,
        meta: &CursorMeta,
        geometry: Option<&MonitorGeometry>,
        source: (u32, u32),
    ) -> Option<ScreenShareMessage> {
        let normalized = if let Some(geometry) = geometry {
            desktop_to_normalized(meta.position, geometry)
        } else {
            Some(super::coords::NormalizedPoint {
                x: meta.position.x.max(0) as f64 / source.0.max(1) as f64,
                y: meta.position.y.max(0) as f64 / source.1.max(1) as f64,
            })
        };
        let Some(normalized) = normalized else { return None; };
        let x = normalized.x.clamp(0.0, 1.0) as f32;
        let y = normalized.y.clamp(0.0, 1.0) as f32;
        if self.last_position == Some((x, y)) && self.last_visible == Some(meta.visible) {
            return None;
        }
        self.last_position = Some((x, y));
        self.last_visible = Some(meta.visible);
        Some(ScreenShareMessage::CursorPosition {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id,
            x,
            y,
            visible: meta.visible,
        })
    }
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
    // BORU-SS-34/35: advertise every codec this host can encode, ordered by
    // preference — hardware VA-API H.264 when usable, then the OpenH264
    // baseline, then AV1. The runtime legacy Accept carries no codec choice,
    // so the host falls back to the first advertised codec; the versioned
    // negotiation path (`ScreenShareOffer`/`ScreenShareAccept`, tested in
    // session.rs) carries the real selection, and `create_encoder` honours it.
    let Some(hello) = manager.hello(session_id, available_encoder_codecs(), capture_width.min(u16::MAX as u32) as u16, capture_height.min(u16::MAX as u32) as u16, capture_fps as u16) else { return SessionTermination::NegotiationFailed };
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
    // BORU-SS-39: choose the initial quality preset from the selected QUIC
    // path kind (Direct → LAN-high headroom, Relay → conservative ceiling,
    // Unknown → balanced). The sharer can override it at any time via
    // `HostCommand::SetQualityPreset`; a manual override wins over the
    // path-derived preset until the session ends (`None` restores auto).
    let initial_path = transport.path_kind();
    let mut preset = QualityPreset::for_path(initial_path);
    let mut user_override: Option<QualityPreset> = None;
    let mut last_path_kind = initial_path;
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
                    // BORU-SS-37: audio only flows once streaming starts; a
                    // unit arriving during negotiation is ignored.
                    Ok(ReadUnit::Audio(_, _)) => false,
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
                // Audio sharing (BORU-SS-37) only starts once the media path
                // is streaming; during negotiation the toggle is a no-op.
                Some(HostCommand::SetAudioEnabled(_)) => false,
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
                // BORU-SS-39: the sharer picked a quality preset before the
                // viewer accepted. The initial encoder config (created when
                // streaming starts) uses it; a later override applies to the
                // live adaptive controller.
                Some(HostCommand::SetQualityPreset(override_preset)) => {
                    preset = override_preset.unwrap_or_else(|| QualityPreset::for_path(last_path_kind));
                    user_override = override_preset;
                    tracing::info!(preset = preset.name(), override = user_override.is_some(), "screen-share: host preset selected before streaming");
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
    // BORU-SS-39: the initial encoder config is derived from the capture
    // session AND the quality preset chosen from the connection path (or the
    // sharer's pre-streaming override). The pre-preset config is kept as the
    // preset reference so later preset/path changes recompute the ceiling
    // relative to the capture rates, independent of the current adaptive
    // level.
    let mut config = CodecConfig::from_capture_config(&capture_config, encode_width, encode_height);
    let preset_reference = config;
    preset.apply_to_config(&mut config);
    // BORU-SS-34/35: build the encoder for the first advertised codec (the
    // legacy runtime Accept carries no codec choice, so the host falls back
    // to the first codec it advertised — hardware VA-API when usable, else
    // OpenH264; the versioned negotiation path carries the real selection).
    // `create_encoder` internally falls back to OpenH264 when a hardware
    // path fails to initialise.
    let host_codec = available_encoder_codecs().into_iter().next().unwrap_or_else(|| "h264".to_string());
    let Ok(mut encoder) = create_encoder(&host_codec, config) else { return SessionTermination::EncodeInitFailed };
    let encoder_codec = encoder.metadata().codec.wire_name();
    // PDF Phase 12: one structured capture-start line with the negotiated
    // codec, dimensions, bitrate, frame rate and backend. Contains no media
    // data (never screen contents or raw frame bytes).
    tracing::info!(
        event = "capture_start",
        backend = capture.backend_name(),
        codec = encoder_codec,
        width = encode_width,
        height = encode_height,
        bitrate_bps = config.target_bitrate_bps,
        frame_rate = capture_fps,
        preset = preset.name(),
        path = ?last_path_kind,
        "screen-share: capture started"
    );
    // BORU-SS-38: advertise the initial stream configuration (including the
    // `source_mode` — Single / PerDisplay / Spanning) BEFORE the first video
    // packet so the viewer knows how the shared desktop maps onto the stream.
    // Old viewers that predate the field decode it as Single (backward
    // compatible); new viewers use it to present the correct source model.
    let initial_mode = current_source
        .as_ref()
        .map(source_mode_for_source)
        .unwrap_or(SourceMode::Single);
    if let Some(source) = current_source.as_ref() {
        if let Ok(message) = stream_config_message(session_id, &config, source, capture_config.target_fps, initial_mode) {
            let _ = control.send(ControlOut::Versioned(message)).await;
        }
    }
    let _ = control
        .send(ControlOut::Versioned(ScreenShareMessage::SourceChanged {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id,
            source_id: current_source.as_ref().map_or(1, |source| source.id.0),
            title: current_source.as_ref().map_or_else(|| format!("Screen: {encode_width}x{encode_height}"), |source| source.title.clone()),
            width: encode_width.min(u16::MAX as u32) as u16,
            height: encode_height.min(u16::MAX as u32) as u16,
            frame_rate: capture_config.target_fps.min(u16::MAX as u32) as u16,
            source_mode: initial_mode,
        }))
        .await;
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
    // BORU-SS-33: metadata cursor mode. When the capture backend attaches
    // cursor metadata to frames (PipeWire spa_meta_cursor / XFixes notify),
    // the host delivers shape-on-change + position-per-move control messages
    // and — when only the cursor moved — skips re-encoding the frame.
    let mut cursor_tracker = CursorTracker::default();
    let mut cursor_shapes_sent: u64 = 0;
    let mut cursor_positions_sent: u64 = 0;
    let mut skipped_unchanged_frames: u64 = 0;
    // Last frame PIXELS that were actually encoded (for unchanged-content
    // detection in metadata mode). None until the first frame is encoded.
    let mut last_encoded_pixels: Option<Vec<u8>> = None;
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
    // BORU-SS-37 system-audio sharing (opt-in). The capture backend runs on
    // its own thread and pushes interleaved f32 PCM into a bounded ring; this
    // loop drains one Opus frame (20 ms) per audio tick and sends it on the
    // control channel with try_send (drop-on-full) so audio can NEVER block
    // the video path. The backend's Drop stops its thread, so every session
    // exit path cleans up automatically.
    let mut audio_enabled = false;
    let mut audio_capture: Option<Box<dyn SystemAudioCapture>> = None;
    let mut audio_consumer: Option<super::audio::AudioSampleConsumer> = None;
    let mut audio_encoder: Option<OpusAudioEncoder> = None;
    let mut audio_sequence: u64 = 0;
    let mut audio_timestamp_us: u64 = 0;
    let mut audio_dropped_packets: u64 = 0;
    let mut audio_interval = tokio::time::interval(Duration::from_millis(AUDIO_FRAME_MS));
    audio_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
                            // Viewer-initiated source switch (PDF Phase 14 /
                            // BORU-SS-38). Policy: ANY viewer (view-only or
                            // control-granted) may request; the host is the
                            // final arbiter and honors the request only when
                            // the requested source is in its CURRENT
                            // enumeration — a monitor that was unplugged, or
                            // an id that was never valid, is denied. Switching
                            // which display is shown grants no control.
                            ScreenShareMessage::RequestSource { session_id: sid, source_id, .. } if sid == session_id => {
                                let requested = CaptureSourceId(source_id);
                                let sources = capture.list_sources().unwrap_or_default();
                                let permitted = sources.iter().any(|source| source.id == requested);
                                if permitted {
                                    tracing::info!(?source_id, "screen-share: host honoring viewer source request");
                                    if let Some(geometry) = switch_capture_source(
                                        &mut capture,
                                        requested,
                                        &capture_config,
                                        &mut config,
                                        encoder.as_mut(),
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
                                } else {
                                    tracing::warn!(?source_id, "screen-share: host denied viewer source request (source not in current enumeration)");
                                }
                            }
                            // Manual lower-quality request from the viewer
                            // (PDF Task 7.3 / QualityUpdate path): clamp the
                            // adaptive controller to the requested ceiling
                            // and apply the resulting config immediately.
                            ScreenShareMessage::QualityUpdate { session_id: sid, target_bitrate_bps, max_frame_rate, scale_factor, .. } if sid == session_id => {
                                let request = ViewerQualityRequest { target_bitrate_bps, max_frame_rate, scale_factor };
                                let decision = adaptive.apply_viewer_request(request);
                                if apply_quality_config(encoder.as_mut(), &mut config, decision) {
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
                        // BORU-SS-37: audio is host→viewer only; a unit from
                        // the viewer is ignored (never blocks the video path).
                        Ok(ReadUnit::Audio(_, _)) => {}
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
                        encoder.as_mut(),
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
                Some(HostCommand::SetAudioEnabled(enabled)) => {
                    // BORU-SS-37: system audio is a SEPARATE optional
                    // capability (opt-in, like clipboard). Enabling grants
                    // the Audio capability (viewer authorizes packets against
                    // it) and starts the platform capture backend; the
                    // capture thread pushes PCM into a bounded ring that the
                    // audio tick drains. A typed unavailable error (no
                    // PipeWire, no WASAPI implementation) keeps the session
                    // view-only — video is never affected.
                    if enabled && !audio_enabled {
                        if let Some(message) = manager.grant_control(session_id, vec![Capability::Audio], events) {
                            let _ = control.send(ControlOut::Legacy(message)).await;
                        }
                        let mut capture = create_system_audio_capture();
                        let (producer, consumer) = audio_sample_ring(AUDIO_RING_SAMPLES);
                        match capture.start(producer) {
                            Ok(()) => match OpusAudioEncoder::new() {
                                Ok(encoder) => {
                                    audio_enabled = true;
                                    audio_capture = Some(capture);
                                    audio_consumer = Some(consumer);
                                    audio_encoder = Some(encoder);
                                    audio_sequence = 0;
                                    audio_timestamp_us = 0;
                                    let _ = events.send(SessionEvent::AudioState { session_id, enabled: true, error: None }).await;
                                    tracing::info!(session = ?session_id, "screen-share: system audio sharing enabled");
                                }
                                Err(error) => {
                                    let _ = events.send(SessionEvent::AudioState { session_id, enabled: false, error: Some(error.to_string()) }).await;
                                    tracing::warn!(error = %error, "screen-share: audio encoder unavailable");
                                }
                            },
                            Err(error) => {
                                let _ = events.send(SessionEvent::AudioState { session_id, enabled: false, error: Some(error.to_string()) }).await;
                                tracing::warn!(kind = ?error.kind(), error = %error, "screen-share: system audio capture unavailable; continuing view-only");
                            }
                        }
                    } else if !enabled && audio_enabled {
                        // Stopping capture is enough to stop the stream; the
                        // capability grant itself is per-session and cleared
                        // on session end. The viewer stops receiving packets
                        // and the app surfaces the disabled state.
                        audio_capture = None; // Drop stops the capture thread.
                        audio_consumer = None;
                        audio_encoder = None;
                        audio_enabled = false;
                        let _ = events.send(SessionEvent::AudioState { session_id, enabled: false, error: None }).await;
                        tracing::info!(session = ?session_id, "screen-share: system audio sharing disabled");
                    }
                }
                // BORU-SS-39: the sharer overrides the quality preset (or
                // restores the path-derived auto preset with None). The
                // chosen ceiling applies immediately — user intent wins over
                // the path-derived preset.
                Some(HostCommand::SetQualityPreset(override_preset)) => {
                    preset = override_preset.unwrap_or_else(|| QualityPreset::for_path(last_path_kind));
                    user_override = override_preset;
                    if apply_preset_override(&mut adaptive, &mut config, encoder.as_mut(), &preset_reference, preset) {
                        tracing::info!(preset = preset.name(), level = adaptive.level(),
                            width = config.width, height = config.height,
                            fps = config.target_fps, bitrate = config.target_bitrate_bps,
                            "screen-share: host applied preset override");
                    } else {
                        tracing::info!(preset = preset.name(), "screen-share: host preset override (no config change)");
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
                                encoder.as_mut(),
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
                        // BORU-SS-33: metadata cursor mode. When the capture
                        // backend delivers the cursor SEPARATELY from the
                        // frame pixels, the host sends shape-on-change +
                        // position-per-move control messages instead of
                        // compositing into the encoded frame.
                        let cursor_meta = frame.cursor.clone();
                        let metadata_mode = cursor_meta.is_some();
                        let geometry = current_source.as_ref().and_then(|source| source.geometry);
                        if let Some(meta) = &cursor_meta {
                            let source_dims = (frame.width, frame.height);
                            let encode_dims = (config.width, config.height);
                            if let Some(message) =
                                cursor_tracker.shape_message(session_id, meta, source_dims, encode_dims)
                            {
                                if control.send(ControlOut::Versioned(message)).await.is_ok() {
                                    cursor_shapes_sent += 1;
                                }
                            }
                            if let Some(message) =
                                cursor_tracker.position_message(session_id, meta, geometry.as_ref(), source_dims)
                            {
                                if control.send(ControlOut::Versioned(message)).await.is_ok() {
                                    cursor_positions_sent += 1;
                                }
                            }
                        }
                        // Unchanged-content skip in metadata mode (BORU-SS-33):
                        // when only the cursor moved, the frame pixels are
                        // identical to the last encoded frame, so re-encoding
                        // wastes CPU/bandwidth for nothing — the viewer keeps
                        // the last decoded frame and re-composites the cursor
                        // at the new position. Never skip while a keyframe is
                        // pending (reconnect/source-switch/recovery) or when
                        // the encoder has no previous frame. In fallback mode
                        // (no cursor metadata) frames are never skipped — the
                        // cursor is composited into the pixels, so an
                        // unchanged frame cannot be detected.
                        let content_unchanged = should_skip_unchanged_frame(
                            metadata_mode,
                            encoder.is_keyframe_pending(),
                            last_encoded_pixels.as_deref(),
                            frame.pixels.as_slice(),
                        );
                        if content_unchanged {
                            skipped_unchanged_frames += 1;
                            if skipped_unchanged_frames == 1 || skipped_unchanged_frames % 150 == 0 {
                                tracing::info!(
                                    skipped_unchanged_frames,
                                    cursor_shapes_sent,
                                    cursor_positions_sent,
                                    "screen-share: metadata cursor mode skipped unchanged frame (cursor-only move)"
                                );
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
                                    let mode = source_mode_for_source(&source);
                                    let message = source_changed_message(session_id, &source, capture_config.target_fps, mode);
                                    control.send(ControlOut::Versioned(message)).await.is_ok()
                                } else {
                                    false
                                };
                                if !announced {
                                    // No tracked source identity (e.g. the
                                    // whole-root fallback): announce the
                                    // geometry change directly. The whole-root
                                    // fallback is `Spanning` (BORU-SS-38).
                                    let _ = control
                                        .send(ControlOut::Versioned(ScreenShareMessage::SourceChanged {
                                            version: SCREEN_SHARE_PROTOCOL_VERSION,
                                            session_id,
                                            source_id: 1,
                                            title: format!("Screen: {}x{}", frame.width, frame.height),
                                            width: geometry.0.min(u16::MAX as u32) as u16,
                                            height: geometry.1.min(u16::MAX as u32) as u16,
                                            frame_rate: capture_config.target_fps.min(u16::MAX as u32) as u16,
                                            source_mode: SourceMode::Spanning,
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
                            let _ = apply_quality_config(encoder.as_mut(), &mut config, decision);
                        }
                        let encode_started = std::time::Instant::now();
                        match encoder.encode(&frame) {
                            Ok(encoded) => {
                                stats.observe_encode(encode_started.elapsed());
                                // Record the encoded pixels for the metadata
                                // cursor-mode unchanged-content skip.
                                last_encoded_pixels = Some(frame.pixels.clone());
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
                                    // BORU-SS-39: detect a Direct↔Relay switch
                                    // on the selected QUIC path and feed it to
                                    // the adaptive controller conservatively.
                                    // The ceiling is recomputed from the new
                                    // path's preset: a lowered ceiling clamps
                                    // immediately (never overshoot a relay); a
                                    // raised ceiling is headroom only and
                                    // recovery stays gradual. A user preset
                                    // override wins over the path-derived
                                    // preset.
                                    let current_path = selected_path_kind(&connection);
                                    if current_path != last_path_kind {
                                        last_path_kind = current_path;
                                        if current_path != PathKind::Unknown {
                                            if user_override.is_none() {
                                                let path_preset = QualityPreset::for_path(current_path);
                                                if path_preset != preset {
                                                    preset = path_preset;
                                                    let mut ceiling = preset_reference;
                                                    path_preset.apply_to_config(&mut ceiling);
                                                    let decision = adaptive.set_ceiling(ceiling);
                                                    if apply_quality_config(encoder.as_mut(), &mut config, decision) {
                                                        tracing::info!(path = ?current_path, preset = preset.name(),
                                                            level = adaptive.level(), bitrate = config.target_bitrate_bps,
                                                            fps = config.target_fps, "screen-share: path change applied quality preset ceiling");
                                                    }
                                                }
                                            } else {
                                                tracing::info!(path = ?current_path, preset = preset.name(), "screen-share: path changed; user preset override retained");
                                            }
                                        }
                                    }
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
                                        codec: encoder.metadata().codec.wire_name().to_string(),
                                        width: config.width,
                                        height: config.height,
                                        fps: config.target_fps,
                                        bitrate_bps: config.target_bitrate_bps as u64,
                                        backend: capture.backend_name().to_string(),
                                        path_kind: last_path_kind,
                                        preset,
                                        adaptive_level: adaptive.level(),
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
                                        cursor_shapes_sent,
                                        cursor_positions_sent,
                                        skipped_unchanged_frames,
                                        "screen-share: performance metrics"
                                    );
                                    if apply_quality_config(encoder.as_mut(), &mut config, decision) {
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
                        tracing::warn!(error = %error, kind = ?error.kind(), "screen-share: capture failed");
                        // PDF Phase 10: monitor unplug / laptop dock-undock.
                        // Recover gracefully instead of ending the session:
                        // re-enumerate and fall back to the first remaining
                        // source, or pause the stream when none remains. The
                        // chat session and the screen-share session both
                        // survive — no crash, no forced end. The failure
                        // kind (MonitorLost vs transient) drives the
                        // recovery decision.
                        if !recover_capture_source(
                            &mut capture,
                            &mut current_source,
                            &capture_config,
                            &mut config,
                            encoder.as_mut(),
                            &mut adaptive,
                            &control,
                            session_id,
                            events,
                            error.kind(),
                        )
                        .await
                        {
                            stream_paused = true;
                            paused_check = std::time::Instant::now();
                        }
                    }
                }
            }
            _ = audio_interval.tick() => {
                // BORU-SS-37: drain one Opus frame (20 ms) per audio tick and
                // send it. try_send (drop-on-full) means a slow control
                // channel drops audio, never blocks the video capture loop.
                if !audio_enabled {
                    continue 'streaming;
                }
                let Some(consumer) = audio_consumer.as_mut() else { continue 'streaming; };
                let Some(encoder) = audio_encoder.as_mut() else { continue 'streaming; };
                let mut frame = vec![0.0f32; AUDIO_SAMPLES_PER_FRAME];
                while consumer.slots() >= AUDIO_SAMPLES_PER_FRAME {
                    let got = consumer.pop_partial_slice(&mut frame).0.len();
                    if got < AUDIO_SAMPLES_PER_FRAME {
                        break;
                    }
                    let Some(packet) = encoder.encode_frame(&frame).ok().flatten() else {
                        continue;
                    };
                    audio_sequence += 1;
                    audio_timestamp_us += AUDIO_FRAME_MS * 1_000;
                    // Audio rides the dedicated AUDIO_KIND stream (BORU-SS-37)
                    // so it never shares a queue with control traffic and the
                    // viewer authorizes it against the Audio grant.
                    let header = AudioHeader {
                        version: SCREEN_SHARE_PROTOCOL_VERSION,
                        session_id: *session_id.as_bytes(),
                        sequence: audio_sequence,
                        timestamp_us: audio_timestamp_us,
                        sample_rate: encoder.sample_rate(),
                        channels: encoder.channels(),
                        payload_len: packet.len() as u32,
                    };
                    if control.try_send(ControlOut::Audio(header, packet)).is_err() {
                        audio_dropped_packets += 1;
                        if audio_dropped_packets == 1 || audio_dropped_packets % 500 == 0 {
                            tracing::warn!(audio_dropped_packets, "screen-share: host dropped audio packets (control queue full)");
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
            Ok(ReadUnit::Control(_)) | Ok(ReadUnit::ScreenShare(_)) | Ok(ReadUnit::Media(_, _)) | Ok(ReadUnit::Audio(_, _)) => {
                let _ = send.reset(0u32.into());
            }
            Err(error) => return Err(error),
        }
    }
}

/// Apply one adaptive-quality decision to the live encoder.
///
/// Resolution/fps/quality-profile changes rebuild the encoder (config
/// generation bump so the viewer re-initialises its decoder); a pure bitrate
/// change uses the cheaper same-resolution rebuild (no generation bump,
/// forced keyframe re-syncs the stream). Returns `true` when a change was
/// applied.
fn apply_quality_config(
    encoder: &mut dyn VideoEncoder,
    config: &mut CodecConfig,
    decision: QualityDecision,
) -> bool {
    if !decision.changed { return false; }
    let next = decision.config;
    if next.width != config.width || next.height != config.height || next.target_fps != config.target_fps
        || next.quality_profile != config.quality_profile
    {
        if encoder.reconfigure(next).is_err() { return false; }
    } else if next.target_bitrate_bps != config.target_bitrate_bps {
        if encoder.reconfigure_bitrate(next.target_bitrate_bps).is_err() { return false; }
    } else {
        return false;
    }
    *config = next;
    true
}

/// Recompute the adaptive ceiling from a preset (relative to the stable
/// preset reference, independent of the current adaptive level) and apply it
/// to the live encoder immediately — the user-override path (BORU-SS-39).
/// Returns `true` when a change was applied.
fn apply_preset_override(
    adaptive: &mut AdaptiveQuality,
    config: &mut CodecConfig,
    encoder: &mut dyn VideoEncoder,
    preset_reference: &CodecConfig,
    preset: QualityPreset,
) -> bool {
    let mut ceiling = *preset_reference;
    preset.apply_to_config(&mut ceiling);
    let decision = adaptive.override_ceiling(ceiling);
    apply_quality_config(encoder, config, decision)
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
/// will actually produce; `mode` carries the source_mode (PDF Phase 14 /
/// BORU-SS-38). Pure helper so the sequencing contract is unit-testable
/// without a live transport.
fn source_changed_message(
    session_id: ScreenShareSessionId,
    source: &CaptureSource,
    target_fps: u32,
    mode: SourceMode,
) -> ScreenShareMessage {
    ScreenShareMessage::SourceChanged {
        version: SCREEN_SHARE_PROTOCOL_VERSION,
        session_id,
        source_id: source.id.0,
        title: source.title.clone(),
        width: (source.width & !1).min(u16::MAX as u32) as u16,
        height: (source.height & !1).min(u16::MAX as u32) as u16,
        frame_rate: target_fps.min(u16::MAX as u32) as u16,
        source_mode: mode,
    }
}

/// The `source_mode` the host should advertise for a source (PDF Phase 14 /
/// BORU-SS-38). A `Desktop` (whole-root/portal) source is `Spanning`; a
/// `Monitor` source is `PerDisplay` (one display at a time, the viewer may
/// request switches); a `Window` source is `Single`. Pure helper so the
/// mapping is unit-testable.
fn source_mode_for_source(source: &CaptureSource) -> SourceMode {
    match source.kind {
        CaptureSourceKind::Desktop => SourceMode::Spanning,
        CaptureSourceKind::Monitor => SourceMode::PerDisplay,
        CaptureSourceKind::Window => SourceMode::Single,
    }
}

/// Build the wire `StreamConfig` message for the given source + live codec
/// config (BORU-SS-38). Sent BEFORE the first video packet of a
/// configuration so the viewer can (re)initialize its decoder with the
/// negotiated geometry AND the `source_mode`. Returns `None` when the
/// geometry cannot be represented (zero/oversized).
fn stream_config_message(
    session_id: ScreenShareSessionId,
    config: &CodecConfig,
    source: &CaptureSource,
    target_fps: u32,
    source_mode: SourceMode,
) -> Result<ScreenShareMessage, ScreenShareError> {
    let width = (source.width & !1).min(u16::MAX as u32) as u16;
    let height = (source.height & !1).min(u16::MAX as u32) as u16;
    if width == 0 || height == 0 {
        return Err(ScreenShareError::new("capture source has no valid geometry"));
    }
    Ok(ScreenShareMessage::StreamConfig {
        version: SCREEN_SHARE_PROTOCOL_VERSION,
        session_id,
        width,
        height,
        frame_rate: target_fps.min(u16::MAX as u32) as u16,
        target_bitrate_bps: config.target_bitrate_bps,
        codec: "h264".to_string(),
        keyframe_interval: config.keyframe_interval.min(u32::MAX as u64) as u32,
        quality_profile: config.quality_profile.as_u8(),
        source_mode,
    })
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
    let mode = source_mode_for_source(source);
    Some((source_changed_message(session_id, source, target_fps, mode), config))
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
    encoder: &mut dyn VideoEncoder,
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
    // 1. Announce the change BEFORE any frame with the new geometry: first
    // the full `StreamConfig` (geometry + bitrate + codec + source_mode),
    // then the `SourceChanged` identity message.
    let mode = source_mode_for_source(source);
    if let Ok(config_message) = stream_config_message(session_id, config, source, capture_config.target_fps, mode) {
        if let Err(error) = control.send(ControlOut::Versioned(config_message)).await {
            tracing::warn!(error = %error, ?source_id, "screen-share: switch source failed (control channel, stream config)");
            return None;
        }
    }
    let message = source_changed_message(session_id, source, capture_config.target_fps, mode);
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
        source_mode: mode,
    }).await;
    tracing::info!(session = ?session_id, ?source_id, title = %source.title, width, height, "screen-share: host switched source");
    Some((width, height))
}

/// What the host should do after a capture failure (PDF Phase 10 /
/// BORU-SS-38 monitor unplug handling). Pure decision so it is
/// Linux-runnable unit-testable without a live backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureRecovery {
    /// The current source is still enumerated: a transient failure. Pause
    /// briefly and let the periodic re-check resume it.
    KeepCurrent,
    /// Switch to this source and continue streaming.
    Fallback(CaptureSource),
    /// No source remains; pause the stream (the session survives).
    Pause,
}

/// Plan the recovery for a capture failure (PDF Phase 10 / BORU-SS-38).
///
/// On a monitor-lost failure (monitor unplug / dock-undock) the current
/// source is gone: fall back to the first remaining source, or pause when
/// none remains. On any OTHER failure the source may still be healthy, so
/// keep it when it is still enumerated (pause briefly) and only fall back /
/// pause when the current source actually disappeared. The session never
/// ends on a capture failure — the caller resumes or pauses, never stalls.
pub fn plan_capture_recovery(
    failure: ScreenShareErrorKind,
    sources: &[CaptureSource],
    current: Option<CaptureSourceId>,
) -> CaptureRecovery {
    let current_still_present = current.is_some_and(|id| sources.iter().any(|s| s.id == id));
    if current_still_present && failure != ScreenShareErrorKind::MonitorLost {
        return CaptureRecovery::KeepCurrent;
    }
    match select_fallback_source(sources, current) {
        Some(source) if !current_still_present || source.id != current.unwrap_or(source.id) => {
            CaptureRecovery::Fallback(source)
        }
        Some(_) => CaptureRecovery::KeepCurrent,
        None => CaptureRecovery::Pause,
    }
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
    encoder: &mut dyn VideoEncoder,
    adaptive: &mut AdaptiveQuality,
    control: &ControlChannel,
    session_id: ScreenShareSessionId,
    events: &mpsc::Sender<SessionEvent>,
    failure: ScreenShareErrorKind,
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
    match plan_capture_recovery(failure, &sources, current_id) {
        CaptureRecovery::KeepCurrent => {
            // The current source is still enumerated: a transient failure.
            // Pause briefly rather than re-arming the failing capture at
            // frame rate; the paused recovery re-checks within a second.
            let reason = if failure == ScreenShareErrorKind::MonitorLost {
                "capture source reported lost; waiting for it to stabilize".into()
            } else {
                "capture failed; pausing until the source is stable".into()
            };
            let _ = events.send(SessionEvent::SourceUnavailable { session_id, reason, fallback: current_source.as_ref().map(|s| s.title.clone()) }).await;
            false
        }
        CaptureRecovery::Fallback(fallback) => {
            if switch_capture_source(capture, fallback.id, capture_config, config, encoder, adaptive, control, session_id, events).await.is_some() {
                *current_source = Some(fallback.clone());
                let _ = events.send(SessionEvent::SourceUnavailable { session_id, reason: "capture source changed after unplug".into(), fallback: Some(fallback.title.clone()) }).await;
                true
            } else {
                false
            }
        }
        CaptureRecovery::Pause => {
            // No source remains — pause the stream (the session survives; a
            // periodic re-enumeration resumes when a monitor re-appears).
            let _ = events.send(SessionEvent::SourceUnavailable { session_id, reason: "no capture source remains (monitor unplugged?)".into(), fallback: None }).await;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen_share::coords::{DesktopPoint, MonitorGeometry};
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

    /// BORU-SS-38: the advertised source mode follows the source kind —
    /// Desktop → Spanning, Monitor → PerDisplay, Window → Single.
    #[test]
    fn source_mode_follows_source_kind() {
        let desktop = CaptureSource {
            id: CaptureSourceId(9),
            kind: CaptureSourceKind::Desktop,
            title: "Entire desktop: 3840x2160".into(),
            width: 3840,
            height: 2160,
            geometry: None,
        };
        let monitor = CaptureSource {
            id: CaptureSourceId(10),
            kind: CaptureSourceKind::Monitor,
            title: "DP-1: 1920x1080".into(),
            width: 1920,
            height: 1080,
            geometry: None,
        };
        let window = CaptureSource {
            id: CaptureSourceId(11),
            kind: CaptureSourceKind::Window,
            title: "Firefox".into(),
            width: 1280,
            height: 800,
            geometry: None,
        };
        assert_eq!(source_mode_for_source(&desktop), SourceMode::Spanning);
        assert_eq!(source_mode_for_source(&monitor), SourceMode::PerDisplay);
        assert_eq!(source_mode_for_source(&window), SourceMode::Single);
    }

    /// BORU-SS-38: the wire `StreamConfig` carries the negotiated geometry
    /// AND the source_mode, and is valid on the wire.
    #[test]
    fn stream_config_message_carries_source_mode_and_geometry() {
        let sid = ScreenShareSessionId::from_bytes([9; 16]);
        let monitor = source(2, "HDMI-A-0", 2560, 1440);
        let config = CodecConfig::default();
        let message = stream_config_message(sid, &config, &monitor, 30, SourceMode::PerDisplay)
            .expect("monitor config must build");
        match &message {
            ScreenShareMessage::StreamConfig { width, height, frame_rate, source_mode, keyframe_interval, quality_profile, .. } => {
                assert_eq!((*width as u32, *height as u32), (2560, 1440));
                assert_eq!(*frame_rate as u32, 30);
                assert_eq!(*source_mode, SourceMode::PerDisplay);
                assert_eq!(*keyframe_interval as u64, config.keyframe_interval);
                assert_eq!(*quality_profile, config.quality_profile.as_u8());
            }
            other => panic!("expected StreamConfig, got {other:?}"),
        }
        // Wire-valid: passes its own validation.
        let bytes = message.encode().expect("stream config must encode");
        assert_eq!(ScreenShareMessage::decode(&bytes).unwrap(), message);
        // A source with no capturable geometry is refused.
        let degenerate = CaptureSource {
            id: CaptureSourceId(3),
            kind: CaptureSourceKind::Monitor,
            title: "Tiny".into(),
            width: 1,
            height: 1,
            geometry: None,
        };
        assert!(stream_config_message(sid, &config, &degenerate, 30, SourceMode::Single).is_err());
    }

    /// BORU-SS-38 unplug handling: a MonitorLost failure with a remaining
    /// source falls back to the first remaining monitor (never ends the
    /// session).
    #[test]
    fn monitor_lost_falls_back_to_next_available_source() {
        // Monitor 2 was unplugged: it is NO LONGER in the enumeration, only
        // monitor 1 remains.
        let sources = vec![source(1, "DP-1", 1920, 1080)];
        let recovery = plan_capture_recovery(ScreenShareErrorKind::MonitorLost, &sources, Some(CaptureSourceId(2)));
        match recovery {
            CaptureRecovery::Fallback(source) => assert_eq!(source.id, CaptureSourceId(1)),
            other => panic!("expected fallback, got {other:?}"),
        }
        // No source remains: pause (the session survives).
        let none_left = plan_capture_recovery(ScreenShareErrorKind::MonitorLost, &[], Some(CaptureSourceId(2)));
        assert_eq!(none_left, CaptureRecovery::Pause);
    }

    /// BORU-SS-38 unplug handling: a transient (non-MonitorLost) failure with
    /// the current source still enumerated keeps it — the caller pauses
    /// briefly instead of needlessly switching.
    #[test]
    fn transient_failure_keeps_current_source_when_still_enumerated() {
        let sources = vec![source(1, "DP-1", 1920, 1080), source(2, "HDMI-A-0", 1280, 720)];
        let recovery = plan_capture_recovery(ScreenShareErrorKind::Generic, &sources, Some(CaptureSourceId(2)));
        assert_eq!(recovery, CaptureRecovery::KeepCurrent);
        // Even a MonitorLost classification with the source still enumerated
        // keeps it (the classification may be stale after a re-enumeration).
        let stale = plan_capture_recovery(ScreenShareErrorKind::MonitorLost, &sources, Some(CaptureSourceId(2)));
        assert_eq!(stale, CaptureRecovery::KeepCurrent);
    }

    /// BORU-SS-38 unplug handling: a generic failure whose current source
    /// disappeared still falls back to the first remaining source.
    #[test]
    fn generic_failure_with_missing_source_still_falls_back() {
        let sources = vec![source(1, "DP-1", 1920, 1080)];
        let recovery = plan_capture_recovery(ScreenShareErrorKind::Generic, &sources, Some(CaptureSourceId(99)));
        match recovery {
            CaptureRecovery::Fallback(source) => assert_eq!(source.id, CaptureSourceId(1)),
            other => panic!("expected fallback, got {other:?}"),
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
        let message = source_changed_message(sid, &odd, 15, SourceMode::PerDisplay);
        match &message {
            ScreenShareMessage::SourceChanged { width, height, source_mode, .. } => {
                assert_eq!((*width, *height), (1918, 1078));
                assert_eq!(*source_mode, SourceMode::PerDisplay);
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

    /// BORU-SS-33: the cursor tracker emits a shape message exactly once per
    /// sprite change (never per frame) and a position message per move.
    #[test]
    fn cursor_tracker_emits_shape_once_and_position_per_move() {
        let sid = ScreenShareSessionId::from_bytes([7; 16]);
        let sprite = CursorSprite::new(32, 32, 16, 16, vec![255u8; 32 * 32 * 4]).unwrap();
        let mut tracker = CursorTracker::default();
        let meta = CursorMeta::with_sprite(DesktopPoint { x: 10, y: 10 }, true, sprite.clone());
        let shape = tracker.shape_message(sid, &meta, (1920, 1080), (640, 360));
        assert!(shape.is_some(), "first shape must be emitted");
        let shape2 = tracker.shape_message(sid, &meta, (1920, 1080), (640, 360));
        assert!(shape2.is_none(), "identical shape must not re-send");
        let moved = CursorMeta::with_sprite(
            DesktopPoint { x: 20, y: 10 },
            true,
            CursorSprite::new(24, 24, 12, 12, vec![128u8; 24 * 24 * 4]).unwrap(),
        );
        let shape3 = tracker.shape_message(sid, &moved, (1920, 1080), (640, 360));
        assert!(shape3.is_some(), "changed sprite must re-send");
        // A position-only update (no sprite) still sends the move.
        let position_only = CursorMeta::position(DesktopPoint { x: 30, y: 10 }, true);
        let pos = tracker.position_message(sid, &position_only, None, (1920, 1080));
        assert!(pos.is_some(), "first position must be emitted");
        let pos2 = tracker.position_message(sid, &position_only, None, (1920, 1080));
        assert!(pos2.is_none(), "duplicate position must not re-send");
        let hidden = CursorMeta::position(DesktopPoint { x: 30, y: 10 }, false);
        let pos3 = tracker.position_message(sid, &hidden, None, (1920, 1080));
        assert!(pos3.is_some(), "visibility change must re-send position");
    }

    /// BORU-SS-33: the cursor tracker normalizes a desktop position against
    /// the source geometry when provided, and treats it as source-relative
    /// when no geometry exists (portal stream coordinates).
    #[test]
    fn cursor_tracker_normalizes_position_with_and_without_geometry() {
        let sid = ScreenShareSessionId::from_bytes([8; 16]);
        let mut tracker = CursorTracker::default();
        let geometry = MonitorGeometry::new(-1920, 0, 1920, 1080);
        // Desktop (-960, 540) is source-relative (960, 540) → 0.5, 0.5.
        let meta = CursorMeta::position(DesktopPoint { x: -960, y: 540 }, true);
        let Some(ScreenShareMessage::CursorPosition { x, y, visible, .. }) =
            tracker.position_message(sid, &meta, Some(&geometry), (1920, 1080))
        else {
            panic!("position must be emitted");
        };
        assert!((x - 0.5).abs() < 1e-4, "x = {x}");
        assert!((y - 0.5).abs() < 1e-4, "y = {y}");
        assert!(visible);
        // Without geometry: position treated as source-relative pixels.
        let mut no_geom = CursorTracker::default();
        let meta2 = CursorMeta::position(DesktopPoint { x: 960, y: 540 }, true);
        let Some(ScreenShareMessage::CursorPosition { x, y, .. }) =
            no_geom.position_message(sid, &meta2, None, (1920, 1080))
        else {
            panic!("position must be emitted");
        };
        assert!((x - 0.5).abs() < 1e-4, "x = {x}");
        assert!((y - 0.5).abs() < 1e-4, "y = {y}");
    }

    /// BORU-SS-33: a shape message carries the sprite scaled to the encode
    /// resolution (so the viewer composites 1:1 into the decoded frame).
    #[test]
    fn cursor_tracker_shape_scales_sprite_to_encode_dims() {
        let sid = ScreenShareSessionId::from_bytes([9; 16]);
        let sprite = CursorSprite::new(32, 32, 16, 16, vec![255u8; 32 * 32 * 4]).unwrap();
        let mut tracker = CursorTracker::default();
        let meta = CursorMeta::with_sprite(DesktopPoint { x: 0, y: 0 }, true, sprite);
        let Some(ScreenShareMessage::CursorShape { width, height, hotspot_x, hotspot_y, pixels, .. }) =
            tracker.shape_message(sid, &meta, (1920, 1080), (640, 360))
        else {
            panic!("shape must be emitted");
        };
        assert_eq!(width, 11);
        assert_eq!(height, 11);
        assert_eq!(hotspot_x, 5);
        assert_eq!(hotspot_y, 5);
        assert_eq!(pixels.len(), 11 * 11 * 4);
        // Wire-valid: the message passes its own validation.
        let message = ScreenShareMessage::CursorShape {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid,
            shape_id: 1,
            width,
            height,
            hotspot_x,
            hotspot_y,
            pixels,
        };
        message.validate().expect("scaled shape must validate");
    }

    /// BORU-SS-33 fallback behaviour: when the capture backend does NOT
    /// deliver cursor metadata (composited-cursor fallback), the unchanged-
    /// content skip is never taken — an unchanged frame is still encoded,
    /// preserving the BORU-SS-12 composited behaviour exactly.
    #[test]
    fn unchanged_frame_never_skipped_without_cursor_metadata() {
        let pixels: &[u8] = &[1, 2, 3, 4];
        // No metadata → never skip, even with identical pixels and no
        // keyframe pending.
        assert!(!should_skip_unchanged_frame(false, false, Some(pixels), pixels));
        // With metadata + identical pixels + no keyframe → skip.
        assert!(should_skip_unchanged_frame(true, false, Some(pixels), pixels));
        // With metadata but a pending keyframe → never skip (recovery frame
        // must be delivered even when pixels are unchanged).
        assert!(!should_skip_unchanged_frame(true, true, Some(pixels), pixels));
        // With metadata but no previous encoded frame → never skip (first
        // frame must be encoded).
        assert!(!should_skip_unchanged_frame(true, false, None, pixels));
        // With metadata but CHANGED pixels → never skip (real content).
        assert!(!should_skip_unchanged_frame(true, false, Some(&[9, 9, 9, 9]), pixels));
    }
}
