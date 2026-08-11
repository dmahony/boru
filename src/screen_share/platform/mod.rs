//! Platform-specific screen-sharing backend modules.

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
