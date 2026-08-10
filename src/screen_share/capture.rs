//! Shared capture-frame representation and bounded capture-to-encoder sink.

use std::collections::VecDeque;

use super::ScreenShareError;

/// Pixel layout of a normalized captured frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 8-bit BGRA, four bytes per pixel.
    Bgra8,
    /// 8-bit RGBA, four bytes per pixel.
    Rgba8,
    /// A platform/GPU surface identified by an opaque handle.
    Gpu,
}

/// One owned frame shared by every capture backend and the encoder boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedFrame {
    /// Monotonic presentation timestamp in microseconds.
    pub timestamp_us: u64,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Normalized pixel format.
    pub pixel_format: PixelFormat,
    /// CPU payload. Empty for GPU-backed frames.
    pub pixels: Vec<u8>,
    /// Optional opaque native/GPU handle. The owner must release it after use.
    pub gpu_handle: Option<u64>,
}

impl CapturedFrame {
    /// Construct an owned CPU frame, validating its dimensions and payload size.
    pub fn cpu(
        timestamp_us: u64,
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
        pixels: Vec<u8>,
    ) -> Result<Self, ScreenShareError> {
        if !matches!(pixel_format, PixelFormat::Bgra8 | PixelFormat::Rgba8) {
            return Err(ScreenShareError::new("CPU frames require BGRA8 or RGBA8"));
        }
        let expected = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| ScreenShareError::new("frame dimensions overflow"))?
            as usize;
        if pixels.len() != expected {
            return Err(ScreenShareError::new(
                "CPU frame payload does not match dimensions",
            ));
        }
        Ok(Self {
            timestamp_us,
            width,
            height,
            pixel_format,
            pixels,
            gpu_handle: None,
        })
    }

    /// Construct a GPU-backed frame without copying its surface to the CPU.
    pub fn gpu(timestamp_us: u64, width: u32, height: u32, handle: u64) -> Self {
        Self {
            timestamp_us,
            width,
            height,
            pixel_format: PixelFormat::Gpu,
            pixels: Vec::new(),
            gpu_handle: Some(handle),
        }
    }
}

/// Bounded, single-owner queue between capture and encoding.
#[derive(Debug)]
pub struct FrameSink {
    frames: VecDeque<CapturedFrame>,
    capacity: usize,
    captured: u64,
    dropped: u64,
    encoded: u64,
}

impl FrameSink {
    /// Create a sink that retains at most `capacity` recent frames.
    pub fn new(capacity: usize) -> Result<Self, ScreenShareError> {
        if capacity == 0 {
            return Err(ScreenShareError::new(
                "frame sink capacity must be non-zero",
            ));
        }
        Ok(Self {
            frames: VecDeque::with_capacity(capacity),
            capacity,
            captured: 0,
            dropped: 0,
            encoded: 0,
        })
    }
    /// Push a frame, dropping the oldest frame when the queue is full.
    pub fn push(&mut self, frame: CapturedFrame) {
        self.captured += 1;
        if self.frames.len() == self.capacity {
            self.frames.pop_front();
            self.dropped += 1;
        }
        self.frames.push_back(frame);
    }
    /// Take the most recent queued frame, discarding older stale frames.
    pub fn pop_latest(&mut self) -> Option<CapturedFrame> {
        let frame = self.frames.pop_back()?;
        self.dropped += self.frames.len() as u64;
        self.frames.clear();
        self.encoded += 1;
        Some(frame)
    }
    /// Number of queued frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    /// Whether no frame is queued.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    /// Capture/encode/drop counters for diagnostics.
    pub fn counters(&self) -> (u64, u64, u64) {
        (self.captured, self.encoded, self.dropped)
    }
}

/// Produces screen frames after the caller has obtained platform permission.
pub trait ScreenCapture: Send {
    /// Capture the next frame, or `None` when the source has stopped.
    fn capture(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError>;
}

/// Synthetic moving test-pattern capture source.
///
/// This is the milestone-7 capture backend: it produces real RGBA frames on
/// any platform without a portal/PipeWire session, so the full
/// capture → encode → transport → decode → render chain can be exercised and
/// verified before real screen capture is wired (platform backends in
/// `platform/`). The pattern changes every frame so motion is visible.
pub struct TestPatternCapture {
    width: u32,
    height: u32,
    timestamp_us: u64,
    frame: u64,
}

impl TestPatternCapture {
    /// Create a source producing `width`×`height` RGBA frames at ~30 fps.
    pub fn new(width: u32, height: u32) -> Result<Self, ScreenShareError> {
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err(ScreenShareError::new(
                "test pattern dimensions must be non-zero even values",
            ));
        }
        Ok(Self { width, height, timestamp_us: 0, frame: 0 })
    }
}

impl ScreenCapture for TestPatternCapture {
    fn capture(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
        let width = self.width;
        let height = self.height;
        let timestamp_us = self.timestamp_us;
        let frame = self.frame;
        self.timestamp_us = self.timestamp_us.saturating_add(33_333);
        self.frame = self.frame.saturating_add(1);
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for y in 0..height {
            for x in 0..width {
                let dx = ((x + frame as u32) % width) as u8;
                let dy = ((y + frame as u32) % height) as u8;
                pixels.extend_from_slice(&[
                    dx.wrapping_mul(3),
                    dy.wrapping_mul(3),
                    (dx ^ dy).wrapping_add(frame as u8),
                    255,
                ]);
            }
        }
        CapturedFrame::cpu(timestamp_us, width, height, PixelFormat::Rgba8, pixels)
            .map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(timestamp_us: u64) -> CapturedFrame {
        CapturedFrame::cpu(timestamp_us, 1, 1, PixelFormat::Bgra8, vec![0; 4]).unwrap()
    }

    #[test]
    fn sink_is_bounded_and_prefers_latest_frame() {
        let mut sink = FrameSink::new(2).unwrap();
        sink.push(frame(1));
        sink.push(frame(2));
        sink.push(frame(3));
        assert_eq!(sink.len(), 2);
        assert_eq!(sink.pop_latest().unwrap().timestamp_us, 3);
        assert_eq!(sink.counters(), (3, 1, 2));
    }

    #[test]
    fn invalid_cpu_payload_is_rejected() {
        assert!(CapturedFrame::cpu(0, 2, 2, PixelFormat::Bgra8, vec![0; 4]).is_err());
    }

    #[test]
    fn test_pattern_capture_produces_moving_rgba_frames() {
        let mut capture = TestPatternCapture::new(4, 4).unwrap();
        assert!(TestPatternCapture::new(3, 3).is_err());
        let first = capture.capture().unwrap().unwrap();
        let second = capture.capture().unwrap().unwrap();
        assert_eq!(first.pixel_format, PixelFormat::Rgba8);
        assert_eq!(first.pixels.len(), 4 * 4 * 4);
        assert_eq!(second.width, 4);
        assert_eq!(second.height, 4);
        assert_ne!(first.pixels, second.pixels, "pattern must move between frames");
        assert!(second.timestamp_us > first.timestamp_us);
    }
}
