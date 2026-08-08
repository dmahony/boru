//! Capture-side boundary for live-call video.
//!
//! Camera integration is intentionally deferred to the capture task. This
//! module must not depend on attachment paths, HTTP streaming, or Iced widgets.

use std::time::Duration;

/// Requested dimensions and cadence for a live camera track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureConfig {
    /// Requested frame width in pixels.
    pub width: u32,
    /// Requested frame height in pixels.
    pub height: u32,
    /// Requested capture interval.
    pub frame_interval: Duration,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            width: 640,
            height: 480,
            frame_interval: Duration::from_millis(33),
        }
    }
}

/// One raw frame leaving the live capture boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedFrame {
    /// Monotonic capture timestamp in microseconds.
    pub timestamp_us: u64,
    /// Raw video bytes owned by the live pipeline.
    pub data: Vec<u8>,
}

/// Capture source abstraction reserved for the camera implementation task.
pub trait CaptureSource: Send {
    /// Return the next captured frame, or `None` when the source is stopped.
    fn next_frame(&mut self) -> Option<CapturedFrame>;
}
