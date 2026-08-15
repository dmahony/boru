//! Platform-specific screen-sharing backend modules.
#![allow(missing_docs)]

#[cfg(target_os = "windows")]
use crate::screen_share::capture::ScreenCapture;

/// Platform-neutral logic shared by the Windows backend. Compiled on every
/// target (including Linux) so the capture state machine, HRESULT
/// classification, and monitor source-id derivation are unit-testable without
/// Windows hardware.
pub mod windows_common;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux::{
    capture_dimensions, choose_cursor_mode, classify_desktop_environment, classify_display_server,
    classify_session_type, clip_to_root, create_capture_source, detect_desktop_environment,
    detect_display_server, detect_session_type, select_sources_options, ActiveCapture,
    CaptureRect, CursorMode, DesktopEnvironment, DisplayServer, LinuxPortalCapture,
    MachineError, PortalCapture, PortalEvent, PortalSessionMachine, PortalState,
    SessionFailure, SessionPhase, SessionType, X11Capture, X11Monitor, X11_DESKTOP_SOURCE_ID,
    CAPTURE_FPS,
};

/// Pure PipeWire format negotiation + CPU frame normalization (BORU-SS-14).
/// Only the Linux backend uses it, but it is compiled on every target so the
/// negotiation/copy logic is unit-testable wherever tests run.
pub mod linux_pw;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
pub use windows::{GraphicsCapture, GraphicsCaptureEvent, GraphicsCaptureState};
#[cfg(target_os = "windows")]
pub use windows_common::CaptureFailureKind;

#[cfg(target_os = "windows")]
pub enum ActiveCapture {
    Graphics(GraphicsCapture, (u32, u32)),
    TestPattern(crate::screen_share::TestPatternCapture, (u32, u32)),
}

#[cfg(target_os = "macos")]
pub enum ActiveCapture {
    TestPattern(crate::screen_share::TestPatternCapture, (u32, u32)),
}

#[cfg(target_os = "windows")]
impl ActiveCapture {
    pub fn capture(
        &mut self,
    ) -> Result<Option<crate::screen_share::CapturedFrame>, crate::screen_share::ScreenShareError>
    {
        match self {
            Self::Graphics(capture, _) => capture.capture(),
            Self::TestPattern(capture, _) => capture.capture(),
        }
    }
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::Graphics(_, dimensions) | Self::TestPattern(_, dimensions) => *dimensions,
        }
    }
    /// Enumerate the capturable sources (monitors) for this backend
    /// (PDF Phase 10: enumerate monitors before starting a share).
    pub fn list_sources(
        &self,
    ) -> Result<Vec<crate::screen_share::CaptureSource>, crate::screen_share::ScreenShareError> {
        match self {
            Self::Graphics(capture, _) => {
                crate::screen_share::capture::DesktopCaptureBackend::list_sources(capture)
            }
            Self::TestPattern(capture, _) => {
                crate::screen_share::capture::DesktopCaptureBackend::list_sources(capture)
            }
        }
    }
    /// Begin capturing `source`; monitor-based backends select the monitor.
    pub fn start(
        &mut self,
        source: crate::screen_share::CaptureSourceId,
        config: &crate::screen_share::CaptureConfig,
    ) -> Result<(), crate::screen_share::ScreenShareError> {
        use crate::screen_share::capture::DesktopCaptureBackend;
        match self {
            Self::Graphics(capture, _) => DesktopCaptureBackend::start(capture, source, config.clone()),
            Self::TestPattern(capture, _) => DesktopCaptureBackend::start(capture, source, config.clone()),
        }
    }
    /// Switch the shared source without recreating the backend (PDF Phase
    /// 10); monitor-based backends re-select (stop + start).
    pub fn switch_source(
        &mut self,
        source: crate::screen_share::CaptureSourceId,
        config: &crate::screen_share::CaptureConfig,
    ) -> Result<(), crate::screen_share::ScreenShareError> {
        use crate::screen_share::capture::DesktopCaptureBackend;
        match self {
            Self::Graphics(capture, _) => {
                DesktopCaptureBackend::stop(capture);
                DesktopCaptureBackend::start(capture, source, config.clone())
            }
            Self::TestPattern(capture, _) => {
                if source != crate::screen_share::CaptureSourceId(0) {
                    return Err(crate::screen_share::ScreenShareError::new("unknown capture source"));
                }
                Ok(())
            }
        }
    }
    /// The source currently being captured, when the backend tracks one.
    pub fn current_source(&self) -> Option<crate::screen_share::CaptureSource> {
        use crate::screen_share::capture::DesktopCaptureBackend;
        match self {
            Self::Graphics(capture, _) => {
                let active = capture.active_source_id()?;
                DesktopCaptureBackend::list_sources(capture)
                    .ok()?
                    .into_iter()
                    .find(|source| source.id == active)
            }
            Self::TestPattern(capture, _) => DesktopCaptureBackend::list_sources(capture)
                .ok()
                .and_then(|mut sources| sources.pop()),
        }
    }
    /// Input backends on non-Linux platforms do not need a root-window
    /// origin (Windows SendInput uses virtual-screen coordinates; the portal
    /// uses relative motion), so the origin is always `(0, 0)` here.
    pub fn input_origin(&self) -> (i32, i32) {
        (0, 0)
    }
    pub fn is_test_pattern(&self) -> bool {
        matches!(self, Self::TestPattern(..))
    }
    pub fn backend_name(&self) -> &'static str {
        match self {
            Self::Graphics(..) => "windows-graphics-capture",
            Self::TestPattern(..) => "windows-test-pattern",
        }
    }
}

#[cfg(target_os = "macos")]
impl ActiveCapture {
    pub fn capture(
        &mut self,
    ) -> Result<Option<crate::screen_share::CapturedFrame>, crate::screen_share::ScreenShareError>
    {
        match self {
            Self::TestPattern(capture, _) => capture.capture(),
        }
    }
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::TestPattern(_, dimensions) => *dimensions,
        }
    }
    /// Enumerate the capturable sources for this backend (PDF Phase 10).
    pub fn list_sources(
        &self,
    ) -> Result<Vec<crate::screen_share::CaptureSource>, crate::screen_share::ScreenShareError> {
        match self {
            Self::TestPattern(capture, _) => {
                crate::screen_share::capture::DesktopCaptureBackend::list_sources(capture)
            }
        }
    }
    /// Begin capturing `source`; the synthetic backend accepts only its one
    /// source.
    pub fn start(
        &mut self,
        source: crate::screen_share::CaptureSourceId,
        config: &crate::screen_share::CaptureConfig,
    ) -> Result<(), crate::screen_share::ScreenShareError> {
        use crate::screen_share::capture::DesktopCaptureBackend;
        match self {
            Self::TestPattern(capture, _) => DesktopCaptureBackend::start(capture, source, config.clone()),
        }
    }
    /// Switch the shared source without recreating the backend (PDF Phase
    /// 10); the synthetic backend accepts only its single source.
    pub fn switch_source(
        &mut self,
        source: crate::screen_share::CaptureSourceId,
        _config: &crate::screen_share::CaptureConfig,
    ) -> Result<(), crate::screen_share::ScreenShareError> {
        if source != crate::screen_share::CaptureSourceId(0) {
            return Err(crate::screen_share::ScreenShareError::new("unknown capture source"));
        }
        Ok(())
    }
    /// The source currently being captured, when the backend tracks one.
    pub fn current_source(&self) -> Option<crate::screen_share::CaptureSource> {
        use crate::screen_share::capture::DesktopCaptureBackend;
        match self {
            Self::TestPattern(capture, _) => DesktopCaptureBackend::list_sources(capture)
                .ok()
                .and_then(|mut sources| sources.pop()),
        }
    }
    pub fn input_origin(&self) -> (i32, i32) {
        (0, 0)
    }
    pub fn is_test_pattern(&self) -> bool {
        matches!(self, Self::TestPattern(..))
    }
    pub fn backend_name(&self) -> &'static str {
        "macos-test-pattern"
    }
}

#[cfg(not(target_os = "linux"))]
pub async fn create_capture_source(force_fallback: bool) -> ActiveCapture {
    #[cfg(target_os = "windows")]
    if !force_fallback {
        if let Ok(capture) = GraphicsCapture::try_create(3) {
            let size = capture.dimensions();
            return ActiveCapture::Graphics(capture, size);
        }
    }
    ActiveCapture::TestPattern(
        crate::screen_share::TestPatternCapture::new(640, 360).unwrap(),
        (640, 360),
    )
}

#[cfg(not(target_os = "linux"))]
pub fn capture_dimensions(capture: &ActiveCapture) -> (u32, u32) {
    capture.dimensions()
}

#[cfg(not(target_os = "linux"))]
pub const CAPTURE_FPS: u32 = 15;
