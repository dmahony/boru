//! Capture boundary for platform screen-frame producers.

use super::ScreenShareError;

/// One owned raw screen frame. Pixel format is implementation-defined for now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedFrame {
    /// Monotonic presentation timestamp in microseconds.
    pub timestamp_us: u64,
    /// Owned frame payload.
    pub pixels: Vec<u8>,
}

/// Produces screen frames after the caller has obtained platform permission.
pub trait ScreenCapture: Send {
    /// Capture the next frame, or `None` when the source has stopped.
    fn capture(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError>;
}
