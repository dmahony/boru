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
    capture::FrameSink, CapturedFrame, PixelFormat, ScreenCapture, ScreenShareError,
    TestPatternCapture,
};
use x11rb::connection::Connection as _;
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

/// Format state negotiated with the PipeWire stream. Read by the main thread.
#[derive(Debug, Clone, Copy)]
struct NegotiatedFormat {
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
}

impl Default for NegotiatedFormat {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            pixel_format: PixelFormat::Bgra8,
        }
    }
}

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
                ScreenShareError::new(format!(
                    "no session bus (session={session_type:?}, desktop={environment:?}): {e}"
                ))
            })?;
        let portal = (
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.ScreenCast",
        );
        let portal_version = query_portal_version(&connection).await;
        let backend = detect_portal_backend(&connection).await;
        tracing::info!(
            session_type = ?session_type,
            desktop = ?environment,
            ?portal_version,
            backend = ?backend,
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

        // 2. SelectSources(types = Monitor). No `multiple` option: exactly one
        // stream is requested, which every portal implementation supports.
        // The desktop-environment permission dialog is NEVER bypassed — on
        // Wayland the compositor shows its picker at Start, on X11 the portal
        // auto-selects the primary monitor.
        let select_options: std::collections::HashMap<&str, zbus::zvariant::Value> =
            [("types", zbus::zvariant::Value::U32(1))].into_iter().collect();
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
            if fmt.width == 0 {
                *fmt = NegotiatedFormat {
                    width: frame.width,
                    height: frame.height,
                    pixel_format: frame.pixel_format,
                };
            }
            if fmt.width != frame.width || fmt.height != frame.height {
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
            let library = libloading::Library::new(PW_LIB)
                .map_err(|e| ScreenShareError::new(format!("cannot load {PW_LIB}: {e}")))?;
            let pw = Pw::load(&library)?;
            let mut argc = 0i32;
            let mut argv: *mut *mut c_char = std::ptr::null_mut();
            (pw.init)(&mut argc, &mut argv);

            let main_loop = (pw.main_loop_new)(std::ptr::null());
            if main_loop.is_null() {
                return Err(ScreenShareError::new("pw_main_loop_new failed"));
            }
            let loop_ = (pw.main_loop_get_loop)(main_loop);
            let context = (pw.context_new)(loop_, std::ptr::null(), 0);
            if context.is_null() {
                (pw.main_loop_destroy)(main_loop);
                return Err(ScreenShareError::new("pw_context_new failed"));
            }
            let core = (pw.context_connect)(context, std::ptr::null_mut(), 0);
            if core.is_null() {
                (pw.context_destroy)(context);
                (pw.main_loop_destroy)(main_loop);
                return Err(ScreenShareError::new(
                    "pw_context_connect failed (is PipeWire running?)",
                ));
            }

            let props = make_stream_properties(&pw)?;
            let params = build_format_params();

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
                return Err(ScreenShareError::new("pw_stream_new_simple failed"));
            }
            (*ctx).stream = stream;

            // Advertise the formats we can consume: BGRx (preferred), BGRA,
            // RGBA. The portal converts its native format to one of these.
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
                return Err(ScreenShareError::new(format!(
                    "pw_stream_connect failed: {result}"
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

unsafe extern "C" fn stream_param_changed(
    data: *mut c_void,
    id: u32,
    param: *const c_void,
) {
    // SPA_PARAM_Format (4) carries the negotiated geometry/format.
    if id != 4 || param.is_null() {
        return;
    }
    let user = data as *mut StreamUserData;
    let Some((width, height, pixel_format)) = parse_format_pod(param) else {
        return;
    };
    let mut fmt = (*user).format.lock().unwrap();
    if fmt.width != width || fmt.height != height || fmt.pixel_format != pixel_format {
        *fmt = NegotiatedFormat {
            width,
            height,
            pixel_format,
        };
        let _ = (*user)
            .event_tx
            .try_send(PortalEvent::FormatChanged { width, height });
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
    if !spa.is_null() && (*spa).n_datas > 0 {
        let dat = (*spa).datas;
        if !dat.is_null() && !(*dat).data.is_null() {
            let chunk = (*dat).chunk;
            let offset = if chunk.is_null() { 0 } else { (*chunk).offset as usize };
            let size = if chunk.is_null() || (*chunk).size == 0 {
                (*dat).maxsize as usize
            } else {
                (*chunk).size as usize
            };
            let src = std::slice::from_raw_parts((*dat).data as *const u8, size);
            let payload = src[offset.min(size)..].to_vec();
            let fmt = *(*user).format.lock().unwrap();
            if fmt.width > 0 && fmt.height > 0 {
                let expected = fmt.width as usize * fmt.height as usize * 4;
                if payload.len() >= expected {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_micros() as u64;
                    let frame = CapturedFrame {
                        timestamp_us: now,
                        width: fmt.width,
                        height: fmt.height,
                        pixel_format: fmt.pixel_format,
                        stride: fmt.width * 4,
                        pixels: payload[..expected].to_vec(),
                        gpu_handle: None,
                        dirty_region: None,
                    };
                    let _ = (*user).frame_tx.try_send(frame);
                }
            }
        }
    }
    (pw.stream_queue_buffer)((*ctx).stream, buffer);
}

const PW_DIRECTION_INPUT: u32 = 0;

const PW_STREAM_FLAG_AUTOCONNECT: u32 = 1 << 0;
const PW_STREAM_FLAG_MAP_BUFFERS: u32 = 1 << 2;

// SPA pod/type constants (spa/include/spa/param/param-types.h,
// spa/include/spa/param/format.h, spa/include/spa/param/video/raw.h).
const SPA_TYPE_Object: u32 = 16;
const SPA_TYPE_Id: u32 = 3;
const SPA_TYPE_Choice: u32 = 20;
const SPA_TYPE_Rectangle: u32 = 10;
const SPA_POD_OBJECT_TYPE_Format: u32 = 4;
const SPA_FORMAT_mediaType: u32 = 0x10001;
const SPA_FORMAT_mediaSubtype: u32 = 0x10002;
const SPA_FORMAT_VIDEO_format: u32 = 0x20001;
const SPA_FORMAT_VIDEO_size: u32 = 0x20003;
const SPA_MEDIA_TYPE_VIDEO: u32 = 1;
const SPA_MEDIA_SUBTYPE_RAW: u32 = 1;
const SPA_VIDEO_FORMAT_BGRx: u32 = 7;
const SPA_VIDEO_FORMAT_RGBA: u32 = 10;
const SPA_VIDEO_FORMAT_BGRA: u32 = 11;

/// Map a negotiated SPA video format id to the CPU pixel format we encode.
fn spa_format_to_pixel_format(format_id: u32) -> Option<PixelFormat> {
    match format_id {
        SPA_VIDEO_FORMAT_BGRx | SPA_VIDEO_FORMAT_BGRA => Some(PixelFormat::Bgra8),
        SPA_VIDEO_FORMAT_RGBA => Some(PixelFormat::Rgba8),
        _ => None,
    }
}

/// Build the SPA format object pod advertising BGRx/BGRA/RGBA.
///
/// Layout (all little-endian, 8-byte aligned):
///   pod header { u32 body_size, u32 type = Object }
///   object body { u32 type = ParamFormat, u32 id = Format }
///   prop { u32 key, u32 flags, pod value }
fn build_format_params() -> Vec<u8> {
    let mut pod: Vec<u8> = Vec::new();
    // Placeholder header: size patched once the body is known.
    pod.extend_from_slice(&[0, 0, 0, 0]);
    pod.extend_from_slice(&SPA_TYPE_Object.to_le_bytes());
    pod.extend_from_slice(&SPA_POD_OBJECT_TYPE_Format.to_le_bytes());
    pod.extend_from_slice(&4u32.to_le_bytes()); // id = SPA_PARAM_Format
    push_prop_id(&mut pod, SPA_FORMAT_mediaType, SPA_MEDIA_TYPE_VIDEO);
    push_prop_id(&mut pod, SPA_FORMAT_mediaSubtype, SPA_MEDIA_SUBTYPE_RAW);
    push_prop_choice_id(
        &mut pod,
        SPA_FORMAT_VIDEO_format,
        &[SPA_VIDEO_FORMAT_BGRx, SPA_VIDEO_FORMAT_BGRA, SPA_VIDEO_FORMAT_RGBA],
    );
    push_prop_rectangle(&mut pod, SPA_FORMAT_VIDEO_size, 640, 360);
    let body_size = pod.len() as u32 - 8;
    pod[0..4].copy_from_slice(&body_size.to_le_bytes());
    pod
}

fn push_prop_id(pod: &mut Vec<u8>, key: u32, value: u32) {
    pod.extend_from_slice(&key.to_le_bytes());
    pod.extend_from_slice(&0u32.to_le_bytes()); // flags
    pod.extend_from_slice(&4u32.to_le_bytes()); // value pod body size
    pod.extend_from_slice(&SPA_TYPE_Id.to_le_bytes());
    pod.extend_from_slice(&value.to_le_bytes());
    pod.extend_from_slice(&0u32.to_le_bytes()); // value padding
}

fn push_prop_rectangle(pod: &mut Vec<u8>, key: u32, width: u32, height: u32) {
    pod.extend_from_slice(&key.to_le_bytes());
    pod.extend_from_slice(&0u32.to_le_bytes()); // flags
    pod.extend_from_slice(&8u32.to_le_bytes()); // value pod body size
    pod.extend_from_slice(&SPA_TYPE_Rectangle.to_le_bytes());
    pod.extend_from_slice(&width.to_le_bytes());
    pod.extend_from_slice(&height.to_le_bytes());
}

fn push_prop_choice_id(pod: &mut Vec<u8>, key: u32, values: &[u32]) {
    let n = values.len();
    // Choice body: kind + flags + child Id pod + alternative values.
    let value_body = 16 + 4 * n;
    pod.extend_from_slice(&key.to_le_bytes());
    pod.extend_from_slice(&0u32.to_le_bytes()); // flags
    pod.extend_from_slice(&(value_body as u32).to_le_bytes());
    pod.extend_from_slice(&SPA_TYPE_Choice.to_le_bytes());
    pod.extend_from_slice(&0u32.to_le_bytes()); // choice type: Enum
    pod.extend_from_slice(&0u32.to_le_bytes()); // choice flags
    pod.extend_from_slice(&4u32.to_le_bytes()); // child pod size
    pod.extend_from_slice(&SPA_TYPE_Id.to_le_bytes());
    pod.extend_from_slice(&values[0].to_le_bytes()); // default = first format
    for v in &values[1..] {
        pod.extend_from_slice(&v.to_le_bytes());
    }
    // The value pod is 8-byte aligned before the next property.
    while pod.len() % 8 != 0 {
        pod.push(0);
    }
}

/// Parse a SPA format object pod into (width, height, pixel_format).
fn parse_format_pod(pod: *const c_void) -> Option<(u32, u32, PixelFormat)> {
    if pod.is_null() {
        return None;
    }
    // SAFETY: the pod is owned by PipeWire and stays valid for the callback.
    let head = unsafe { std::slice::from_raw_parts(pod as *const u8, 8) };
    if head.len() < 8 || u32::from_le_bytes(head[4..8].try_into().ok()?) != SPA_TYPE_Object {
        return None;
    }
    let total = u32::from_le_bytes(head[0..4].try_into().ok()?) as usize;
    // Clamp reads to the declared pod body so a short pod cannot overrun.
    let body = unsafe { std::slice::from_raw_parts(pod.add(8) as *const u8, total) };
    if body.len() < 8 {
        return None;
    }
    // body[0..4] = object type (ParamFormat), body[4..8] = id; props follow.
    let mut offset = 8usize;
    let mut format_id: Option<u32> = None;
    let mut size: Option<(u32, u32)> = None;
    while offset + 16 <= body.len() {
        let key = u32::from_le_bytes(body[offset..offset + 4].try_into().ok()?);
        let value_body_size =
            u32::from_le_bytes(body[offset + 8..offset + 12].try_into().ok()?) as usize;
        let value_type = u32::from_le_bytes(body[offset + 12..offset + 16].try_into().ok()?);
        // Value pod header: body size at offset+16, type at offset+20, data at
        // offset+20; value data starts at offset+16.
        let value_data = &body[offset + 16..];
        match (key, value_type) {
            (SPA_FORMAT_VIDEO_format, SPA_TYPE_Choice) => {
                // choice body: type(4) flags(4) child pod(size+type) value...
                // The chosen value is the child pod value (offset 16 within the
                // choice value) when the child is an Id.
                if value_body_size >= 20 && value_data.len() >= 20 {
                    let child_type = u32::from_le_bytes(value_data[12..16].try_into().ok()?);
                    if child_type == SPA_TYPE_Id {
                        format_id = Some(u32::from_le_bytes(value_data[16..20].try_into().ok()?));
                    }
                }
            }
            (SPA_FORMAT_VIDEO_format, SPA_TYPE_Id) => {
                if value_data.len() >= 4 {
                    format_id = Some(u32::from_le_bytes(value_data[0..4].try_into().ok()?));
                }
            }
            (SPA_FORMAT_VIDEO_size, SPA_TYPE_Rectangle) => {
                if value_data.len() >= 8 {
                    let w = u32::from_le_bytes(value_data[0..4].try_into().ok()?);
                    let h = u32::from_le_bytes(value_data[4..8].try_into().ok()?);
                    size = Some((w, h));
                }
            }
            _ => {}
        }
        let value_pod_size = (8 + value_body_size + 7) & !7;
        offset += 8 + value_pod_size;
    }
    let format_id = format_id?;
    let (width, height) = size?;
    let pixel_format = spa_format_to_pixel_format(format_id)?;
    Some((width, height, pixel_format))
}

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

/// Direct X11 capture: grabs the root window via `GetImage` and converts the
/// ZPixmap buffer to RGBA8. This is the no-portal fallback — it makes real
/// desktop sharing work on any X11 display without xdg-desktop-portal or
/// PipeWire. Pixels are interpreted through the root visual's channel masks,
/// so both LSBFirst (BGRX, typical x86) and MSBFirst (XRGB) servers convert
/// correctly. An XShm fast path can replace the per-frame GetImage copy later
/// without changing this interface.
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
        })
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
        self.width = width;
        self.height = height;
        let reply = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.root, 0, 0, width as u16, height as u16, u32::MAX)
            .map_err(|e| ScreenShareError::new(format!("X11 GetImage failed: {e}")))?
            .reply()
            .map_err(|e| ScreenShareError::new(format!("X11 GetImage reply failed: {e}")))?;
        let pixels = convert_zpixmap_rgba(
            &reply.data,
            width as usize,
            height as usize,
            self.depth,
            self.lsb_first,
            self.red_mask,
            self.green_mask,
            self.blue_mask,
        )?;
        let timestamp_us = self.timestamp_us;
        self.timestamp_us = self.timestamp_us.saturating_add(33_333);
        CapturedFrame::cpu(timestamp_us, width, height, PixelFormat::Rgba8, pixels).map(Some)
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
            ActiveCapture::X11(capture) => capture.capture(),
            ActiveCapture::TestPattern(capture, _) => capture.capture(),
        }
    }

    /// Active capture geometry for codec configuration.
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            ActiveCapture::Portal(capture) => {
                capture.negotiated_size().unwrap_or((DEMO_WIDTH, DEMO_HEIGHT))
            }
            ActiveCapture::X11(capture) => (capture.width, capture.height),
            ActiveCapture::TestPattern(_, size) => *size,
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
pub async fn create_capture_source(force_fallback: bool) -> ActiveCapture {
    #[cfg(target_os = "linux")]
    {
        if !force_fallback {
            if let Ok(capture) = LinuxPortalCapture::connect().await {
                return ActiveCapture::Portal(capture);
            }
            if let Ok(capture) = X11Capture::connect() {
                return ActiveCapture::X11(capture);
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

    #[test]
    fn cancellation_ends_selection() {
        let mut c = PortalCapture::new(2).unwrap();
        c.begin_selection().unwrap();
        c.cancel();
        assert_eq!(c.state(), PortalState::Ended);
    }

    #[test]
    fn format_pod_builder_produces_parsable_object() {
        let pod = build_format_params();
        assert!(pod.len() > 32);
        // Header: body size + Object type.
        assert_eq!(u32::from_le_bytes(pod[4..8].try_into().unwrap()), SPA_TYPE_Object);
        let parsed = parse_format_pod(pod.as_ptr() as *const c_void);
        // The pod advertises BGRx first; parse returns the first value.
        let (width, height, pixel_format) = parsed.expect("pod must parse");
        assert_eq!((width, height), (640, 360));
        assert_eq!(pixel_format, PixelFormat::Bgra8);
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
        assert!(parse_format_pod(std::ptr::null()).is_none());
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
}
