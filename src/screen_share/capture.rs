//! Shared capture-frame representation, the platform-neutral capture backend
//! abstraction, and a bounded capture-to-encoder sink.

use std::collections::VecDeque;

use super::coords::MonitorGeometry;
use super::codec::QualityProfile;
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

impl PixelFormat {
    /// Bytes per pixel for CPU-addressable formats; `None` for GPU surfaces.
    pub fn bytes_per_pixel(self) -> Option<u32> {
        match self {
            PixelFormat::Bgra8 | PixelFormat::Rgba8 => Some(4),
            PixelFormat::Gpu => None,
        }
    }
}

/// Axis-aligned rectangle in frame pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRect {
    /// Left edge, in pixels.
    pub x: u32,
    /// Top edge, in pixels.
    pub y: u32,
    /// Rectangle width, in pixels.
    pub width: u32,
    /// Rectangle height, in pixels.
    pub height: u32,
}

/// Which parts of a frame changed since the previous frame.
///
/// Damage-aware backends (X11 damage tracking, portal cursor overlays, etc.)
/// attach this so downstream stages can skip re-encoding unchanged regions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirtyRegion {
    /// The entire frame is dirty (e.g. the first frame or unknown).
    Full,
    /// Only the listed rectangles changed.
    Rects(Vec<FrameRect>),
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
    /// Row stride in bytes (`>= width * bytes_per_pixel`). Tightly packed CPU
    /// frames use `width * bytes_per_pixel`; GPU frames report `0` because the
    /// row pitch lives on the native surface.
    pub stride: u32,
    /// CPU payload. Empty for GPU-backed frames.
    pub pixels: Vec<u8>,
    /// Optional opaque native/GPU handle. The owner must release it after use.
    pub gpu_handle: Option<u64>,
    /// Optional dirty-region metadata. `None` means the backend did not
    /// provide damage information (the whole frame should be treated as new).
    pub dirty_region: Option<DirtyRegion>,
}

impl CapturedFrame {
    /// Construct an owned, tightly packed CPU frame, validating its dimensions
    /// and payload size. The row stride is derived from the pixel format.
    pub fn cpu(
        timestamp_us: u64,
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
        pixels: Vec<u8>,
    ) -> Result<Self, ScreenShareError> {
        let bpp = pixel_format
            .bytes_per_pixel()
            .ok_or_else(|| ScreenShareError::new("CPU frames require BGRA8 or RGBA8"))?;
        let stride = width
            .checked_mul(bpp)
            .ok_or_else(|| ScreenShareError::new("frame dimensions overflow"))?;
        Self::cpu_with_stride(timestamp_us, width, height, pixel_format, stride, pixels)
    }

    /// Construct an owned CPU frame with an explicit row stride (bytes per
    /// row). This supports sources whose rows carry padding, e.g. capture APIs
    /// that round row pitch up to a hardware alignment.
    pub fn cpu_with_stride(
        timestamp_us: u64,
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
        stride: u32,
        pixels: Vec<u8>,
    ) -> Result<Self, ScreenShareError> {
        let bpp = pixel_format
            .bytes_per_pixel()
            .ok_or_else(|| ScreenShareError::new("CPU frames require BGRA8 or RGBA8"))?;
        let min_stride = width
            .checked_mul(bpp)
            .ok_or_else(|| ScreenShareError::new("frame dimensions overflow"))?;
        if stride < min_stride {
            return Err(ScreenShareError::new(
                "frame stride is smaller than width * bytes-per-pixel",
            ));
        }
        let expected = (stride as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| ScreenShareError::new("frame dimensions overflow"))?;
        if pixels.len() != expected {
            return Err(ScreenShareError::new(
                "CPU frame payload does not match stride * height",
            ));
        }
        Ok(Self {
            timestamp_us,
            width,
            height,
            pixel_format,
            stride,
            pixels,
            gpu_handle: None,
            dirty_region: None,
        })
    }

    /// Construct a GPU-backed frame without copying its surface to the CPU.
    pub fn gpu(timestamp_us: u64, width: u32, height: u32, handle: u64) -> Self {
        Self {
            timestamp_us,
            width,
            height,
            pixel_format: PixelFormat::Gpu,
            stride: 0,
            pixels: Vec::new(),
            gpu_handle: Some(handle),
            dirty_region: None,
        }
    }

    /// Attach dirty-region metadata to the frame.
    pub fn with_dirty_region(mut self, region: DirtyRegion) -> Self {
        self.dirty_region = Some(region);
        self
    }
}

/// Platform-independent identifier for a capture source.
///
/// OS-specific handles (HMONITOR, Wayland output names, X11 RandR ids, ...)
/// never cross the public interface: platform backends enumerate sources and
/// keep a private mapping from this id to their native handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CaptureSourceId(pub u64);

/// What kind of display surface a source captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureSourceKind {
    /// A physical display / monitor.
    Monitor,
    /// A single application window.
    Window,
    /// The whole virtual desktop.
    Desktop,
}

/// A capturable source advertised by a backend via [`DesktopCaptureBackend::list_sources`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureSource {
    /// Stable id used to select this source with [`DesktopCaptureBackend::start`].
    pub id: CaptureSourceId,
    /// What kind of surface this source captures.
    pub kind: CaptureSourceKind,
    /// Human-readable name for UI (e.g. `DP-1: 1920x1080` or a window title).
    pub title: String,
    /// Native pixel width of the source.
    pub width: u32,
    /// Native pixel height of the source.
    pub height: u32,
    /// Virtual-desktop geometry of the source (physical pixels), when the
    /// backend can describe where the source sits in the desktop layout.
    ///
    /// This is what lets the host normalize coordinates against the shared
    /// source rather than the global desktop: a cursor at desktop
    /// `(-960, 540)` on a monitor whose origin is `(-1920, 0)` is at
    /// source-relative `(960, 540)`. Backends that cannot describe an origin
    /// (e.g. the synthetic test pattern) leave this `None`, in which case the
    /// source is treated as a primary-at-origin desktop.
    pub geometry: Option<MonitorGeometry>,
}

/// Configuration for a capture session.
///
/// Carries both capture-side settings (`target_fps`,
/// `preferred_pixel_format`) and the encode knobs the host forwards into the
/// encoder config (PDF Task 7.1): bitrate, keyframe interval and quality
/// profile. Backends only consume the capture fields; the encode fields ride
/// the same config so the whole pipeline is configured from one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureConfig {
    /// Target capture rate in frames per second.
    pub target_fps: u32,
    /// Preferred normalized pixel format. Backends may substitute an
    /// equivalent format the platform provides; the produced frames always
    /// carry their actual [`PixelFormat`].
    pub preferred_pixel_format: PixelFormat,
    /// Target encode bitrate in bits per second (forwarded to the codec).
    pub target_bitrate_bps: u32,
    /// Maximum distance between keyframes, in frames (forwarded to the codec).
    pub keyframe_interval: u64,
    /// Quality/latency profile applied to the encoder (PDF Task 7.1).
    pub quality_profile: QualityProfile,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            target_fps: 15,
            preferred_pixel_format: PixelFormat::Bgra8,
            target_bitrate_bps: super::codec::DEFAULT_BITRATE_BPS,
            keyframe_interval: super::codec::DEFAULT_KEYFRAME_INTERVAL,
            quality_profile: QualityProfile::Balanced,
        }
    }
}

/// Platform-neutral screen capture backend.
///
/// This is the boundary every platform backend (Windows WinRT Graphics
/// Capture, Wayland portal/PipeWire, X11, ...) implements. OS-specific
/// handles stay inside the platform modules: callers interact only with
/// [`CaptureSourceId`]s, [`CaptureConfig`], and [`CapturedFrame`]s.
///
/// Lifecycle contract:
/// - `start` is only valid from the stopped state; starting twice is an error.
/// - `next_frame` is only valid while capturing; calling it before `start`
///   (or after `stop`) is an error.
/// - `stop` is deliberately idempotent: stopping an already-stopped backend
///   is a no-op, so teardown paths never have to track state twice.
pub trait DesktopCaptureBackend: Send {
    /// Enumerate the sources currently available to this backend.
    fn list_sources(&self) -> Result<Vec<CaptureSource>, ScreenShareError>;
    /// Begin capturing `source` with `config`.
    ///
    /// Errors if the backend is already capturing or if `source` is unknown.
    fn start(
        &mut self,
        source: CaptureSourceId,
        config: CaptureConfig,
    ) -> Result<(), ScreenShareError>;
    /// Return the next captured frame, or `None` when no frame is ready yet.
    ///
    /// Errors if called before `start` or after `stop`.
    fn next_frame(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError>;
    /// Stop capturing. Safe to call when not started (no-op).
    fn stop(&mut self);
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
///
/// It also implements [`DesktopCaptureBackend`] as the reference backend for
/// the platform-neutral lifecycle: one synthetic `Desktop` source, strict
/// start/next/stop state enforcement, and unit-testable invalid-call handling.
pub struct TestPatternCapture {
    width: u32,
    height: u32,
    timestamp_us: u64,
    frame: u64,
    started: bool,
    config: Option<CaptureConfig>,
}

impl TestPatternCapture {
    /// Create a source producing `width`×`height` RGBA frames at ~30 fps.
    pub fn new(width: u32, height: u32) -> Result<Self, ScreenShareError> {
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err(ScreenShareError::new(
                "test pattern dimensions must be non-zero even values",
            ));
        }
        Ok(Self {
            width,
            height,
            timestamp_us: 0,
            frame: 0,
            started: false,
            config: None,
        })
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
        CapturedFrame::cpu(timestamp_us, width, height, PixelFormat::Rgba8, pixels).map(Some)
    }
}

impl DesktopCaptureBackend for TestPatternCapture {
    fn list_sources(&self) -> Result<Vec<CaptureSource>, ScreenShareError> {
        Ok(vec![CaptureSource {
            id: CaptureSourceId(0),
            kind: CaptureSourceKind::Desktop,
            title: "Test pattern (synthetic)".to_string(),
            width: self.width,
            height: self.height,
            geometry: None,
        }])
    }

    fn start(
        &mut self,
        source: CaptureSourceId,
        config: CaptureConfig,
    ) -> Result<(), ScreenShareError> {
        if self.started {
            return Err(ScreenShareError::new("capture already started"));
        }
        if source != CaptureSourceId(0) {
            return Err(ScreenShareError::new("unknown capture source"));
        }
        if config.target_fps == 0 {
            return Err(ScreenShareError::new("target fps must be non-zero"));
        }
        self.started = true;
        self.config = Some(config);
        Ok(())
    }

    fn next_frame(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
        if !self.started {
            return Err(ScreenShareError::new(
                "capture is not started; call start() before next_frame()",
            ));
        }
        // The synthetic pattern path is shared with the ScreenCapture impl,
        // which itself is deliberately ungated for platform fallback use.
        <Self as ScreenCapture>::capture(self)
    }

    fn stop(&mut self) {
        self.started = false;
        self.config = None;
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
        assert_ne!(
            first.pixels, second.pixels,
            "pattern must move between frames"
        );
        assert!(second.timestamp_us > first.timestamp_us);
    }

    #[test]
    fn cpu_frames_derive_tight_stride_and_no_dirty_region() {
        let f = frame(1);
        assert_eq!(f.stride, 4, "1px * 4 bytes-per-pixel");
        assert_eq!(f.dirty_region, None);
        let gpu = CapturedFrame::gpu(1, 640, 360, 7);
        assert_eq!(gpu.stride, 0, "GPU frames report no CPU row pitch");
        assert!(gpu.pixels.is_empty());
        assert_eq!(gpu.gpu_handle, Some(7));
    }

    #[test]
    fn cpu_with_stride_accepts_padded_rows() {
        let f =
            CapturedFrame::cpu_with_stride(0, 2, 2, PixelFormat::Rgba8, 12, vec![0; 24]).unwrap();
        assert_eq!(f.stride, 12);
        assert_eq!(f.pixels.len(), 24);
    }

    #[test]
    fn cpu_with_stride_rejects_stride_smaller_than_row() {
        assert!(
            CapturedFrame::cpu_with_stride(0, 2, 2, PixelFormat::Rgba8, 4, vec![0; 8]).is_err(),
            "stride 4 < width 2 * 4 bpp = 8"
        );
    }

    #[test]
    fn dirty_region_metadata_is_carried_on_frame() {
        let region = DirtyRegion::Rects(vec![FrameRect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        }]);
        let f = frame(1).with_dirty_region(region.clone());
        assert_eq!(f.dirty_region, Some(region));
    }

    #[test]
    fn desktop_backend_lists_sources() {
        let backend = TestPatternCapture::new(4, 4).unwrap();
        let sources = backend.list_sources().unwrap();
        assert_eq!(sources.len(), 1);
        let source = &sources[0];
        assert_eq!(source.kind, CaptureSourceKind::Desktop);
        assert_eq!((source.width, source.height), (4, 4));
        assert!(!source.title.is_empty());
    }

    #[test]
    fn desktop_backend_start_next_stop_round_trip() {
        let mut backend = TestPatternCapture::new(4, 4).unwrap();
        backend
            .start(CaptureSourceId(0), CaptureConfig::default())
            .unwrap();
        let first = backend.next_frame().unwrap().expect("frame after start");
        assert_eq!((first.width, first.height), (4, 4));
        assert_eq!(first.pixel_format, PixelFormat::Rgba8);
        let second = backend.next_frame().unwrap().expect("second frame");
        assert_ne!(first.pixels, second.pixels, "pattern must keep moving");
        backend.stop();
        // After stop the backend is idle again: a fresh session is allowed.
        backend
            .start(CaptureSourceId(0), CaptureConfig::default())
            .unwrap();
        assert!(backend.next_frame().unwrap().is_some());
        backend.stop();
    }

    #[test]
    fn desktop_backend_rejects_start_twice() {
        let mut backend = TestPatternCapture::new(4, 4).unwrap();
        backend
            .start(CaptureSourceId(0), CaptureConfig::default())
            .unwrap();
        let err = backend
            .start(CaptureSourceId(0), CaptureConfig::default())
            .unwrap_err();
        assert!(err.to_string().contains("already started"));
        backend.stop();
    }

    #[test]
    fn desktop_backend_rejects_next_frame_before_start() {
        let mut backend = TestPatternCapture::new(4, 4).unwrap();
        let err = backend.next_frame().unwrap_err();
        assert!(err.to_string().contains("not started"));
    }

    #[test]
    fn desktop_backend_rejects_next_frame_after_stop() {
        let mut backend = TestPatternCapture::new(4, 4).unwrap();
        backend
            .start(CaptureSourceId(0), CaptureConfig::default())
            .unwrap();
        backend.stop();
        let err = backend.next_frame().unwrap_err();
        assert!(err.to_string().contains("not started"));
    }

    #[test]
    fn desktop_backend_stop_when_idle_is_noop() {
        let mut backend = TestPatternCapture::new(4, 4).unwrap();
        backend.stop();
        backend.stop();
        assert!(
            backend.next_frame().is_err(),
            "idle stops must not start capture"
        );
    }

    #[test]
    fn desktop_backend_rejects_unknown_source() {
        let mut backend = TestPatternCapture::new(4, 4).unwrap();
        let err = backend
            .start(CaptureSourceId(99), CaptureConfig::default())
            .unwrap_err();
        assert!(err.to_string().contains("unknown capture source"));
    }

    #[test]
    fn desktop_backend_rejects_zero_target_fps() {
        let mut backend = TestPatternCapture::new(4, 4).unwrap();
        let config = CaptureConfig {
            target_fps: 0,
            ..CaptureConfig::default()
        };
        let err = backend.start(CaptureSourceId(0), config).unwrap_err();
        assert!(err.to_string().contains("target fps"));
    }
}
