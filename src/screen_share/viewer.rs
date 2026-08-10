//! Viewer boundary reserved for rendering decoded screen frames.

/// A viewer consumes decoded frames in a later milestone.
pub trait ScreenViewer: Send {
    /// Present one decoded frame.
    fn present(&mut self, frame: &super::CapturedFrame);
}
