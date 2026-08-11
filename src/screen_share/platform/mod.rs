//! Platform-specific screen-sharing backend modules.

#[cfg(not(target_os = "linux"))]
use crate::screen_share::capture::ScreenCapture;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux::{
    capture_dimensions, create_capture_source, ActiveCapture, CAPTURE_FPS, LinuxPortalCapture,
    PortalCapture, PortalEvent, PortalState,
};
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
pub use windows::{GraphicsCapture, GraphicsCaptureEvent, GraphicsCaptureState};

/// Capture source used by platforms without a native capture implementation.
///
/// Windows currently uses this minimum viable backend so the complete
/// screen-sharing codec/transport path can be exercised cross-platform. The
/// WinRT Graphics Capture adapter is kept in [`windows`] and will replace this
/// source once its asynchronous frame-pool integration is complete.
#[cfg(not(target_os = "linux"))]
pub enum ActiveCapture {
    /// Synthetic moving frames used as the Windows/macOS fallback.
    TestPattern(crate::screen_share::TestPatternCapture, (u32, u32)),
}

#[cfg(not(target_os = "linux"))]
impl ActiveCapture {
    /// Capture the next frame, if one is ready.
    pub fn capture(
        &mut self,
    ) -> Result<Option<crate::screen_share::CapturedFrame>, crate::screen_share::ScreenShareError>
    {
        match self {
            Self::TestPattern(capture, _) => capture.capture(),
        }
    }

    /// Active capture geometry for codec configuration.
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::TestPattern(_, dimensions) => *dimensions,
        }
    }

    /// Whether the synthetic fallback is active.
    pub fn is_test_pattern(&self) -> bool {
        matches!(self, Self::TestPattern(..))
    }

    /// Human-readable backend name for startup diagnostics.
    pub fn backend_name(&self) -> &'static str {
        "windows-test-pattern"
    }
}

/// Selection factory for the host-side capture source. On Linux this tries
/// the real portal/PipeWire backend first and falls back to the synthetic
/// test pattern when no portal is available; other platforms always use the
/// fallback until their backends are wired.
#[cfg(not(target_os = "linux"))]
pub async fn create_capture_source(_force_fallback: bool) -> ActiveCapture {
    ActiveCapture::TestPattern(
        crate::screen_share::TestPatternCapture::new(640, 360).unwrap(),
        (640, 360),
    )
}

/// Non-Linux capture source dimensions helper (constant fallback geometry).
#[cfg(not(target_os = "linux"))]
pub fn capture_dimensions(_capture: &ActiveCapture) -> (u32, u32) {
    (640, 360)
}

/// Frame rate for the fallback capture path on non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub const CAPTURE_FPS: u32 = 15;
