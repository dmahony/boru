//! Calls & screen-share domain (BORU-APP-008).
//!
//! Owns the calls/screen-share application responsibilities moved out of the
//! monolithic IcedChat shell (app.rs), following the BORU-ARCH-04 domain
//! pattern (DomainState + DomainMessage + update() + view()).
//!
//! - [`CallsState`] owns the call lifecycle UI state (active/outgoing/incoming
//!   calls, video frames, camera selection, call timers) and the screen-share
//!   host/viewer session state (the 44 `screen_share_*` fields).
//! - [`CallsMessage`] covers state-only transitions; heavier arms that need
//!   shell context (call actor, protocol handle, notifications, runtime)
//!   stay as `AppMessage` variants dispatched to [`IcedChat::update_calls`] /
//!   [`IcedChat::update_screen_share`], reading/writing `self.calls_state.*`.
//! - View builders (`view_outgoing_call`, `view_active_call`) render the call
//!   screens from the domain state; the screen-share panels remain in
//!   `app/chat.rs` and `app/screen_share_surface.rs` (view layer).
//!
//! `IcedChat` holds exactly one `calls_state: CallsState`; there is no mirror
//! of this state anywhere else (PDF §14 "same state in both modules" stop
//! condition).
use super::*;

// ── Domain types (moved from app.rs, BORU-APP-008) ──────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutgoingCallStatus {
    Ringing,
    Declined,
    Busy,
    Failed,
}

/// User-facing call failure text; detailed variants remain diagnostic-only.
fn friendly_call_error(error: &boru_core::call::manager::CallError) -> &'static str {
    use boru_core::call::manager::CallError;
    match error {
        CallError::Rejected => "Call declined",
        CallError::Busy => "User is busy",
        CallError::Connection | CallError::Unauthorized | CallError::Authorization => {
            "Could not reach user"
        }
        CallError::Device => "Microphone unavailable",
        CallError::Protocol | CallError::NegotiationTimeout => "No compatible audio codec",
    }
}

fn friendly_call_end(reason: &boru_core::call::manager::CallEndReason) -> &'static str {
    use boru_core::call::manager::CallEndReason;
    match reason {
        CallEndReason::ConnectionLost => "Network connection lost",
        CallEndReason::DeviceError => "Microphone unavailable",
        _ => "Call ended",
    }
}

fn friendly_call_error_text(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("busy") {
        "User is busy"
    } else if lower.contains("reject") || lower.contains("declin") {
        "Call declined"
    } else if lower.contains("microphone") || lower.contains("audio device") {
        "Microphone unavailable"
    } else if lower.contains("camera") && lower.contains("permission") {
        "Camera permission denied"
    } else if lower.contains("camera") {
        "Camera unavailable"
    } else if lower.contains("codec") && lower.contains("video") {
        "No compatible video codec"
    } else if lower.contains("codec") {
        "No compatible audio codec"
    } else {
        "Could not reach user"
    }
}

/// Rendered as a modal overlay; the microphone/camera are NOT activated
/// until the user presses Accept (which maps to `AcceptIncomingCall`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IncomingCall {
    pub(crate) call_id: CallId,
    pub(crate) peer: PublicKey,
    pub(crate) kind: CallKind,
}

#[cfg(feature = "screen-sharing")]
/// Host-side lifecycle of a locally initiated screen-share session.
///
/// Maps 1:1 to the seven states Phase 13 of the screen-share UX spec asks
/// the sharer UI to display: requesting, awaiting acceptance, sharing,
/// paused, reconnecting, stopped, error. `Idle` is the no-session baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScreenShareHostState {
    /// No local sharing session.
    Idle,
    /// The user clicked Share and the host driver is starting (dialing /
    /// enumerating sources). Nothing is being captured or streamed yet.
    Requesting,
    /// Hello/offer sent; waiting for the viewer's explicit Accept/Reject.
    Inviting,
    /// Viewer accepted; capture and streaming are active.
    Streaming,
    /// The stream is paused because no capture source is available (monitor
    /// unplug / dock-undock with no fallback). The session survives; picking
    /// a source resumes it.
    Paused,
    /// The media path failed transiently and is being re-established.
    /// The chat/friend session survives; only the media stream reconnects.
    Reconnecting,
    /// Terminal: the share stopped (user clicked Stop, the viewer ended the
    /// session, or the peer declined). Clears to `Idle` when the user
    /// dismisses the notice or starts a new share.
    Stopped,
    /// Terminal: the share failed with a user-safe reason. Clears to `Idle`
    /// when the user dismisses the notice or retries.
    Error(String),
}

// ── Domain state (BORU-ARCH-04 pattern) ─────────────────────────

/// Call + screen-share domain state (BORU-APP-008).
///
/// Moved verbatim from `IcedChat` (app.rs) so the calls/screen-share domain
/// owns its state. Field defaults match the old constructor initializers.
#[derive(Debug)]
pub(crate) struct CallsState {
    /// Active call id while a call is in progress (ringing or connected).
    pub(crate) active_call_id: Option<CallId>,
    /// Peer of the current outgoing/incoming call (used for the ringing /
    /// active screens and call history).
    pub(crate) outgoing_call_peer: Option<PublicKey>,
    /// Ringing/declined/busy/failed status shown on the outgoing-call screen.
    pub(crate) outgoing_call_status: Option<OutgoingCallStatus>,
    /// Whether the local microphone is muted during the active call.
    pub(crate) call_audio_muted: bool,
    /// Whether the local camera is enabled during the active call.
    pub(crate) call_camera_enabled: bool,
    /// Selected camera label shown by the call controls. The media actor owns
    /// capture; keeping the UI selection here avoids pretending that a device
    /// switch is complete before the actor acknowledges it.
    pub(crate) call_camera_selection: String,
    #[cfg(feature = "video-calls")]
    pub(crate) latest_remote_frame: Option<VideoFrame>,
    #[cfg(feature = "video-calls")]
    pub(crate) latest_local_frame: Option<VideoFrame>,
    /// Monotonic start time used for the in-call duration display.
    pub(crate) call_started_at: Option<std::time::Instant>,
    /// Kind and origin of the current call, retained until its terminal event
    /// so local history can distinguish active, missed, and declined calls.
    pub(crate) call_kind: Option<CallKind>,
    pub(crate) call_was_incoming: bool,
    pub(crate) call_declined: bool,
    /// Pending incoming call shown as an overlay; media is only activated
    /// when the user explicitly accepts.
    pub(crate) incoming_call: Option<IncomingCall>,
    #[cfg(feature = "screen-sharing")]
    /// Receiver for inbound screen-share session events (set by main.rs).
    pub(crate) screen_share_events_rx: Option<Arc<Mutex<Receiver<SessionEvent>>>>,
    #[cfg(feature = "screen-sharing")]
    /// Receiver for inbound screen-share media units (set by main.rs).
    pub(crate) screen_share_media_rx: Option<Arc<Mutex<Receiver<InboundMedia>>>>,
    #[cfg(feature = "screen-sharing")]
    /// Receiver for inbound screen-share audio units (set by main.rs).
    /// Audio is only delivered after the host grants `Capability::Audio`.
    pub(crate) screen_share_audio_rx: Option<Arc<Mutex<Receiver<InboundAudio>>>>,
    #[cfg(feature = "screen-sharing")]
    /// Stop flag for the viewer audio playback worker (BORU-SS-37).
    pub(crate) screen_share_audio_stop: Option<Arc<AtomicBool>>,
    #[cfg(feature = "screen-sharing")]
    /// System-audio sharing active on either side (BORU-SS-37). Driven by
    /// `SessionEvent::ControlChanged` (viewer) and `SessionEvent::AudioState`
    /// (host); audio is a separate optional capability, never implied by
    /// remote control.
    pub(crate) screen_share_audio_active: bool,
    #[cfg(feature = "screen-sharing")]
    /// BORU-SSUI-06: typed, user-safe reason from the last
    /// `SessionEvent::AudioState` when system audio could not be shared
    /// (e.g. no PipeWire runtime). `Some` disables the sender audio toggle
    /// and surfaces the reason as tooltip/status text; `None` means the
    /// toggle is usable. Presentation mirror of the authoritative
    /// AudioState.error — never duplicated inside the control itself.
    pub(crate) screen_share_audio_error: Option<String>,
    #[cfg(feature = "screen-sharing")]
    /// Sender the host session task uses to emit session events (set by main.rs).
    pub(crate) screen_share_events_tx: Option<tokio::sync::mpsc::Sender<SessionEvent>>,
    #[cfg(feature = "screen-sharing")]
    /// Protocol handle used to respond to invitations on the inbound connection.
    pub(crate) screen_share_protocol: Option<ScreenShareProtocol>,
    #[cfg(feature = "screen-sharing")]
    /// Watch receiver delivering the latest decoded frame to the viewer panel.
    pub(crate) screen_share_frame_watch:
        Option<Arc<Mutex<tokio::sync::watch::Receiver<Option<CapturedFrame>>>>>,
    #[cfg(feature = "screen-sharing")]
    /// Watch receiver delivering periodic viewer pipeline stats (~1 Hz) for
    /// the developer diagnostics overlay (PDF Phase 12).
    pub(crate) screen_share_stats_watch:
        Option<Arc<Mutex<tokio::sync::watch::Receiver<Option<ScreenShareStatsSnapshot>>>>>,
    #[cfg(feature = "screen-sharing")]
    /// Latest viewer-side pipeline stats for the diagnostics overlay.
    pub(crate) screen_share_viewer_stats: Option<ScreenShareStatsSnapshot>,
    #[cfg(feature = "screen-sharing")]
    /// Latest host-side session metrics (config + pipeline snapshot) for the
    /// diagnostics overlay, published by the host streaming loop.
    pub(crate) screen_share_host_metrics: Option<ScreenShareSessionMetrics>,
    #[cfg(feature = "screen-sharing")]
    /// Developer diagnostics overlay on the screen-share surface. Mirrors the
    /// `--dev-ui` / `BORU_DEV_UI=1` / `dev-ui`-feature gate wired in main.rs
    /// (PDF Phase 12: overlay behind a debug flag).
    pub(crate) screen_share_dev_overlay: bool,
    #[cfg(feature = "screen-sharing")]
    /// Host-side sharing state; drives the persistent indicator.
    pub(crate) screen_share_host_state: ScreenShareHostState,
    #[cfg(feature = "screen-sharing")]
    /// Stop flag for the running host session task.
    pub(crate) screen_share_host_stop: Option<Arc<AtomicBool>>,
    #[cfg(feature = "screen-sharing")]
    /// Pending invitation (inviter label + session id); no media is accepted
    /// until the user explicitly accepts.
    pub(crate) screen_share_invite: Option<(String, ScreenShareSessionId)>,
    #[cfg(feature = "screen-sharing")]
    /// Viewer is actively rendering an accepted session.
    pub(crate) screen_share_viewing: bool,
    #[cfg(feature = "screen-sharing")]
    /// Session currently being viewed.
    pub(crate) screen_share_view_session: Option<ScreenShareSessionId>,
    #[cfg(feature = "screen-sharing")]
    /// Stop flag for the viewer decode worker.
    pub(crate) screen_share_decode_stop: Option<Arc<AtomicBool>>,
    #[cfg(feature = "screen-sharing")]
    /// Viewer presentation mode.
    pub(crate) screen_share_fullscreen: bool,
    #[cfg(feature = "screen-sharing")]
    /// Timestamp of the frame currently held in `screen_share_frame_handle`.
    pub(crate) screen_share_last_frame_ts: Option<u64>,
    #[cfg(feature = "screen-sharing")]
    /// Rendered handle of the latest decoded frame (RGBA).
    pub(crate) screen_share_frame_handle: Option<iced::widget::image::Handle>,
    #[cfg(feature = "screen-sharing")]
    /// Latest cursor sprite received via `CursorShape` (BORU-SS-33). `None`
    /// until the host sends a shape; the viewer composites it over decoded
    /// frames at the latest position when `screen_share_cursor_visible` and
    /// `screen_share_cursor_enabled` are true.
    pub(crate) screen_share_cursor_sprite: Option<CursorSprite>,
    #[cfg(feature = "screen-sharing")]
    /// Latest normalized cursor position received via `CursorPosition`
    /// (BORU-SS-33); `None` until the host reports a position.
    pub(crate) screen_share_cursor_pos: Option<(f32, f32)>,
    #[cfg(feature = "screen-sharing")]
    /// Whether the host reports the remote cursor as visible.
    pub(crate) screen_share_cursor_visible: bool,
    #[cfg(feature = "screen-sharing")]
    /// Viewer toggle (CUR-1): show/hide the remote cursor overlay.
    pub(crate) screen_share_cursor_enabled: bool,
    #[cfg(feature = "screen-sharing")]
    /// Cached raw RGBA pixels of the latest decoded frame, so a cursor
    /// shape/position/toggle update can re-composite WITHOUT waiting for a
    /// new video frame (BORU-SS-33: cursor moves must not force re-encode).
    /// `(width, height, pixels)`.
    pub(crate) screen_share_cursor_frame_rgba: Option<(u32, u32, Vec<u8>)>,
    #[cfg(feature = "screen-sharing")]
    /// Host-side pending control request (session id, viewer label, caps).
    pub(crate) screen_share_control_request:
        Option<(ScreenShareSessionId, String, Vec<Capability>)>,
    #[cfg(feature = "screen-sharing")]
    /// Control active on either side (drives the indicator + input capture).
    pub(crate) screen_share_control_active: bool,
    #[cfg(feature = "screen-sharing")]
    /// Clipboard sync granted on either side (PDF Task 9.3 / BORU-SS-25).
    /// Set from the capabilities in `ControlChanged`; clipboard is a separate
    /// optional capability, never implied by remote control.
    pub(crate) screen_share_clipboard_active: bool,
    #[cfg(feature = "screen-sharing")]
    /// Command sender into the host driver task (grants/revokes).
    pub(crate) screen_share_host_cmd_tx: Option<tokio::sync::mpsc::Sender<HostCommand>>,
    #[cfg(feature = "screen-sharing")]
    /// Last pointer-move send time (throttles per-event QUIC streams).
    pub(crate) screen_share_last_pointer_sent: Option<Instant>,
    #[cfg(feature = "screen-sharing")]
    /// Last pointer position sent, to skip near-identical moves.
    pub(crate) screen_share_last_pointer_pos: Option<(f32, f32)>,
    #[cfg(feature = "screen-sharing")]
    /// Current held-modifier bitmask (PDF Task 9.2), attached to every input
    /// message and updated by modifier keysyms from the keyboard subscription.
    pub(crate) screen_share_modifiers: u32,
    #[cfg(feature = "screen-sharing")]
    /// Viewer surface presentation mode (fit / actual / explicit zoom).
    pub(crate) screen_share_view_mode: ScreenShareViewMode,
    #[cfg(feature = "screen-sharing")]
    /// Pan center in source pixels (`None` = source center).
    pub(crate) screen_share_pan: Option<(f32, f32)>,
    #[cfg(feature = "screen-sharing")]
    /// Active pan drag: last pointer position over the surface.
    pub(crate) screen_share_drag: Option<iced::Point>,
    #[cfg(feature = "screen-sharing")]
    /// Last hover position over the surface (wheel-zoom anchor).
    pub(crate) screen_share_hover: Option<iced::Point>,
    #[cfg(feature = "screen-sharing")]
    /// Size of the last decoded frame (`width`, `height`), for the surface
    /// geometry. Set from `CapturedFrame` when a new frame arrives.
    pub(crate) screen_share_src_size: Option<(u32, u32)>,
    #[cfg(feature = "screen-sharing")]
    /// Monitors available to the host, captured before the share starts
    /// (PDF Phase 10: "enumerate available monitors before starting a
    /// share"). Populated by `SessionEvent::SourcesEnumerated`; the monitor
    /// switching UX (BORU-SS-29) presents this list to the sharer.
    pub(crate) screen_share_sources: Option<Vec<CaptureSource>>,
    #[cfg(feature = "screen-sharing")]
    /// The capture source currently selected by the sharer (host side).
    /// Defaults to the first enumerated source; updated when the user picks
    /// a source from the picker or when `SourceChanged` arrives.
    pub(crate) screen_share_selected_source: Option<CaptureSourceId>,
    #[cfg(feature = "screen-sharing")]
    /// BORU-SSUI-04: the user's chosen quality preset (None = Auto /
    /// path-derived auto preset). Presentation-only mirror of the
    /// `ScreenShareSetPreset` dispatch; the host's effective preset is
    /// authoritative and reported via `screen_share_host_metrics`.
    pub(crate) screen_share_selected_preset: Option<QualityPreset>,
    #[cfg(feature = "screen-sharing")]
    /// Who the local viewer is watching (short public key from the invite),
    /// shown while `screen_share_viewing` so the viewer always knows who is
    /// sharing (PDF Phase 13: "show who is sharing").
    pub(crate) screen_share_viewing_peer: Option<String>,
    #[cfg(feature = "screen-sharing")]
    /// Ticks spent in a terminal notice state (`Stopped` / `Error`); the
    /// notice auto-clears to `Idle` after `SCREEN_SHARE_NOTICE_TICKS`
    /// 1-second ConnMonitorTicks so a stale status never blocks a restart.
    pub(crate) screen_share_notice_ticks: u8,
}

/// State-only transitions for the calls/screen-share domain (BORU-APP-008).
///
/// Routed through [`CallsState::update`] from the shell's `update_calls` /
/// `update_screen_share` dispatch. Arms that need shell context (call actor,
/// protocol handle, host command sender, notifications, runtime) remain
/// `AppMessage` variants handled inline in `IcedChat::update_calls` /
/// `update_screen_share`, reading/writing `self.calls_state.<field>`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CallsMessage {
    /// User selected a camera label ("next" cycles the two supported labels;
    /// explicit labels from the settings picker are stored verbatim).
    SelectCamera(String),
    /// Microphone selection (no-op today; kept for the settings surface).
    SelectMicrophone(String),
    /// Speaker selection (no-op today; kept for the settings surface).
    SelectSpeaker(String),
    /// 1 Hz tick while a call screen is open (currently no-op).
    CallUiTick,
    #[cfg(feature = "screen-sharing")]
    /// Toggle the native viewer between inline and fullscreen presentation.
    ToggleScreenShareFullscreen,
    #[cfg(feature = "screen-sharing")]
    /// Set the viewer surface presentation mode / pan center.
    ScreenShareSetView {
        mode: ScreenShareViewMode,
        pan: Option<(f32, f32)>,
    },
    #[cfg(feature = "screen-sharing")]
    /// Begin a pan drag at the given surface position.
    ScreenSharePanStart { pos: iced::Point },
    #[cfg(feature = "screen-sharing")]
    /// Continue a pan drag, optionally with an explicit zoom scale.
    ScreenSharePanMove { pos: iced::Point, scale: f32 },
    #[cfg(feature = "screen-sharing")]
    /// End the pan drag.
    ScreenSharePanEnd,
    #[cfg(feature = "screen-sharing")]
    /// Dismiss the terminal screen-share notice (Stopped/Error).
    ScreenShareDismissNotice,
}

impl CallsState {
    /// Create the calls/screen-share domain state with the same defaults the
    /// inline `app.rs` fields used.
    pub(crate) fn new() -> Self {
        Self {
            active_call_id: None,
            outgoing_call_peer: None,
            outgoing_call_status: None,
            call_audio_muted: false,
            call_camera_enabled: false,
            call_camera_selection: "Front camera".to_string(),
            #[cfg(feature = "video-calls")]
            latest_remote_frame: None,
            #[cfg(feature = "video-calls")]
            latest_local_frame: None,
            call_started_at: None,
            call_kind: None,
            call_was_incoming: false,
            call_declined: false,
            incoming_call: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_events_rx: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_media_rx: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_audio_rx: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_audio_stop: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_audio_active: false,
            #[cfg(feature = "screen-sharing")]
            screen_share_audio_error: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_events_tx: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_protocol: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_frame_watch: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_stats_watch: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_viewer_stats: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_host_metrics: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_dev_overlay: false,
            #[cfg(feature = "screen-sharing")]
            screen_share_host_state: ScreenShareHostState::Idle,
            #[cfg(feature = "screen-sharing")]
            screen_share_host_stop: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_invite: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_viewing: false,
            #[cfg(feature = "screen-sharing")]
            screen_share_view_session: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_decode_stop: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_fullscreen: false,
            #[cfg(feature = "screen-sharing")]
            screen_share_last_frame_ts: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_frame_handle: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_cursor_sprite: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_cursor_pos: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_cursor_visible: false,
            #[cfg(feature = "screen-sharing")]
            screen_share_cursor_enabled: true,
            #[cfg(feature = "screen-sharing")]
            screen_share_cursor_frame_rgba: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_control_request: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_control_active: false,
            #[cfg(feature = "screen-sharing")]
            screen_share_clipboard_active: false,
            #[cfg(feature = "screen-sharing")]
            screen_share_host_cmd_tx: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_last_pointer_sent: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_last_pointer_pos: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_modifiers: 0,
            #[cfg(feature = "screen-sharing")]
            screen_share_view_mode: ScreenShareViewMode::default(),
            #[cfg(feature = "screen-sharing")]
            screen_share_pan: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_drag: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_hover: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_src_size: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_sources: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_selected_source: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_selected_preset: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_viewing_peer: None,
            #[cfg(feature = "screen-sharing")]
            screen_share_notice_ticks: 0,
        }
    }

    /// Apply one domain message (state-only transitions).
    ///
    /// Only this domain's state is mutated. No shell side effect is required
    /// for any current message, so nothing is returned; the shell just routes
    /// the matching `AppMessage` variant here and returns `Task::none()`.
    pub(crate) fn update(&mut self, msg: CallsMessage) {
        match msg {
            CallsMessage::SelectCamera(selection) => {
                self.call_camera_selection = if selection == "next" {
                    if self.call_camera_selection == "Front camera" {
                        "Back camera".to_string()
                    } else {
                        "Front camera".to_string()
                    }
                } else {
                    selection
                };
            }
            CallsMessage::SelectMicrophone(_)
            | CallsMessage::SelectSpeaker(_)
            | CallsMessage::CallUiTick => {}
            #[cfg(feature = "screen-sharing")]
            CallsMessage::ToggleScreenShareFullscreen => {
                self.screen_share_fullscreen = !self.screen_share_fullscreen;
            }
            #[cfg(feature = "screen-sharing")]
            CallsMessage::ScreenShareSetView { mode, pan } => {
                self.screen_share_view_mode = mode;
                // Fit/Actual reset the pan to the source center (None); an
                // explicit pan (wheel zoom anchor) is preserved as given.
                self.screen_share_pan =
                    if matches!(mode, ScreenShareViewMode::Fit | ScreenShareViewMode::Actual) {
                        None
                    } else {
                        pan.or(self.screen_share_pan)
                    };
            }
            #[cfg(feature = "screen-sharing")]
            CallsMessage::ScreenSharePanStart { pos } => {
                self.screen_share_drag = Some(pos);
                self.screen_share_hover = Some(pos);
            }
            #[cfg(feature = "screen-sharing")]
            CallsMessage::ScreenSharePanMove { pos, scale } => {
                self.screen_share_hover = Some(pos);
                if let Some(start) = self.screen_share_drag {
                    let (dx, dy) = (pos.x - start.x, pos.y - start.y);
                    // Dragging content: the visible region follows the cursor.
                    let (cx, cy) = self.screen_share_pan.unwrap_or_else(|| {
                        self.screen_share_src_size
                            .map(|(w, h)| (w as f32 / 2.0, h as f32 / 2.0))
                            .unwrap_or((0.0, 0.0))
                    });
                    let scale = if scale > 0.0 { scale } else { 1.0 };
                    let src = self
                        .screen_share_src_size
                        .map(|(w, h)| (w as f32, h as f32))
                        .unwrap_or((0.0, 0.0));
                    self.screen_share_pan = Some((
                        (cx - dx / scale).clamp(0.0, src.0),
                        (cy - dy / scale).clamp(0.0, src.1),
                    ));
                    self.screen_share_drag = Some(pos);
                }
            }
            #[cfg(feature = "screen-sharing")]
            CallsMessage::ScreenSharePanEnd => {
                self.screen_share_drag = None;
            }
            #[cfg(feature = "screen-sharing")]
            CallsMessage::ScreenShareDismissNotice => {
                self.screen_share_host_state = ScreenShareHostState::Idle;
                self.screen_share_notice_ticks = 0;
            }
        }
    }
}

impl IcedChat {
    pub(crate) fn view_outgoing_call(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, text};
        use iced::{Alignment, Length};

        let peer = self.calls_state.outgoing_call_peer;
        let name = peer
            .as_ref()
            .map(|key| self.resolve_name(key))
            .unwrap_or_else(|| crate::i18n::t("calls.unknown_contact"));
        let status = match self.calls_state.outgoing_call_status {
            Some(OutgoingCallStatus::Ringing) => crate::i18n::t("calls.ringing"),
            Some(OutgoingCallStatus::Declined) => crate::i18n::t("calls.declined"),
            Some(OutgoingCallStatus::Busy) => crate::i18n::t("calls.busy"),
            Some(OutgoingCallStatus::Failed) => crate::i18n::t("calls.failed"),
            None => crate::i18n::t("calls.outgoing"),
        };
        let initials = crate::presentation::initials(&name);
        let avatar_label = if initials.is_empty() {
            "?".to_string()
        } else {
            initials
        };
        let calls = crate::theme::BoruTheme::default().calls;
        let typography = crate::theme::BoruTheme::default().typography;
        let avatar = container(text(avatar_label).size(typography.call_avatar_glyph))
            .width(Length::Fixed(calls.avatar_size))
            .height(Length::Fixed(calls.avatar_size))
            .center_x(Length::Fixed(calls.avatar_size))
            .center_y(Length::Fixed(calls.avatar_size))
            .style(|theme| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg_surface_secondary(theme))),
                border: iced::Border {
                    radius: crate::theme::BoruTheme::for_theme(theme)
                        .radii
                        .call_avatar
                        .into(),
                    ..Default::default()
                },
                ..Default::default()
            });
        let controls: iced::Element<'_, AppMessage> = match self.calls_state.active_call_id {
            Some(call_id) => button(text(crate::i18n::t("common.cancel")))
                .on_press(AppMessage::HangUp(call_id))
                .padding([SPACE_8, SPACE_24])
                .style(BUTTON_DANGER)
                .into(),
            None => iced::widget::Space::new()
                .height(Length::Fixed(calls.controls_gap))
                .into(),
        };
        container(
            column![
                avatar,
                text(name).size(typography.call_name),
                text(status).size(typography.call_status),
                controls
            ]
            .spacing(SPACE_16)
            .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
    }

    pub(crate) fn view_active_call(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, row, text};
        use iced::{Alignment, Length};

        let name = self
            .calls_state
            .outgoing_call_peer
            .as_ref()
            .map(|peer| self.resolve_name(peer))
            .unwrap_or_else(|| crate::i18n::t("calls.unknown_contact"));
        let initials = crate::presentation::initials(&name);
        let avatar_label = if initials.is_empty() {
            "?".to_string()
        } else {
            initials
        };
        let calls = crate::theme::BoruTheme::default().calls;
        let typography = crate::theme::BoruTheme::default().typography;
        let remote_fallback = || {
            container(
                column![
                    text(avatar_label.clone()).size(typography.call_avatar_glyph_large),
                    text(name.clone()).size(typography.call_remote_name)
                ]
                .spacing(SPACE_8)
                .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(|theme| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg_surface_secondary(theme))),
                ..Default::default()
            })
        };
        #[cfg(feature = "video-calls")]
        let remote = self
            .calls_state
            .latest_remote_frame
            .as_ref()
            .filter(|frame| {
                contain_fit_rect(frame.width as f32, frame.height as f32, 1.0, 1.0).is_some()
            })
            .map(|frame| {
                // Iced performs the final dynamic viewport calculation;
                // Contain is the rendering equivalent of contain_fit_rect
                // and preserves the source ratio with letterboxing.
                iced::widget::image(iced::widget::image::Handle::from_rgba(
                    frame.width,
                    frame.height,
                    frame.rgba.to_vec(),
                ))
                .content_fit(iced::ContentFit::Contain)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
            });
        #[cfg(not(feature = "video-calls"))]
        let remote: Option<iced::Element<'_, AppMessage>> = None;
        // The remote stage is the main area: show the latest remote frame
        // whenever one is available (remote camera on), and fall back to the
        // avatar/name block when the remote camera is off (no frame yet).
        // The LOCAL camera state must not gate the remote stage — turning off
        // your own camera only affects the local PiP.
        let remote_main: iced::Element<'_, AppMessage> =
            remote.unwrap_or_else(|| remote_fallback().into());
        #[cfg(feature = "video-calls")]
        let local = self
            .calls_state
            .latest_local_frame
            .as_ref()
            .and_then(|frame| {
                let fit = contain_fit_rect(
                    frame.width as f32,
                    frame.height as f32,
                    calls.pip_w,
                    calls.pip_h,
                )?;
                Some(
                    iced::widget::image(iced::widget::image::Handle::from_rgba(
                        frame.width,
                        frame.height,
                        frame.rgba.to_vec(),
                    ))
                    .content_fit(iced::ContentFit::Contain)
                    .width(Length::Fixed(fit.width))
                    .height(Length::Fixed(fit.height))
                    .into(),
                )
            });
        #[cfg(not(feature = "video-calls"))]
        let local: Option<iced::Element<'_, AppMessage>> = None;
        let local_pip: iced::Element<'_, AppMessage> = local.unwrap_or_else(|| {
            container(text(crate::i18n::t("calls.you")).size(typography.call_pip_label))
                .width(Length::Fixed(calls.pip_w))
                .height(Length::Fixed(calls.pip_h))
                .center_x(Length::Fixed(calls.pip_w))
                .center_y(Length::Fixed(calls.pip_h))
                .style(|theme| iced::widget::container::Style {
                    background: Some(iced::Background::Color(bg_surface_secondary(theme))),
                    border: iced::Border {
                        radius: crate::theme::BoruTheme::for_theme(theme).radii.lg.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
        });
        let elapsed = self
            .calls_state
            .call_started_at
            .map(|start| start.elapsed().as_secs())
            .unwrap_or_default();
        let duration = format!("{:02}:{:02}", elapsed / 60, elapsed % 60);
        let status = if self.calls_state.call_audio_muted {
            crate::i18n::t("calls.connected_mic_muted")
        } else {
            crate::i18n::t("calls.connected_audio")
        };
        let mute_label = if self.calls_state.call_audio_muted {
            crate::i18n::t("calls.unmute")
        } else {
            crate::i18n::t("calls.mute")
        };
        let mute = button(text(mute_label)).on_press_maybe(
            self.calls_state
                .active_call_id
                .map(|_| AppMessage::ToggleCallMute),
        );
        let camera_label = if self.calls_state.call_camera_enabled {
            crate::i18n::t("calls.camera_off")
        } else {
            crate::i18n::t("calls.camera_on")
        };
        let camera = button(text(camera_label)).on_press_maybe(
            self.calls_state
                .active_call_id
                .map(|_| AppMessage::ToggleCallCamera),
        );
        let switch_camera = button(text(crate::i18n::t_args(
            "calls.switch_camera",
            &[("camera", &self.calls_state.call_camera_selection)],
        )))
        .on_press_maybe(
            self.calls_state
                .active_call_id
                .map(|_| AppMessage::SelectCamera("next".to_string())),
        );
        let hang_up = button(text(crate::i18n::t("calls.hang_up")))
            .on_press_maybe(self.calls_state.active_call_id.map(AppMessage::HangUp))
            .style(BUTTON_DANGER);
        let stage = container(local_pip)
            .width(Length::Fixed(calls.pip_w))
            .height(Length::Fixed(calls.pip_h))
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Bottom)
            .style(|theme| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg_surface_secondary(theme))),
                border: iced::Border {
                    radius: crate::theme::BoruTheme::for_theme(theme).radii.lg.into(),
                    ..Default::default()
                },
                ..Default::default()
            });
        // Wrapping keeps the action bar reachable when the call surface is
        // hosted in a reduced-width content pane.  At the normal desktop
        // width this is a single row, so the default appearance is unchanged.
        let controls = row![mute, camera, switch_camera, hang_up]
            .spacing(SPACE_12)
            .align_y(iced::Alignment::Center)
            .wrap();
        container(
            column![
                container(remote_main)
                    .width(Length::Fill)
                    .height(Length::Fill),
                stage,
                text(name).size(typography.call_name_active),
                text(duration).size(typography.call_duration),
                text(status).size(typography.call_status),
                controls
            ]
            .spacing(SPACE_12)
            .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(SPACE_16)
        .into()
    }

    /// State-layer update for call screens (BORU-AUDIT-22 spec step 5).
    ///
    /// Handles every AppMessage variant owned by the calls feature: starting
    /// voice/video calls, call lifecycle events, accept/reject/hangup, mute/
    /// camera toggles, device selection and call command results. The root
    /// `update()` dispatches these variants here via a combined match arm.
    pub(crate) fn update_calls(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
            AppMessage::StartVoiceCall(peer) => {
                // BORU-CP-12 (PDF Task 4.3): a new client must not attempt
                // an unsupported operation against an old/unknown client.
                // With a capability gate wired, the call starts only when
                // the peer negotiates a compatible voice version; otherwise
                // the action is blocked with a clear explanation.
                if self.capability_gate.is_some()
                    && self
                        .negotiated_feature_version(
                            &peer,
                            boru_core::control_plane::features::VOICE,
                        )
                        .is_none()
                {
                    tracing::warn!(
                        peer = %peer,
                        feature = boru_core::control_plane::features::VOICE,
                        "voice call blocked: peer does not negotiate a compatible voice capability"
                    );
                    self.notifications_state.show_toast(
                        "Voice calls unavailable — this peer's client does not support voice calls."
                            .to_string(),
                            160,
                        );
                    return iced::Task::none();
                }
                tracing::info!(
                    peer = %peer,
                    feature = boru_core::control_plane::features::VOICE,
                    negotiated_version = ?self
                        .negotiated_feature_version(&peer, boru_core::control_plane::features::VOICE),
                    "voice call initiated"
                );
                self.call_return_screen = Some(self.screen.clone());
                self.calls_state.outgoing_call_peer = Some(peer);
                self.calls_state.call_kind = Some(CallKind::Voice);
                self.calls_state.call_was_incoming = false;
                self.calls_state.call_declined = false;
                self.calls_state.outgoing_call_status = Some(OutgoingCallStatus::Ringing);
                self.screen = Screen::OutgoingCall;
                let handle = self.call_handle.clone();
                iced::Task::perform(
                    async move {
                        handle
                            .start_voice_call(peer)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    AppMessage::CallStarted,
                )
            }
            AppMessage::StartVideoCall(peer) => {
                // BORU-CP-12: video calls require a negotiated video
                // capability (which also implies voice support).
                if self.capability_gate.is_some()
                    && self
                        .negotiated_feature_version(
                            &peer,
                            boru_core::control_plane::features::VIDEO,
                        )
                        .is_none()
                {
                    tracing::warn!(
                        peer = %peer,
                        feature = boru_core::control_plane::features::VIDEO,
                        "video call blocked: peer does not negotiate a compatible video capability"
                    );
                    self.notifications_state.show_toast(
                        "Video calls unavailable — this peer's client does not support video calls."
                            .to_string(),
                            160,
                        );
                    return iced::Task::none();
                }
                tracing::info!(
                    peer = %peer,
                    feature = boru_core::control_plane::features::VIDEO,
                    negotiated_version = ?self
                        .negotiated_feature_version(&peer, boru_core::control_plane::features::VIDEO),
                    "video call initiated"
                );
                self.call_return_screen = Some(self.screen.clone());
                self.calls_state.outgoing_call_peer = Some(peer);
                self.calls_state.call_kind = Some(CallKind::Video);
                self.calls_state.call_was_incoming = false;
                self.calls_state.call_declined = false;
                self.calls_state.outgoing_call_status = Some(OutgoingCallStatus::Ringing);
                self.screen = Screen::OutgoingCall;
                let handle = self.call_handle.clone();
                iced::Task::perform(
                    async move {
                        handle
                            .start_video_call(peer)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    AppMessage::CallStarted,
                )
            }
            AppMessage::CallStarted(result) => {
                match result {
                    Ok(call_id) => self.calls_state.active_call_id = Some(call_id),
                    Err(error) => {
                        tracing::warn!(error = %error, "call start failed");
                        self.calls_state.outgoing_call_status = Some(OutgoingCallStatus::Failed);
                        self.notifications_state
                            .show_toast_message(friendly_call_error_text(&error).to_string());
                    }
                }
                iced::Task::none()
            }
            AppMessage::CallEventReceived(event) => {
                match &event {
                    CallEvent::Incoming {
                        call_id,
                        peer,
                        kind,
                    } => {
                        self.calls_state.active_call_id = Some(*call_id);
                        self.calls_state.outgoing_call_peer = Some(*peer);
                        self.calls_state.call_kind = Some(*kind);
                        self.calls_state.call_was_incoming = true;
                        self.calls_state.call_declined = false;
                        self.calls_state.incoming_call = Some(IncomingCall {
                            call_id: *call_id,
                            peer: *peer,
                            kind: *kind,
                        });
                        self.emit_incoming_call_notification(peer);
                    }
                    CallEvent::OutgoingRinging { peer, .. } => {
                        self.calls_state.outgoing_call_peer = Some(*peer);
                        self.calls_state.outgoing_call_status = Some(OutgoingCallStatus::Ringing);
                        self.screen = Screen::OutgoingCall;
                    }
                    CallEvent::Connecting { call_id } => {
                        self.calls_state.active_call_id = Some(*call_id)
                    }
                    CallEvent::Active { call_id, peer, .. } => {
                        self.calls_state.active_call_id = Some(*call_id);
                        self.calls_state.outgoing_call_peer = Some(*peer);
                        self.calls_state.call_was_incoming = self
                            .calls_state
                            .incoming_call
                            .as_ref()
                            .is_some_and(|call| call.call_id == *call_id);
                        self.calls_state.call_started_at = Some(Instant::now());
                        self.screen = Screen::ActiveCall;
                        // The call is now in progress; the consent overlay is no longer needed.
                        if self
                            .calls_state
                            .incoming_call
                            .as_ref()
                            .is_some_and(|call| call.call_id == *call_id)
                        {
                            self.calls_state.incoming_call = None;
                        }
                    }
                    CallEvent::MediaStateChanged {
                        call_id,
                        audio_muted,
                        video_enabled,
                    } => {
                        self.calls_state.active_call_id = Some(*call_id);
                        self.calls_state.call_audio_muted = *audio_muted;
                        self.calls_state.call_camera_enabled = *video_enabled;
                    }
                    CallEvent::Ended { call_id, .. } => {
                        if self.calls_state.active_call_id == Some(*call_id) {
                            if let CallEvent::Ended { reason, .. } = &event {
                                self.notifications_state
                                    .show_toast_message(friendly_call_end(reason).to_string());
                            }
                            if let (Some(peer), Some(kind)) = (
                                self.calls_state.outgoing_call_peer,
                                self.calls_state.call_kind,
                            ) {
                                let duration = self
                                    .calls_state
                                    .call_started_at
                                    .map(|started| started.elapsed());
                                let outcome = if duration.is_some() {
                                    CallHistoryOutcome::Completed
                                } else if self.calls_state.call_declined {
                                    CallHistoryOutcome::Declined
                                } else if self.calls_state.call_was_incoming {
                                    CallHistoryOutcome::Missed
                                } else {
                                    CallHistoryOutcome::Failed
                                };
                                self.record_call_history(peer, kind, outcome, duration);
                            }
                            self.calls_state.active_call_id = None;
                            self.calls_state.outgoing_call_peer = None;
                            self.calls_state.outgoing_call_status = None;
                            self.calls_state.call_started_at = None;
                            self.calls_state.call_kind = None;
                            self.calls_state.call_was_incoming = false;
                            self.calls_state.call_declined = false;
                            if let Some(screen) = self.call_return_screen.take() {
                                self.screen = screen;
                            }
                        }
                        if self
                            .calls_state
                            .incoming_call
                            .as_ref()
                            .is_some_and(|call| call.call_id == *call_id)
                        {
                            self.calls_state.incoming_call = None;
                        }
                    }
                    CallEvent::Failed { call_id, reason } => match call_id {
                        Some(cid) => {
                            if self.calls_state.active_call_id == Some(*cid) {
                                if matches!(reason, boru_core::call::manager::CallError::Rejected) {
                                    if let (Some(peer), Some(kind)) = (
                                        self.calls_state.outgoing_call_peer,
                                        self.calls_state.call_kind,
                                    ) {
                                        self.record_call_history(
                                            peer,
                                            kind,
                                            CallHistoryOutcome::Declined,
                                            None,
                                        );
                                    }
                                }
                                self.calls_state.active_call_id = None;
                                self.calls_state.outgoing_call_status = Some(match reason {
                                    boru_core::call::manager::CallError::Rejected => {
                                        OutgoingCallStatus::Declined
                                    }
                                    boru_core::call::manager::CallError::Busy => {
                                        OutgoingCallStatus::Busy
                                    }
                                    boru_core::call::manager::CallError::Connection => {
                                        OutgoingCallStatus::Failed
                                    }
                                    _ => OutgoingCallStatus::Failed,
                                });
                                self.notifications_state
                                    .show_toast_message(friendly_call_error(reason).to_string());
                                self.calls_state.call_kind = None;
                                self.calls_state.call_was_incoming = false;
                                self.calls_state.call_declined = false;
                            }
                            if self
                                .calls_state
                                .incoming_call
                                .as_ref()
                                .is_some_and(|call| call.call_id == *cid)
                            {
                                self.calls_state.incoming_call = None;
                            }
                        }
                        None => {
                            self.calls_state.incoming_call = None;
                        }
                    },
                    _ => {}
                }
                iced::Task::none()
            }
            AppMessage::AcceptIncomingCall(call_id) => {
                let handle = self.call_handle.clone();
                iced::Task::perform(
                    async move { handle.accept(call_id).await.map_err(|e| e.to_string()) },
                    AppMessage::CallCommandFinished,
                )
            }
            AppMessage::RejectIncomingCall(call_id) => {
                self.calls_state.call_declined = true;
                let handle = self.call_handle.clone();
                iced::Task::perform(
                    async move { handle.reject(call_id).await.map_err(|e| e.to_string()) },
                    AppMessage::CallCommandFinished,
                )
            }
            AppMessage::HangUp(call_id) => {
                // Clear call UI state synchronously so the caller leaves the
                // ringing/active screen immediately (BORU-CALL-6.4 contract).
                // The manager's later CallEvent::Ended is a no-op for this
                // call because active_call_id has already been cleared, so
                // call history is recorded here with the same outcome logic
                // as the Ended handler (BORU-CALL-14).
                if self.calls_state.active_call_id == Some(call_id) {
                    if let (Some(peer), Some(kind)) = (
                        self.calls_state.outgoing_call_peer,
                        self.calls_state.call_kind,
                    ) {
                        let duration = self
                            .calls_state
                            .call_started_at
                            .map(|started| started.elapsed());
                        let outcome = if duration.is_some() {
                            CallHistoryOutcome::Completed
                        } else if self.calls_state.call_declined {
                            CallHistoryOutcome::Declined
                        } else if self.calls_state.call_was_incoming {
                            CallHistoryOutcome::Missed
                        } else {
                            CallHistoryOutcome::Failed
                        };
                        self.record_call_history(peer, kind, outcome, duration);
                    }
                    self.calls_state.active_call_id = None;
                    self.calls_state.outgoing_call_peer = None;
                    self.calls_state.outgoing_call_status = None;
                    self.calls_state.call_started_at = None;
                    self.calls_state.call_kind = None;
                    self.calls_state.call_was_incoming = false;
                    self.calls_state.call_declined = false;
                    if let Some(screen) = self.call_return_screen.take() {
                        self.screen = screen;
                    }
                }
                let handle = self.call_handle.clone();
                iced::Task::perform(
                    async move { handle.hangup(call_id).await.map_err(|e| e.to_string()) },
                    AppMessage::CallCommandFinished,
                )
            }
            AppMessage::ToggleCallMute => {
                if let Some(call_id) = self.calls_state.active_call_id {
                    self.calls_state.call_audio_muted = !self.calls_state.call_audio_muted;
                    let handle = self.call_handle.clone();
                    let muted = self.calls_state.call_audio_muted;
                    iced::Task::perform(
                        async move {
                            handle
                                .set_muted(call_id, muted)
                                .await
                                .map_err(|e| e.to_string())
                        },
                        AppMessage::CallCommandFinished,
                    )
                } else {
                    iced::Task::none()
                }
            }
            AppMessage::ToggleCallCamera => {
                if let Some(call_id) = self.calls_state.active_call_id {
                    self.calls_state.call_camera_enabled = !self.calls_state.call_camera_enabled;
                    let handle = self.call_handle.clone();
                    let enabled = self.calls_state.call_camera_enabled;
                    iced::Task::perform(
                        async move {
                            handle
                                .set_camera_enabled(call_id, enabled)
                                .await
                                .map_err(|e| e.to_string())
                        },
                        AppMessage::CallCommandFinished,
                    )
                } else {
                    iced::Task::none()
                }
            }
            AppMessage::SelectCamera(selection) => {
                self.calls_state
                    .update(CallsMessage::SelectCamera(selection));
                iced::Task::none()
            }
            AppMessage::SelectMicrophone(selection) => {
                self.calls_state
                    .update(CallsMessage::SelectMicrophone(selection));
                iced::Task::none()
            }
            AppMessage::SelectSpeaker(selection) => {
                self.calls_state
                    .update(CallsMessage::SelectSpeaker(selection));
                iced::Task::none()
            }
            AppMessage::CallUiTick => {
                self.calls_state.update(CallsMessage::CallUiTick);
                iced::Task::none()
            }
            AppMessage::CallCommandFinished(Err(error)) => {
                tracing::warn!(error = %error, "call command failed");
                self.notifications_state
                    .show_toast_message(friendly_call_error_text(&error).to_string());
                iced::Task::none()
            }
            AppMessage::CallCommandFinished(Ok(())) => iced::Task::none(),
            // update() only dispatches the calls variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }

    /// State-layer update for the screen-share domain (BORU-APP-008).
    ///
    /// Handles every `AppMessage` variant owned by the screen-share feature:
    /// starting and stopping shares, invitations, session events, decoded
    /// frames, viewer stats, control requests/grants, clipboard sync, quality
    /// and source selection, pointer/keyboard input, view/pan and notice
    /// dismissal. The root `update()` dispatches these variants here via a
    /// combined match arm.
    #[cfg(feature = "screen-sharing")]
    pub(crate) fn update_screen_share(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
            #[cfg(feature = "screen-sharing")]
            AppMessage::StartScreenShare(peer) => self.start_screen_share(peer),
            #[cfg(feature = "screen-sharing")]
            AppMessage::StopScreenShare => {
                // Stop the host task (it sends EndSession on its connection)
                // and the decode worker, then reset every viewer/host flag.
                if let Some(stop) = &self.calls_state.screen_share_host_stop {
                    stop.store(true, Ordering::Relaxed);
                }
                if let Some(stop) = &self.calls_state.screen_share_decode_stop {
                    stop.store(true, Ordering::Relaxed);
                }
                if let (Some(protocol), Some(session_id)) = (
                    &self.calls_state.screen_share_protocol,
                    self.calls_state.screen_share_view_session,
                ) {
                    let protocol = protocol.clone();
                    let _ = self.runtime_handle.spawn(async move {
                        let _ = protocol
                            .send_control(
                                session_id,
                                ControlMessage::EndSession {
                                    version: SCREEN_SHARE_PROTOCOL_VERSION,
                                    session_id,
                                },
                            )
                            .await;
                    });
                }
                self.reset_screen_share_state();
                // Terminal notice: the user stopped the share. Visible until
                // dismissed or a new share starts (PDF Phase 13: show a clear
                // "stopped" state).
                self.calls_state.screen_share_host_state = ScreenShareHostState::Stopped;
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::AcceptScreenShare => self.accept_screen_share(),
            #[cfg(feature = "screen-sharing")]
            AppMessage::DeclineScreenShare => {
                if let Some((_, session_id)) = self.calls_state.screen_share_invite.take() {
                    if let Some(protocol) = &self.calls_state.screen_share_protocol {
                        let protocol = protocol.clone();
                        return iced::Task::perform(
                            async move {
                                protocol
                                    .send_control(
                                        session_id,
                                        ControlMessage::Reject {
                                            version: SCREEN_SHARE_PROTOCOL_VERSION,
                                            session_id,
                                            reason: "declined".to_string(),
                                        },
                                    )
                                    .await
                                    .map_err(|e| e.to_string())
                            },
                            |result| AppMessage::ScreenShareCommandFinished(result),
                        );
                    }
                }
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ToggleScreenShareFullscreen => {
                self.calls_state
                    .update(CallsMessage::ToggleScreenShareFullscreen);
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareEventReceived(event) => self.apply_screen_share_event(event),
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareFrameReceived(frame) => {
                if let Some(frame) = frame {
                    // Only rebuild the rendered handle when a genuinely new
                    // frame arrives; the worker publishes newest-frame-wins.
                    if self.calls_state.screen_share_last_frame_ts != Some(frame.timestamp_us) {
                        if frame.pixel_format == PixelFormat::Rgba8 {
                            // Cache the raw RGBA so a cursor shape/position/
                            // toggle update re-composites WITHOUT waiting for
                            // a new video frame (BORU-SS-33: cursor motion
                            // must not force a full-frame re-encode).
                            self.calls_state.screen_share_cursor_frame_rgba =
                                Some((frame.width, frame.height, frame.pixels.clone()));
                            if let Some(handle) = self.screen_share_build_cursor_frame(
                                frame.width,
                                frame.height,
                                frame.pixels,
                            ) {
                                self.calls_state.screen_share_frame_handle = Some(handle);
                            }
                        }
                        self.calls_state.screen_share_src_size = Some((frame.width, frame.height));
                        self.calls_state.screen_share_last_frame_ts = Some(frame.timestamp_us);
                    }
                }
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareStatsReceived(stats) => {
                // Developer diagnostics overlay (PDF Phase 12): keep the
                // latest viewer-side snapshot; no payload data is carried.
                self.calls_state.screen_share_viewer_stats = stats;
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareCommandFinished(result) => {
                if let Err(error) = result {
                    tracing::warn!(error, "screen-share control send failed");
                    self.notifications_state.show_toast(error, 160);
                }
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareRequestControl => self.request_screen_share_control(),
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareRequestClipboard => self.request_screen_share_clipboard(),
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareSendClipboard => self.screen_share_send_clipboard(),
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareHostSendClipboard => self.screen_share_host_send_clipboard(),
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareClipboardRead(text) => {
                self.screen_share_apply_clipboard_read(text)
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareGrantControl(capabilities) => {
                if let Some(tx) = &self.calls_state.screen_share_host_cmd_tx {
                    let _ = tx.try_send(HostCommand::GrantControl(capabilities));
                }
                self.calls_state.screen_share_control_request = None;
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareDenyControl => {
                self.calls_state.screen_share_control_request = None;
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareToggleAudio => {
                // BORU-SS-37: host toggles system-audio sharing (opt-in).
                // The host driver grants the Audio capability and starts/
                // stops capture; the viewer authorizes packets against the
                // grant. Audio never affects the video path.
                if let Some(tx) = &self.calls_state.screen_share_host_cmd_tx {
                    let _ = tx.try_send(HostCommand::SetAudioEnabled(
                        !self.calls_state.screen_share_audio_active,
                    ));
                }
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareRevokeControl => {
                if let Some(tx) = &self.calls_state.screen_share_host_cmd_tx {
                    let _ = tx.try_send(HostCommand::RevokeControl);
                }
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareLowerQuality => self.send_screen_share_quality(60, 60),
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareFullQuality => self.send_screen_share_quality(100, 100),
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareSelectSource(source_id) => {
                // PDF Phase 13: sharer picks the monitor to share. The choice
                // is applied by the host driver whether the viewer has
                // already accepted (in-session switch + SourceChanged) or is
                // still deciding (pre-acceptance re-select). The local
                // selection marker updates immediately so the picker UI can
                // highlight the chosen source.
                self.calls_state.screen_share_selected_source = Some(source_id);
                if let Some(tx) = &self.calls_state.screen_share_host_cmd_tx {
                    let _ = tx.try_send(HostCommand::SwitchSource(source_id));
                }
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareSetPreset(preset) => {
                // BORU-SS-39: sharer overrides the quality preset (None
                // restores the path-derived auto preset). The host driver
                // applies the ceiling whether streaming already started or
                // the viewer is still deciding.
                // BORU-SSUI-04: mirror the user's choice so the segmented
                // control shows exactly one selected segment (None = Auto).
                self.calls_state.screen_share_selected_preset = preset;
                if let Some(tx) = &self.calls_state.screen_share_host_cmd_tx {
                    let _ = tx.try_send(HostCommand::SetQualityPreset(preset));
                }
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareDismissNotice => {
                self.calls_state
                    .update(CallsMessage::ScreenShareDismissNotice);
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenSharePointerMove { x, y } => {
                let modifiers = self.calls_state.screen_share_modifiers;
                self.send_screen_share_input(InputEventKind::PointerMove, 0, x, y, false, modifiers)
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenSharePointerButton {
                x,
                y,
                button,
                pressed,
            } => {
                let modifiers = self.calls_state.screen_share_modifiers;
                self.send_screen_share_input(
                    InputEventKind::PointerButton,
                    button,
                    x,
                    y,
                    pressed,
                    modifiers,
                )
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareWheel { x, y, dx, dy } => {
                // Explicit wheel event (PDF Task 9.2): the direction maps to an
                // X11 wheel button (4 up, 5 down, 6 left, 7 right) which every
                // platform backend understands.
                let direction = if dy.abs() >= dx.abs() {
                    if dy > 0.0 {
                        4
                    } else if dy < 0.0 {
                        5
                    } else {
                        return iced::Task::none();
                    }
                } else if dx > 0.0 {
                    7
                } else if dx < 0.0 {
                    6
                } else {
                    return iced::Task::none();
                };
                let modifiers = self.calls_state.screen_share_modifiers;
                self.send_screen_share_input(
                    InputEventKind::Wheel,
                    direction,
                    x,
                    y,
                    true,
                    modifiers,
                )
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareKeyEvent { code, pressed } => {
                // Track held modifiers explicitly (PDF Task 9.2): modifier
                // keysyms update the mask, every event carries the current
                // mask, and a dedicated ModifierChange event is emitted so the
                // host sees the state change as a first-class message even
                // without the raw key event.
                if let Some(bit) = keysym_modifier_bit(code) {
                    if pressed {
                        self.calls_state.screen_share_modifiers |= bit;
                    } else {
                        self.calls_state.screen_share_modifiers &= !bit;
                    }
                    let modifiers = self.calls_state.screen_share_modifiers;
                    return iced::Task::batch(vec![
                        self.send_screen_share_input(
                            InputEventKind::Key,
                            code,
                            0.0,
                            0.0,
                            pressed,
                            modifiers,
                        ),
                        self.send_screen_share_input(
                            InputEventKind::ModifierChange,
                            modifiers,
                            0.0,
                            0.0,
                            false,
                            modifiers,
                        ),
                    ]);
                }
                let modifiers = self.calls_state.screen_share_modifiers;
                self.send_screen_share_input(
                    InputEventKind::Key,
                    code,
                    0.0,
                    0.0,
                    pressed,
                    modifiers,
                )
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareSetView { mode, pan } => {
                self.calls_state
                    .update(CallsMessage::ScreenShareSetView { mode, pan });
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ToggleScreenShareCursor => {
                self.calls_state.screen_share_cursor_enabled =
                    !self.calls_state.screen_share_cursor_enabled;
                // Re-composite the cached frame with the new cursor state so
                // the overlay toggles immediately without waiting for a new
                // video frame.
                if let Some((w, h, pixels)) = self.calls_state.screen_share_cursor_frame_rgba.take()
                {
                    if let Some(handle) = self.screen_share_build_cursor_frame(w, h, pixels) {
                        self.calls_state.screen_share_frame_handle = Some(handle);
                    }
                }
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenSharePanStart { pos } => {
                self.calls_state
                    .update(CallsMessage::ScreenSharePanStart { pos });
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenSharePanMove { pos, scale } => {
                self.calls_state
                    .update(CallsMessage::ScreenSharePanMove { pos, scale });
                iced::Task::none()
            }
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenSharePanEnd => {
                self.calls_state.update(CallsMessage::ScreenSharePanEnd);
                iced::Task::none()
            }
            // update() only dispatches the screen-share variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }
    /// 1 Hz shell-tick hook: auto-dismiss a terminal screen-share notice
    /// (Stopped/Error) after ~8 ticks so a stale status never blocks a fresh
    /// share. The shell's `ConnMonitorTick` calls this (same pattern as
    /// `RoomsState::periodic_room_advertisement`); the panel also offers
    /// Dismiss/retry immediately.
    #[cfg(feature = "screen-sharing")]
    pub(crate) fn tick_screen_share_notice(&mut self) {
        if matches!(
            self.calls_state.screen_share_host_state,
            ScreenShareHostState::Stopped | ScreenShareHostState::Error(_)
        ) {
            self.calls_state.screen_share_notice_ticks =
                self.calls_state.screen_share_notice_ticks.saturating_add(1);
            if self.calls_state.screen_share_notice_ticks >= 8 {
                self.calls_state.screen_share_host_state = ScreenShareHostState::Idle;
                self.calls_state.screen_share_notice_ticks = 0;
            }
        }
    }
}

impl IcedChat {
    /// Record local-only call metadata in the deterministic direct chat.
    ///
    /// Only the formatted text is stored. No call ID, peer address, media,
    /// or signalling payload is written to the message store.
    fn record_call_history(
        &mut self,
        peer: PublicKey,
        kind: CallKind,
        outcome: CallHistoryOutcome,
        duration: Option<std::time::Duration>,
    ) {
        let Some(text) = call_history_text(kind, outcome, duration) else {
            return;
        };
        let topic = direct_topic(&self.local_public, &peer);
        if topic == self.topic {
            self.push_system(text);
            self.save_room_to_history();
            return;
        }

        let timestamp = now_ms() as u64;
        let mut hash_input = topic.as_bytes().to_vec();
        hash_input.extend_from_slice(&timestamp.to_le_bytes());
        hash_input.extend_from_slice(text.as_bytes());
        let hash = *blake3::hash(&hash_input).as_bytes();
        let store_path = self.data_dir.join("message_store.db");
        if let Err(error) = MessageStore::open(store_path).and_then(|store| {
            store.insert_chat_message(
                &hash,
                topic.as_bytes(),
                self.local_public.as_bytes(),
                timestamp,
                "system",
                &text,
                None,
                None,
                self.local_public.as_bytes(),
            )
        }) {
            warn!(%error, "failed to persist call history");
        }
    }
}

impl IcedChat {
    #[cfg(feature = "screen-sharing")]
    /// Start a local sharing session with `peer`: spawn the host driver task
    /// (dial → Hello → negotiate → capture/encode/send) and show the
    /// persistent indicator while it runs.
    fn start_screen_share(&mut self, peer: PublicKey) -> iced::Task<AppMessage> {
        // A terminal notice (Stopped/Error) never blocks a fresh share: the
        // user can restart directly from the panel without dismissing first.
        if !matches!(
            self.calls_state.screen_share_host_state,
            ScreenShareHostState::Idle
                | ScreenShareHostState::Stopped
                | ScreenShareHostState::Error(_)
        ) {
            return iced::Task::none();
        }
        // BORU-CP-12 (PDF Task 4.3): a new client must not attempt an
        // unsupported operation against an old/unknown client. With a
        // capability gate wired, screen sharing starts only when the peer
        // negotiates a compatible screen-share version.
        if self.capability_gate.is_some()
            && self
                .negotiated_feature_version(&peer, boru_core::control_plane::features::SCREEN_SHARE)
                .is_none()
        {
            tracing::warn!(
                peer = %peer,
                feature = boru_core::control_plane::features::SCREEN_SHARE,
                "screen share blocked: peer does not negotiate a compatible screen-share capability"
            );
            self.notifications_state.show_toast(
                "Screen sharing unavailable — this peer's client does not support screen sharing."
                    .to_string(),
                160,
            );
            return iced::Task::none();
        }
        tracing::info!(
            peer = %peer,
            feature = boru_core::control_plane::features::SCREEN_SHARE,
            negotiated_version = ?self.negotiated_feature_version(
                &peer,
                boru_core::control_plane::features::SCREEN_SHARE,
            ),
            "screen share initiated"
        );
        let Some(events_tx) = self.calls_state.screen_share_events_tx.clone() else {
            return iced::Task::none();
        };
        self.calls_state.screen_share_host_state = ScreenShareHostState::Requesting;
        self.calls_state.screen_share_notice_ticks = 0;
        let stop = Arc::new(AtomicBool::new(false));
        self.calls_state.screen_share_host_stop = Some(stop.clone());
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(8);
        self.calls_state.screen_share_host_cmd_tx = Some(cmd_tx);
        let endpoint = self.endpoint.clone();
        let local_public = self.local_public;
        // conversation_id is informational in the protocol and not used for
        // media routing; M7 shows the invitation in the active conversation.
        let conversation_id = 0u64;
        // run_host_session's streaming loop performs synchronous capture
        // (X11 GetImage) and encode (openh264) — up to ~500ms of blocking per
        // frame with no yield. On the shared multi-thread runtime that blocks
        // one worker and starves the QUIC connection driver task if it parks
        // on the same worker (observed: media stream data buffered forever,
        // cwnd frozen at the initial window, udp_tx frozen; the viewer never
        // receives frames and idle-times-out). Run the whole host session on
        // a dedicated thread with its own current-thread runtime so the app
        // runtime and its connection drivers are never blocked.
        std::thread::Builder::new()
            .name("boru-screen-share-host".to_string())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to create screen-share host runtime");
                rt.block_on(run_host_session(
                    endpoint,
                    peer,
                    local_public,
                    conversation_id,
                    events_tx,
                    stop,
                    cmd_rx,
                ));
            })
            .expect("failed to spawn screen-share host thread");
        iced::Task::none()
    }

    #[cfg(feature = "screen-sharing")]
    /// Viewer requests explicit control (pointer + keyboard) from the host.
    fn request_screen_share_control(&self) -> iced::Task<AppMessage> {
        if self.calls_state.screen_share_control_active {
            return iced::Task::none();
        }
        let Some(protocol) = self.calls_state.screen_share_protocol.clone() else {
            return iced::Task::none();
        };
        let Some(session_id) = self.calls_state.screen_share_view_session else {
            return iced::Task::none();
        };
        iced::Task::perform(
            async move {
                protocol
                    .send_control(
                        session_id,
                        ControlMessage::RequestControl {
                            version: SCREEN_SHARE_PROTOCOL_VERSION,
                            session_id,
                            capabilities: vec![
                                Capability::ControlPointer,
                                Capability::ControlKeyboard,
                            ],
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| AppMessage::ScreenShareCommandFinished(result),
        )
    }

    #[cfg(feature = "screen-sharing")]
    /// Viewer requests a manual quality change (PDF Task 7.3 / QualityUpdate
    /// path). `scale_percent` maps to the viewer-facing "Lower quality" (60%)
    /// and "Full quality" (100%) buttons. The host clamps its adaptive
    /// controller to the requested ceiling.
    fn send_screen_share_quality(
        &self,
        scale_percent: u8,
        bitrate_percent: u8,
    ) -> iced::Task<AppMessage> {
        let Some(protocol) = self.calls_state.screen_share_protocol.clone() else {
            return iced::Task::none();
        };
        let Some(session_id) = self.calls_state.screen_share_view_session else {
            return iced::Task::none();
        };
        // Absolute presets: the host clamps to its own base, so "full" sends
        // values at/above any sane base (bitrate unlimited by validation,
        // fps capped at the protocol max 240) and "lower" sends a conservative
        // reduced ceiling (1 Mbps @ 10 fps @ 60% resolution).
        let (target_bitrate_bps, max_frame_rate, scale_factor) = if bitrate_percent >= 100 {
            (100_000_000, 240u16, scale_percent.clamp(1, 100))
        } else {
            (1_000_000, 10u16, scale_percent.clamp(1, 100))
        };
        let session_id_for_message = session_id;
        iced::Task::perform(
            async move {
                protocol
                    .send_screen_share(
                        session_id_for_message,
                        ScreenShareMessage::QualityUpdate {
                            version: SCREEN_SHARE_PROTOCOL_VERSION,
                            session_id: session_id_for_message,
                            target_bitrate_bps,
                            max_frame_rate,
                            scale_factor,
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| AppMessage::ScreenShareCommandFinished(result),
        )
    }

    #[cfg(feature = "screen-sharing")]
    /// Send one authorized input event viewer → host. Pointer moves are
    /// throttled (~30/s) and near-identical points skipped; every event echoes
    /// the host's grant nonce so stale input is rejected. The explicit `kind`
    /// (PDF Task 9.2) says what the event is (move/button/wheel/key/modifier),
    /// and `modifiers` carries the viewer's current held-modifier bitmask.
    fn send_screen_share_input(
        &mut self,
        kind: InputEventKind,
        code: u32,
        x: f32,
        y: f32,
        pressed: bool,
        modifiers: u32,
    ) -> iced::Task<AppMessage> {
        if !self.calls_state.screen_share_viewing || !self.calls_state.screen_share_control_active {
            return iced::Task::none();
        }
        if kind == InputEventKind::PointerMove && !pressed {
            let now = Instant::now();
            let throttled = self
                .calls_state
                .screen_share_last_pointer_sent
                .is_some_and(|t| now.duration_since(t) < Duration::from_millis(33));
            let same = self
                .calls_state
                .screen_share_last_pointer_pos
                .is_some_and(|(lx, ly)| (lx - x).abs() < 0.005 && (ly - y).abs() < 0.005);
            if throttled || same {
                return iced::Task::none();
            }
            self.calls_state.screen_share_last_pointer_sent = Some(now);
            self.calls_state.screen_share_last_pointer_pos = Some((x, y));
        }
        let Some(protocol) = self.calls_state.screen_share_protocol.clone() else {
            return iced::Task::none();
        };
        let Some(session_id) = self.calls_state.screen_share_view_session else {
            return iced::Task::none();
        };
        let manager = protocol.manager();
        iced::Task::perform(
            async move {
                let nonce = manager
                    .lock()
                    .await
                    .permissions(session_id)
                    .and_then(|permissions| permissions.token().map(|token| *token.nonce()));
                match nonce {
                    Some(nonce) => protocol
                        .send_control(
                            session_id,
                            ControlMessage::Input {
                                version: SCREEN_SHARE_PROTOCOL_VERSION,
                                session_id,
                                nonce,
                                kind,
                                code,
                                x,
                                y,
                                pressed,
                                modifiers,
                            },
                        )
                        .await
                        .map_err(|e| e.to_string()),
                    None => Err("control not granted".to_string()),
                }
            },
            |result| AppMessage::ScreenShareCommandFinished(result),
        )
    }

    #[cfg(feature = "screen-sharing")]
    /// Viewer requests the SEPARATE clipboard capability (PDF Task 9.3 /
    /// BORU-SS-25). This is a distinct `RequestControl` for `Clipboard` only —
    /// it is never implied by requesting or granting remote control.
    fn request_screen_share_clipboard(&self) -> iced::Task<AppMessage> {
        if self.calls_state.screen_share_clipboard_active {
            return iced::Task::none();
        }
        let Some(protocol) = self.calls_state.screen_share_protocol.clone() else {
            return iced::Task::none();
        };
        let Some(session_id) = self.calls_state.screen_share_view_session else {
            return iced::Task::none();
        };
        iced::Task::perform(
            async move {
                protocol
                    .send_control(
                        session_id,
                        ControlMessage::RequestControl {
                            version: SCREEN_SHARE_PROTOCOL_VERSION,
                            session_id,
                            capabilities: vec![Capability::Clipboard],
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| AppMessage::ScreenShareCommandFinished(result),
        )
    }

    #[cfg(feature = "screen-sharing")]
    /// Viewer pushes its local text clipboard to the host. Reads the local
    /// clipboard asynchronously; the result is sent in
    /// `screen_share_apply_clipboard_read` (which also gates on the granted
    /// Clipboard capability).
    fn screen_share_send_clipboard(&self) -> iced::Task<AppMessage> {
        if !self.calls_state.screen_share_viewing || !self.calls_state.screen_share_clipboard_active
        {
            return iced::Task::none();
        }
        iced::clipboard::read().map(AppMessage::ScreenShareClipboardRead)
    }

    #[cfg(feature = "screen-sharing")]
    /// Host pushes its local text clipboard to the viewer. Reads the local
    /// clipboard asynchronously; the result is applied in
    /// `screen_share_apply_clipboard_read`.
    fn screen_share_host_send_clipboard(&self) -> iced::Task<AppMessage> {
        if self.calls_state.screen_share_host_state != ScreenShareHostState::Streaming
            || !self.calls_state.screen_share_clipboard_active
        {
            return iced::Task::none();
        }
        iced::clipboard::read().map(AppMessage::ScreenShareClipboardRead)
    }

    #[cfg(feature = "screen-sharing")]
    /// Apply the result of an async local-clipboard read. When clipboard sync
    /// is granted, the text is pushed over the reliable control channel as a
    /// versioned `ScreenShareMessage::Clipboard` — viewer→host via the
    /// protocol handler, host→viewer via the host driver command. The local
    /// `screen_share_clipboard_active` flag is the capability gate; an empty
    /// clipboard or a missing grant is a no-op.
    fn screen_share_apply_clipboard_read(&self, text: Option<String>) -> iced::Task<AppMessage> {
        let Some(text) = text else {
            return iced::Task::none();
        };
        if text.is_empty() || text.len() > MAX_CLIPBOARD_TEXT {
            return iced::Task::none();
        }
        if !self.calls_state.screen_share_clipboard_active {
            return iced::Task::none();
        }
        if self.calls_state.screen_share_host_state == ScreenShareHostState::Streaming {
            // Host → viewer: route through the host driver so the payload is
            // sent on the same control channel the streaming loop owns.
            if let Some(tx) = &self.calls_state.screen_share_host_cmd_tx {
                // Wrap the payload so Debug/log of the command can never leak
                // clipboard contents (PDF Phase 12 guardrail).
                let _ = tx.try_send(HostCommand::SendClipboard(RedactedText::new(text)));
            }
            return iced::Task::none();
        }
        let Some(protocol) = self.calls_state.screen_share_protocol.clone() else {
            return iced::Task::none();
        };
        let Some(session_id) = self.calls_state.screen_share_view_session else {
            return iced::Task::none();
        };
        let manager = protocol.manager();
        iced::Task::perform(
            async move {
                let nonce = manager
                    .lock()
                    .await
                    .permissions(session_id)
                    .and_then(|permissions| permissions.token().map(|token| *token.nonce()));
                match nonce {
                    Some(nonce) => protocol
                        .send_screen_share(
                            session_id,
                            ScreenShareMessage::Clipboard {
                                version: SCREEN_SHARE_PROTOCOL_VERSION,
                                session_id,
                                nonce,
                                text: RedactedText::new(text),
                            },
                        )
                        .await
                        .map_err(|e| e.to_string()),
                    None => Err("clipboard capability not granted".to_string()),
                }
            },
            |result| AppMessage::ScreenShareCommandFinished(result),
        )
    }

    #[cfg(feature = "screen-sharing")]
    /// Accept the pending invitation: respond Accept on the inbound QUIC
    /// connection and spawn the viewer decode worker for the session.
    fn accept_screen_share(&mut self) -> iced::Task<AppMessage> {
        let Some((sharer, session_id)) = self.calls_state.screen_share_invite.take() else {
            return iced::Task::none();
        };
        self.calls_state.screen_share_viewing = true;
        self.calls_state.screen_share_view_session = Some(session_id);
        // Who is sharing (PDF Phase 13): keep the sharer identity for the
        // viewer panel so the viewer always knows whose screen is displayed.
        self.calls_state.screen_share_viewing_peer = Some(sharer);
        // Respond Accept on the same connection the invitation arrived on.
        let mut send_task = iced::Task::none();
        if let Some(protocol) = &self.calls_state.screen_share_protocol {
            let protocol = protocol.clone();
            send_task = iced::Task::perform(
                async move {
                    let result = protocol
                        .send_control(
                            session_id,
                            ControlMessage::Accept {
                                version: SCREEN_SHARE_PROTOCOL_VERSION,
                                session_id,
                            },
                        )
                        .await
                        .map_err(|e| e.to_string());
                    tracing::info!(error = ?result.as_ref().err(), "screen-share: viewer Accept send result");
                    result
                },
                |result| AppMessage::ScreenShareCommandFinished(result),
            );
        }
        // Spawn the decode worker: drains inbound media for this session,
        // feeds the bounded ViewerPipeline off the UI thread, and publishes
        // the newest decoded frame to the watch channel the subscription reads.
        let Some(media_rx) = self.calls_state.screen_share_media_rx.clone() else {
            self.calls_state.screen_share_viewing = false;
            self.calls_state.screen_share_view_session = None;
            return send_task;
        };
        let decoder = match OpenH264Decoder::default_profile() {
            Ok(decoder) => decoder,
            Err(_) => {
                self.calls_state.screen_share_viewing = false;
                self.calls_state.screen_share_view_session = None;
                return send_task;
            }
        };
        let pipeline =
            match ViewerPipeline::new(decoder, *session_id.as_bytes(), DEFAULT_QUEUE_CAPACITY) {
                Ok(pipeline) => pipeline,
                Err(_) => {
                    self.calls_state.screen_share_viewing = false;
                    self.calls_state.screen_share_view_session = None;
                    return send_task;
                }
            };
        let decode_stop = Arc::new(AtomicBool::new(false));
        self.calls_state.screen_share_decode_stop = Some(decode_stop.clone());
        let (watch_tx, watch_rx) = tokio::sync::watch::channel(None);
        self.calls_state.screen_share_frame_watch =
            Some(Arc::new(tokio::sync::Mutex::new(watch_rx)));
        // Viewer pipeline stats watch for the developer diagnostics overlay
        // (PDF Phase 12), fed ~1 Hz by the decode worker.
        let (stats_tx, stats_rx) = tokio::sync::watch::channel(None);
        self.calls_state.screen_share_stats_watch =
            Some(Arc::new(tokio::sync::Mutex::new(stats_rx)));
        let protocol = self.calls_state.screen_share_protocol.clone();
        let runtime_handle = self.runtime_handle.clone();
        runtime_handle.spawn(async move {
            decode_worker(
                media_rx,
                session_id,
                pipeline,
                watch_tx,
                stats_tx,
                protocol,
                decode_stop,
            )
            .await;
        });
        // Spawn the audio playback worker (BORU-SS-37): drains inbound audio
        // for this session, decodes Opus and plays through cpal. Runs on the
        // same tokio runtime; a missing output device (headless) logs a typed
        // unavailable error and the viewer continues view-only.
        if let Some(audio_rx) = self.calls_state.screen_share_audio_rx.clone() {
            let audio_stop = Arc::new(AtomicBool::new(false));
            self.calls_state.screen_share_audio_stop = Some(audio_stop.clone());
            let runtime_handle = self.runtime_handle.clone();
            runtime_handle.spawn(async move {
                audio_worker(audio_rx, session_id, audio_stop).await;
            });
        }
        send_task
    }

    #[cfg(feature = "screen-sharing")]
    /// Apply one protocol session event to the deterministic UI session state.
    fn apply_screen_share_event(&mut self, event: SessionEvent) -> iced::Task<AppMessage> {
        // Never Debug-log a ClipboardReceived event — it carries clipboard
        // contents (PDF guardrail: never log clipboard contents).
        if !matches!(event, SessionEvent::ClipboardReceived { .. }) {
            tracing::info!(?event, "screen-share: session event received");
        } else {
            tracing::info!("screen-share: clipboard event received");
        }
        match event {
            SessionEvent::Invitation {
                session_id,
                host_id,
                ..
            } => {
                if !self.calls_state.screen_share_viewing
                    && self.calls_state.screen_share_host_state == ScreenShareHostState::Idle
                    && self.calls_state.screen_share_invite.is_none()
                {
                    self.calls_state.screen_share_invite =
                        Some((host_id.fmt_short().to_string(), session_id));
                }
                iced::Task::none()
            }
            SessionEvent::NegotiationInvitation {
                session_id,
                host_id,
                ..
            } => {
                // Versioned negotiation offers (PDF Task 3.1) surface through
                // the same invitation prompt as legacy Hello invitations; the
                // host identity is what the recipient needs to decide.
                if !self.calls_state.screen_share_viewing
                    && self.calls_state.screen_share_host_state == ScreenShareHostState::Idle
                    && self.calls_state.screen_share_invite.is_none()
                {
                    self.calls_state.screen_share_invite =
                        Some((host_id.fmt_short().to_string(), session_id));
                }
                iced::Task::none()
            }
            SessionEvent::Accepted { .. } => {
                if self.calls_state.screen_share_host_state != ScreenShareHostState::Idle {
                    // Capture is active now — the persistent indicator stays on.
                    self.calls_state.screen_share_host_state = ScreenShareHostState::Streaming;
                }
                iced::Task::none()
            }
            SessionEvent::Rejected { reason, .. } => {
                // The peer declined or the session failed before streaming.
                // Surface a terminal notice: a peer "declined" is a normal
                // stop outcome, anything else is an error worth reading.
                if self.calls_state.screen_share_host_state != ScreenShareHostState::Idle {
                    if reason.eq_ignore_ascii_case("declined") {
                        self.calls_state.screen_share_host_state = ScreenShareHostState::Stopped;
                    } else {
                        self.calls_state.screen_share_host_state =
                            ScreenShareHostState::Error(reason);
                    }
                    self.calls_state.screen_share_notice_ticks = 0;
                    self.calls_state.screen_share_host_stop = None;
                }
                iced::Task::none()
            }
            SessionEvent::Reconnecting { session_id } => {
                // The media path failed transiently (PDF Task 3.3). The
                // chat/friend session survives — only the media stream
                // reconnects. If we are the VIEWER of this session, keep
                // viewing (do NOT tear down the decode worker) and re-accept
                // on the new connection so the host can resume streaming.
                if self.calls_state.screen_share_view_session == Some(session_id) {
                    self.calls_state.screen_share_viewing = true;
                    let Some(protocol) = self.calls_state.screen_share_protocol.clone() else {
                        return iced::Task::none();
                    };
                    return iced::Task::perform(
                        async move {
                            // Re-Accept on the new connection, then request a
                            // fresh keyframe so the decoder resynchronises
                            // without waiting for the next periodic keyframe.
                            let result = protocol
                                .send_control(
                                    session_id,
                                    ControlMessage::Accept {
                                        version: SCREEN_SHARE_PROTOCOL_VERSION,
                                        session_id,
                                    },
                                )
                                .await
                                .map_err(|e| e.to_string());
                            let _ = protocol
                                .send_screen_share(
                                    session_id,
                                    ScreenShareMessage::KeyframeRequest {
                                        version: SCREEN_SHARE_PROTOCOL_VERSION,
                                        session_id,
                                    },
                                )
                                .await;
                            result
                        },
                        |result| AppMessage::ScreenShareCommandFinished(result),
                    );
                }
                if self.calls_state.screen_share_host_state != ScreenShareHostState::Idle {
                    // Host side: surface the reconnecting state to the user.
                    self.calls_state.screen_share_host_state = ScreenShareHostState::Reconnecting;
                }
                iced::Task::none()
            }
            SessionEvent::Reconnected { .. } => {
                // The media path is back; resume the persistent indicator.
                if self.calls_state.screen_share_host_state == ScreenShareHostState::Reconnecting {
                    self.calls_state.screen_share_host_state = ScreenShareHostState::Streaming;
                }
                iced::Task::none()
            }
            SessionEvent::Ended { session_id } => {
                if self.calls_state.screen_share_view_session == Some(session_id) {
                    self.calls_state.screen_share_viewing = false;
                    self.calls_state.screen_share_view_session = None;
                    self.calls_state.screen_share_viewing_peer = None;
                    self.calls_state.screen_share_last_frame_ts = None;
                    self.calls_state.screen_share_frame_handle = None;
                    if let Some(stop) = &self.calls_state.screen_share_decode_stop {
                        stop.store(true, Ordering::Relaxed);
                    }
                    self.calls_state.screen_share_decode_stop = None;
                    // Stop the audio playback worker (BORU-SS-37) and clear
                    // the audio-active flag; the session is over.
                    if let Some(stop) = &self.calls_state.screen_share_audio_stop {
                        stop.store(true, Ordering::Relaxed);
                    }
                    self.calls_state.screen_share_audio_stop = None;
                    self.calls_state.screen_share_audio_active = false;
                    self.calls_state.screen_share_audio_error = None;
                }
                if self.calls_state.screen_share_host_state != ScreenShareHostState::Idle {
                    // Terminal notice: the share stopped (peer ended it or
                    // the transport died). Stays visible until dismissed or
                    // a new share starts.
                    self.calls_state.screen_share_host_state = ScreenShareHostState::Stopped;
                    self.calls_state.screen_share_notice_ticks = 0;
                    self.calls_state.screen_share_host_stop = None;
                }
                iced::Task::none()
            }
            SessionEvent::ControlRequest {
                session_id,
                peer_id,
                capabilities,
            } => {
                // Host side: show the explicit consent prompt with grant choices.
                if self.calls_state.screen_share_host_state != ScreenShareHostState::Idle {
                    self.calls_state.screen_share_control_request =
                        Some((session_id, peer_id.fmt_short().to_string(), capabilities));
                }
                iced::Task::none()
            }
            SessionEvent::ControlChanged {
                active,
                capabilities,
                ..
            } => {
                self.calls_state.screen_share_control_active = active;
                // Clipboard is a SEPARATE optional capability (PDF Task 9.3 /
                // BORU-SS-25): it follows the granted capability list, not
                // the `active` control flag. Granting remote control never
                // enables clipboard sync on its own.
                self.calls_state.screen_share_clipboard_active =
                    active && capabilities.contains(&Capability::Clipboard);
                // System audio is likewise a SEPARATE optional capability
                // (BORU-SS-37); it follows the granted list like clipboard.
                self.calls_state.screen_share_audio_active =
                    active && capabilities.contains(&Capability::Audio);
                if !active {
                    self.calls_state.screen_share_control_request = None;
                }
                iced::Task::none()
            }
            SessionEvent::AudioState { enabled, error, .. } => {
                // BORU-SS-37: the host reports audio sharing state. `error`
                // carries a typed, user-safe reason when capture could not
                // start (e.g. no PipeWire runtime); the session continues
                // view-only and the toast tells the sharer why.
                // BORU-SSUI-06: mirror the reason so the sender audio
                // switch can be disabled with the same typed reason as a
                // tooltip/status text (authoritative capability signal).
                self.calls_state.screen_share_audio_active = enabled;
                self.calls_state.screen_share_audio_error = error.clone();
                if let Some(reason) = error {
                    let message = format!("System audio unavailable — {reason}");
                    tracing::warn!("{message}");
                    self.notifications_state.show_toast(message, 160);
                }
                iced::Task::none()
            }
            SessionEvent::ClipboardReceived { text, .. } => {
                // Text-only clipboard sync (PDF Task 9.3 / BORU-SS-25). The
                // payload was already authorized against the Clipboard
                // capability by the screen-share layer; place it on the local
                // clipboard. Never log the contents (PDF guardrail).
                tracing::info!("screen-share: peer clipboard text applied");
                return iced::clipboard::write(text.into_inner());
            }
            SessionEvent::SourcesEnumerated { sources, .. } => {
                // PDF Phase 10/13: the host enumerated its monitors before
                // the share started. Store the list for the source picker;
                // the first source is the host's default selection, and once
                // the sources are known the session moves from "requesting"
                // to "awaiting acceptance" (the offer is in flight).
                if self.calls_state.screen_share_selected_source.is_none() {
                    self.calls_state.screen_share_selected_source =
                        sources.first().map(|source| source.id);
                }
                self.calls_state.screen_share_sources = Some(sources);
                if self.calls_state.screen_share_host_state == ScreenShareHostState::Requesting {
                    self.calls_state.screen_share_host_state = ScreenShareHostState::Inviting;
                }
                iced::Task::none()
            }
            SessionEvent::SourceChanged {
                source_id,
                width,
                height,
                title,
                ..
            } => {
                // PDF Phase 10: the shared source changed (host switched
                // monitor or the platform renegotiated geometry). The wire
                // SourceChanged message already went out BEFORE the media
                // dimensions change; keep the viewer surface geometry and
                // the picker's selected marker in sync here.
                tracing::info!(title = %title, width, height, "screen-share: source changed");
                self.calls_state.screen_share_src_size = Some((width, height));
                self.calls_state.screen_share_selected_source = Some(CaptureSourceId(source_id));
                iced::Task::none()
            }
            SessionEvent::CursorShape { sprite, .. } => {
                // BORU-SS-33: cache the new sprite and re-composite the
                // latest frame immediately so the shape update is visible
                // without waiting for a new video frame.
                self.calls_state.screen_share_cursor_sprite = Some(sprite);
                if let Some((w, h, pixels)) = self.calls_state.screen_share_cursor_frame_rgba.take()
                {
                    if let Some(handle) = self.screen_share_build_cursor_frame(w, h, pixels) {
                        self.calls_state.screen_share_frame_handle = Some(handle);
                    }
                }
                iced::Task::none()
            }
            SessionEvent::CursorPosition { x, y, visible, .. } => {
                // BORU-SS-33: update the overlay position and re-composite
                // the cached frame. The host sends CursorPosition per move
                // and skips re-encoding when only the cursor moved, so this
                // path must not require a new video frame.
                self.calls_state.screen_share_cursor_pos = Some((x, y));
                self.calls_state.screen_share_cursor_visible = visible;
                if let Some((w, h, pixels)) = self.calls_state.screen_share_cursor_frame_rgba.take()
                {
                    if let Some(handle) = self.screen_share_build_cursor_frame(w, h, pixels) {
                        self.calls_state.screen_share_frame_handle = Some(handle);
                    }
                }
                iced::Task::none()
            }
            SessionEvent::SourceUnavailable {
                reason, fallback, ..
            } => {
                // PDF Phase 10: monitor unplug / laptop dock-undock handled
                // gracefully. The host either fell back to another source
                // (toast, keep streaming) or paused the stream with no
                // source left (surface the PAUSED state so the sharer knows
                // frames stopped; picking a source from the panel resumes).
                match fallback {
                    Some(name) => {
                        let message = format!("Screen share paused — {reason} (using {name})");
                        tracing::warn!("{message}");
                        self.notifications_state.show_toast(message, 160);
                    }
                    None => {
                        tracing::warn!(reason = %reason, "screen-share: stream paused — no source available");
                        if self.calls_state.screen_share_host_state != ScreenShareHostState::Idle {
                            self.calls_state.screen_share_host_state = ScreenShareHostState::Paused;
                        }
                    }
                }
                iced::Task::none()
            }
            SessionEvent::Metrics { metrics, .. } => {
                // Developer diagnostics overlay (PDF Phase 12): keep the
                // latest host-side metrics (config + pipeline snapshot).
                // Local-only; contains no media payloads.
                self.calls_state.screen_share_host_metrics = Some(metrics);
                iced::Task::none()
            }
        }
    }

    #[cfg(feature = "screen-sharing")]
    /// Reset every viewer/host screen-share flag. Must mirror everything the
    /// start path sets so no boolean leak leaves the UI stuck.
    fn reset_screen_share_state(&mut self) {
        self.calls_state.screen_share_host_state = ScreenShareHostState::Idle;
        self.calls_state.screen_share_host_stop = None;
        self.calls_state.screen_share_host_cmd_tx = None;
        self.calls_state.screen_share_invite = None;
        self.calls_state.screen_share_viewing = false;
        self.calls_state.screen_share_view_session = None;
        self.calls_state.screen_share_viewing_peer = None;
        self.calls_state.screen_share_decode_stop = None;
        self.calls_state.screen_share_fullscreen = false;
        self.calls_state.screen_share_last_frame_ts = None;
        self.calls_state.screen_share_frame_handle = None;
        self.calls_state.screen_share_cursor_sprite = None;
        self.calls_state.screen_share_cursor_pos = None;
        self.calls_state.screen_share_cursor_visible = false;
        self.calls_state.screen_share_cursor_enabled = true;
        self.calls_state.screen_share_cursor_frame_rgba = None;
        self.calls_state.screen_share_frame_watch = None;
        self.calls_state.screen_share_stats_watch = None;
        self.calls_state.screen_share_viewer_stats = None;
        self.calls_state.screen_share_host_metrics = None;
        self.calls_state.screen_share_control_request = None;
        self.calls_state.screen_share_control_active = false;
        self.calls_state.screen_share_clipboard_active = false;
        self.calls_state.screen_share_audio_stop = None;
        self.calls_state.screen_share_audio_active = false;
        self.calls_state.screen_share_audio_error = None;
        self.calls_state.screen_share_last_pointer_sent = None;
        self.calls_state.screen_share_last_pointer_pos = None;
        self.calls_state.screen_share_modifiers = 0;
        self.calls_state.screen_share_view_mode = ScreenShareViewMode::default();
        self.calls_state.screen_share_pan = None;
        self.calls_state.screen_share_drag = None;
        self.calls_state.screen_share_hover = None;
        self.calls_state.screen_share_src_size = None;
        self.calls_state.screen_share_sources = None;
        self.calls_state.screen_share_selected_source = None;
        self.calls_state.screen_share_selected_preset = None;
        self.calls_state.screen_share_notice_ticks = 0;
    }

    /// Build the rendered handle for a fresh RGBA frame, compositing the
    /// remote cursor overlay (BORU-SS-33) when one is cached, enabled, and
    /// visible. Returns `None` only when the frame is unusable.
    #[cfg(feature = "screen-sharing")]
    fn screen_share_build_cursor_frame(
        &self,
        width: u32,
        height: u32,
        mut pixels: Vec<u8>,
    ) -> Option<iced::widget::image::Handle> {
        let expected = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        if pixels.len() != expected {
            return None;
        }
        if self.calls_state.screen_share_cursor_enabled
            && self.calls_state.screen_share_cursor_visible
        {
            if let (Some(sprite), Some((x, y))) = (
                &self.calls_state.screen_share_cursor_sprite,
                self.calls_state.screen_share_cursor_pos,
            ) {
                // Position is normalized against the shared source; scale to
                // this frame's pixel space (frame == source dimensions).
                let sx = ((x * width as f32).round() as i64).clamp(0, width as i64 - 1) as u32;
                let sy = ((y * height as f32).round() as i64).clamp(0, height as i64 - 1) as u32;
                composite_cursor_rgba(
                    &mut pixels,
                    width,
                    height,
                    SourcePoint { x: sx, y: sy },
                    sprite,
                );
            }
        }
        Some(iced::widget::image::Handle::from_rgba(
            width, height, pixels,
        ))
    }
}
// ── Call subscription (spec step 7: per-feature subscriptions) ──

struct CallRxHandle(Arc<Mutex<Receiver<CallEvent>>>);

impl std::hash::Hash for CallRxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

pub(crate) fn call_subscription(
    rx: Arc<Mutex<Receiver<CallEvent>>>,
) -> iced::Subscription<AppMessage> {
    iced::Subscription::run_with(CallRxHandle(rx), |handle| {
        let rx = Arc::clone(&handle.0);
        Box::pin(n0_future::stream::unfold(rx, |rx| async move {
            let event = rx.lock().await.recv().await?;
            Some((AppMessage::CallEventReceived(event), rx))
        }))
    })
}

// ── Screen-share subscriptions (BORU-APP-008) ────────────────────

#[cfg(feature = "screen-sharing")]
pub fn screen_share_keyboard_subscription() -> iced::Subscription<AppMessage> {
    use iced::keyboard::{self, key};
    keyboard::listen().filter_map(|event: keyboard::Event| -> Option<AppMessage> {
        match event {
            keyboard::Event::KeyPressed { key, .. } => {
                key_to_keysym(&key).map(|code| AppMessage::ScreenShareKeyEvent {
                    code,
                    pressed: true,
                })
            }
            keyboard::Event::KeyReleased { key, .. } => {
                key_to_keysym(&key).map(|code| AppMessage::ScreenShareKeyEvent {
                    code,
                    pressed: false,
                })
            }
            _ => None,
        }
    })
}

#[cfg(feature = "screen-sharing")]
/// Map a keysym to its held-modifier bit (PDF Task 9.2). Shift/Control/Alt/
/// Meta both sides map to the same bit; other keysyms return None.
fn keysym_modifier_bit(code: u32) -> Option<u32> {
    match code {
        0xFFE1 | 0xFFE2 => Some(MOD_SHIFT), // Shift_L / Shift_R
        0xFFE3 | 0xFFE4 => Some(MOD_CTRL),  // Control_L / Control_R
        0xFFE9 | 0xFFEA => Some(MOD_ALT),   // Alt_L / Alt_R
        0xFFE7 | 0xFFE8 | 0xFFEB | 0xFFEC => Some(MOD_META), // Meta / Super
        _ => None,
    }
}

#[cfg(feature = "screen-sharing")]
/// Map an iced `keyboard::Key` to a portable X11 keysym (the wire input code;
/// the Linux portal passes it straight to NotifyKeyboardKeysym, Windows maps
/// it to a virtual-key code). Unsupported keys map to None and are dropped.
fn key_to_keysym(key: &iced::keyboard::Key) -> Option<u32> {
    use iced::keyboard::key;
    match key {
        key::Key::Character(c) => c.chars().next().map(|ch| ch as u32),
        key::Key::Named(named) => Some(match named {
            key::Named::Enter => 0xFF0D,
            key::Named::Backspace => 0xFF08,
            key::Named::Tab => 0xFF09,
            key::Named::Space => 0x20,
            key::Named::Escape => 0xFF1B,
            key::Named::ArrowUp => 0xFF52,
            key::Named::ArrowDown => 0xFF54,
            key::Named::ArrowLeft => 0xFF51,
            key::Named::ArrowRight => 0xFF53,
            key::Named::Home => 0xFF50,
            key::Named::End => 0xFF57,
            key::Named::PageUp => 0xFF55,
            key::Named::PageDown => 0xFF56,
            key::Named::Insert => 0xFF63,
            key::Named::Delete => 0xFFFF,
            key::Named::Shift => 0xFFE1,
            key::Named::Control => 0xFFE3,
            key::Named::Alt => 0xFFE9,
            key::Named::AltGraph => 0xFFEA,
            key::Named::CapsLock => 0xFFE5,
            key::Named::F1 => 0xFFBE,
            key::Named::F2 => 0xFFBF,
            key::Named::F3 => 0xFFC0,
            key::Named::F4 => 0xFFC1,
            key::Named::F5 => 0xFFC2,
            key::Named::F6 => 0xFFC3,
            key::Named::F7 => 0xFFC4,
            key::Named::F8 => 0xFFC5,
            key::Named::F9 => 0xFFC6,
            key::Named::F10 => 0xFFC7,
            key::Named::F11 => 0xFFC8,
            key::Named::F12 => 0xFFC9,
            _ => return None,
        }),
        key::Key::Unidentified => None,
    }
}

#[cfg(feature = "screen-sharing")]
struct ScreenShareEventsRxHandle(Arc<Mutex<Receiver<SessionEvent>>>);

#[cfg(feature = "screen-sharing")]
impl std::hash::Hash for ScreenShareEventsRxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

#[cfg(feature = "screen-sharing")]
pub(crate) fn screen_share_events_subscription(
    rx: Option<Arc<Mutex<Receiver<SessionEvent>>>>,
) -> iced::Subscription<AppMessage> {
    // When screen sharing is unavailable, the fallback receiver is
    // intentionally closed. Its one-shot stream may end while the recipe is
    // still registered; that is benign because the recipe hash changes when
    // a real session receiver is installed, causing iced to spawn a fresh
    // stream. The application-lifetime call/network receivers must not use
    // this fallback pattern.
    let rx = rx.unwrap_or_else(|| {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(tx);
        Arc::new(tokio::sync::Mutex::new(rx))
    });
    iced::Subscription::run_with(ScreenShareEventsRxHandle(rx), |handle| {
        let rx = Arc::clone(&handle.0);
        Box::pin(n0_future::stream::unfold(rx, |rx| async move {
            let event = rx.lock().await.recv().await?;
            Some((AppMessage::ScreenShareEventReceived(event), rx))
        }))
    })
}

#[cfg(feature = "screen-sharing")]
struct ScreenShareFrameWatchHandle(Arc<Mutex<tokio::sync::watch::Receiver<Option<CapturedFrame>>>>);

#[cfg(feature = "screen-sharing")]
impl std::hash::Hash for ScreenShareFrameWatchHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

#[cfg(feature = "screen-sharing")]
pub(crate) fn screen_share_frame_subscription(
    watch: Option<Arc<Mutex<tokio::sync::watch::Receiver<Option<CapturedFrame>>>>>,
) -> iced::Subscription<AppMessage> {
    // This closed fallback is likewise a one-shot "not available" stream.
    // A real watch receiver has a different Arc identity and therefore a
    // different subscription recipe when screen sharing starts.
    let watch = watch.unwrap_or_else(|| {
        let (tx, rx) = tokio::sync::watch::channel(None);
        drop(tx);
        Arc::new(tokio::sync::Mutex::new(rx))
    });
    iced::Subscription::run_with(ScreenShareFrameWatchHandle(watch), |handle| {
        let rx = Arc::clone(&handle.0);
        Box::pin(n0_future::stream::unfold(rx, |rx| async move {
            let mut guard = rx.lock().await;
            if guard.changed().await.is_err() {
                return None;
            }
            let frame = guard.borrow_and_update().clone();
            drop(guard);
            Some((AppMessage::ScreenShareFrameReceived(frame), rx))
        }))
    })
}

#[cfg(feature = "screen-sharing")]
struct ScreenShareStatsWatchHandle(
    Arc<Mutex<tokio::sync::watch::Receiver<Option<ScreenShareStatsSnapshot>>>>,
);

#[cfg(feature = "screen-sharing")]
impl std::hash::Hash for ScreenShareStatsWatchHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

#[cfg(feature = "screen-sharing")]
pub(crate) fn screen_share_stats_subscription(
    watch: Option<Arc<Mutex<tokio::sync::watch::Receiver<Option<ScreenShareStatsSnapshot>>>>>,
) -> iced::Subscription<AppMessage> {
    // Closed fallback matches the frame-watch pattern: a real watch receiver
    // has a different Arc identity and therefore a different subscription
    // recipe when screen sharing starts.
    let watch = watch.unwrap_or_else(|| {
        let (tx, rx) = tokio::sync::watch::channel(None);
        drop(tx);
        Arc::new(tokio::sync::Mutex::new(rx))
    });
    iced::Subscription::run_with(ScreenShareStatsWatchHandle(watch), |handle| {
        let rx = Arc::clone(&handle.0);
        Box::pin(n0_future::stream::unfold(rx, |rx| async move {
            let mut guard = rx.lock().await;
            if guard.changed().await.is_err() {
                return None;
            }
            let stats = guard.borrow_and_update().clone();
            drop(guard);
            Some((AppMessage::ScreenShareStatsReceived(stats), rx))
        }))
    })
}


/// Whether call actions may be offered for the active conversation.
/// Call actions are restricted to established, unblocked direct friends, and
/// a second call must not be started while another call is active.
pub(crate) fn call_buttons_enabled(is_direct_friend: bool, is_blocked: bool, call_in_progress: bool) -> bool {
    is_direct_friend && !is_blocked && !call_in_progress
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_state_new_defaults_match_previous_inline_fields() {
        let state = CallsState::new();
        assert!(state.active_call_id.is_none());
        assert!(state.outgoing_call_peer.is_none());
        assert!(state.outgoing_call_status.is_none());
        assert!(!state.call_audio_muted);
        assert!(!state.call_camera_enabled);
        assert_eq!(state.call_camera_selection, "Front camera");
        assert!(state.call_started_at.is_none());
        assert!(state.call_kind.is_none());
        assert!(!state.call_was_incoming);
        assert!(!state.call_declined);
        assert!(state.incoming_call.is_none());
        #[cfg(feature = "screen-sharing")]
        {
            assert!(state.screen_share_events_rx.is_none());
            assert!(state.screen_share_media_rx.is_none());
            assert!(state.screen_share_audio_rx.is_none());
            assert!(!state.screen_share_audio_active);
            assert!(state.screen_share_audio_error.is_none());
            assert!(state.screen_share_events_tx.is_none());
            assert!(state.screen_share_protocol.is_none());
            assert!(state.screen_share_frame_watch.is_none());
            assert!(state.screen_share_stats_watch.is_none());
            assert!(state.screen_share_viewer_stats.is_none());
            assert!(state.screen_share_host_metrics.is_none());
            assert!(!state.screen_share_dev_overlay);
            assert_eq!(state.screen_share_host_state, ScreenShareHostState::Idle);
            assert!(state.screen_share_host_stop.is_none());
            assert!(state.screen_share_invite.is_none());
            assert!(!state.screen_share_viewing);
            assert!(state.screen_share_view_session.is_none());
            assert!(state.screen_share_decode_stop.is_none());
            assert!(!state.screen_share_fullscreen);
            assert!(state.screen_share_last_frame_ts.is_none());
            assert!(state.screen_share_frame_handle.is_none());
            assert!(state.screen_share_cursor_sprite.is_none());
            assert!(state.screen_share_cursor_pos.is_none());
            assert!(!state.screen_share_cursor_visible);
            assert!(
                state.screen_share_cursor_enabled,
                "cursor overlay defaults on"
            );
            assert!(state.screen_share_cursor_frame_rgba.is_none());
            assert!(state.screen_share_control_request.is_none());
            assert!(!state.screen_share_control_active);
            assert!(!state.screen_share_clipboard_active);
            assert!(state.screen_share_host_cmd_tx.is_none());
            assert!(state.screen_share_last_pointer_sent.is_none());
            assert!(state.screen_share_last_pointer_pos.is_none());
            assert_eq!(state.screen_share_modifiers, 0);
            assert_eq!(state.screen_share_view_mode, ScreenShareViewMode::default());
            assert!(state.screen_share_pan.is_none());
            assert!(state.screen_share_drag.is_none());
            assert!(state.screen_share_hover.is_none());
            assert!(state.screen_share_src_size.is_none());
            assert!(state.screen_share_sources.is_none());
            assert!(state.screen_share_selected_source.is_none());
            assert!(state.screen_share_selected_preset.is_none());
            assert!(state.screen_share_viewing_peer.is_none());
            assert_eq!(state.screen_share_notice_ticks, 0);
        }
    }

    #[test]
    fn select_camera_routes_through_domain_update() {
        let mut state = CallsState::new();
        // "next" cycles between the two supported labels.
        state.update(CallsMessage::SelectCamera("next".to_string()));
        assert_eq!(state.call_camera_selection, "Back camera");
        state.update(CallsMessage::SelectCamera("next".to_string()));
        assert_eq!(state.call_camera_selection, "Front camera");
        // An explicit label from the settings picker is stored verbatim.
        state.update(CallsMessage::SelectCamera("USB Camera".to_string()));
        assert_eq!(state.call_camera_selection, "USB Camera");
        // No-op selectors keep the domain message surface coherent.
        state.update(CallsMessage::SelectMicrophone("Default".to_string()));
        state.update(CallsMessage::SelectSpeaker("Default".to_string()));
        state.update(CallsMessage::CallUiTick);
        assert_eq!(state.call_camera_selection, "USB Camera");
    }

    #[cfg(feature = "screen-sharing")]
    #[test]
    fn screen_share_view_and_pan_transitions_are_state_only() {
        let mut state = CallsState::new();
        state.screen_share_src_size = Some((1920, 1080));

        // SetView: explicit pan preserved in zoomed modes, reset in Fit/Actual.
        state.update(CallsMessage::ScreenShareSetView {
            mode: ScreenShareViewMode::Zoom(2.0),
            pan: Some((100.0, 200.0)),
        });
        assert_eq!(state.screen_share_view_mode, ScreenShareViewMode::Zoom(2.0));
        assert_eq!(state.screen_share_pan, Some((100.0, 200.0)));

        state.update(CallsMessage::ScreenShareSetView {
            mode: ScreenShareViewMode::Fit,
            pan: None,
        });
        assert_eq!(state.screen_share_view_mode, ScreenShareViewMode::Fit);
        assert_eq!(state.screen_share_pan, None);

        // Pan drag: start sets drag/hover, move pans the center clamped to
        // the source size, end clears the drag.
        state.update(CallsMessage::ScreenSharePanStart {
            pos: iced::Point::new(10.0, 10.0),
        });
        assert_eq!(state.screen_share_drag, Some(iced::Point::new(10.0, 10.0)));
        state.update(CallsMessage::ScreenSharePanMove {
            pos: iced::Point::new(30.0, 20.0),
            scale: 2.0,
        });
        let pan = state.screen_share_pan.expect("pan set after drag move");
        assert!(pan.0 > 0.0 && pan.1 > 0.0);
        assert!(pan.0 <= 1920.0 && pan.1 <= 1080.0);
        state.update(CallsMessage::ScreenSharePanEnd);
        assert_eq!(state.screen_share_drag, None);
    }

    #[cfg(feature = "screen-sharing")]
    #[test]
    fn screen_share_fullscreen_and_notice_transitions() {
        let mut state = CallsState::new();
        assert!(!state.screen_share_fullscreen);
        state.update(CallsMessage::ToggleScreenShareFullscreen);
        assert!(state.screen_share_fullscreen);
        state.update(CallsMessage::ToggleScreenShareFullscreen);
        assert!(!state.screen_share_fullscreen);

        // Terminal notice states clear back to Idle on dismissal.
        state.screen_share_host_state = ScreenShareHostState::Error("capture ended".to_string());
        state.screen_share_notice_ticks = 4;
        state.update(CallsMessage::ScreenShareDismissNotice);
        assert_eq!(state.screen_share_host_state, ScreenShareHostState::Idle);
        assert_eq!(state.screen_share_notice_ticks, 0);
    }
}
