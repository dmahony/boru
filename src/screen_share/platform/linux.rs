//! Linux ScreenCast backend.
//!
//! Two layers live here:
//!
//! 1. [`PortalCapture`] — the portal/PipeWire state machine and bounded frame
//!    queue (kept for API compatibility and tests).
//! 2. [`LinuxPortalCapture`] — the REAL capture backend: an
//!    xdg-desktop-portal ScreenCast client (zbus) that obtains portal
//!    consent and negotiates a PipeWire stream, plus a dlopen-based PipeWire
//!    client that consumes buffers and feeds them into the CPU frame path.
//!    This mirrors the fail-closed connect pattern of
//!    `remote_input::LinuxPortalRemoteInput::connect()`.
//! 3. [`PortalSessionMachine`] — pure D-Bus lifecycle state machine
//!    (create → select → start → stream → close, plus every failure path),
//!    unit-tested without a session bus/portal/compositor.
//! 4. [`X11Capture`] — direct X11 GetImage capture (PDF Task 6.1): RandR
//!    monitor enumeration + selected-geometry capture behind
//!    [`DesktopCaptureBackend`], with whole-root [`ScreenCapture`] used by
//!    the `ActiveCapture::X11` fallback path. Display-server detection
//!    ([`detect_display_server`]) decides whether the portal or the direct
//!    backend is preferred under Wayland/XWayland vs native X11.
//!
//! The live zbus connection and session object path are kept for the whole
//! capture lifetime and teardown is explicit: [`LinuxPortalCapture::close`]
//! stops the PipeWire capture thread and calls
//! `org.freedesktop.portal.Session.Close`; [`Drop`] repeats it best-effort.
//!
//! The PipeWire client is deliberately dlopen-based (`libpipewire-0.3.so.0`,
//! a runtime dependency present on any desktop with xdg-desktop-portal) so
//! building does not require PipeWire development headers. When the session
//! bus, portal, or PipeWire is unavailable the factory fails closed and the
//! caller falls back to the synthetic [`TestPatternCapture`].
#![allow(missing_docs)]

use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CString};
use std::sync::mpsc::{sync_channel, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::screen_share::{
    capture::FrameSink, CapturedFrame, CaptureConfig, CaptureSource, CaptureSourceId,
    CaptureSourceKind, CursorMeta, DesktopCaptureBackend, DesktopPoint, DirtyRegion, FrameRect,
    MonitorGeometry, PixelFormat, ScreenCapture, ScreenShareError, TestPatternCapture,
};
use super::linux_pw::{
    build_format_pod, normalize_buffer, parse_format_pod, parse_spa_cursor_meta, NegotiatedFormat,
    SPA_META_Cursor, SPA_PARAM_Buffers, SPA_PARAM_Format,
};
use super::windows_common::monitor_source_id;
use x11rb::connection::Connection as _;
use x11rb::protocol::damage::{self, ConnectionExt as _};
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder};

/// State of the XDG ScreenCast portal session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalState {
    Idle,
    Selecting,
    Streaming,
    Ending,
    Ended,
}

/// A portal-approved Linux capture session fed by PipeWire buffers.
#[derive(Debug)]
pub struct PortalCapture {
    state: PortalState,
    sink: FrameSink,
    format: Option<(u32, u32, PixelFormat)>,
    pending_events: VecDeque<PortalEvent>,
}

/// Lifecycle and format events emitted by the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalEvent {
    SourcePickerOpened,
    SourceSelected,
    FormatChanged { width: u32, height: u32 },
    Ended,
}

impl PortalCapture {
    /// Create an idle session with a bounded frame queue.
    pub fn new(queue_capacity: usize) -> Result<Self, ScreenShareError> {
        Ok(Self {
            state: PortalState::Idle,
            sink: FrameSink::new(queue_capacity)?,
            format: None,
            pending_events: VecDeque::new(),
        })
    }
    /// Request the native XDG Desktop Portal source picker.
    pub fn begin_selection(&mut self) -> Result<(), ScreenShareError> {
        if self.state != PortalState::Idle {
            return Err(ScreenShareError::new("portal session is already active"));
        }
        self.state = PortalState::Selecting;
        self.pending_events
            .push_back(PortalEvent::SourcePickerOpened);
        Ok(())
    }
    /// Handle a portal cancellation without leaving PipeWire resources alive.
    pub fn cancel(&mut self) {
        if matches!(self.state, PortalState::Selecting | PortalState::Streaming) {
            self.state = PortalState::Ended;
            self.pending_events.push_back(PortalEvent::Ended);
        }
    }
    /// Mark the portal stream as selected and ready to receive PipeWire buffers.
    pub fn source_selected(&mut self) -> Result<(), ScreenShareError> {
        if self.state != PortalState::Selecting {
            return Err(ScreenShareError::new(
                "portal source was not being selected",
            ));
        }
        self.state = PortalState::Streaming;
        self.pending_events.push_back(PortalEvent::SourceSelected);
        Ok(())
    }
    /// Normalize one PipeWire BGRA/RGBA buffer and enqueue it.
    pub fn push_pipewire_frame(&mut self, frame: CapturedFrame) -> Result<(), ScreenShareError> {
        if self.state != PortalState::Streaming {
            return Err(ScreenShareError::new(
                "PipeWire frame received outside streaming state",
            ));
        }
        let current = (frame.width, frame.height, frame.pixel_format);
        if self.format.map(|f| (f.0, f.1)) != Some((frame.width, frame.height)) {
            if self.format.is_some() {
                self.pending_events.push_back(PortalEvent::FormatChanged {
                    width: frame.width,
                    height: frame.height,
                });
            }
            self.format = Some(current);
        }
        self.sink.push(frame);
        Ok(())
    }
    /// Signal that the OS/portal closed the stream.
    pub fn stream_closed(&mut self) {
        self.state = PortalState::Ended;
        self.pending_events.push_back(PortalEvent::Ended);
    }
    /// Read the next lifecycle event.
    pub fn next_event(&mut self) -> Option<PortalEvent> {
        self.pending_events.pop_front()
    }
    /// Return bounded queue diagnostics: captured, encoded, dropped.
    pub fn counters(&self) -> (u64, u64, u64) {
        self.sink.counters()
    }
    /// Current portal state.
    pub fn state(&self) -> PortalState {
        self.state
    }
}

impl ScreenCapture for PortalCapture {
    fn capture(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
        Ok(self.sink.pop_latest())
    }
}

// ── Portal session lifecycle state machine ─────────────────────────────────
//
// The D-Bus layer is deliberately outside [`PortalSessionMachine`]: the
// machine only records what has happened and what may happen next, so the
// full ScreenCast lifecycle (create → select → start → stream → close, plus
// every failure path) is unit-testable on Linux without a session bus,
// portal, or compositor. [`LinuxPortalCapture`] drives the machine with real
// zbus calls.

/// Reason a portal session reached a failed terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFailure {
    /// The D-Bus session bus is unreachable.
    NoSessionBus,
    /// `CreateSession` returned an error or a malformed session path.
    CreateSessionFailed,
    /// `SelectSources` returned an error.
    SelectSourcesFailed,
    /// `Start` failed at the D-Bus transport level (call or reply malformed).
    StartFailed,
    /// `Start` completed with a non-zero response code (user denied or portal
    /// error). Carries the portal response code.
    StartRejected(u32),
    /// `Start` did not complete before the portal timeout.
    StartTimeout,
    /// The Request object's `Response` signal stream closed before a response.
    ResponseStreamClosed,
    /// `Start` succeeded but the reply carried no usable stream node id.
    MissingNodeId,
}

/// Lifecycle phase of an xdg-desktop-portal ScreenCast session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    /// No session has been created yet.
    Idle,
    /// `CreateSession` is in flight, awaiting the session object path.
    Creating,
    /// Session created; `SelectSources` has been issued.
    Selecting,
    /// `Start` has been issued; awaiting the asynchronous `Response` signal.
    Starting,
    /// `Start` returned success; a PipeWire node id is available and frames
    /// may be captured.
    Streaming,
    /// Clean teardown requested (`Session.Close` + PipeWire stop in flight).
    Closing,
    /// Terminal: session closed (by us or by the portal).
    Closed,
    /// Terminal: the session failed with a reason.
    Failed(SessionFailure),
}

/// Error returned for an invalid portal state-machine transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineError {
    /// The transition is not legal from the current phase.
    InvalidTransition { from: SessionPhase },
}

impl std::fmt::Display for MachineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTransition { from } => {
                write!(f, "invalid portal transition from {from:?}")
            }
        }
    }
}

impl std::error::Error for MachineError {}

/// Pure state machine for the xdg-desktop-portal ScreenCast session lifecycle.
///
/// The machine enforces the portal call ordering (`CreateSession` →
/// `SelectSources` → `Start` → `Response`), tracks every terminal failure,
/// and models clean teardown (`begin_close` → `on_closed`) plus
/// portal-initiated close (`on_portal_closed`). Invalid transitions return
/// [`MachineError`]; once terminal, every further transition is rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortalSessionMachine {
    phase: SessionPhase,
    /// Number of close requests (idempotency diagnostics for tests).
    close_requests: u32,
}

impl Default for PortalSessionMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl PortalSessionMachine {
    /// A brand-new session in [`SessionPhase::Idle`].
    pub fn new() -> Self {
        Self {
            phase: SessionPhase::Idle,
            close_requests: 0,
        }
    }

    /// Current lifecycle phase.
    pub fn phase(&self) -> SessionPhase {
        self.phase
    }

    /// Number of close requests made so far.
    pub fn close_requests(&self) -> u32 {
        self.close_requests
    }

    /// True once the session reached a terminal state (closed or failed).
    pub fn is_terminal(&self) -> bool {
        matches!(self.phase, SessionPhase::Closed | SessionPhase::Failed(_))
    }

    fn transition(&mut self, from: SessionPhase, to: SessionPhase) -> Result<(), MachineError> {
        if self.phase != from {
            return Err(MachineError::InvalidTransition { from: self.phase });
        }
        self.phase = to;
        Ok(())
    }

    /// Idle → Creating: `CreateSession` is issued.
    pub fn create_session(&mut self) -> Result<(), MachineError> {
        self.transition(SessionPhase::Idle, SessionPhase::Creating)
    }

    /// Creating → Selecting: the portal returned a session object path.
    pub fn on_session_created(&mut self) -> Result<(), MachineError> {
        self.transition(SessionPhase::Creating, SessionPhase::Selecting)
    }

    /// Selecting → Starting: `SelectSources` succeeded; `Start` is issued next.
    pub fn select_sources(&mut self) -> Result<(), MachineError> {
        self.transition(SessionPhase::Selecting, SessionPhase::Starting)
    }

    /// Validates that `Start` is in flight (no state change).
    pub fn start(&mut self) -> Result<(), MachineError> {
        self.transition(SessionPhase::Starting, SessionPhase::Starting)
    }

    /// Starting → Streaming: `Response(0)` arrived and the stream node id was
    /// extracted. Frames may now be captured.
    pub fn on_start_response_ok(&mut self) -> Result<(), MachineError> {
        self.transition(SessionPhase::Starting, SessionPhase::Streaming)
    }

    /// Starting → Failed: the portal rejected the source selection.
    pub fn on_start_response_rejected(&mut self, code: u32) -> Result<(), MachineError> {
        self.transition(
            SessionPhase::Starting,
            SessionPhase::Failed(SessionFailure::StartRejected(code)),
        )
    }

    /// Starting → Failed: no response arrived within the portal timeout.
    pub fn on_start_timeout(&mut self) -> Result<(), MachineError> {
        self.transition(SessionPhase::Starting, SessionPhase::Failed(SessionFailure::StartTimeout))
    }

    /// Starting → Failed: the Request object's Response stream closed.
    pub fn on_response_stream_closed(&mut self) -> Result<(), MachineError> {
        self.transition(
            SessionPhase::Starting,
            SessionPhase::Failed(SessionFailure::ResponseStreamClosed),
        )
    }

    /// Starting → Failed: `Response(0)` arrived but the streams array was
    /// unusable (no `node_id` entry).
    pub fn on_missing_node_id(&mut self) -> Result<(), MachineError> {
        self.transition(SessionPhase::Starting, SessionPhase::Failed(SessionFailure::MissingNodeId))
    }

    /// any active phase → Failed: record a D-Bus transport failure.
    pub fn on_failure(&mut self, failure: SessionFailure) -> Result<(), MachineError> {
        if self.is_terminal() {
            return Err(MachineError::InvalidTransition { from: self.phase });
        }
        self.phase = SessionPhase::Failed(failure);
        Ok(())
    }

    /// any active phase → Closed: the portal/compositor ended the session
    /// (user revoked the share from the DE, compositor stopped, etc.).
    pub fn on_portal_closed(&mut self) -> Result<(), MachineError> {
        if self.is_terminal() {
            return Err(MachineError::InvalidTransition { from: self.phase });
        }
        self.phase = SessionPhase::Closed;
        Ok(())
    }

    /// any active phase → Closing: clean teardown requested. Rejected when a
    /// close is already in flight or the session is terminal (close is
    /// once-per-session).
    pub fn begin_close(&mut self) -> Result<(), MachineError> {
        if self.is_terminal() || self.phase == SessionPhase::Closing {
            return Err(MachineError::InvalidTransition { from: self.phase });
        }
        self.close_requests += 1;
        self.phase = SessionPhase::Closing;
        Ok(())
    }

    /// Closing → Closed: `Session.Close` completed (or the connection dropped).
    pub fn on_closed(&mut self) -> Result<(), MachineError> {
        self.transition(SessionPhase::Closing, SessionPhase::Closed)
    }
}

// ── Desktop environment / session detection (GNOME, KDE, wlroots) ──────────

/// Session type reported by `XDG_SESSION_TYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionType {
    Wayland,
    X11,
    #[default]
    Unknown,
}

/// Desktop environment reported by `XDG_CURRENT_DESKTOP` (best effort).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DesktopEnvironment {
    Gnome,
    Kde,
    /// A wlroots-based compositor (sway, Hyprland, wayfire, …).
    Wlroots,
    /// Another environment that sets `XDG_CURRENT_DESKTOP`.
    Other,
    #[default]
    Unknown,
}

/// Classify a `XDG_SESSION_TYPE` value. Pure for tests.
pub fn classify_session_type(value: &str) -> SessionType {
    match value {
        "wayland" => SessionType::Wayland,
        "x11" => SessionType::X11,
        _ => SessionType::Unknown,
    }
}

/// Classify a `XDG_CURRENT_DESKTOP` value. Pure for tests.
pub fn classify_desktop_environment(value: &str) -> DesktopEnvironment {
    let desktop = value.to_ascii_lowercase();
    if desktop.contains("gnome") {
        DesktopEnvironment::Gnome
    } else if desktop.contains("kde") || desktop.contains("plasma") {
        DesktopEnvironment::Kde
    } else if desktop.contains("wlroots")
        || ["sway", "hyprland", "wayfire", "river", "labwc", "cage", "gamescope", "dwl"]
            .iter()
            .any(|compositor| desktop.contains(compositor))
    {
        DesktopEnvironment::Wlroots
    } else if desktop.is_empty() {
        DesktopEnvironment::Unknown
    } else {
        DesktopEnvironment::Other
    }
}

/// The current session type from the environment (unset → [`SessionType::Unknown`]).
pub fn detect_session_type() -> SessionType {
    std::env::var("XDG_SESSION_TYPE")
        .map(|value| classify_session_type(&value))
        .unwrap_or_default()
}

/// The current desktop environment from the environment (unset →
/// [`DesktopEnvironment::Unknown`]).
pub fn detect_desktop_environment() -> DesktopEnvironment {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|value| classify_desktop_environment(&value))
        .unwrap_or_default()
}

// ── Display-server detection (Wayland / XWayland / X11) ────────────────────
//
// PDF Task 6.1: "Detect when Boru is actually running under Wayland/XWayland
// and prefer the portal backend when appropriate." A direct X11 GetImage
// capture only sees XWayland windows when the session is Wayland, so the
// portal ScreenCast backend must be preferred there. Under a native X11
// session the direct backend needs no portal daemon at all.

/// Which display server the process is running under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayServer {
    /// A real Wayland session (`WAYLAND_DISPLAY` set and/or
    /// `XDG_SESSION_TYPE=wayland`), with no usable X server.
    Wayland,
    /// A Wayland session where `DISPLAY` also points at an XWayland server.
    /// Direct X11 capture would only see XWayland windows, so the portal is
    /// the correct capture path.
    XWayland,
    /// A native X11 session (`XDG_SESSION_TYPE=x11` or only `DISPLAY` set).
    /// Direct X11 capture works without a portal daemon.
    X11,
    #[default]
    Unknown,
}

/// Classify the display server from the three environment variables that
/// decide it. Pure for tests; callers pass `Option<&str>` (unset vars → `None`).
pub fn classify_display_server(
    wayland_display: Option<&str>,
    xdg_session_type: Option<&str>,
    display: Option<&str>,
) -> DisplayServer {
    let wayland_session =
        wayland_display.is_some() || xdg_session_type == Some("wayland");
    if wayland_session {
        if display.is_some() {
            DisplayServer::XWayland
        } else {
            DisplayServer::Wayland
        }
    } else if xdg_session_type == Some("x11") || display.is_some() {
        DisplayServer::X11
    } else {
        DisplayServer::Unknown
    }
}

impl DisplayServer {
    /// Whether the xdg-desktop-portal ScreenCast backend should be preferred
    /// over the direct X11 backend. True for Wayland and XWayland (the only
    /// sessions where a direct X11 capture is wrong or incomplete).
    pub fn prefers_portal(self) -> bool {
        matches!(self, DisplayServer::Wayland | DisplayServer::XWayland)
    }
}

/// The display server Boru is actually running under, from the environment.
pub fn detect_display_server() -> DisplayServer {
    classify_display_server(
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        std::env::var("DISPLAY").ok().as_deref(),
    )
}

// ── Portal cursor modes (PDF Task 5.3) ──────────────────────────────────────
//
// xdg-desktop-portal ScreenCast lets the client request how the cursor is
// drawn in the stream via the `cursor_mode` option of SelectSources
// (available since interface version 2). The portal advertises the supported
// modes in the `AvailableCursorModes` property; requesting a mode that is not
// advertised makes the portal CLOSE the session, so we only ever request a
// mode the portal advertises (verified against upstream docs, BORU-SS-15).

/// Cursor mode bit values from `org.freedesktop.portal.ScreenCast`
/// `AvailableCursorModes` / `cursor_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMode {
    /// 1: the cursor is not part of the screen cast stream.
    Hidden = 1,
    /// 2: the cursor is embedded as part of the stream buffers.
    Embedded = 2,
    /// 4: the cursor is sent as PipeWire stream metadata (not composited).
    Metadata = 4,
}

impl CursorMode {
    /// The portal bit value for this mode.
    pub fn bit(self) -> u32 {
        self as u32
    }
}

/// Choose the cursor mode to request given the portal's advertised bitmask.
///
/// Boru prefers `Metadata` (BORU-SS-33 / PDF Task 5.3): the compositor sends
/// the cursor as `spa_meta_cursor` stream metadata instead of compositing it
/// into the PipeWire buffers, so the host can forward shape-on-change +
/// position-per-move control messages and SKIP re-encoding the frame when
/// only the cursor moved. `Embedded` is the fallback for portals that do not
/// advertise metadata mode — the cursor is baked into the buffers and every
/// viewer sees it with zero extra work (BORU-SS-12 fallback). Never request a
/// mode the portal advertises as unavailable (that closes the session).
pub fn choose_cursor_mode(available: u32) -> CursorMode {
    if available & CursorMode::Metadata.bit() != 0 {
        CursorMode::Metadata
    } else if available & CursorMode::Embedded.bit() != 0 {
        CursorMode::Embedded
    } else {
        CursorMode::Hidden
    }
}

/// Build the `SelectSources` options vardict: monitor source types plus the
/// requested cursor mode when one was negotiated. Pure so it is unit-testable
/// without a session bus.
pub fn select_sources_options(
    cursor_mode: Option<CursorMode>,
) -> std::collections::HashMap<&'static str, zbus::zvariant::Value<'static>> {
    let mut options: std::collections::HashMap<&str, zbus::zvariant::Value<'static>> =
        [("types", zbus::zvariant::Value::U32(1))].into_iter().collect();
    if let Some(mode) = cursor_mode {
        options.insert("cursor_mode", zbus::zvariant::Value::U32(mode.bit()));
    }
    options
}

/// Query the ScreenCast `AvailableCursorModes` property via
/// `org.freedesktop.DBus.Properties.Get`. Best-effort diagnostics; `None`
/// when the portal is too old (property added in interface version 2) or the
/// call fails — callers then default to `Hidden` without sending the option.
async fn query_available_cursor_modes(connection: &zbus::Connection) -> Option<u32> {
    let reply = connection
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.portal.ScreenCast", "AvailableCursorModes"),
        )
        .await
        .ok()?;
    let value: zbus::zvariant::OwnedValue = reply.body().deserialize().ok()?;
    match &*value {
        zbus::zvariant::Value::U32(available) => Some(*available),
        _ => None,
    }
}

// ── Real XDG Desktop Portal ScreenCast + PipeWire capture ───────────────────
//
// ScreenCast flow (org.freedesktop.portal.ScreenCast on the session bus):
//   1. CreateSession(session_handle_token) → session object path.
//   2. SelectSources(session, {types}) — monitor sources on X11 auto-select
//      the primary monitor; on Wayland the compositor shows the picker.
//   3. Start(session, "", {handle_token}) — blocks until a source is chosen,
//      returns a PipeWire node id.
//   4. Connect a PipeWire INPUT stream to that node and consume buffers.
// The PipeWire client is dlopen'd at runtime; all PipeWire objects live on a
// dedicated thread so raw pointers never cross threads.

// Negotiated stream geometry + wire layout live in `linux_pw::NegotiatedFormat`
// (pure, unit-testable, shared with the future DMA-BUF path).

/// The real Linux capture backend: portal consent + PipeWire stream.
///
/// The live zbus connection and the ScreenCast session object path are kept
/// for the whole capture lifetime: xdg-desktop-portal tears a session down
/// when the creating client disconnects from the session bus, so dropping the
/// connection (as the previous version did right after `Start`) would kill
/// the stream server-side. Teardown is explicit: [`LinuxPortalCapture::close`]
/// stops the PipeWire thread, calls `org.freedesktop.portal.Session.Close`,
/// and marks the lifecycle machine [`SessionPhase::Closed`]; [`Drop`] performs
/// the same cleanup best-effort.
#[derive(Debug)]
pub struct LinuxPortalCapture {
    portal: PortalCapture,
    machine: PortalSessionMachine,
    frames: Receiver<CapturedFrame>,
    events: Receiver<PortalEvent>,
    format: Arc<Mutex<NegotiatedFormat>>,
    /// Cross-thread handle used to stop the PipeWire capture thread.
    pipewire: Option<PipeWireHandle>,
    /// Live session-bus connection kept for the session lifetime.
    connection: Option<zbus::Connection>,
    /// The ScreenCast session object path (for `Session.Close`).
    session_path: Option<zbus::zvariant::OwnedObjectPath>,
    /// Detected desktop environment (diagnostics / error context).
    environment: DesktopEnvironment,
    /// ScreenCast interface version reported by the portal, if queryable.
    portal_version: Option<u32>,
    /// Detected portal backend bus names (diagnostics / error context).
    backend: Option<String>,
    /// Cursor mode negotiated with the portal (PDF Task 5.3). `Embedded`
    /// means the compositor bakes the cursor into the PipeWire buffers;
    /// `Hidden` means the stream has no cursor.
    cursor_mode: CursorMode,
}

impl LinuxPortalCapture {
    /// Portal timeout for the interactive `Start` call. A desktop user is
    /// expected to pick a source; headless/CI environments fail closed.
    pub const PORTAL_TIMEOUT: Duration = Duration::from_secs(20);

    /// Timeout for the `Session.Close` teardown call.
    pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);

    /// Establish a full ScreenCast session: portal consent, PipeWire stream,
    /// and the capture object that yields real desktop frames. Fails closed
    /// (Err) when no session bus, portal, or PipeWire server is reachable.
    ///
    /// The desktop environment and portal backend are detected for
    /// diagnostics; the D-Bus flow itself is the same on GNOME, KDE Plasma 6,
    /// and wlroots-style portals (the frontend normalises backend quirks).
    pub async fn connect() -> Result<Self, ScreenShareError> {
        let environment = detect_desktop_environment();
        let session_type = detect_session_type();
        let mut machine = PortalSessionMachine::new();
        let connection = zbus::Connection::session()
            .await
            .map_err(|e| {
                let _ = machine.on_failure(SessionFailure::NoSessionBus);
                ScreenShareError::missing_portal(format!(
                    "no session bus — is xdg-desktop-portal available in this desktop session? (session={session_type:?}, desktop={environment:?}): {e}"
                ))
            })?;
        let portal = (
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.ScreenCast",
        );
        let portal_version = query_portal_version(&connection).await;
        let backend = detect_portal_backend(&connection).await;
        let available_cursor_modes = query_available_cursor_modes(&connection).await;
        let cursor_mode = available_cursor_modes.map(choose_cursor_mode).unwrap_or(CursorMode::Hidden);
        tracing::info!(
            session_type = ?session_type,
            desktop = ?environment,
            ?portal_version,
            backend = ?backend,
            available_cursor_modes = ?available_cursor_modes,
            ?cursor_mode,
            "screen-share: connecting to xdg-desktop-portal ScreenCast"
        );

        // 1. CreateSession(session_handle_token) → session object path.
        let _ = machine.create_session();
        let token = format!("boru_{:016x}", rand::random::<u64>());
        let options: std::collections::HashMap<&str, zbus::zvariant::Value> = [(
            "session_handle_token",
            zbus::zvariant::Value::from(token.as_str()),
        )]
        .into_iter()
        .collect();
        let reply = connection
            .call_method(Some(portal.0), portal.1, Some(portal.2), "CreateSession", &options)
            .await
            .map_err(|e| {
                let _ = machine.on_failure(SessionFailure::CreateSessionFailed);
                ScreenShareError::new(format!(
                    "portal CreateSession failed (desktop={environment:?}, backend={backend:?}, version={portal_version:?}): {e}"
                ))
            })?;
        let session: zbus::zvariant::OwnedObjectPath = reply
            .body()
            .deserialize()
            .map_err(|e| {
                let _ = machine.on_failure(SessionFailure::CreateSessionFailed);
                ScreenShareError::new(format!("portal session reply malformed: {e}"))
            })?;
        let _ = machine.on_session_created();

        // 2. SelectSources(types = Monitor [, cursor_mode]). No `multiple`
        // option: exactly one stream is requested, which every portal
        // implementation supports. The desktop-environment permission dialog
        // is NEVER bypassed — on Wayland the compositor shows its picker at
        // Start, on X11 the portal auto-selects the primary monitor.
        // Cursor handling (PDF Task 5.3): request `Embedded` when the portal
        // advertises it so the compositor bakes the cursor into the stream
        // buffers (matching Boru's composite-into-frames strategy); otherwise
        // omit the option entirely (portal default = Hidden). Requesting an
        // unadvertised mode would close the session, so we only send the
        // option when the portal told us it is available.
        let cursor_option = available_cursor_modes.map(choose_cursor_mode);
        let select_options = select_sources_options(cursor_option);
        connection
            .call_method(Some(portal.0), portal.1, Some(portal.2), "SelectSources", &(session.clone(), select_options))
            .await
            .map_err(|e| {
                let _ = machine.on_failure(SessionFailure::SelectSourcesFailed);
                ScreenShareError::new(format!(
                    "portal SelectSources failed (desktop={environment:?}, backend={backend:?}): {e}"
                ))
            })?;
        let _ = machine.select_sources();

        // 3. Start(session, "", {handle_token}) — blocks until the user picks
        // a source on Wayland; bound so headless environments fail closed
        // instead of hanging the session. Portal requests complete
        // asynchronously: Start returns a request object path and emits
        // Response(u32, a{sv}) on that path. Waiting for the method reply body
        // here would never yield the stream list.
        let start_token = format!("boru_start_{:016x}", rand::random::<u64>());
        let start_options: std::collections::HashMap<&str, zbus::zvariant::Value> =
            [("handle_token", zbus::zvariant::Value::from(start_token.as_str()))]
                .into_iter()
                .collect();
        let _ = machine.start();
        let request_path: zbus::zvariant::OwnedObjectPath = tokio::time::timeout(
            Self::PORTAL_TIMEOUT,
            connection.call_method(Some(portal.0), portal.1, Some(portal.2), "Start", &(session.clone(), "", start_options)),
        )
        .await
        .map_err(|_| {
            let _ = machine.on_start_timeout();
            ScreenShareError::new(format!(
                "portal Start timed out (no source selected; desktop={environment:?}, backend={backend:?})"
            ))
        })?
        .map_err(|e| {
            let _ = machine.on_failure(SessionFailure::StartFailed);
            ScreenShareError::new(format!("portal Start failed: {e}"))
        })?
        .body()
        .deserialize()
        .map_err(|e| {
            let _ = machine.on_failure(SessionFailure::StartFailed);
            ScreenShareError::new(format!("portal Start request malformed: {e}"))
        })?;
        let request = zbus::Proxy::new(
            &connection,
            portal.0,
            request_path.as_str(),
            "org.freedesktop.portal.Request",
        )
        .await
        .map_err(|e| {
            let _ = machine.on_failure(SessionFailure::StartFailed);
            ScreenShareError::new(format!("portal request proxy failed: {e}"))
        })?;
        let mut responses = request
            .receive_signal("Response")
            .await
            .map_err(|e| {
                let _ = machine.on_failure(SessionFailure::StartFailed);
                ScreenShareError::new(format!("portal response subscription failed: {e}"))
            })?;
        let response = tokio::time::timeout(Self::PORTAL_TIMEOUT, n0_future::StreamExt::next(&mut responses))
            .await
            .map_err(|_| {
                let _ = machine.on_start_timeout();
                ScreenShareError::new(format!(
                    "portal Start timed out waiting for the picker response (desktop={environment:?}, backend={backend:?})"
                ))
            })?
            .ok_or_else(|| {
                let _ = machine.on_response_stream_closed();
                ScreenShareError::new("portal response stream closed")
            })?;
        let (response_code, body): (u32, zbus::zvariant::OwnedValue) = response
            .body()
            .deserialize()
            .map_err(|e| {
                let _ = machine.on_failure(SessionFailure::StartFailed);
                ScreenShareError::new(format!("portal response malformed: {e}"))
            })?;
        if response_code != 0 {
            let _ = machine.on_start_response_rejected(response_code);
            return Err(ScreenShareError::new(format!(
                "portal source selection rejected ({response_code}; desktop={environment:?}, backend={backend:?})"
            )));
        }
        let node_id = extract_stream_node_id(&body).ok_or_else(|| {
            let _ = machine.on_missing_node_id();
            ScreenShareError::new("portal Start reply missing stream node id")
        })?;
        let _ = machine.on_start_response_ok();

        // 4. Connect a PipeWire INPUT stream to the returned node id. All
        // PipeWire objects live on a dedicated thread (see PipeWireClient).
        let (frame_tx, frames) = sync_channel::<CapturedFrame>(4);
        let (event_tx, events) = sync_channel::<PortalEvent>(4);
        let format = Arc::new(Mutex::new(NegotiatedFormat::default()));
        let pipewire = PipeWireClient::connect(node_id, frame_tx, event_tx, format.clone())
            .map_err(|e| ScreenShareError::new(format!("PipeWire capture failed: {e}")))?;

        let mut portal = PortalCapture::new(4)?;
        portal.source_selected()?;
        Ok(Self {
            portal,
            machine,
            frames,
            events,
            format,
            pipewire: Some(pipewire),
            connection: Some(connection),
            session_path: Some(session),
            environment,
            portal_version,
            backend,
            cursor_mode,
        })
    }

    /// Tear the portal session down cleanly: stop the PipeWire capture
    /// thread, call `org.freedesktop.portal.Session.Close` on the session
    /// object, and mark the lifecycle machine [`SessionPhase::Closed`].
    /// Idempotent: a second call on a terminal machine is a no-op.
    pub async fn close(&mut self) {
        if self.machine.is_terminal() {
            return;
        }
        let _ = self.machine.begin_close();
        self.stop_pipewire();
        if let (Some(connection), Some(session_path)) = (&self.connection, &self.session_path) {
            // Session.Close is the documented way to end a portal session
            // without waiting for the client bus connection to disappear; the
            // portal then tears down the PipeWire node.
            let _ = tokio::time::timeout(
                Self::CLOSE_TIMEOUT,
                connection.call_method(
                    Some("org.freedesktop.portal.Desktop"),
                    session_path.as_str(),
                    Some("org.freedesktop.portal.Session"),
                    "Close",
                    &(),
                ),
            )
            .await;
        }
        let _ = self.machine.on_closed();
        self.portal.stream_closed();
        self.connection = None;
        self.session_path = None;
    }

    /// Stop the PipeWire capture thread (bounded wait). Safe to call once;
    /// the thread frees its own PipeWire objects after the loop returns.
    fn stop_pipewire(&mut self) {
        if let Some(mut handle) = self.pipewire.take() {
            handle.stop();
        }
    }

    /// Read the next lifecycle event from the PipeWire thread.
    pub fn poll_event(&mut self) -> Option<PortalEvent> {
        self.events.try_recv().ok()
    }

    /// Current negotiated frame size, if the stream has produced one.
    pub fn negotiated_size(&self) -> Option<(u32, u32)> {
        let f = self.format.lock().unwrap();
        if f.width > 0 && f.height > 0 {
            Some((f.width, f.height))
        } else {
            None
        }
    }

    /// Current portal state (mapped from the lifecycle machine).
    pub fn state(&self) -> PortalState {
        match self.machine.phase() {
            SessionPhase::Idle => PortalState::Idle,
            SessionPhase::Creating | SessionPhase::Selecting | SessionPhase::Starting => {
                PortalState::Selecting
            }
            SessionPhase::Streaming => PortalState::Streaming,
            SessionPhase::Closing | SessionPhase::Closed | SessionPhase::Failed(_) => {
                PortalState::Ended
            }
        }
    }

    /// Detected desktop environment (diagnostics).
    pub fn environment(&self) -> DesktopEnvironment {
        self.environment
    }

    /// ScreenCast interface version reported by the portal, if queryable.
    pub fn portal_version(&self) -> Option<u32> {
        self.portal_version
    }

    /// Detected portal backend bus names, if any (diagnostics).
    pub fn portal_backend(&self) -> Option<&str> {
        self.backend.as_deref()
    }

    /// Cursor mode negotiated with the portal (PDF Task 5.3). `Embedded`
    /// means the compositor bakes the cursor into the stream buffers;
    /// `Hidden` (or a too-old portal) means the stream has no cursor.
    pub fn cursor_mode(&self) -> CursorMode {
        self.cursor_mode
    }
}

impl Drop for LinuxPortalCapture {
    fn drop(&mut self) {
        // Stop the PipeWire capture thread synchronously (pw_main_loop_quit is
        // documented as callable from any thread); the thread frees its own
        // PipeWire objects after the loop returns.
        self.stop_pipewire();
        // Best-effort portal Session.Close on a short-lived thread: Drop
        // cannot await, and the caller may be inside an active tokio runtime
        // (the host session thread), so use a fresh current-thread runtime.
        if let (Some(connection), Some(session_path)) = (self.connection.clone(), self.session_path.clone()) {
            let _ = std::thread::Builder::new()
                .name("boru-portal-close".into())
                .spawn(move || {
                    if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        runtime.block_on(async {
                            let _ = tokio::time::timeout(
                                LinuxPortalCapture::CLOSE_TIMEOUT,
                                connection.call_method(
                                    Some("org.freedesktop.portal.Desktop"),
                                    session_path.as_str(),
                                    Some("org.freedesktop.portal.Session"),
                                    "Close",
                                    &(),
                                ),
                            )
                            .await;
                        });
                    }
                });
        }
        self.portal.stream_closed();
    }
}

impl ScreenCapture for LinuxPortalCapture {
    fn capture(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
        // A failed session can never produce frames again; a closed session
        // ends gracefully with no frames (the caller stops on its own).
        match self.machine.phase() {
            SessionPhase::Failed(_) => {
                return Err(ScreenShareError::new("portal session failed"));
            }
            SessionPhase::Closed => return Ok(None),
            _ => {}
        }
        // Drain lifecycle events first so format changes are observed before
        // the frame that triggered them.
        while let Ok(event) = self.events.try_recv() {
            match event {
                PortalEvent::Ended => {
                    let _ = self.machine.on_portal_closed();
                    self.portal.stream_closed();
                    return Err(ScreenShareError::new("portal stream ended"));
                }
                _ => {}
            }
        }
        // Return the newest queued frame, dropping stale ones.
        let mut latest: Option<CapturedFrame> = None;
        while let Ok(frame) = self.frames.try_recv() {
            latest = Some(frame);
        }
        if let Some(frame) = &latest {
            let mut fmt = self.format.lock().unwrap();
            // The wire layout is recorded by the stream's param callback;
            // here we only track the normalized geometry for codec
            // configuration. The first frame seeds the size; later frames
            // with a different size model a display resolution change.
            if fmt.width == 0 || fmt.width != frame.width || fmt.height != frame.height {
                fmt.width = frame.width;
                fmt.height = frame.height;
            }
            drop(fmt);
            let _ = self.portal.push_pipewire_frame(frame.clone());
        }
        Ok(self.portal.sink.pop_latest())
    }
}

// ── PipeWire dlopen client ───────────────────────────────────────────────────

const PW_LIB: &str = "libpipewire-0.3.so.0";

/// Minimal pw_buffer mirror (layout matches `struct pw_buffer` in stream.h).
#[repr(C)]
struct PwBuffer {
    buffer: *mut SpaBuffer,
    user_data: *mut c_void,
    size: u64,
    requested: u64,
}

/// Minimal spa_buffer mirror (layout matches `struct spa_buffer` in buffer.h).
#[repr(C)]
struct SpaBuffer {
    n_metas: u32,
    n_datas: u32,
    metas: *mut c_void,
    datas: *mut SpaData,
}

/// Minimal spa_meta mirror (layout matches `struct spa_meta` in buffer.h).
/// Each meta is `{ uint32 type; uint32 size; void *data; }`.
#[repr(C)]
struct SpaMeta {
    type_: u32,
    size: u32,
    data: *mut c_void,
}

/// Minimal spa_data mirror (layout matches `struct spa_data` in buffer.h).
#[repr(C)]
struct SpaData {
    type_: u32,
    flags: u32,
    fd: i64,
    mapoffset: u32,
    maxsize: u32,
    data: *mut c_void,
    chunk: *mut SpaChunk,
}

/// Minimal spa_chunk mirror (layout matches `struct spa_chunk` in buffer.h).
#[repr(C)]
struct SpaChunk {
    offset: u32,
    size: u32,
    stride: i32,
    flags: i32,
}

/// PipeWire stream events table (layout matches `struct pw_stream_events`).
#[repr(C)]
struct PwStreamEvents {
    version: u32,
    destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    state_changed: Option<unsafe extern "C" fn(*mut c_void, i32, i32, *const c_char)>,
    control_info: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void)>,
    io_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *mut c_void, u32)>,
    param_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void)>,
    add_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut PwBuffer)>,
    remove_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut PwBuffer)>,
    process: Option<unsafe extern "C" fn(*mut c_void)>,
    drained: Option<unsafe extern "C" fn(*mut c_void)>,
    command: Option<unsafe extern "C" fn(*mut c_void, *const c_void)>,
    trigger_done: Option<unsafe extern "C" fn(*mut c_void)>,
}

/// Owned PipeWire objects and the function table. Lives on the capture thread.
struct PipeWireCtx {
    library: libloading::Library,
    pw: Pw,
    main_loop: *mut c_void,
    context: *mut c_void,
    core: *mut c_void,
    stream: *mut c_void,
    /// The format pod bytes handed to pw_stream_connect must outlive the stream.
    params: Vec<u8>,
}

// SAFETY: raw pointers are only dereferenced on the thread that owns `ctx`.
unsafe impl Send for PipeWireCtx {}

/// Per-stream callback payload, passed as `pw_stream_new_simple` user data.
struct StreamUserData {
    ctx: *mut PipeWireCtx,
    frame_tx: SyncSender<CapturedFrame>,
    event_tx: SyncSender<PortalEvent>,
    format: Arc<Mutex<NegotiatedFormat>>,
}

// SAFETY: as for PipeWireCtx — all access happens on the capture thread.
unsafe impl Send for StreamUserData {}

/// Function table for the PipeWire ABI we use.
struct Pw {
    init: unsafe extern "C" fn(*mut i32, *mut *mut *mut c_char),
    main_loop_new: unsafe extern "C" fn(props: *const c_void) -> *mut c_void,
    main_loop_get_loop: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    main_loop_run: unsafe extern "C" fn(*mut c_void) -> i32,
    main_loop_quit: unsafe extern "C" fn(*mut c_void) -> i32,
    main_loop_destroy: unsafe extern "C" fn(*mut c_void),
    context_new: unsafe extern "C" fn(loop_: *mut c_void, props: *const c_void, user_data_size: usize) -> *mut c_void,
    context_connect: unsafe extern "C" fn(*mut c_void, props: *mut c_void, user_data_size: usize) -> *mut c_void,
    context_destroy: unsafe extern "C" fn(*mut c_void),
    core_disconnect: unsafe extern "C" fn(*mut c_void) -> i32,
    stream_new_simple: unsafe extern "C" fn(
        loop_: *mut c_void,
        name: *const c_char,
        props: *mut c_void,
        events: *const PwStreamEvents,
        data: *mut c_void,
    ) -> *mut c_void,
    stream_connect: unsafe extern "C" fn(
        stream: *mut c_void,
        direction: u32,
        target_id: u32,
        flags: u32,
        params: *const *const c_void,
        n_params: u32,
    ) -> i32,
    stream_destroy: unsafe extern "C" fn(*mut c_void),
    stream_disconnect: unsafe extern "C" fn(*mut c_void) -> i32,
    stream_dequeue_buffer: unsafe extern "C" fn(*mut c_void) -> *mut PwBuffer,
    stream_queue_buffer: unsafe extern "C" fn(*mut c_void, *mut PwBuffer) -> i32,
    properties_new: unsafe extern "C" fn(key: *const c_char, ...) -> *mut c_void,
    properties_set: unsafe extern "C" fn(*mut c_void, key: *const c_char, value: *const c_char) -> i32,
    properties_free: unsafe extern "C" fn(*mut c_void),
}

impl Pw {
    fn load(library: &libloading::Library) -> Result<Self, ScreenShareError> {
        macro_rules! sym {
            ($name:literal) => {
                unsafe {
                    *library
                        .get::<unsafe extern "C" fn()>(concat!($name, "\0").as_bytes())
                        .map_err(|e| ScreenShareError::new(format!("symbol {} missing: {e}", $name)))?
                }
            };
        }
        Ok(Self {
            init: unsafe { std::mem::transmute(sym!("pw_init")) },
            main_loop_new: unsafe { std::mem::transmute(sym!("pw_main_loop_new")) },
            main_loop_get_loop: unsafe { std::mem::transmute(sym!("pw_main_loop_get_loop")) },
            main_loop_run: unsafe { std::mem::transmute(sym!("pw_main_loop_run")) },
            main_loop_quit: unsafe { std::mem::transmute(sym!("pw_main_loop_quit")) },
            main_loop_destroy: unsafe { std::mem::transmute(sym!("pw_main_loop_destroy")) },
            context_new: unsafe { std::mem::transmute(sym!("pw_context_new")) },
            context_connect: unsafe { std::mem::transmute(sym!("pw_context_connect")) },
            context_destroy: unsafe { std::mem::transmute(sym!("pw_context_destroy")) },
            core_disconnect: unsafe { std::mem::transmute(sym!("pw_core_disconnect")) },
            stream_new_simple: unsafe { std::mem::transmute(sym!("pw_stream_new_simple")) },
            stream_connect: unsafe { std::mem::transmute(sym!("pw_stream_connect")) },
            stream_destroy: unsafe { std::mem::transmute(sym!("pw_stream_destroy")) },
            stream_disconnect: unsafe { std::mem::transmute(sym!("pw_stream_disconnect")) },
            stream_dequeue_buffer: unsafe { std::mem::transmute(sym!("pw_stream_dequeue_buffer")) },
            stream_queue_buffer: unsafe { std::mem::transmute(sym!("pw_stream_queue_buffer")) },
            properties_new: unsafe { std::mem::transmute(sym!("pw_properties_new")) },
            properties_set: unsafe { std::mem::transmute(sym!("pw_properties_set")) },
            properties_free: unsafe { std::mem::transmute(sym!("pw_properties_free")) },
        })
    }
}

struct PipeWireClient;

impl PipeWireClient {
    /// Connect a capture stream to the given portal node, spawn the PipeWire
    /// main loop on a background thread, and return the cross-thread handle
    /// used to stop it during session teardown.
    fn connect(
        node_id: u32,
        frame_tx: SyncSender<CapturedFrame>,
        event_tx: SyncSender<PortalEvent>,
        format: Arc<Mutex<NegotiatedFormat>>,
    ) -> Result<PipeWireHandle, ScreenShareError> {
        // SAFETY: every raw pointer below is created and used on the spawned
        // thread. `ctx` is boxed and its pointer handed to the thread; the
        // stream events borrow the same context for their whole lifetime,
        // which ends when the loop quits.
        unsafe {
            let library = libloading::Library::new(PW_LIB).map_err(|e| {
                ScreenShareError::missing_pipewire(format!(
                    "cannot load {PW_LIB} — install PipeWire (e.g. `apt install pipewire`) or run inside a desktop session that provides it: {e}"
                ))
            })?;
            let pw = Pw::load(&library)?;
            let mut argc = 0i32;
            let mut argv: *mut *mut c_char = std::ptr::null_mut();
            (pw.init)(&mut argc, &mut argv);

            let main_loop = (pw.main_loop_new)(std::ptr::null());
            if main_loop.is_null() {
                return Err(ScreenShareError::pipewire_connect(
                    "pw_main_loop_new failed (PipeWire runtime problem)",
                ));
            }
            let loop_ = (pw.main_loop_get_loop)(main_loop);
            let context = (pw.context_new)(loop_, std::ptr::null(), 0);
            if context.is_null() {
                (pw.main_loop_destroy)(main_loop);
                return Err(ScreenShareError::pipewire_connect(
                    "pw_context_new failed (PipeWire runtime problem)",
                ));
            }
            let core = (pw.context_connect)(context, std::ptr::null_mut(), 0);
            if core.is_null() {
                (pw.context_destroy)(context);
                (pw.main_loop_destroy)(main_loop);
                return Err(ScreenShareError::pipewire_connect(
                    "pw_context_connect failed — no PipeWire server reachable (is `pipewire` running in this session?)",
                ));
            }

            let props = make_stream_properties(&pw)?;
            let params = build_format_pod();

            let ctx = Box::into_raw(Box::new(PipeWireCtx {
                library,
                pw,
                main_loop,
                context,
                core,
                stream: std::ptr::null_mut(),
                params,
            }));

            let user_data = Box::into_raw(Box::new(StreamUserData {
                ctx,
                frame_tx,
                event_tx,
                format: format.clone(),
            }));

            let events = PwStreamEvents {
                version: 2,
                destroy: None,
                state_changed: Some(stream_state_changed),
                control_info: None,
                io_changed: None,
                param_changed: Some(stream_param_changed),
                add_buffer: None,
                remove_buffer: None,
                process: Some(stream_process),
                drained: None,
                command: None,
                trigger_done: None,
            };

            let stream = ((*ctx).pw.stream_new_simple)(
                loop_,
                c"boru-screen-capture".as_ptr(),
                props,
                &events,
                user_data as *mut c_void,
            );
            if stream.is_null() {
                ((*ctx).pw.properties_free)(props);
                drop(Box::from_raw(user_data));
                drop(Box::from_raw(ctx));
                return Err(ScreenShareError::pipewire_connect(
                    "pw_stream_new_simple failed (PipeWire runtime problem)",
                ));
            }
            (*ctx).stream = stream;

            // Advertise the formats we can consume (BGRx/RGBx/BGRA/RGBA and
            // the 24-bit BGR/RGB, in preference order — see
            // linux_pw::advertised_format_ids). The portal converts its
            // native format to one of these.
            let flags = PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS;
            let result = ((*ctx).pw.stream_connect)(
                stream,
                PW_DIRECTION_INPUT,
                node_id,
                flags,
                [(*ctx).params.as_ptr() as *const c_void].as_ptr(),
                1,
            );
            if result < 0 {
                ((*ctx).pw.stream_destroy)(stream);
                drop(Box::from_raw(user_data));
                drop(Box::from_raw(ctx));
                return Err(ScreenShareError::pipewire_connect(format!(
                    "pw_stream_connect failed ({result}) — the portal node {node_id} could not be linked; is the portal stream still alive?"
                )));
            }

            // `thread::spawn` requires every captured value to be Send. Raw
            // pointers are not, so carry them as usize (Send) and reconstruct
            // on the thread; the boxed objects stay alive until the thread
            // drops them.
            let ctx_addr = ctx as usize;
            let user_addr = user_data as usize;
            // The teardown handle needs the main-loop pointer and the quit
            // function; pw_main_loop_quit is documented as callable from any
            // thread (PipeWire's own examples call it from signal handlers).
            let main_loop_addr = main_loop as usize;
            let main_loop_quit = (*ctx).pw.main_loop_quit;
            let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
            std::thread::Builder::new()
                .name("boru-pipewire-capture".into())
                .spawn(move || {
                    run_pipewire_thread(
                        ctx_addr as *mut PipeWireCtx,
                        user_addr as *mut StreamUserData,
                        done_tx,
                    )
                })
                .map_err(|e| ScreenShareError::new(format!("spawn pipewire thread: {e}")))?;

            Ok(PipeWireHandle {
                main_loop: main_loop_addr,
                main_loop_quit,
                done: done_rx,
            })
        }
    }
}

/// Cross-thread handle that stops the PipeWire capture thread.
///
/// `pw_main_loop_quit` is documented as callable from any thread (PipeWire's
/// own examples call it from signal handlers), so the main-thread owner can
/// ask the capture thread to wind down without sharing raw pointers. The
/// `done` channel is signalled after the thread has freed its PipeWire
/// objects.
#[derive(Debug)]
struct PipeWireHandle {
    main_loop: usize,
    main_loop_quit: unsafe extern "C" fn(*mut c_void) -> i32,
    done: Receiver<()>,
}

impl PipeWireHandle {
    /// Quit the PipeWire main loop and wait (bounded) for the capture thread
    /// to finish its teardown. Safe to call once; the caller owns the handle.
    fn stop(&mut self) {
        // SAFETY: pw_main_loop_quit may be called from any thread while the
        // loop object is alive; the loop stays alive until the capture thread
        // destroys it after main_loop_run returns.
        unsafe {
            (self.main_loop_quit)(self.main_loop as *mut c_void);
        }
        // The thread frees its own PipeWire objects; bound the wait so a
        // wedged thread cannot hang session teardown.
        let _ = self.done.recv_timeout(Duration::from_secs(2));
    }
}

/// Build the PipeWire stream properties (null-terminated varargs call).
unsafe fn make_stream_properties(pw: &Pw) -> Result<*mut c_void, ScreenShareError> {
    let media_type = CString::new("media.type").unwrap();
    let video = CString::new("Video").unwrap();
    let category = CString::new("media.category").unwrap();
    let capture = CString::new("Capture").unwrap();
    let role = CString::new("media.role").unwrap();
    let screen = CString::new("Screen").unwrap();
    let node_name = CString::new("node.name").unwrap();
    let node_value = CString::new("boru-screen-capture").unwrap();
    let props = (pw.properties_new)(
        media_type.as_ptr(),
        video.as_ptr(),
        category.as_ptr(),
        capture.as_ptr(),
        role.as_ptr(),
        screen.as_ptr(),
        node_name.as_ptr(),
        node_value.as_ptr(),
        std::ptr::null::<c_char>(),
    );
    if props.is_null() {
        return Err(ScreenShareError::new("pw_properties_new failed"));
    }
    Ok(props)
}

/// Run the PipeWire main loop until quit; forwards frames and events, then
/// frees every PipeWire object and signals the teardown handle.
fn run_pipewire_thread(ctx: *mut PipeWireCtx, user_data: *mut StreamUserData, done: Sender<()>) {
    unsafe {
        let _ = ((*ctx).pw.main_loop_run)((*ctx).main_loop);
        let _ = ((*ctx).pw.stream_disconnect)((*ctx).stream);
        let _ = ((*ctx).pw.stream_destroy)((*ctx).stream);
        let _ = ((*ctx).pw.core_disconnect)((*ctx).core);
        let _ = ((*ctx).pw.context_destroy)((*ctx).context);
        let _ = ((*ctx).pw.main_loop_destroy)((*ctx).main_loop);
        drop(Box::from_raw(user_data));
        drop(Box::from_raw(ctx));
    }
    let _ = done.send(());
}

unsafe extern "C" fn stream_state_changed(
    data: *mut c_void,
    _old: i32,
    _state: i32,
    _error: *const c_char,
) {
    let _ = data;
}

/// Read a SPA pod as a byte slice for the duration of a stream callback.
/// The declared body size is clamped so a corrupt pod cannot produce an
/// unbounded slice.
unsafe fn pod_bytes(param: *const c_void) -> Option<&'static [u8]> {
    if param.is_null() {
        return None;
    }
    let head = std::slice::from_raw_parts(param as *const u8, 8);
    let total = u32::from_le_bytes(head[0..4].try_into().ok()?) as usize;
    if total > 65536 {
        return None;
    }
    Some(std::slice::from_raw_parts(param as *const u8, 8 + total))
}

// SPA_PARAM_* names mirror PipeWire's C enum (SPA_PARAM_Format, ...).
#[allow(non_upper_case_globals)]
unsafe extern "C" fn stream_param_changed(
    data: *mut c_void,
    id: u32,
    param: *const c_void,
) {
    let user = data as *mut StreamUserData;
    match id {
        // SPA_PARAM_Format (4) carries the negotiated geometry/format. The
        // portal re-sends it whenever the display resolution changes; each
        // real change bumps the negotiation generation and emits
        // FormatChanged so consumers can react (the host loop reconfigures
        // the encoder from the frame geometry; this event is diagnostics +
        // the generation counter).
        SPA_PARAM_Format => {
            let Some(bytes) = pod_bytes(param) else { return; };
            let Some((width, height, layout)) = parse_format_pod(bytes) else {
                return;
            };
            let mut fmt = (*user).format.lock().unwrap();
            if fmt.width != width || fmt.height != height || fmt.layout != layout {
                *fmt = NegotiatedFormat {
                    width,
                    height,
                    layout,
                    generation: fmt.generation.saturating_add(1),
                };
                tracing::info!(
                    generation = fmt.generation,
                    width,
                    height,
                    ?layout,
                    "screen-share: pipewire stream renegotiated format"
                );
                let _ = (*user)
                    .event_tx
                    .try_send(PortalEvent::FormatChanged { width, height });
            }
        }
        // SPA_PARAM_Buffers (5) is sent when PipeWire (re)allocates buffers
        // for a new negotiated format. Buffers are copied out immediately in
        // process(), so there is nothing to track; log for renegotiation
        // diagnostics.
        SPA_PARAM_Buffers => {
            tracing::debug!("screen-share: pipewire buffers (re)allocated");
        }
        _ => {}
    }
}

unsafe extern "C" fn stream_process(data: *mut c_void) {
    let user = data as *mut StreamUserData;
    let ctx = (*user).ctx;
    let pw = &(*ctx).pw;
    let buffer = (pw.stream_dequeue_buffer)((*ctx).stream);
    if buffer.is_null() {
        return;
    }
    let spa = (*buffer).buffer;
    // BORU-SS-33: in Metadata cursor mode (4) the cursor arrives as
    // `spa_meta_cursor` buffer metadata instead of being composited into the
    // pixels. Parse the metas BEFORE the CPU path so the cursor can ride the
    // same frame as separate shape/position metadata.
    let mut spa_cursor = None;
    if !spa.is_null() && (*spa).n_metas > 0 && !(*spa).metas.is_null() {
        let metas = std::slice::from_raw_parts((*spa).metas as *const SpaMeta, (*spa).n_metas as usize);
        for meta in metas {
            if meta.type_ == SPA_META_Cursor && !meta.data.is_null() && meta.size > 0 {
                // SAFETY: `data` is a valid pointer for `size` bytes owned by
                // the buffer (PipeWire owns the meta blob); the parser only
                // reads within bounds.
                let meta_bytes = std::slice::from_raw_parts(meta.data as *const u8, meta.size as usize);
                spa_cursor = parse_spa_cursor_meta(meta_bytes);
                break;
            }
        }
    }
    if !spa.is_null() && (*spa).n_datas > 0 {
        let dat = (*spa).datas;
        if !dat.is_null() && !(*dat).data.is_null() {
            // CPU-mapped path: with PW_STREAM_FLAG_MAP_BUFFERS the data
            // pointer is a valid CPU address for the whole mapped region
            // (maxsize), whatever the backing memory type. A future DMA-BUF
            // path reads (*dat).type_ == SPA_DATA_DmaBuf and (*dat).fd
            // instead, and delivers a CapturedFrame with gpu_handle set.
            let chunk = (*dat).chunk;
            let offset = if chunk.is_null() { 0 } else { (*chunk).offset as usize };
            let stride = if chunk.is_null() { 0 } else { (*chunk).stride };
            // SAFETY: `data` is a valid CPU mapping of maxsize bytes for the
            // duration of the callback (MAP_BUFFERS guarantees this); the
            // slice is only read by normalize_buffer, which bounds-checks.
            let src = std::slice::from_raw_parts((*dat).data as *const u8, (*dat).maxsize as usize);
            let fmt = *(*user).format.lock().unwrap();
            if fmt.width > 0 && fmt.height > 0 {
                match normalize_buffer(src, offset, fmt.width, fmt.height, fmt.layout, stride) {
                    Ok(pixels) => {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_micros() as u64;
                        match CapturedFrame::cpu(
                            now,
                            fmt.width,
                            fmt.height,
                            fmt.layout.to_pixel_format(),
                            pixels,
                        ) {
                            Ok(frame) => {
                                let frame = if let Some(cursor) = &spa_cursor {
                                    let meta = match &cursor.sprite {
                                        Some(sprite) => CursorMeta::with_sprite(
                                            DesktopPoint { x: cursor.x, y: cursor.y },
                                            cursor.visible,
                                            sprite.clone(),
                                        ),
                                        None => CursorMeta::position(
                                            DesktopPoint { x: cursor.x, y: cursor.y },
                                            cursor.visible,
                                        ),
                                    };
                                    frame.with_cursor(meta)
                                } else {
                                    frame
                                };
                                let _ = (*user).frame_tx.try_send(frame);
                            }
                            Err(error) => {
                                tracing::debug!(error = %error, "screen-share: pipewire frame rejected");
                            }
                        }
                    }
                    Err(error) => {
                        // A buffer that does not match the current negotiated
                        // geometry (e.g. a stale buffer from before a
                        // resolution-change renegotiation) is dropped; the
                        // next buffer carries the new format.
                        tracing::debug!(error = %error, "screen-share: pipewire buffer dropped");
                    }
                }
            }
        }
    }
    (pw.stream_queue_buffer)((*ctx).stream, buffer);
}

const PW_DIRECTION_INPUT: u32 = 0;

const PW_STREAM_FLAG_AUTOCONNECT: u32 = 1 << 0;
const PW_STREAM_FLAG_MAP_BUFFERS: u32 = 1 << 2;

// SPA pod/type constants, the format advertisement pod builder, the
// negotiated-format parser, and the CPU buffer normalization now live in
// `linux_pw` (pure + unit-testable). The stream callbacks above use
// `parse_format_pod` / `normalize_buffer` from there.

/// Extract the first stream node id from a portal Start reply body.
///
/// The reply is a dictionary `{ "streams": [ { "node_id": u32, ... }, ... ] }`.
/// zvariant 5 does not implement `TryFrom<&Value>` for Vec/HashMap, so walk
/// the Value enum directly instead of downcasting.
fn extract_stream_node_id(body: &zbus::zvariant::Value) -> Option<u32> {
    use zbus::zvariant::Value;
    let streams_key = "streams".to_string();
    let node_key = "node_id".to_string();
    let Value::Dict(dict) = body else { return None };
    let streams = dict.get::<String, Value>(&streams_key).ok()??;
    let Value::Array(array) = streams else { return None };
    for item in array.iter() {
        let Value::Dict(stream) = item else { continue };
        let node = stream.get::<String, Value>(&node_key).ok()??;
        let Value::U32(node_id) = node else { continue };
        return Some(node_id);
    }
    None
}

/// Query the ScreenCast interface version (`org.freedesktop.DBus.Properties.Get`
/// on the portal object). Best-effort diagnostics; `None` when the portal does
/// not expose the property or the call fails.
async fn query_portal_version(connection: &zbus::Connection) -> Option<u32> {
    let reply = connection
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.portal.ScreenCast", "version"),
        )
        .await
        .ok()?;
    let value: zbus::zvariant::OwnedValue = reply.body().deserialize().ok()?;
    match &*value {
        zbus::zvariant::Value::U32(version) => Some(*version),
        _ => None,
    }
}

/// Detect the active portal backend by listing session-bus names
/// (`org.freedesktop.DBus.ListNames`). Best-effort diagnostics; `None` when
/// the bus does not answer or no backend name is visible.
///
/// Backend implementations register names under `org.freedesktop.impl.portal`
/// (e.g. `…gnome`, `…kde`, `…wlr`, `…gtk`); the frontend bus name is
/// `org.freedesktop.portal.Desktop`.
async fn detect_portal_backend(connection: &zbus::Connection) -> Option<String> {
    let reply = connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "ListNames",
            &(),
        )
        .await
        .ok()?;
    let names: Vec<String> = reply.body().deserialize().ok()?;
    let impl_names: Vec<&str> = names
        .iter()
        .filter(|name| name.contains("impl.portal"))
        .map(String::as_str)
        .collect();
    if !impl_names.is_empty() {
        return Some(impl_names.join(","));
    }
    names
        .iter()
        .find(|name| name.starts_with("org.freedesktop.portal.") && !name.contains("impl.portal"))
        .cloned()
}

// ── Direct X11 capture backend ─────────────────────────────────────────────

/// A capture rectangle in root-window coordinates (physical pixels).
///
/// `x`/`y` are the top-left corner inside the root window; `width`/`height`
/// are the capture size. GetImage accepts `i16`/`u16` coordinates, which
/// matches RandR monitor geometry exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureRect {
    /// Left edge in root coordinates.
    pub x: i16,
    /// Top edge in root coordinates.
    pub y: i16,
    /// Capture width in pixels.
    pub width: u16,
    /// Capture height in pixels.
    pub height: u16,
}

/// One monitor advertised by the X11 backend.
///
/// `x`/`y` are the monitor origin in root-window coordinates (RandR CRTC
/// coordinates); `width`/`height` are the monitor's pixel size. Monitors
/// left of / above the root origin can have negative origins, matching the
/// coordinate model in [`crate::screen_share::coords::MonitorGeometry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct X11Monitor {
    /// Stable source id (FNV-1a of the RandR name, mirroring the Windows
    /// backend's `monitor_source_id`).
    pub id: CaptureSourceId,
    /// RandR output/monitor name (e.g. `DP-1`, `HDMI-A-0`), or a fallback
    /// like `Screen` when RandR cannot supply a name.
    pub name: String,
    /// Left edge in root coordinates.
    pub x: i16,
    /// Top edge in root coordinates.
    pub y: i16,
    /// Pixel width.
    pub width: u16,
    /// Pixel height.
    pub height: u16,
    /// Whether RandR marks this monitor as primary.
    pub primary: bool,
}

/// Nominal frame period (µs) for the X11 capture clock (~30 fps).
const X11_FRAME_PERIOD_US: u64 = 33_333;

/// XDamage subscription for one capture session (BORU-SS-32).
///
/// The XDamage extension tracks pixel changes on the root window. Each
/// capture tick the accumulated region is drained into an XFIXES region
/// (`DamageSubtract` with `repair=None`) and read back (`FetchRegion`); the
/// reported rectangles are in root-window coordinates and are
/// clipped/translated to the selected capture geometry when attaching
/// [`DirtyRegion`] metadata. DamageNotify events are delivered to the
/// creating client automatically, so the event queue is drained each tick to
/// keep the connection socket from filling — the region query, not the
/// events, is the authoritative damage source.
struct DamageTracker {
    /// The XDamage object id (RAW_RECTANGLES level).
    damage: damage::Damage,
    /// XFIXES region receiving the accumulated damage on each subtract.
    region: xfixes::Region,
    /// True until the first frame after (re)start: the first frame is
    /// always reported fully dirty so the viewer gets a complete baseline.
    first_frame: bool,
    /// Number of frames skipped because no damage occurred (metrics).
    skipped: u64,
}

/// Result of querying the damage extension for one capture tick.
enum DamageQuery {
    /// DAMAGE/XFIXES unavailable (or tracking failed): capture every frame
    /// as before, without dirty metadata.
    Unavailable,
    /// No pixels changed since the last frame — the frame can be skipped.
    Clean,
    /// The first frame after (re)start: capture and report `Full`.
    Full,
    /// Root-space damage rectangles that changed. May be empty after
    /// clipping to the capture geometry (damage outside the capture).
    Dirty(Vec<FrameRect>),
}

/// Direct X11 capture: grabs a rectangle of the root window via `GetImage`
/// and converts the ZPixmap buffer to RGBA8. This is the no-portal fallback —
/// it makes real desktop sharing work on any X11 display without
/// xdg-desktop-portal or PipeWire. Pixels are interpreted through the root
/// visual's channel masks, so both LSBFirst (BGRX, typical x86) and MSBFirst
/// (XRGB) servers convert correctly. An XShm fast path can replace the
/// per-frame GetImage copy later without changing this interface.
///
/// The backend implements both [`ScreenCapture`] (whole-root capture, used by
/// the `ActiveCapture::X11` fallback path) and [`DesktopCaptureBackend`]
/// (monitor enumeration + selected-geometry capture, PDF Task 6.1).
pub struct X11Capture {
    conn: x11rb::rust_connection::RustConnection,
    root: u32,
    width: u32,
    height: u32,
    depth: u8,
    lsb_first: bool,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    timestamp_us: u64,
    /// The monitor rectangle selected via [`DesktopCaptureBackend::start`].
    selected: Option<CaptureRect>,
    /// Whether [`DesktopCaptureBackend::start`] has been called.
    started: bool,
    /// The source id selected via [`DesktopCaptureBackend::start`], used to
    /// report the current source for source-change handling (PDF Phase 10).
    current_source: Option<CaptureSourceId>,
    /// XDamage tracking state; `None` when the server lacks DAMAGE/XFIXES or
    /// tracking was not set up (falls back to always-capture).
    damage: Option<DamageTracker>,
    /// Forces the next captured frame to [`DirtyRegion::Full`] regardless of
    /// the damage region (e.g. the root window resized and the whole frame
    /// must be treated as new).
    force_full_next: bool,
}

impl X11Capture {
    /// Connect to `$DISPLAY` and describe the root window. Fails closed when
    /// no display is reachable or the root visual is not 24/32-bit.
    pub fn connect() -> Result<Self, ScreenShareError> {
        let (conn, screen_num) = x11rb::connect(None)
            .map_err(|e| ScreenShareError::new(format!("X11 connect failed: {e}")))?;
        // Copy everything out of the borrowed setup/screen/visual data before
        // moving `conn` into the struct (the setup borrows the connection).
        let (root, width, height, depth, lsb_first, red_mask, green_mask, blue_mask) = {
            let setup = conn.setup();
            let screen = &setup.roots[screen_num];
            let depth = screen.root_depth;
            if !matches!(depth, 24 | 32) {
                return Err(ScreenShareError::new(format!(
                    "unsupported X11 root depth {depth} (need a 24 or 32-bit visual)"
                )));
            }
            let visual = screen
                .allowed_depths
                .iter()
                .flat_map(|d| d.visuals.iter())
                .find(|v| v.visual_id == screen.root_visual)
                .ok_or_else(|| ScreenShareError::new("X11 root visual not found"))?;
            (
                screen.root,
                screen.width_in_pixels as u32,
                screen.height_in_pixels as u32,
                depth,
                setup.image_byte_order == ImageOrder::LSB_FIRST,
                visual.red_mask,
                visual.green_mask,
                visual.blue_mask,
            )
        };
        Ok(Self {
            conn,
            root,
            width,
            height,
            depth,
            lsb_first,
            red_mask,
            green_mask,
            blue_mask,
            timestamp_us: 0,
            selected: None,
            started: false,
            current_source: None,
            damage: None,
            force_full_next: false,
        })
    }

    /// Enumerate monitors via RandR.
    ///
    /// Tries the modern RandR 1.5 `GetMonitors` path first (it reports
    /// physical monitor names and the primary flag). Falls back to a
    /// CRTC walk (`GetScreenResourcesCurrent` + `GetCrtcInfo` +
    /// `GetOutputInfo`) for older servers, and finally to a single
    /// root-window screen so capture still works without RandR at all.
    pub fn list_monitors(&self) -> Result<Vec<X11Monitor>, ScreenShareError> {
        if let Ok(monitors) = self.randr_monitors() {
            if !monitors.is_empty() {
                return Ok(monitors);
            }
        }
        if let Ok(monitors) = self.crtc_monitors() {
            if !monitors.is_empty() {
                return Ok(monitors);
            }
        }
        Ok(vec![self.root_monitor()])
    }

    /// RandR 1.5 `GetMonitors` path (RANDR 1.5+). Returns `Err` when the
    /// request itself fails (old server, extension missing) so callers fall
    /// back to the CRTC walk.
    fn randr_monitors(&self) -> Result<Vec<X11Monitor>, ScreenShareError> {
        let reply = self
            .conn
            .randr_get_monitors(self.root, true)
            .map_err(|e| ScreenShareError::new(format!("X11 RandR GetMonitors failed: {e}")))?
            .reply()
            .map_err(|e| ScreenShareError::new(format!("X11 RandR GetMonitors reply failed: {e}")))?;
        let mut out = Vec::with_capacity(reply.monitors.len());
        for (index, monitor) in reply.monitors.iter().enumerate() {
            let name = self
                .atom_name(monitor.name)
                .unwrap_or_else(|| format!("Monitor {index}"));
            out.push(X11Monitor {
                id: monitor_source_id(&name),
                name,
                x: monitor.x,
                y: monitor.y,
                width: monitor.width,
                height: monitor.height,
                primary: monitor.primary,
            });
        }
        Ok(out)
    }

    /// CRTC walk for servers without RandR 1.5: one source per active CRTC,
    /// named from its first output.
    fn crtc_monitors(&self) -> Result<Vec<X11Monitor>, ScreenShareError> {
        let resources = self
            .conn
            .randr_get_screen_resources_current(self.root)
            .map_err(|e| ScreenShareError::new(format!("X11 RandR screen resources failed: {e}")))?
            .reply()
            .map_err(|e| {
                ScreenShareError::new(format!("X11 RandR screen resources reply failed: {e}"))
            })?;
        let primary_output = self
            .conn
            .randr_get_output_primary(self.root)
            .map_err(|e| ScreenShareError::new(format!("X11 RandR output primary failed: {e}")))?
            .reply()
            .map_err(|e| {
                ScreenShareError::new(format!("X11 RandR output primary reply failed: {e}"))
            })?
            .output;
        let mut out = Vec::with_capacity(resources.crtcs.len());
        for (index, crtc) in resources.crtcs.iter().enumerate() {
            let info = self
                .conn
                .randr_get_crtc_info(*crtc, resources.config_timestamp)
                .map_err(|e| ScreenShareError::new(format!("X11 RandR CRTC info failed: {e}")))?
                .reply()
                .map_err(|e| {
                    ScreenShareError::new(format!("X11 RandR CRTC info reply failed: {e}"))
                })?;
            if info.width == 0 || info.height == 0 {
                continue; // inactive CRTC
            }
            let name = info
                .outputs
                .first()
                .and_then(|output| {
                    let out_info = self
                        .conn
                        .randr_get_output_info(*output, resources.config_timestamp)
                        .ok()?
                        .reply()
                        .ok()?;
                    if out_info.name.is_empty() {
                        None
                    } else {
                        Some(String::from_utf8_lossy(&out_info.name).into_owned())
                    }
                })
                .unwrap_or_else(|| format!("Screen {index}"));
            out.push(X11Monitor {
                id: monitor_source_id(&name),
                name,
                x: info.x,
                y: info.y,
                width: info.width,
                height: info.height,
                primary: info.outputs.contains(&primary_output),
            });
        }
        Ok(out)
    }

    /// Resolve an atom to its string name, if the server knows it.
    fn atom_name(&self, atom: u32) -> Option<String> {
        let reply = self.conn.get_atom_name(atom).ok()?.reply().ok()?;
        if reply.name.is_empty() {
            None
        } else {
            Some(String::from_utf8_lossy(&reply.name).into_owned())
        }
    }

    /// Last-resort single source: the whole root window as one "screen".
    fn root_monitor(&self) -> X11Monitor {
        X11Monitor {
            id: monitor_source_id("Screen"),
            name: "Screen".to_string(),
            x: 0,
            y: 0,
            width: self.width.min(u16::MAX as u32) as u16,
            height: self.height.min(u16::MAX as u32) as u16,
            primary: true,
        }
    }

    /// Subscribe to XDamage on the root window (BORU-SS-32).
    ///
    /// Negotiates DAMAGE 1.0 and XFIXES 5.0 (each version query must precede
    /// every other request of its extension — damageproto.txt §8,
    /// xfixesproto.txt §8), then creates the damage object at
    /// [`damage::ReportLevel::RAW_RECTANGLES`] — the most detailed report
    /// level, one DamageNotify per modified rectangle — and an XFIXES region
    /// that `DamageSubtract` fills with the accumulated region each frame.
    /// Falls back to `damage: None` (always capture) when either extension
    /// is missing or a setup request fails, so damage awareness never breaks
    /// capture.
    fn setup_damage(&mut self) {
        let damage_present = self
            .conn
            .query_extension(b"DAMAGE")
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|reply| reply.present);
        let xfixes_present = self
            .conn
            .query_extension(b"XFIXES")
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|reply| reply.present);
        if !damage_present || !xfixes_present {
            tracing::info!(
                damage = damage_present,
                xfixes = xfixes_present,
                "screen-share: X11 damage tracking unavailable (DAMAGE/XFIXES extension missing)"
            );
            return;
        }
        let setup = (|| -> Result<(damage::Damage, xfixes::Region), ScreenShareError> {
            self.conn
                .damage_query_version(1, 0)
                .map_err(|e| ScreenShareError::new(format!("X11 DAMAGE query_version failed: {e}")))?
                .reply()
                .map_err(|e| ScreenShareError::new(format!("X11 DAMAGE query_version reply failed: {e}")))?;
            self.conn
                .xfixes_query_version(5, 0)
                .map_err(|e| ScreenShareError::new(format!("X11 XFIXES query_version failed: {e}")))?
                .reply()
                .map_err(|e| ScreenShareError::new(format!("X11 XFIXES query_version reply failed: {e}")))?;
            let damage_id = self
                .conn
                .generate_id()
                .map_err(|e| ScreenShareError::new(format!("X11 generate_id failed: {e}")))?;
            self.conn
                .damage_create(damage_id, self.root, damage::ReportLevel::RAW_RECTANGLES)
                .map_err(|e| ScreenShareError::new(format!("X11 DamageCreate failed: {e}")))?;
            let region_id = self
                .conn
                .generate_id()
                .map_err(|e| ScreenShareError::new(format!("X11 generate_id failed: {e}")))?;
            self.conn
                .xfixes_create_region(region_id, &[])
                .map_err(|e| ScreenShareError::new(format!("X11 XFIXES CreateRegion failed: {e}")))?;
            Ok((damage_id, region_id))
        })();
        match setup {
            Ok((damage_id, region_id)) => {
                tracing::info!("screen-share: X11 damage tracking active (RAW_RECTANGLES on root)");
                self.damage = Some(DamageTracker {
                    damage: damage_id,
                    region: region_id,
                    first_frame: true,
                    skipped: 0,
                });
            }
            Err(error) => {
                tracing::warn!(error = %error, "screen-share: X11 damage tracking setup failed; falling back to full capture");
            }
        }
    }

    /// Destroy the damage object and region (idempotent; safe on teardown).
    fn teardown_damage(&mut self) {
        if let Some(tracker) = self.damage.take() {
            let _ = self.conn.damage_destroy(tracker.damage);
            let _ = self.conn.xfixes_destroy_region(tracker.region);
        }
    }

    /// Drain the X event queue so DamageNotify events never fill the
    /// connection socket. The events are delivered to the damage-creating
    /// client automatically, but the accumulated-region query
    /// ([`Self::fetch_damage_rects`]) is the authoritative damage source.
    fn drain_events(&self) {
        while let Ok(Some(_)) = self.conn.poll_for_event() {}
    }

    /// Query whether anything changed since the last frame.
    ///
    /// - [`DamageQuery::Unavailable`] when damage tracking is off — callers
    ///   keep the pre-damage behavior (capture every frame, no dirty
    ///   metadata).
    /// - [`DamageQuery::Full`] for the first frame after (re)start, so the
    ///   viewer receives a complete baseline.
    /// - [`DamageQuery::Clean`] when no pixel changed — callers skip the
    ///   frame (no GetImage, no encode, no transmit).
    /// - [`DamageQuery::Dirty`] with root-space rectangles otherwise.
    ///
    /// Damage-query failures (server dropped the damage object, etc.) are
    /// non-fatal: tracking is disabled and capture continues in the
    /// [`DamageQuery::Unavailable`] fallback.
    fn query_damage(&mut self) -> DamageQuery {
        if self.damage.is_none() {
            return DamageQuery::Unavailable;
        }
        self.drain_events();
        // A resize/geometry change forces a full repaint regardless of the
        // damage region (the whole frame is new to the viewer).
        if self.force_full_next {
            self.force_full_next = false;
            return DamageQuery::Full;
        }
        // The first frame after (re)start is always fully dirty: the viewer
        // needs a complete baseline even if nothing changed since the
        // damage object was created.
        if self.damage.as_ref().is_some_and(|tracker| tracker.first_frame) {
            let tracker = self.damage.as_mut().expect("checked above");
            tracker.first_frame = false;
            return DamageQuery::Full;
        }
        match self.fetch_damage_rects() {
            Ok(rects) if rects.is_empty() => {
                if let Some(tracker) = self.damage.as_mut() {
                    tracker.skipped += 1;
                }
                DamageQuery::Clean
            }
            Ok(rects) => DamageQuery::Dirty(rects),
            Err(error) => {
                tracing::warn!(error = %error, "screen-share: X11 damage query failed; disabling damage tracking");
                self.damage = None;
                DamageQuery::Unavailable
            }
        }
    }

    /// Drain the accumulated damage into the XFIXES region (`DamageSubtract`
    /// with `repair=None` — per damageproto.txt §9 this sets the region to
    /// the accumulated damage and clears the object) and read the resulting
    /// root-space rectangles back (`FetchRegion`).
    fn fetch_damage_rects(&mut self) -> Result<Vec<FrameRect>, ScreenShareError> {
        let Some(tracker) = self.damage.as_ref() else {
            return Ok(Vec::new());
        };
        self.conn
            .damage_subtract(tracker.damage, 0u32, tracker.region)
            .map_err(|e| ScreenShareError::new(format!("X11 DamageSubtract failed: {e}")))?;
        let reply = self
            .conn
            .xfixes_fetch_region(tracker.region)
            .map_err(|e| ScreenShareError::new(format!("X11 XFIXES FetchRegion failed: {e}")))?
            .reply()
            .map_err(|e| ScreenShareError::new(format!("X11 XFIXES FetchRegion reply failed: {e}")))?;
        Ok(reply
            .rectangles
            .iter()
            .map(|rect| FrameRect {
                x: rect.x.max(0) as u32,
                y: rect.y.max(0) as u32,
                width: rect.width as u32,
                height: rect.height as u32,
            })
            .filter(|rect| rect.width > 0 && rect.height > 0)
            .collect())
    }

    /// Number of frames skipped by damage tracking since (re)start.
    pub fn damage_skipped_frames(&self) -> u64 {
        self.damage.as_ref().map_or(0, |tracker| tracker.skipped)
    }

    /// Capture the given root-window rectangle, converting ZPixmap to RGBA8.
    /// The rectangle is clipped to the root bounds (RandR monitors can sit
    /// partially outside after resolution changes).
    ///
    /// Damage-aware (BORU-SS-32): when XDamage tracking is active the
    /// accumulated region is queried first and an unchanged screen returns
    /// `Ok(None)` without a GetImage — the host then skips encode and
    /// transmit entirely. Frames that ARE captured carry their [`DirtyRegion`]
    /// (first frame / resize → `Full`, otherwise the clipped rectangles), so
    /// downstream stages can skip unchanged regions in the future.
    fn capture_rect(&mut self, rect: CaptureRect) -> Result<Option<CapturedFrame>, ScreenShareError> {
        let Some((x, y, width, height)) = clip_to_root(rect, self.width, self.height) else {
            return Ok(None);
        };
        let clipped = CaptureRect { x, y, width, height };
        // Skip the GetImage entirely when the DAMAGE extension reports no
        // pixel change since the last frame.
        let region = match self.query_damage() {
            DamageQuery::Unavailable => None,
            DamageQuery::Clean => {
                self.advance_skipped_clock();
                return Ok(None);
            }
            DamageQuery::Full => Some(DirtyRegion::Full),
            DamageQuery::Dirty(rects) => {
                let region = damage_region_for_capture(&rects, clipped);
                if region.is_empty() {
                    // Damage occurred but lies entirely outside the capture
                    // geometry (e.g. another monitor repainted): nothing to
                    // send for this source.
                    self.advance_skipped_clock();
                    return Ok(None);
                }
                Some(region)
            }
        };
        let reply = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.root,
                clipped.x,
                clipped.y,
                clipped.width,
                clipped.height,
                u32::MAX,
            )
            .map_err(|e| ScreenShareError::new(format!("X11 GetImage failed: {e}")))?
            .reply()
            .map_err(|e| ScreenShareError::new(format!("X11 GetImage reply failed: {e}")))?;
        let pixels = convert_zpixmap_rgba(
            &reply.data,
            clipped.width as usize,
            clipped.height as usize,
            self.depth,
            self.lsb_first,
            self.red_mask,
            self.green_mask,
            self.blue_mask,
        )?;
        let timestamp_us = self.timestamp_us;
        self.timestamp_us = self.timestamp_us.saturating_add(X11_FRAME_PERIOD_US);
        let mut frame = CapturedFrame::cpu(
            timestamp_us,
            clipped.width as u32,
            clipped.height as u32,
            PixelFormat::Rgba8,
            pixels,
        )?;
        frame.dirty_region = region;
        Ok(Some(frame))
    }

    /// Advance the frame clock by one frame period without capturing
    /// (damage-aware skip), so the next captured frame's timestamp stays
    /// close to wall-clock instead of jumping after a long static period.
    fn advance_skipped_clock(&mut self) {
        self.timestamp_us = self.timestamp_us.saturating_add(X11_FRAME_PERIOD_US);
    }
}

impl ScreenCapture for X11Capture {
    fn capture(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
        // Refresh geometry every frame (the screen can resize); the capture
        // buffer is rebuilt only when the size actually changed.
        let geometry = self
            .conn
            .get_geometry(self.root)
            .map_err(|e| ScreenShareError::new(format!("X11 get_geometry failed: {e}")))?
            .reply()
            .map_err(|e| ScreenShareError::new(format!("X11 get_geometry reply failed: {e}")))?;
        let width = geometry.width as u32;
        let height = geometry.height as u32;
        if width == 0 || height == 0 {
            return Ok(None);
        }
        // A root resize makes the whole frame new: force a full repaint even
        // when the damage region is empty, so the viewer resynchronises to
        // the new geometry promptly.
        if width != self.width || height != self.height {
            self.force_full_next = true;
        }
        self.width = width;
        self.height = height;
        // The unstarted whole-root fallback never goes through
        // `DesktopCaptureBackend::start`, so subscribe to XDamage lazily on
        // the first capture.
        if self.damage.is_none() {
            self.setup_damage();
        }
        self.capture_rect(CaptureRect {
            x: 0,
            y: 0,
            width: width.min(u16::MAX as u32) as u16,
            height: height.min(u16::MAX as u32) as u16,
        })
    }
}

impl DesktopCaptureBackend for X11Capture {
    fn list_sources(&self) -> Result<Vec<CaptureSource>, ScreenShareError> {
        self.list_monitors()
            .map(|monitors| monitors.iter().map(x11_monitor_source).collect())
    }

    fn start(
        &mut self,
        source: CaptureSourceId,
        config: CaptureConfig,
    ) -> Result<(), ScreenShareError> {
        if self.started {
            return Err(ScreenShareError::new("capture already started"));
        }
        if config.target_fps == 0 {
            return Err(ScreenShareError::new("target fps must be non-zero"));
        }
        let monitor = self
            .list_monitors()?
            .into_iter()
            .find(|monitor| monitor.id == source)
            .ok_or_else(|| ScreenShareError::new("unknown X11 capture source"))?;
        self.selected = Some(CaptureRect {
            x: monitor.x,
            y: monitor.y,
            width: monitor.width,
            height: monitor.height,
        });
        self.current_source = Some(source);
        // Damage-aware capture (BORU-SS-32): subscribe to XDamage on the
        // root window. A tracker left over from the whole-root fallback is
        // reset so the first frame of the newly selected source is reported
        // fully dirty — the viewer needs a fresh baseline for the new
        // geometry.
        match self.damage.as_mut() {
            Some(tracker) => {
                tracker.first_frame = true;
                tracker.skipped = 0;
            }
            None => self.setup_damage(),
        }
        self.force_full_next = false;
        self.started = true;
        Ok(())
    }

    fn next_frame(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
        if !self.started {
            return Err(ScreenShareError::new(
                "capture is not started; call start() before next_frame()",
            ));
        }
        let Some(rect) = self.selected else {
            return Err(ScreenShareError::new("capture has no selected source"));
        };
        // A monitor whose rectangle lies entirely outside the root window is
        // gone (unplugged / undocked). Surface it as a typed error instead of
        // a silent "no frame" so the host can recover gracefully (PDF Phase
        // 10: re-enumerate, fall back to a remaining source, or pause).
        if clip_to_root(rect, self.width, self.height).is_none() {
            return Err(ScreenShareError::new(
                "capture source unavailable (monitor unplugged or outside the root)",
            ));
        }
        self.capture_rect(rect)
    }

    fn stop(&mut self) {
        self.started = false;
        self.selected = None;
        self.current_source = None;
        self.teardown_damage();
    }
}

/// Clip a capture rectangle to the root window bounds.
///
/// RandR can report monitor rectangles that extend past the root (e.g. after
/// a resolution change) or with negative origins; GetImage requires a
/// rectangle fully inside the drawable. Returns `None` when the rectangle is
/// completely outside the root.
pub fn clip_to_root(
    rect: CaptureRect,
    root_width: u32,
    root_height: u32,
) -> Option<(i16, i16, u16, u16)> {
    let left = rect.x.max(0) as i64;
    let top = rect.y.max(0) as i64;
    let right = (rect.x as i64 + rect.width as i64).min(root_width as i64);
    let bottom = (rect.y as i64 + rect.height as i64).min(root_height as i64);
    if left >= right || top >= bottom {
        return None;
    }
    Some((left as i16, top as i16, (right - left) as u16, (bottom - top) as u16))
}

/// Translate a root-space damage region into a capture-local [`DirtyRegion`]
/// (BORU-SS-32).
///
/// `capture` is the root-space rectangle that was actually captured (already
/// clipped to the root bounds), so every returned rectangle is guaranteed to
/// lie inside the captured frame. Rectangles outside the capture are
/// dropped; when the damage does not intersect the capture at all the result
/// is [`DirtyRegion::Rects`] with no entries (the caller should skip the
/// frame). When more than [`MAX_DIRTY_RECTS`] rectangles survive, the region
/// collapses to [`DirtyRegion::Full`] — the metadata stays bounded and a
/// near-full repaint is cheaper to encode as a full frame.
pub fn damage_region_for_capture(rects: &[FrameRect], capture: CaptureRect) -> DirtyRegion {
    const MAX_DIRTY_RECTS: usize = 16;
    let x0 = capture.x.max(0) as u32;
    let y0 = capture.y.max(0) as u32;
    let right_limit = x0 + capture.width as u32;
    let bottom_limit = y0 + capture.height as u32;
    let mut out = Vec::with_capacity(rects.len().min(MAX_DIRTY_RECTS));
    for rect in rects {
        let left = rect.x.max(x0);
        let top = rect.y.max(y0);
        let right = (rect.x + rect.width).min(right_limit);
        let bottom = (rect.y + rect.height).min(bottom_limit);
        if left >= right || top >= bottom {
            continue;
        }
        out.push(FrameRect {
            x: left - x0,
            y: top - y0,
            width: right - left,
            height: bottom - top,
        });
        if out.len() > MAX_DIRTY_RECTS {
            return DirtyRegion::Full;
        }
    }
    DirtyRegion::Rects(out)
}

/// Build a [`CaptureSource`] from an enumerated X11 monitor.
///
/// Pure helper so the source-advertisement shape (id, kind, title, native
/// size, desktop geometry) is unit-tested without a live X server; the
/// backend calls it from [`DesktopCaptureBackend::list_sources`]. The
/// geometry carries the monitor's root-window origin (which may be negative
/// for monitors left of / above the root origin), so the host can normalize
/// coordinates against the shared source.
pub fn x11_monitor_source(monitor: &X11Monitor) -> CaptureSource {
    CaptureSource {
        id: monitor.id,
        kind: CaptureSourceKind::Monitor,
        title: format!("{}: {}x{}", monitor.name, monitor.width, monitor.height),
        width: monitor.width as u32,
        height: monitor.height as u32,
        geometry: Some(MonitorGeometry::new(
            monitor.x as i32,
            monitor.y as i32,
            monitor.width as u32,
            monitor.height as u32,
        )),
    }
}

/// Convert an X11 ZPixmap `GetImage` buffer into RGBA8 using the root
/// visual's channel masks. Depth 24/32 visuals pack every pixel into 32 bits
/// (the high byte is padding for depth 24); the pixel value is reassembled in
/// the server's image byte order before the masks are applied, which makes the
/// conversion correct for both LSBFirst (BGRX on x86) and MSBFirst (XRGB)
/// servers.
fn convert_zpixmap_rgba(
    data: &[u8],
    width: usize,
    height: usize,
    depth: u8,
    lsb_first: bool,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
) -> Result<Vec<u8>, ScreenShareError> {
    let bpp = if matches!(depth, 24 | 32) { 4 } else { 0 };
    if bpp == 0 {
        return Err(ScreenShareError::new(format!(
            "unsupported ZPixmap depth {depth}"
        )));
    }
    let expected = width * height * bpp;
    if data.len() < expected {
        return Err(ScreenShareError::new(format!(
            "X11 image buffer too small: {} bytes for {width}x{height}@{depth}",
            data.len()
        )));
    }
    if red_mask == 0 || green_mask == 0 || blue_mask == 0 {
        return Err(ScreenShareError::new("X11 visual channel masks are empty"));
    }
    let red_shift = red_mask.trailing_zeros();
    let green_shift = green_mask.trailing_zeros();
    let blue_shift = blue_mask.trailing_zeros();
    let red_max = red_mask >> red_shift;
    let green_max = green_mask >> green_shift;
    let blue_max = blue_mask >> blue_shift;
    let mut out = Vec::with_capacity(width * height * 4);
    for chunk in data.chunks_exact(bpp).take(width * height) {
        let pixel = if lsb_first {
            u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        } else {
            u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        };
        let r = (((pixel & red_mask) >> red_shift) as u32 * 255) / red_max;
        let g = (((pixel & green_mask) >> green_shift) as u32 * 255) / green_max;
        let b = (((pixel & blue_mask) >> blue_shift) as u32 * 255) / blue_max;
        out.extend_from_slice(&[r as u8, g as u8, b as u8, 255]);
    }
    Ok(out)
}

// ── Selection factory ────────────────────────────────────────────────────────

/// The capture source chosen by [`create_capture_source`].
pub enum ActiveCapture {
    /// A real portal/PipeWire capture with its negotiated geometry.
    Portal(LinuxPortalCapture),
    /// A direct X11 GetImage capture of the root window.
    X11(X11Capture),
    /// Synthetic fallback (demo/CI path) with the given geometry.
    TestPattern(TestPatternCapture, (u32, u32)),
}

impl ActiveCapture {
    /// Capture the next frame, if one is ready.
    pub fn capture(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
        match self {
            ActiveCapture::Portal(capture) => capture.capture(),
            // The X11 backend captures the SELECTED monitor once started
            // (PDF Phase 10 monitor sharing); before start it falls back to
            // whole-root capture so an unstarted backend still produces
            // frames instead of erroring out.
            ActiveCapture::X11(capture) => {
                if capture.started {
                    <X11Capture as DesktopCaptureBackend>::next_frame(capture)
                } else {
                    <X11Capture as ScreenCapture>::capture(capture)
                }
            }
            ActiveCapture::TestPattern(capture, _) => capture.capture(),
        }
    }

    /// Active capture geometry for codec configuration.
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            ActiveCapture::Portal(capture) => {
                capture.negotiated_size().unwrap_or((DEMO_WIDTH, DEMO_HEIGHT))
            }
            // Once a monitor is selected, the shared source IS that monitor
            // (PDF Phase 10); before selection fall back to the root size.
            ActiveCapture::X11(capture) => capture
                .selected
                .map(|rect| (rect.width as u32, rect.height as u32))
                .unwrap_or((capture.width, capture.height)),
            ActiveCapture::TestPattern(_, size) => *size,
        }
    }

    /// Enumerate the capturable sources (monitors) available to this backend
    /// (PDF Phase 10: "enumerate available monitors before starting a
    /// share"). The portal backend exposes a single pseudo-source because
    /// Wayland monitor selection is performed by the portal dialog, not by
    /// the capture client; the test-pattern backend exposes its one source.
    pub fn list_sources(&self) -> Result<Vec<CaptureSource>, ScreenShareError> {
        match self {
            ActiveCapture::Portal(capture) => {
                let (width, height) = capture.negotiated_size().unwrap_or((DEMO_WIDTH, DEMO_HEIGHT));
                Ok(vec![CaptureSource {
                    id: CaptureSourceId(1),
                    kind: CaptureSourceKind::Desktop,
                    title: format!("Portal output: {width}x{height}"),
                    width,
                    height,
                    geometry: None,
                }])
            }
            ActiveCapture::X11(capture) => DesktopCaptureBackend::list_sources(capture),
            ActiveCapture::TestPattern(capture, _) => DesktopCaptureBackend::list_sources(capture),
        }
    }

    /// Begin capturing `source`. Monitor-based backends select the monitor
    /// (X11 RandR rect); the portal and test-pattern backends capture a
    /// fixed surface, so start is a no-op / single-source validation.
    pub fn start(
        &mut self,
        source: CaptureSourceId,
        config: &CaptureConfig,
    ) -> Result<(), ScreenShareError> {
        match self {
            ActiveCapture::Portal(_) => Ok(()),
            ActiveCapture::X11(capture) => DesktopCaptureBackend::start(capture, source, config.clone()),
            ActiveCapture::TestPattern(capture, _) => DesktopCaptureBackend::start(capture, source, config.clone()),
        }
    }

    /// Switch the shared source without recreating the backend (PDF Phase
    /// 10: the sharer can switch monitors without ending the chat session).
    /// Monitor-based backends re-select the source (stop + start); the
    /// portal backend returns a typed error because Wayland source switching
    /// requires a fresh portal selection; the test-pattern backend accepts
    /// only its single source.
    pub fn switch_source(
        &mut self,
        source: CaptureSourceId,
        config: &CaptureConfig,
    ) -> Result<(), ScreenShareError> {
        match self {
            ActiveCapture::Portal(_) => Err(ScreenShareError::new(
                "portal backend cannot switch sources; re-select through the portal",
            )),
            ActiveCapture::X11(capture) => {
                DesktopCaptureBackend::stop(capture);
                DesktopCaptureBackend::start(capture, source, config.clone())
            }
            ActiveCapture::TestPattern(capture, _) => {
                if source != CaptureSourceId(0) {
                    return Err(ScreenShareError::new("unknown capture source"));
                }
                // The synthetic source is a no-op switch; ensure the backend
                // is started so the lifecycle stays consistent (an already
                // started backend reports "already started", which is fine).
                let _ = DesktopCaptureBackend::start(capture, source, config.clone());
                Ok(())
            }
        }
    }

    /// The source currently being captured, when the backend tracks one.
    pub fn current_source(&self) -> Option<CaptureSource> {
        match self {
            ActiveCapture::Portal(capture) => {
                let (width, height) = capture.negotiated_size().unwrap_or((DEMO_WIDTH, DEMO_HEIGHT));
                Some(CaptureSource {
                    id: CaptureSourceId(1),
                    kind: CaptureSourceKind::Desktop,
                    title: format!("Portal output: {width}x{height}"),
                    width,
                    height,
                    geometry: None,
                })
            }
            ActiveCapture::X11(capture) => capture
                .current_source
                .and_then(|id| capture.list_sources().ok()?.into_iter().find(|source| source.id == id)),
            ActiveCapture::TestPattern(capture, _) => {
                DesktopCaptureBackend::list_sources(capture).ok().and_then(|mut sources| sources.pop())
            }
        }
    }

    /// Top-left of the captured rectangle in root-window coordinates.
    ///
    /// The X11 backend captures the selected monitor rect (or the whole root
    /// at `(0, 0)`); absolute XTest pointer motion needs this origin so the
    /// host can map capture pixels back to root coordinates (PDF Task 6.2).
    /// The portal and test-pattern paths have no root offset.
    pub fn input_origin(&self) -> (i32, i32) {
        match self {
            ActiveCapture::X11(capture) => capture
                .selected
                .map(|rect| (rect.x as i32, rect.y as i32))
                .unwrap_or((0, 0)),
            _ => (0, 0),
        }
    }

    /// Whether the synthetic fallback is active (viewer/UI diagnostics).
    pub fn is_test_pattern(&self) -> bool {
        matches!(self, ActiveCapture::TestPattern(..))
    }

    /// Human-readable backend name for startup diagnostics.
    pub fn backend_name(&self) -> &'static str {
        match self {
            ActiveCapture::Portal(_) => "portal",
            ActiveCapture::X11(_) => "x11",
            ActiveCapture::TestPattern(..) => "test-pattern",
        }
    }
}

const DEMO_WIDTH: u32 = 640;
const DEMO_HEIGHT: u32 = 360;
const DEMO_FPS: u32 = 15;

/// Try the real platform capture first, then the direct X11 backend, falling
/// back to the synthetic test pattern. `force_fallback` is a test hook;
/// production callers pass `false`.
///
/// Backend order is display-server aware (PDF Task 6.1): under Wayland or
/// XWayland the portal is preferred (a direct X11 capture would only see
/// XWayland windows); under a native X11 session the direct backend needs no
/// portal daemon and is tried first.
pub async fn create_capture_source(force_fallback: bool) -> ActiveCapture {
    #[cfg(target_os = "linux")]
    {
        if !force_fallback {
            let portal_first = detect_display_server().prefers_portal();
            let portal = async {
                LinuxPortalCapture::connect()
                    .await
                    .ok()
                    .map(ActiveCapture::Portal)
            };
            let x11 = || X11Capture::connect().ok().map(ActiveCapture::X11);
            if portal_first {
                if let Some(capture) = portal.await {
                    return capture;
                }
                if let Some(capture) = x11() {
                    return capture;
                }
            } else {
                if let Some(capture) = x11() {
                    return capture;
                }
                if let Some(capture) = portal.await {
                    return capture;
                }
            }
        }
    }
    ActiveCapture::TestPattern(
        TestPatternCapture::new(DEMO_WIDTH, DEMO_HEIGHT).unwrap(),
        (DEMO_WIDTH, DEMO_HEIGHT),
    )
}

/// Active capture dimensions used for codec configuration and pointer mapping.
pub fn capture_dimensions(capture: &ActiveCapture) -> (u32, u32) {
    capture.dimensions()
}

/// Default frame rate for the real capture path (15 fps keeps encode cost low).
pub const CAPTURE_FPS: u32 = DEMO_FPS;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen_share::platform::linux_pw;

    /// Drive a machine into the requested phase (used by teardown tests).
    fn machine_in_phase(phase: SessionPhase) -> PortalSessionMachine {
        let mut machine = PortalSessionMachine::new();
        match phase {
            SessionPhase::Idle => {}
            SessionPhase::Creating => {
                machine.create_session().unwrap();
            }
            SessionPhase::Selecting => {
                machine.create_session().unwrap();
                machine.on_session_created().unwrap();
            }
            SessionPhase::Starting => {
                machine.create_session().unwrap();
                machine.on_session_created().unwrap();
                machine.select_sources().unwrap();
            }
            SessionPhase::Streaming => {
                machine.create_session().unwrap();
                machine.on_session_created().unwrap();
                machine.select_sources().unwrap();
                machine.start().unwrap();
                machine.on_start_response_ok().unwrap();
            }
            other => panic!("cannot construct phase {other:?}"),
        }
        machine
    }

    #[test]
    fn portal_machine_happy_path_lifecycle() {
        let mut machine = PortalSessionMachine::new();
        assert_eq!(machine.phase(), SessionPhase::Idle);
        assert!(!machine.is_terminal());
        machine.create_session().unwrap();
        machine.on_session_created().unwrap();
        assert_eq!(machine.phase(), SessionPhase::Selecting);
        machine.select_sources().unwrap();
        assert_eq!(machine.phase(), SessionPhase::Starting);
        machine.start().unwrap();
        machine.on_start_response_ok().unwrap();
        assert_eq!(machine.phase(), SessionPhase::Streaming);
        assert!(!machine.is_terminal());
        // Clean teardown: Closing → Closed.
        machine.begin_close().unwrap();
        assert_eq!(machine.phase(), SessionPhase::Closing);
        assert_eq!(machine.close_requests(), 1);
        machine.on_closed().unwrap();
        assert_eq!(machine.phase(), SessionPhase::Closed);
        assert!(machine.is_terminal());
    }

    #[test]
    fn portal_machine_rejection_fails_and_blocks_further_transitions() {
        let mut machine = machine_in_phase(SessionPhase::Starting);
        machine.on_start_response_rejected(2).unwrap();
        let failed = SessionPhase::Failed(SessionFailure::StartRejected(2));
        assert_eq!(machine.phase(), failed);
        assert!(machine.is_terminal());
        // A failed session rejects every further transition.
        assert_eq!(machine.begin_close(), Err(MachineError::InvalidTransition { from: failed }));
        assert_eq!(machine.on_closed(), Err(MachineError::InvalidTransition { from: failed }));
        assert_eq!(machine.on_portal_closed(), Err(MachineError::InvalidTransition { from: failed }));
        assert_eq!(machine.on_failure(SessionFailure::StartTimeout), Err(MachineError::InvalidTransition { from: failed }));
    }

    #[test]
    fn portal_machine_start_failure_paths_are_terminal() {
        // Timeout waiting for Start to return the request path.
        let mut machine = machine_in_phase(SessionPhase::Starting);
        machine.on_start_timeout().unwrap();
        assert_eq!(machine.phase(), SessionPhase::Failed(SessionFailure::StartTimeout));
        assert!(machine.is_terminal());

        // Timeout waiting for the Response signal.
        let mut machine = machine_in_phase(SessionPhase::Starting);
        machine.on_start_timeout().unwrap();
        assert!(machine.is_terminal());

        // Response stream closed before a response arrived.
        let mut machine = machine_in_phase(SessionPhase::Starting);
        machine.on_response_stream_closed().unwrap();
        assert_eq!(machine.phase(), SessionPhase::Failed(SessionFailure::ResponseStreamClosed));

        // Response(0) but no usable node id.
        let mut machine = machine_in_phase(SessionPhase::Starting);
        machine.on_missing_node_id().unwrap();
        assert_eq!(machine.phase(), SessionPhase::Failed(SessionFailure::MissingNodeId));

        // D-Bus transport failure on Start.
        let mut machine = machine_in_phase(SessionPhase::Starting);
        machine.on_failure(SessionFailure::StartFailed).unwrap();
        assert_eq!(machine.phase(), SessionPhase::Failed(SessionFailure::StartFailed));
    }

    #[test]
    fn portal_machine_failure_escape_covers_early_dbus_errors() {
        let mut machine = PortalSessionMachine::new();
        machine.on_failure(SessionFailure::NoSessionBus).unwrap();
        assert_eq!(machine.phase(), SessionPhase::Failed(SessionFailure::NoSessionBus));

        let mut machine = PortalSessionMachine::new();
        machine.create_session().unwrap();
        machine.on_failure(SessionFailure::CreateSessionFailed).unwrap();
        assert_eq!(machine.phase(), SessionPhase::Failed(SessionFailure::CreateSessionFailed));

        let mut machine = machine_in_phase(SessionPhase::Selecting);
        machine.on_failure(SessionFailure::SelectSourcesFailed).unwrap();
        assert_eq!(machine.phase(), SessionPhase::Failed(SessionFailure::SelectSourcesFailed));
    }

    #[test]
    fn portal_machine_portal_closed_ends_active_session() {
        for phase in [
            SessionPhase::Creating,
            SessionPhase::Selecting,
            SessionPhase::Starting,
            SessionPhase::Streaming,
        ] {
            let mut machine = machine_in_phase(phase);
            machine.on_portal_closed().unwrap();
            assert_eq!(machine.phase(), SessionPhase::Closed, "phase {phase:?}");
            assert!(machine.is_terminal());
        }
    }

    #[test]
    fn portal_machine_close_from_every_active_phase() {
        for phase in [
            SessionPhase::Idle,
            SessionPhase::Creating,
            SessionPhase::Selecting,
            SessionPhase::Starting,
            SessionPhase::Streaming,
        ] {
            let mut machine = machine_in_phase(phase);
            machine.begin_close().unwrap();
            assert_eq!(machine.phase(), SessionPhase::Closing, "phase {phase:?}");
            machine.on_closed().unwrap();
            assert_eq!(machine.phase(), SessionPhase::Closed, "phase {phase:?}");
        }
    }

    #[test]
    fn portal_machine_close_is_once_per_session() {
        let mut machine = machine_in_phase(SessionPhase::Streaming);
        machine.begin_close().unwrap();
        // A second close while already Closing is rejected.
        assert_eq!(machine.begin_close(), Err(MachineError::InvalidTransition { from: SessionPhase::Closing }));
        machine.on_closed().unwrap();
        assert_eq!(machine.phase(), SessionPhase::Closed);
        assert_eq!(machine.close_requests(), 1);
        // Terminal states reject everything, including another close.
        assert_eq!(machine.on_closed(), Err(MachineError::InvalidTransition { from: SessionPhase::Closed }));
        assert_eq!(machine.begin_close(), Err(MachineError::InvalidTransition { from: SessionPhase::Closed }));
    }

    #[test]
    fn portal_machine_rejects_invalid_orderings() {
        let mut machine = PortalSessionMachine::new();
        assert!(machine.create_session().is_ok());
        assert!(machine.create_session().is_err()); // already Creating
        assert!(machine.start().is_err()); // Start before SelectSources
        assert!(machine.on_start_response_ok().is_err()); // response before Start
        assert!(machine.on_session_created().is_ok()); // Creating → Selecting
        assert!(machine.select_sources().is_ok()); // Selecting → Starting
        assert!(machine.select_sources().is_err()); // already Starting
        assert!(machine.on_session_created().is_err()); // already past Creating
        assert!(machine.start().is_ok()); // Start validated in Starting
        assert!(machine.on_start_response_ok().is_ok()); // → Streaming
        assert!(machine.on_start_response_ok().is_err()); // already Streaming
    }

    #[test]
    fn portal_machine_state_maps_to_portal_state() {
        // The lifecycle machine drives LinuxPortalCapture::state(); verify the
        // mapping contract used there stays stable.
        assert_eq!(PortalSessionMachine::new().phase(), SessionPhase::Idle);
        let streaming = machine_in_phase(SessionPhase::Streaming);
        assert_eq!(streaming.phase(), SessionPhase::Streaming);
        let mut closing = streaming;
        closing.begin_close().unwrap();
        assert_eq!(closing.phase(), SessionPhase::Closing);
    }

    #[test]
    fn desktop_environment_classification() {
        assert_eq!(classify_desktop_environment("GNOME"), DesktopEnvironment::Gnome);
        assert_eq!(classify_desktop_environment("ubuntu:GNOME"), DesktopEnvironment::Gnome);
        assert_eq!(classify_desktop_environment("KDE"), DesktopEnvironment::Kde);
        assert_eq!(classify_desktop_environment("KDE-plasma"), DesktopEnvironment::Kde);
        assert_eq!(classify_desktop_environment("X-KDE-plasma:5"), DesktopEnvironment::Kde);
        assert_eq!(classify_desktop_environment("sway"), DesktopEnvironment::Wlroots);
        assert_eq!(classify_desktop_environment("Hyprland"), DesktopEnvironment::Wlroots);
        assert_eq!(classify_desktop_environment("wayfire"), DesktopEnvironment::Wlroots);
        assert_eq!(classify_desktop_environment("wlroots"), DesktopEnvironment::Wlroots);
        assert_eq!(classify_desktop_environment(""), DesktopEnvironment::Unknown);
        assert_eq!(classify_desktop_environment("Cinnamon"), DesktopEnvironment::Other);
        assert_eq!(classify_desktop_environment("XFCE"), DesktopEnvironment::Other);
    }

    #[test]
    fn session_type_classification() {
        assert_eq!(classify_session_type("wayland"), SessionType::Wayland);
        assert_eq!(classify_session_type("x11"), SessionType::X11);
        assert_eq!(classify_session_type(""), SessionType::Unknown);
        assert_eq!(classify_session_type("mir"), SessionType::Unknown);
    }

    /// Cursor-mode selection (BORU-SS-33 / PDF Task 5.3): Metadata is
    /// preferred when the portal advertises it (the compositor sends the
    /// cursor as `spa_meta_cursor` stream metadata, so cursor motion does
    /// not force a full-frame re-encode); Embedded is the composited
    /// fallback; Hidden only when neither is available.
    #[test]
    fn cursor_mode_prefers_metadata_then_embedded_when_advertised() {
        // Metadata (4) advertised on its own or alongside others.
        assert_eq!(choose_cursor_mode(4), CursorMode::Metadata);
        assert_eq!(choose_cursor_mode(1 | 2 | 4), CursorMode::Metadata);
        assert_eq!(choose_cursor_mode(2 | 4), CursorMode::Metadata);
        // Only Embedded (2) advertised → Embedded fallback (composited).
        assert_eq!(choose_cursor_mode(2), CursorMode::Embedded);
        // Only Hidden (1) or nothing advertised → Hidden fallback.
        assert_eq!(choose_cursor_mode(1), CursorMode::Hidden);
        assert_eq!(choose_cursor_mode(0), CursorMode::Hidden);
    }

    /// The SelectSources options vardict carries types=Monitor plus the
    /// negotiated cursor_mode when one is provided; omitting the option
    /// leaves the portal default (Hidden) untouched.
    #[test]
    fn select_sources_options_include_cursor_mode_when_negotiated() {
        let with_embedded = select_sources_options(Some(CursorMode::Embedded));
        assert_eq!(with_embedded.get("types"), Some(&zbus::zvariant::Value::U32(1)));
        assert_eq!(with_embedded.get("cursor_mode"), Some(&zbus::zvariant::Value::U32(2)));

        let hidden = select_sources_options(Some(CursorMode::Hidden));
        assert_eq!(hidden.get("cursor_mode"), Some(&zbus::zvariant::Value::U32(1)));

        let none = select_sources_options(None);
        assert_eq!(none.get("types"), Some(&zbus::zvariant::Value::U32(1)));
        assert!(none.get("cursor_mode").is_none());
    }

    #[test]
    fn cursor_mode_bits_match_portal_values() {
        assert_eq!(CursorMode::Hidden.bit(), 1);
        assert_eq!(CursorMode::Embedded.bit(), 2);
        assert_eq!(CursorMode::Metadata.bit(), 4);
    }

    #[test]
    fn cancellation_ends_selection() {
        let mut c = PortalCapture::new(2).unwrap();
        c.begin_selection().unwrap();
        c.cancel();
        assert_eq!(c.state(), PortalState::Ended);
    }

    #[test]
    fn format_pod_builder_produces_parsable_object() {
        let pod = build_format_pod();
        assert!(pod.len() > 32);
        // Header: body size + Object type.
        assert_eq!(
            u32::from_le_bytes(pod[4..8].try_into().unwrap()),
            linux_pw::SPA_TYPE_Object
        );
        let parsed = parse_format_pod(&pod);
        // The pod advertises BGRx first; parse returns the first value.
        let (width, height, layout) = parsed.expect("pod must parse");
        assert_eq!((width, height), (640, 360));
        assert_eq!(layout, linux_pw::PwPixelLayout::Bgra8);
    }

    #[test]
    fn create_capture_source_falls_back_to_test_pattern() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut capture = rt.block_on(create_capture_source(true));
        assert!(capture.is_test_pattern());
        assert_eq!(capture.dimensions(), (DEMO_WIDTH, DEMO_HEIGHT));
        let frame = capture.capture().unwrap().unwrap();
        assert_eq!(frame.pixel_format, PixelFormat::Rgba8);
        assert_eq!(frame.pixels.len(), (DEMO_WIDTH * DEMO_HEIGHT * 4) as usize);
    }

    #[test]
    fn portal_capture_rejects_frames_outside_streaming() {
        let mut c = PortalCapture::new(2).unwrap();
        let frame = CapturedFrame::cpu(0, 2, 2, PixelFormat::Bgra8, vec![0; 16]).unwrap();
        assert!(c.push_pipewire_frame(frame).is_err());
        c.begin_selection().unwrap();
        c.source_selected().unwrap();
        let frame = CapturedFrame::cpu(0, 2, 2, PixelFormat::Bgra8, vec![0; 16]).unwrap();
        assert!(c.push_pipewire_frame(frame).is_ok());
        assert_eq!(c.state(), PortalState::Streaming);
    }

    #[test]
    fn parse_rejects_non_object_pod() {
        assert!(parse_format_pod(&[]).is_none());
        assert!(parse_format_pod(&[0, 0, 0, 0, 1, 0, 0, 0]).is_none());
    }

    #[test]
    fn zpixmap_lsb_first_bgrx_converts_to_rgba() {
        // Depth 24, LSBFirst (x86): pixel bytes are B,G,R,X. Two pixels:
        // (0x30,0x20,0x10) → RGB(0x10,0x20,0x30) and (0xAA,0xBB,0xCC) → RGB(0xCC,0xBB,0xAA).
        let data = [0x30, 0x20, 0x10, 0x00, 0xAA, 0xBB, 0xCC, 0x00];
        let out = convert_zpixmap_rgba(
            &data, 2, 1, 24, true, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF,
        )
        .unwrap();
        assert_eq!(out, vec![0x10, 0x20, 0x30, 255, 0xCC, 0xBB, 0xAA, 255]);
    }

    #[test]
    fn zpixmap_msb_first_xrgb_converts_to_rgba() {
        // Depth 24, MSBFirst (big-endian): pixel bytes are X,R,G,B.
        let data = [0x00, 0x10, 0x20, 0x30];
        let out = convert_zpixmap_rgba(
            &data, 1, 1, 24, false, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF,
        )
        .unwrap();
        assert_eq!(out, vec![0x10, 0x20, 0x30, 255]);
    }

    #[test]
    fn zpixmap_respects_nonstandard_channel_masks() {
        // 5-6-5 masks (R=0xF800, G=0x07E0, B=0x001F). A pixel with
        // R=0x1F,G=0x3F,B=0x1F packs as 0xF800|0x07E0|0x001F = 0xFFFF;
        // LSBFirst bytes are [0xFF, 0xFF, 0x00, 0x00].
        let data = [0xFF, 0xFF, 0x00, 0x00];
        let out = convert_zpixmap_rgba(
            &data, 1, 1, 24, true, 0x0000_F800, 0x0000_07E0, 0x0000_001F,
        )
        .unwrap();
        // R: 31/31*255 = 255; G: 63/63*255 = 255; B: 255.
        assert_eq!(out, vec![255, 255, 255, 255]);
    }

    #[test]
    fn zpixmap_rejects_unsupported_depth_and_short_buffer() {
        assert!(
            convert_zpixmap_rgba(&[0; 8], 2, 1, 16, true, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF)
                .is_err()
        );
        assert!(
            convert_zpixmap_rgba(&[0; 4], 2, 1, 24, true, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF)
                .is_err()
        );
        assert!(
            convert_zpixmap_rgba(&[0; 8], 2, 1, 24, true, 0, 0, 0x0000_00FF).is_err()
        );
    }

    // ── Display-server detection (BORU-SS-16 / PDF Task 6.1) ────────────────

    #[test]
    fn display_server_classification() {
        // Wayland session without DISPLAY → Wayland.
        assert_eq!(
            classify_display_server(Some("wayland-0"), Some("wayland"), None),
            DisplayServer::Wayland
        );
        assert_eq!(
            classify_display_server(Some("wayland-0"), None, None),
            DisplayServer::Wayland
        );
        // Wayland session WITH DISPLAY → XWayland (X server is XWayland).
        assert_eq!(
            classify_display_server(Some("wayland-0"), Some("wayland"), Some(":0")),
            DisplayServer::XWayland
        );
        assert_eq!(
            classify_display_server(Some("wayland-0"), None, Some(":0")),
            DisplayServer::XWayland
        );
        // Native X11: XDG_SESSION_TYPE=x11 or only DISPLAY set.
        assert_eq!(
            classify_display_server(None, Some("x11"), Some(":0")),
            DisplayServer::X11
        );
        assert_eq!(
            classify_display_server(None, None, Some(":0")),
            DisplayServer::X11
        );
        // Nothing set → Unknown (headless CI, ssh without X forwarding).
        assert_eq!(
            classify_display_server(None, None, None),
            DisplayServer::Unknown
        );
        assert_eq!(
            classify_display_server(None, Some("tty"), None),
            DisplayServer::Unknown
        );
    }

    #[test]
    fn display_server_portal_preference() {
        // Wayland/XWayland must prefer the portal (direct X11 capture would
        // only see XWayland windows).
        assert!(DisplayServer::Wayland.prefers_portal());
        assert!(DisplayServer::XWayland.prefers_portal());
        // Native X11 and unknown sessions use the direct backend first.
        assert!(!DisplayServer::X11.prefers_portal());
        assert!(!DisplayServer::Unknown.prefers_portal());
    }

    #[test]
    fn display_server_round_trips_through_environment() {
        // The env-reader is a thin wrapper over classify; sanity-check that a
        // wayland+DISPLAY env combination is seen as XWayland.
        std::env::set_var("WAYLAND_DISPLAY", "wayland-1");
        std::env::set_var("DISPLAY", ":0");
        std::env::set_var("XDG_SESSION_TYPE", "wayland");
        assert_eq!(detect_display_server(), DisplayServer::XWayland);
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::set_var("XDG_SESSION_TYPE", "x11");
        assert_eq!(detect_display_server(), DisplayServer::X11);
        std::env::remove_var("DISPLAY");
        std::env::remove_var("XDG_SESSION_TYPE");
        assert_eq!(detect_display_server(), DisplayServer::Unknown);
    }

    // ── X11 geometry selection (BORU-SS-16 / PDF Task 6.1) ──────────────────

    #[test]
    fn clip_to_root_keeps_fully_inside_rect() {
        let rect = CaptureRect { x: 100, y: 50, width: 800, height: 600 };
        assert_eq!(clip_to_root(rect, 1920, 1080), Some((100, 50, 800, 600)));
    }

    #[test]
    fn clip_to_root_clamps_partial_overflow() {
        // Monitor sits past the right/bottom edge of a 1920x1080 root.
        let rect = CaptureRect { x: 1800, y: 1000, width: 400, height: 300 };
        assert_eq!(clip_to_root(rect, 1920, 1080), Some((1800, 1000, 120, 80)));
    }

    #[test]
    fn clip_to_root_rejects_fully_outside_rect() {
        // Monitor entirely beyond the root bounds → nothing to capture.
        let rect = CaptureRect { x: 2000, y: 0, width: 100, height: 100 };
        assert_eq!(clip_to_root(rect, 1920, 1080), None);
        let rect = CaptureRect { x: 0, y: 1200, width: 100, height: 100 };
        assert_eq!(clip_to_root(rect, 1920, 1080), None);
    }

    #[test]
    fn clip_to_root_clamps_negative_origin() {
        // RandR can report a monitor left of the root origin.
        let rect = CaptureRect { x: -100, y: -50, width: 400, height: 300 };
        assert_eq!(clip_to_root(rect, 1920, 1080), Some((0, 0, 300, 250)));
    }

    #[test]
    fn x11_monitor_source_advertises_geometry() {
        let monitor = X11Monitor {
            id: monitor_source_id("DP-1"),
            name: "DP-1".to_string(),
            x: 1920,
            y: 0,
            width: 2560,
            height: 1440,
            primary: false,
        };
        let source = x11_monitor_source(&monitor);
        assert_eq!(source.id, monitor_source_id("DP-1"));
        assert_eq!(source.kind, CaptureSourceKind::Monitor);
        assert_eq!(source.title, "DP-1: 2560x1440");
        assert_eq!(source.width, 2560);
        assert_eq!(source.height, 1440);
        assert_eq!(
            source.geometry,
            Some(MonitorGeometry::new(1920, 0, 2560, 1440))
        );
    }

    #[test]
    fn x11_monitor_source_handles_negative_origin() {
        // A monitor left of / above the primary has a negative origin; the
        // CaptureSource must carry it so coordinate mapping stays correct.
        let monitor = X11Monitor {
            id: monitor_source_id("HDMI-A-0"),
            name: "HDMI-A-0".to_string(),
            x: -1920,
            y: -120,
            width: 1920,
            height: 1080,
            primary: false,
        };
        let source = x11_monitor_source(&monitor);
        assert_eq!(
            source.geometry,
            Some(MonitorGeometry::new(-1920, -120, 1920, 1080))
        );
    }

    #[test]
    fn x11_monitor_id_is_stable_and_distinct() {
        let dp1 = monitor_source_id("DP-1");
        let again = monitor_source_id("DP-1");
        let hdmi = monitor_source_id("HDMI-A-0");
        assert_eq!(dp1, again);
        assert_ne!(dp1, hdmi);
    }

    // ── Damage-region accumulation / clipping (BORU-SS-32) ──────────────────

    fn root_rect(x: u32, y: u32, width: u32, height: u32) -> FrameRect {
        FrameRect { x, y, width, height }
    }

    #[test]
    fn damage_region_clips_and_translates_to_capture_coords() {
        // Root 1920x1080, capturing the monitor at (960, 0) size 960x540.
        let capture = CaptureRect { x: 960, y: 0, width: 960, height: 540 };
        let region = damage_region_for_capture(
            &[
                root_rect(1000, 100, 100, 50),
                // Starts inside the capture, straddles its right edge →
                // clamped, not dropped.
                root_rect(1800, 400, 100, 100),
                // Entirely outside the capture (right of x=1920) → dropped.
                root_rect(1920, 0, 100, 100),
            ],
            capture,
        );
        assert_eq!(
            region,
            DirtyRegion::Rects(vec![
                root_rect(40, 100, 100, 50),
                root_rect(840, 400, 100, 100),
            ])
        );
        assert!(!region.is_empty());
    }

    #[test]
    fn damage_region_fully_outside_capture_is_empty() {
        // Damage on another monitor (left of the captured one) must not
        // trigger a frame for this source.
        let capture = CaptureRect { x: 1920, y: 0, width: 1920, height: 1080 };
        let region = damage_region_for_capture(&[root_rect(10, 10, 50, 50)], capture);
        assert!(matches!(region, DirtyRegion::Rects(ref r) if r.is_empty()));
        assert!(region.is_empty());
    }

    #[test]
    fn damage_region_clamps_to_capture_bounds() {
        // Damage rect straddling the capture edge is clamped, not dropped.
        let capture = CaptureRect { x: 0, y: 0, width: 100, height: 100 };
        let region = damage_region_for_capture(&[root_rect(80, 80, 100, 100)], capture);
        assert_eq!(region, DirtyRegion::Rects(vec![root_rect(80, 80, 20, 20)]));
    }

    #[test]
    fn damage_region_collapses_to_full_when_too_many_rects() {
        // A near-full repaint reports many rectangles; collapsing to Full
        // keeps the metadata bounded.
        let capture = CaptureRect { x: 0, y: 0, width: 1024, height: 1024 };
        let rects: Vec<FrameRect> = (0..20)
            .map(|i| root_rect(i * 40, i * 40, 40, 40))
            .collect();
        let region = damage_region_for_capture(&rects, capture);
        assert_eq!(region, DirtyRegion::Full);
        assert!(!region.is_empty(), "Full is never 'empty'");
    }

    #[test]
    fn damage_region_handles_negative_capture_origin() {
        // A monitor left of the root origin captures from root (0,0); damage
        // rects are root-relative and translate into capture-local coords.
        let capture = CaptureRect { x: -1920, y: 0, width: 1920, height: 1080 };
        let region = damage_region_for_capture(&[root_rect(0, 0, 100, 100)], capture);
        assert_eq!(region, DirtyRegion::Rects(vec![root_rect(0, 0, 100, 100)]));
    }

    #[test]
    fn empty_dirty_region_semantics() {
        // Rects(empty) means "nothing changed"; Full and None never do.
        assert!(DirtyRegion::Rects(vec![]).is_empty());
        assert!(!DirtyRegion::Full.is_empty());
        assert!(!DirtyRegion::Rects(vec![root_rect(0, 0, 1, 1)]).is_empty());
    }

    /// Live X11 backend test — REQUIRES a real X server (`$DISPLAY` set, e.g.
    /// a desktop session, Xvfb, or Xwayland). Skipped by default; run with
    /// `cargo test --features screen-sharing -- --ignored x11_live_`.
    ///
    /// Verifies: `X11Capture::connect`, monitor enumeration (RandR
    /// GetMonitors → CRTC fallback → root fallback), and a real GetImage
    /// capture through the [`DesktopCaptureBackend`] lifecycle.
    #[test]
    #[ignore = "requires a real X server (DISPLAY set)"]
    fn x11_live_enumerates_and_captures_selected_monitor() {
        let mut capture = X11Capture::connect().expect("connect to $DISPLAY");
        let monitors = capture.list_monitors().expect("enumerate monitors");
        assert!(!monitors.is_empty(), "at least one monitor expected");
        let primary = monitors
            .iter()
            .find(|m| m.primary)
            .or_else(|| monitors.first())
            .expect("primary or first monitor");
        let sources = capture.list_sources().expect("list sources");
        assert_eq!(sources.len(), monitors.len());
        let source = x11_monitor_source(primary);
        capture
            .start(source.id, CaptureConfig::default())
            .expect("start primary monitor");
        let frame = capture
            .next_frame()
            .expect("next_frame after start")
            .expect("a frame from GetImage");
        assert_eq!(frame.width, primary.width as u32);
        assert_eq!(frame.height, primary.height as u32);
        assert_eq!(frame.pixel_format, PixelFormat::Rgba8);
        // Damage-aware capture (BORU-SS-32): when the DAMAGE/XFIXES
        // extensions are present the first frame is the full baseline and
        // every captured frame carries dirty metadata. (Without the
        // extensions, `dirty_region` stays None and behaviour is unchanged.)
        if capture.damage.is_some() {
            assert_eq!(
                frame.dirty_region,
                Some(DirtyRegion::Full),
                "first frame after start must be the full baseline"
            );
        }
        // Lifecycle enforcement: double start is an error, next_frame after
        // stop is an error, stop is idempotent.
        assert!(capture
            .start(source.id, CaptureConfig::default())
            .is_err());
        capture.stop();
        capture.stop(); // idempotent
        assert!(capture.next_frame().is_err());
    }

    /// Live damage-skip — REQUIRES a real X server with the DAMAGE and
    /// XFIXES extensions (Xvfb provides both). Verifies the frame-level skip
    /// end to end: the first frame is fully dirty, then a static screen
    /// produces `None` (no GetImage, no encode) on subsequent ticks.
    #[test]
    #[ignore = "requires a real X server (DISPLAY set)"]
    fn x11_live_damage_tracking_skips_static_screen() {
        let mut capture = X11Capture::connect().expect("connect to $DISPLAY");
        let monitors = capture.list_monitors().expect("enumerate monitors");
        let source = monitors
            .iter()
            .find(|m| m.primary)
            .or_else(|| monitors.first())
            .map(x11_monitor_source)
            .expect("primary or first monitor");
        capture
            .start(source.id, CaptureConfig::default())
            .expect("start primary monitor");
        if capture.damage.is_none() {
            eprintln!("skipping: DAMAGE/XFIXES extensions unavailable on this server");
            capture.stop();
            return;
        }
        let first = capture
            .next_frame()
            .expect("next_frame after start")
            .expect("first frame from GetImage");
        assert_eq!(first.dirty_region, Some(DirtyRegion::Full));
        // A static Xvfb root produces no further damage: most of the next
        // ticks must be skipped (Ok(None)) rather than GetImage+encode every
        // tick. A real desktop may repaint (clock, cursor) so the assertion
        // tolerates a few returned frames — but a fully static screen must
        // never return 20/20 frames.
        let mut frames = 0u32;
        let mut skips = 0u64;
        for _ in 0..20 {
            match capture.next_frame() {
                Ok(Some(frame)) => {
                    assert!(
                        frame.dirty_region.is_some(),
                        "captured frame must carry dirty metadata"
                    );
                    frames += 1;
                }
                Ok(None) => skips += 1,
                Err(error) => panic!("next_frame failed: {error}"),
            }
        }
        assert!(
            skips >= 1,
            "static screen must skip at least one frame (frames={frames}, skips={skips})"
        );
        assert!(
            capture.damage_skipped_frames() >= skips,
            "backend skip counter must reflect observed skips"
        );
        capture.stop();
    }

    /// Live whole-root capture — REQUIRES a real X server. Exercises the
    /// `ScreenCapture` trait used by the `ActiveCapture::X11` fallback path.
    #[test]
    #[ignore = "requires a real X server (DISPLAY set)"]
    fn x11_live_screen_capture_whole_root() {
        let mut capture = X11Capture::connect().expect("connect to $DISPLAY");
        let frame = capture.capture().expect("whole-root capture").expect("frame");
        assert!(frame.width > 0 && frame.height > 0);
        assert_eq!(frame.pixel_format, PixelFormat::Rgba8);
        assert_eq!(
            frame.pixels.len(),
            (frame.width * frame.height * 4) as usize
        );
    }
}
