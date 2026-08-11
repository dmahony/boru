//! Platform-specific screen-sharing backend modules.
#![allow(missing_docs)]

#[cfg(target_os = "windows")]
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
