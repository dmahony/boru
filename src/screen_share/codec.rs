//! Replaceable codec boundary and the initial low-latency H.264 implementation.
//!
//! The [`VideoEncoder`] trait is the PDF Task 2.2 encoder abstraction: the five
//! lifecycle operations — configure, encode, force_keyframe,
//! reconfigure_bitrate, shutdown — plus the codec-agnostic [`CodecConfig`] and
//! [`EncodedPacket`] types, so future hardware codecs (VA-API, NVENC, DXVA)
//! can implement the same boundary without depending on OpenH264.
//!
//! The concrete [`OpenH264Encoder`] (PDF Task 7.1 baseline) drives OpenH264
//! through its documented `EncoderConfig` knobs only: usage type, complexity,
//! QP range, bitrate, frame rate, rate-control mode and intra-frame period
//! (see the OpenH264 docs for `SCREEN_CONTENT_REAL_TIME`, complexity levels
//! and QP semantics). The [`QualityProfile`] enum maps the Boru-visible
//! quality knob onto those documented settings.
#![allow(missing_docs)]

use super::capture::{CaptureConfig, CapturedFrame, PixelFormat};
use super::ScreenShareError;

pub const DEFAULT_WIDTH: u32 = 1920;
pub const DEFAULT_HEIGHT: u32 = 1080;
pub const DEFAULT_FPS: u32 = 30;
pub const DEFAULT_BITRATE_BPS: u32 = 4_000_000;
pub const DEFAULT_KEYFRAME_INTERVAL: u64 = 60;
pub const DEFAULT_QUEUE_CAPACITY: usize = 2;

/// Monotonic microsecond clock for stage timestamps.
///
/// Capture backends that stamp with `SystemTime` (PipeWire, linux.rs) use the
/// same clock, so `encode_timestamp_us - timestamp_us` measures the
/// capture→encode stage latency end to end.
pub fn now_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

/// 720p target profile (PDF Task 7.1): 1280x720 @ 30 fps.
pub const TARGET_720P30_WIDTH: u32 = 1280;
pub const TARGET_720P30_HEIGHT: u32 = 720;
/// 720p30 default bitrate (2.5 Mbps is a sane LAN/relay balance for
/// screen content at 720p30).
pub const TARGET_720P30_BITRATE_BPS: u32 = 2_500_000;

/// 1080p target profile (PDF Task 7.1): 1920x1080 @ 30 fps.
pub const TARGET_1080P30_WIDTH: u32 = 1920;
pub const TARGET_1080P30_HEIGHT: u32 = 1080;
/// 1080p30 default bitrate (4 Mbps — the existing default).
pub const TARGET_1080P30_BITRATE_BPS: u32 = 4_000_000;

/// Quality/latency trade-off exposed through configuration (PDF Task 7.1).
///
/// Maps onto the documented OpenH264 encoder knobs — usage type
/// (`SCREEN_CONTENT_REAL_TIME` is the screen-sharing usage mode), complexity
/// (Low/Medium/High) and QP range (lower QP = higher quality, more CPU).
/// The wire representation is a small `u8` (`as_u8`/`from_u8`) so it can ride
/// on the versioned `StreamConfig` protocol message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QualityProfile {
    /// Default: screen-content real-time usage, medium complexity, default QP.
    #[default]
    Balanced,
    /// Fastest encode, lowest CPU: low complexity, slightly wider QP range.
    LowLatency,
    /// Crispest output: high complexity, tighter QP range (higher CPU).
    HighQuality,
}

impl QualityProfile {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Balanced => "balanced",
            Self::LowLatency => "low-latency",
            Self::HighQuality => "high-quality",
        }
    }
    /// Stable wire value: 0 = Balanced, 1 = LowLatency, 2 = HighQuality.
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Balanced => 0,
            Self::LowLatency => 1,
            Self::HighQuality => 2,
        }
    }
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Balanced),
            1 => Some(Self::LowLatency),
            2 => Some(Self::HighQuality),
            _ => None,
        }
    }
}

/// Encoder/decoder configuration negotiated for a screen stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecConfig {
    pub width: u32,
    pub height: u32,
    pub target_fps: u32,
    pub target_bitrate_bps: u32,
    pub keyframe_interval: u64,
    pub max_queue_depth: usize,
    /// Quality/latency profile applied to the encoder (PDF Task 7.1).
    pub quality_profile: QualityProfile,
}

impl Default for CodecConfig {
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            target_fps: DEFAULT_FPS,
            target_bitrate_bps: DEFAULT_BITRATE_BPS,
            keyframe_interval: DEFAULT_KEYFRAME_INTERVAL,
            max_queue_depth: DEFAULT_QUEUE_CAPACITY,
            quality_profile: QualityProfile::Balanced,
        }
    }
}

impl CodecConfig {
    pub(crate) fn validate(self) -> Result<Self, ScreenShareError> {
        if self.width == 0
            || self.height == 0
            || !self.width.is_multiple_of(2)
            || !self.height.is_multiple_of(2)
        {
            return Err(ScreenShareError::new(
                "codec dimensions must be non-zero even values",
            ));
        }
        if self.target_fps == 0
            || self.target_bitrate_bps == 0
            || self.keyframe_interval == 0
            || self.max_queue_depth == 0
        {
            return Err(ScreenShareError::new(
                "codec rates and queue depth must be non-zero",
            ));
        }
        Ok(self)
    }

    /// 720p @ 30 fps target profile (PDF Task 7.1).
    pub fn profile_720p30() -> Self {
        Self {
            width: TARGET_720P30_WIDTH,
            height: TARGET_720P30_HEIGHT,
            target_fps: 30,
            target_bitrate_bps: TARGET_720P30_BITRATE_BPS,
            ..Self::default()
        }
    }

    /// 1080p @ 30 fps target profile (PDF Task 7.1).
    pub fn profile_1080p30() -> Self {
        Self {
            width: TARGET_1080P30_WIDTH,
            height: TARGET_1080P30_HEIGHT,
            target_fps: 30,
            target_bitrate_bps: TARGET_1080P30_BITRATE_BPS,
            ..Self::default()
        }
    }

    /// Build the codec config from a capture-session config, so bitrate,
    /// frame rate, keyframe interval and quality profile all flow from the
    /// same CaptureConfig the capture backend was started with.
    pub fn from_capture_config(capture: &CaptureConfig, width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            target_fps: capture.target_fps,
            target_bitrate_bps: capture.target_bitrate_bps,
            keyframe_interval: capture.keyframe_interval,
            quality_profile: capture.quality_profile,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecMetadata {
    pub codec: CodecKind,
    pub config: CodecConfig,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecKind {
    /// Software H.264 via OpenH264 (the portable baseline; always available).
    H264,
    /// Hardware H.264 via Linux VA-API (libva, MIT-style). Falls back to
    /// [`Self::H264`] when libva or a usable GPU driver is missing.
    H264Vaapi,
    /// Hardware H.264 via Windows Media Foundation (IMFTransform). Typed
    /// unavailable on non-Windows builds; on Windows it is behind the same
    /// [`VideoEncoder`] boundary.
    H264Mf,
    /// AV1 via rav1e (BSD-2-Clause) encode / rav1d (BSD-2-Clause) decode.
    ///
    /// AV1 is royalty-free (AOMedia) and both libraries are permissively
    /// licensed, keeping Boru MIT/Apache-2.0-compatible. See
    /// docs/screenshare-feature-review.md §3.5 and the BORU-SS-35 commit for
    /// the H.265/HEVC licensing gate — H.265 is deliberately NOT added here
    /// (patent-encumbered: HEVC Advance / MPEG LA pools; a software encoder
    /// like x265 is GPL and must not be linked).
    Av1,
}

impl CodecKind {
    /// Wire codec name advertised in `Hello.codecs` / `StreamConfig.codec`.
    ///
    /// The viewer decodes the resulting stream with the same H.264 baseline
    /// decoder regardless of which encoder produced it, so the wire name is
    /// descriptive (for negotiation/statistics) rather than a decode gate.
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::H264Vaapi => "h264_vaapi",
            Self::H264Mf => "h264_mf",
            Self::Av1 => "av1",
        }
    }

    /// Reverse of [`Self::wire_name`]; unknown names map to `None`.
    pub fn from_wire_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "h264" => Some(Self::H264),
            "h264_vaapi" | "vaapi" => Some(Self::H264Vaapi),
            "h264_mf" | "mf" => Some(Self::H264Mf),
            "av1" => Some(Self::Av1),
            _ => None,
        }
    }

    /// Back-compat alias for [`Self::wire_name`] (BORU-SS-35 name).
    pub const fn name(self) -> &'static str {
        self.wire_name()
    }

    /// Back-compat alias for [`Self::from_wire_name`] (BORU-SS-35 name).
    pub fn from_name(name: &str) -> Option<Self> {
        Self::from_wire_name(name)
    }

    /// The software fallback for every kind (OpenH264 produces a baseline
    /// H.264 stream the viewer always decodes).
    pub const fn software(self) -> Self {
        Self::H264
    }

    /// Whether this kind is a hardware-accelerated encoder (as opposed to a
    /// software encoder like OpenH264 or rav1e).
    pub const fn is_hardware(self) -> bool {
        matches!(self, Self::H264Vaapi | Self::H264Mf)
    }
}

/// One encoded access unit (a keyframe or delta frame) passed to a transport.
///
/// Carries the capture timestamp, the encode-stage timestamp, sequence number,
/// keyframe flag, and the encoder generation/resolution it was produced with,
/// so downstream consumers (the decoder and the protocol layer) never need
/// codec-specific types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedPacket {
    /// Capture timestamp (from `CapturedFrame.timestamp_us`), same clock as
    /// `encode_timestamp_us`.
    pub timestamp_us: u64,
    /// Encode-stage timestamp, stamped when encoding completed
    /// (PDF Task 7.2: capture and encode stages timestamped so end-to-end
    /// latency can be measured).
    pub encode_timestamp_us: u64,
    pub sequence: u64,
    pub keyframe: bool,
    pub config_generation: u64,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

/// Back-compat alias for [`EncodedPacket`] (the pre-Task-2.2 name).
pub type EncodedFrame = EncodedPacket;

/// Codec-independent video encoder boundary (PDF Task 2.2).
///
/// The five operations are the complete lifecycle:
/// - [`configure`](Self::configure) (re)configures resolution/bitrate/fps
///   mid-session without a session restart where the codec permits;
/// - [`encode`](Self::encode) produces one encoded access unit/packet;
/// - [`force_keyframe`](Self::force_keyframe) makes the next unit an
///   independently decodable keyframe;
/// - [`reconfigure_bitrate`](Self::reconfigure_bitrate) changes only the
///   target bitrate, keeping resolution and frame rate;
/// - [`shutdown`](Self::shutdown) releases codec resources.
///
/// No method exposes an OpenH264 (or any vendor) type.
pub trait VideoEncoder: Send {
    /// (Re)configure the encoder for a new stream geometry/rate. Changing the
    /// resolution must not require restarting the surrounding session; codecs
    /// that cannot change resolution live rebuild internally and bump the
    /// config generation so the decoder re-creates.
    fn configure(&mut self, config: CodecConfig) -> Result<(), ScreenShareError>;
    /// Encode one captured frame into an access unit/packet.
    fn encode(&mut self, frame: &CapturedFrame) -> Result<EncodedPacket, ScreenShareError>;
    /// Force the next encoded packet to be an independently decodable keyframe.
    fn force_keyframe(&mut self);
    /// Change only the target bitrate, keeping resolution and frame rate.
    /// Returns an error when the codec cannot change bitrate mid-session.
    fn reconfigure_bitrate(&mut self, bitrate_bps: u32) -> Result<(), ScreenShareError>;
    /// Release codec resources. Idempotent; calls after shutdown error.
    fn shutdown(&mut self) -> Result<(), ScreenShareError> {
        Ok(())
    }

    /// Current codec metadata (codec kind, active config, generation).
    fn metadata(&self) -> CodecMetadata;

    /// Whether the next [`encode`](Self::encode) must produce an
    /// independently decodable keyframe (reconnect, source switch, viewer
    /// recovery request). The host uses this to decide whether a
    /// cursor-only metadata frame can be skipped (BORU-SS-33): when a
    /// keyframe is pending, the frame must be encoded even if the pixels
    /// are unchanged. Defaults to `false`; encoders that track pending
    /// keyframes override this.
    fn is_keyframe_pending(&self) -> bool {
        false
    }

    /// Back-compat alias for [`Self::force_keyframe`].
    fn request_keyframe(&mut self) {
        self.force_keyframe();
    }
    /// Back-compat alias for [`Self::configure`].
    fn reconfigure(&mut self, config: CodecConfig) -> Result<(), ScreenShareError> {
        self.configure(config)
    }
    /// Reset the encoder to its currently active config (forces a keyframe).
    fn reset(&mut self) -> Result<(), ScreenShareError> {
        self.reconfigure(self.metadata().config)
    }
}

/// After reset, non-keyframes are dropped until an independently decodable unit arrives.
pub trait VideoDecoder: Send {
    fn decode(&mut self, frame: &EncodedFrame) -> Result<Option<CapturedFrame>, ScreenShareError>;
    fn metadata(&self) -> CodecMetadata;
    fn reset(&mut self) -> Result<(), ScreenShareError>;
}

pub trait ScreenShareCodec: VideoEncoder + VideoDecoder {}
impl<T: VideoEncoder + VideoDecoder> ScreenShareCodec for T {}

fn fail(error: impl std::fmt::Display) -> ScreenShareError {
    ScreenShareError::new(error.to_string())
}

/// Create an encoder for `config`, preferring hardware acceleration when it
/// is available and falling back to the OpenH264 software encoder.
///
/// # Fallback orchestration (PDF Task 2.2 hardware path)
///
/// The codec names this host can actually encode with, ordered by preference
/// (hardware H.264 first, then the software H.264 baseline, then AV1). Used to
/// build `Hello.codecs` / `ScreenShareOffer.codecs` so negotiation advertises
/// the real encoders; the viewer still decodes the resulting baseline H.264
/// with its existing decoder.
pub fn available_encoder_codecs() -> Vec<String> {
    let mut codecs = Vec::new();
    #[cfg(target_os = "linux")]
    if crate::screen_share::vaapi::vaapi_encode_available() {
        codecs.push(CodecKind::H264Vaapi.wire_name().to_string());
    }
    codecs.push(CodecKind::H264.wire_name().to_string());
    codecs.push(CodecKind::Av1.wire_name().to_string());
    codecs
}

/// Create an encoder for the explicitly requested codec kind, falling back to
/// OpenH264 on any hardware-init failure (same orchestration as
/// [`create_encoder`]).
///
/// Hardware kinds that are NOT implemented on the current platform return a
/// typed [`ScreenShareErrorKind::HardwareAccelerationUnavailable`] error that
/// the caller maps to the software fallback — never a silent mis-encode. The
/// Windows Media Foundation path (`h264_mf`, IMFTransform) is documented but
/// not yet wired; requesting it yields a clear runtime error instead of a
/// fake "hardware" encode.
pub fn create_encoder_for(
    kind: CodecKind,
    config: CodecConfig,
) -> Result<Box<dyn VideoEncoder>, ScreenShareError> {
    match kind {
        CodecKind::H264 => Ok(Box::new(OpenH264Encoder::new(config)?)),
        #[cfg(target_os = "linux")]
        CodecKind::H264Vaapi => match crate::screen_share::vaapi::VaapiEncoder::new(config) {
            Ok(encoder) => {
                tracing::info!(
                    codec = CodecKind::H264Vaapi.wire_name(),
                    "screen-share: hardware encoder initialised (VA-API)"
                );
                Ok(Box::new(encoder))
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    kind = ?error.kind(),
                    codec = CodecKind::H264Vaapi.wire_name(),
                    "screen-share: VA-API hardware encoder unavailable; falling back to OpenH264"
                );
                Ok(Box::new(OpenH264Encoder::new(config)?))
            }
        },
        #[cfg(not(target_os = "linux"))]
        CodecKind::H264Vaapi => {
            tracing::warn!(
                codec = CodecKind::H264Vaapi.wire_name(),
                "screen-share: VA-API is Linux-only; falling back to OpenH264"
            );
            Ok(Box::new(OpenH264Encoder::new(config)?))
        }
        CodecKind::H264Mf => {
            // Media Foundation H.264 encoder (IMFTransform) — documented
            // upstream API (learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder).
            // Not yet wired: return a typed unavailable error so callers can
            // fall back instead of believing hardware acceleration happened.
            Err(ScreenShareError::hardware_acceleration_unavailable(
                "Windows Media Foundation H.264 encoder (h264_mf) is not wired in this build; use the OpenH264 fallback",
            ))
        }
        CodecKind::Av1 => Ok(Box::new(Av1Encoder::new(config)?)),
    }
}

fn rgba_to_rgb(frame: &CapturedFrame) -> Result<Vec<u8>, ScreenShareError> {
    if !matches!(frame.pixel_format, PixelFormat::Bgra8 | PixelFormat::Rgba8) {
        return Err(ScreenShareError::new(
            "H.264 requires a CPU BGRA8 or RGBA8 frame",
        ));
    }
    let expected = frame
        .width
        .checked_mul(frame.height)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| ScreenShareError::new("frame dimensions overflow"))?
        as usize;
    if frame.pixels.len() != expected {
        return Err(ScreenShareError::new(
            "frame payload does not match dimensions",
        ));
    }
    let mut rgb = Vec::with_capacity(expected / 4 * 3);
    for pixel in frame.pixels.chunks_exact(4) {
        if frame.pixel_format == PixelFormat::Bgra8 {
            rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        } else {
            rgb.extend_from_slice(&pixel[..3]);
        }
    }
    Ok(rgb)
}

fn scale_rgb(
    rgb: &[u8],
    source_width: u32,
    source_height: u32,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let mut result = vec![0; width as usize * height as usize * 3];
    for y in 0..height {
        for x in 0..width {
            let sx = x as usize * source_width as usize / width as usize;
            let sy = y as usize * source_height as usize / height as usize;
            let from = (sy * source_width as usize + sx) * 3;
            let to = (y as usize * width as usize + x as usize) * 3;
            result[to..to + 3].copy_from_slice(&rgb[from..from + 3]);
        }
    }
    result
}

#[allow(missing_debug_implementations)]
pub struct OpenH264Encoder {
    encoder: openh264::encoder::Encoder,
    config: CodecConfig,
    generation: u64,
    sequence: u64,
    frames_since_keyframe: u64,
    keyframe_requested: bool,
    shutdown: bool,
}

impl OpenH264Encoder {
    pub fn new(config: CodecConfig) -> Result<Self, ScreenShareError> {
        let config = config.validate()?;
        Ok(Self {
            encoder: make_encoder(config)?,
            config,
            generation: 0,
            sequence: 0,
            frames_since_keyframe: 0,
            keyframe_requested: true,
            shutdown: false,
        })
    }
    pub fn default_profile() -> Result<Self, ScreenShareError> {
        Self::new(CodecConfig::default())
    }

    fn ensure_running(&self) -> Result<(), ScreenShareError> {
        if self.shutdown {
            return Err(ScreenShareError::new("encoder is shut down"));
        }
        Ok(())
    }
}

fn make_encoder(config: CodecConfig) -> Result<openh264::encoder::Encoder, ScreenShareError> {
    use openh264::encoder::{
        BitRate, Complexity, EncoderConfig, IntraFramePeriod, QpRange, RateControlMode, UsageType,
    };
    // PDF Task 7.1 quality profile → documented OpenH264 settings.
    //
    // Usage type: SCREEN_CONTENT_REAL_TIME is OpenH264's screen-sharing mode
    // (camera mode biases toward noisy sensor content and moves more bits into
    // temporal detail; screen mode suits static desktops with text/cursors).
    // Complexity: Low = fastest (fewer CPU cycles/frame), High = crispest.
    // QP range: a tighter max keeps quality high; a wider max lets the rate
    // controller compress more aggressively under the bitrate budget.
    let (complexity, qp_max) = match config.quality_profile {
        QualityProfile::LowLatency => (Complexity::Low, 45),
        QualityProfile::Balanced => (Complexity::Medium, 41),
        QualityProfile::HighQuality => (Complexity::High, 36),
    };
    let settings = EncoderConfig::new()
        .bitrate(BitRate::from_bps(config.target_bitrate_bps))
        .max_frame_rate(openh264::encoder::FrameRate::from_hz(
            config.target_fps as f32,
        ))
        .rate_control_mode(RateControlMode::Bitrate)
        .usage_type(UsageType::ScreenContentRealTime)
        .complexity(complexity)
        .qp(QpRange::new(0, qp_max))
        .skip_frames(false)
        .scene_change_detect(false)
        .background_detection(false)
        .long_term_reference(false)
        .intra_frame_period(IntraFramePeriod::from_num_frames(
            config.keyframe_interval as u32,
        ));
    // NOTE: skip_frames MUST stay false. With skipping enabled, a static
    // screen (nobody interacting with the host) makes the encoder emit no
    // decodable data after the first keyframe — the viewer then freezes on
    // that first frame ("it shows a screen I was on previously"). Every
    // captured frame must yield a P-frame, even when the content is
    // unchanged; the periodic intra_frame_period keyframe keeps the stream
    // self-recovering. Interactive video is better served by always-encoded
    // frames than by bitrate-optimized silence.
    openh264::encoder::Encoder::with_api_config(openh264::OpenH264API::from_source(), settings)
        .map_err(fail)
}

impl VideoEncoder for OpenH264Encoder {
    fn configure(&mut self, config: CodecConfig) -> Result<(), ScreenShareError> {
        self.ensure_running()?;
        let config = config.validate()?;
        self.encoder = make_encoder(config)?;
        self.config = config;
        self.generation += 1;
        self.frames_since_keyframe = 0;
        self.keyframe_requested = true;
        Ok(())
    }

    fn is_keyframe_pending(&self) -> bool {
        self.keyframe_requested
    }

    fn encode(&mut self, frame: &CapturedFrame) -> Result<EncodedPacket, ScreenShareError> {
        self.ensure_running()?;
        if frame.width == 0 || frame.height == 0 {
            return Err(ScreenShareError::new("frame dimensions must be non-zero"));
        }
        let rgb = scale_rgb(
            &rgba_to_rgb(frame)?,
            frame.width,
            frame.height,
            self.config.width,
            self.config.height,
        );
        if self.keyframe_requested || self.frames_since_keyframe >= self.config.keyframe_interval {
            self.encoder.force_intra_frame();
            self.keyframe_requested = false;
        }
        let source = openh264::formats::RgbSliceU8::new(
            &rgb,
            (self.config.width as usize, self.config.height as usize),
        );
        // Fast path: `from_rgb8_source` uses the integer `write_yuv_scalar`
        // converter. `from_rgb_source` goes through the f32 per-pixel
        // `write_yuv_by_pixel` path, which is dramatically slower at HD
        // resolutions (measured ~40ms extra per 1080p frame).
        let yuv = openh264::formats::YUVBuffer::from_rgb8_source(source);
        let stream = self
            .encoder
            .encode_at(
                &yuv,
                openh264::Timestamp::from_millis(frame.timestamp_us / 1_000),
            )
            .map_err(fail)?;
        let keyframe = matches!(
            stream.frame_type(),
            openh264::encoder::FrameType::IDR | openh264::encoder::FrameType::I
        );
        if keyframe {
            self.frames_since_keyframe = 0;
        } else {
            self.frames_since_keyframe += 1;
        }
        let encoded = EncodedPacket {
            timestamp_us: frame.timestamp_us,
            // Encode-stage timestamp (PDF Task 7.2): stamped on the same
            // SystemTime clock as PipeWire capture timestamps so
            // `encode_timestamp_us - timestamp_us` is the capture→encode
            // stage latency, measurable end to end.
            encode_timestamp_us: now_micros(),
            sequence: self.sequence,
            keyframe,
            config_generation: self.generation,
            width: self.config.width,
            height: self.config.height,
            bytes: stream.to_vec(),
        };
        self.sequence += 1;
        Ok(encoded)
    }

    fn force_keyframe(&mut self) {
        self.keyframe_requested = true;
    }

    fn reconfigure_bitrate(&mut self, bitrate_bps: u32) -> Result<(), ScreenShareError> {
        self.ensure_running()?;
        if bitrate_bps == 0 {
            return Err(ScreenShareError::new("bitrate must be non-zero"));
        }
        if bitrate_bps == self.config.target_bitrate_bps {
            return Ok(());
        }
        // The Rust openh264 wrapper does not expose OpenH264's native
        // ENCODER_OPTION_BITRATE setter, so the codec-permitted mid-session
        // path is to rebuild the encoder with the same resolution/fps and the
        // new target bitrate. Resolution is unchanged, so the config
        // generation does NOT bump — the decoder keeps its instance and
        // re-syncs on the forced keyframe (SPS/PPS describe geometry, not
        // bitrate).
        let mut config = self.config;
        config.target_bitrate_bps = bitrate_bps;
        self.encoder = make_encoder(config)?;
        self.config = config;
        self.frames_since_keyframe = 0;
        self.keyframe_requested = true;
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), ScreenShareError> {
        // OpenH264 has no explicit resource release beyond drop; mark the
        // instance shut down so later calls fail loudly instead of encoding
        // into a codec the caller believes is released.
        self.shutdown = true;
        Ok(())
    }

    fn metadata(&self) -> CodecMetadata {
        CodecMetadata {
            codec: CodecKind::H264,
            config: self.config,
            generation: self.generation,
        }
    }
}

#[allow(missing_debug_implementations)]
pub struct OpenH264Decoder {
    decoder: openh264::decoder::Decoder,
    metadata: CodecMetadata,
    waiting_for_keyframe: bool,
}

impl OpenH264Decoder {
    pub fn new(config: CodecConfig) -> Result<Self, ScreenShareError> {
        let config = config.validate()?;
        Ok(Self {
            decoder: openh264::decoder::Decoder::new().map_err(fail)?,
            metadata: CodecMetadata {
                codec: CodecKind::H264,
                config,
                generation: 0,
            },
            waiting_for_keyframe: false,
        })
    }
    pub fn default_profile() -> Result<Self, ScreenShareError> {
        Self::new(CodecConfig::default())
    }
}

impl VideoDecoder for OpenH264Decoder {
    fn decode(&mut self, frame: &EncodedFrame) -> Result<Option<CapturedFrame>, ScreenShareError> {
        if frame.bytes.is_empty() || (self.waiting_for_keyframe && !frame.keyframe) {
            return Ok(None);
        }
        if frame.config_generation != self.metadata.generation {
            self.metadata.generation = frame.config_generation;
            self.metadata.config.width = frame.width;
            self.metadata.config.height = frame.height;
            self.decoder = openh264::decoder::Decoder::new().map_err(fail)?;
            self.waiting_for_keyframe = !frame.keyframe;
            if self.waiting_for_keyframe {
                return Ok(None);
            }
        }
        let Some(yuv) = self.decoder.decode(&frame.bytes).map_err(fail)? else {
            return Ok(None);
        };
        use openh264::formats::YUVSource;
        let (width, height) = yuv.dimensions();
        let mut rgb = vec![0; yuv.rgb8_len()];
        yuv.write_rgb8(&mut rgb);
        let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
        for pixel in rgb.chunks_exact(3) {
            rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
        }
        Ok(Some(CapturedFrame {
            timestamp_us: frame.timestamp_us,
            width: width as u32,
            height: height as u32,
            pixel_format: PixelFormat::Rgba8,
            stride: width as u32 * 4,
            pixels: rgba,
            gpu_handle: None,
            dirty_region: None,
            cursor: None,
        }))
    }
    fn metadata(&self) -> CodecMetadata {
        self.metadata
    }
    fn reset(&mut self) -> Result<(), ScreenShareError> {
        self.decoder = openh264::decoder::Decoder::new().map_err(fail)?;
        self.waiting_for_keyframe = true;
        Ok(())
    }
}

/// Convert an RGB8 (packed 3 bytes/pixel) buffer to planar I420 YUV.
///
/// 4:2:0 subsampling: each 2×2 luma block shares one U and one V sample
/// (average of the four pixels). Coefficients are the standard BT.601
/// full-range conversion used by the H.264 path's `write_yuv` converter so
/// both codecs agree on color semantics.
fn rgb_to_i420(rgb: &[u8], width: u32, height: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let w = width as usize;
    let h = height as usize;
    let mut y = vec![0u8; w * h];
    let mut u = vec![0u8; (w / 2) * (h / 2)];
    let mut v = vec![0u8; (w / 2) * (h / 2)];
    for row in 0..h {
        for col in 0..w {
            let i = (row * w + col) * 3;
            let r = rgb[i] as f32;
            let g = rgb[i + 1] as f32;
            let b = rgb[i + 2] as f32;
            let yy = (0.299 * r + 0.587 * g + 0.114 * b)
                .round()
                .clamp(0.0, 255.0) as u8;
            y[row * w + col] = yy;
            if row % 2 == 0 && col % 2 == 0 {
                let mut r_acc = 0.0f32;
                let mut g_acc = 0.0f32;
                let mut b_acc = 0.0f32;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let ii = ((row + dy).min(h - 1) * w + (col + dx).min(w - 1)) * 3;
                        r_acc += rgb[ii] as f32;
                        g_acc += rgb[ii + 1] as f32;
                        b_acc += rgb[ii + 2] as f32;
                    }
                }
                let r = r_acc / 4.0;
                let g = g_acc / 4.0;
                let b = b_acc / 4.0;
                u[(row / 2) * (w / 2) + col / 2] = (-0.169 * r - 0.331 * g + 0.5 * b + 128.0)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                v[(row / 2) * (w / 2) + col / 2] = (0.5 * r - 0.419 * g - 0.081 * b + 128.0)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
    (y, u, v)
}

/// Convert planar I420 YUV to RGBA8 (BT.601 full-range inverse of
/// [`rgb_to_i420`]).
fn i420_to_rgba(y: &[u8], u: &[u8], v: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut rgba = vec![0u8; w * h * 4];
    for row in 0..h {
        for col in 0..w {
            let yy = y[row * w + col] as f32;
            let uu = u[(row / 2) * (w / 2) + col / 2] as f32 - 128.0;
            let vv = v[(row / 2) * (w / 2) + col / 2] as f32 - 128.0;
            let r = (yy + 1.402 * vv).round().clamp(0.0, 255.0) as u8;
            let g = (yy - 0.344 * uu - 0.714 * vv).round().clamp(0.0, 255.0) as u8;
            let b = (yy + 1.772 * uu).round().clamp(0.0, 255.0) as u8;
            let o = (row * w + col) * 4;
            rgba[o..o + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    rgba
}

/// AV1 encoder backed by rav1e (BSD-2-Clause).
///
/// The codec boundary is identical to [`OpenH264Encoder`]: configure, encode,
/// force_keyframe, reconfigure_bitrate, shutdown. rav1e is configured for
/// low-latency screen content (speed preset 10, single-frame RDO lookahead,
/// fixed keyframe interval, constant-bitrate mode).
///
/// Licensing note (BORU-SS-35): rav1e and rav1d are both BSD-2-Clause /
/// AOM-style permissively licensed, so AV1 keeps Boru MIT/Apache-2.0
/// compatible. H.265/HEVC is deliberately NOT added: x265 is GPL-2.0 and
/// even BSD-licensed HEVC encoders are patent-encumbered (HEVC Advance /
/// MPEG LA pools); see docs/screenshare-feature-review.md §3.5.
#[allow(missing_debug_implementations)]
pub struct Av1Encoder {
    ctx: rav1e::Context<u8>,
    config: CodecConfig,
    generation: u64,
    sequence: u64,
    frames_since_keyframe: u64,
    keyframe_requested: bool,
    shutdown: bool,
    /// Metadata for frames submitted to rav1e whose packets have not yet been
    /// drained. rav1e's low-latency pipeline holds several frames in its RDO
    /// lookahead before emitting the first packet (measured: ~4 frames at
    /// speed preset 10), so each `encode()` call may produce the packet for an
    /// EARLIER frame. The queue maps emitted packets back to the correct
    /// timestamp/sequence.
    pending: std::collections::VecDeque<(u64, u64)>,
    /// Packets drained from rav1e that could not be returned on the call that
    /// produced them. `encode()` returns exactly one packet per call, so when
    /// rav1e emits one packet per submitted frame (steady state after the
    /// fixed lookahead warm-up) this queue holds at most one entry; it exists
    /// so a burst (e.g. keyframe request absorption) never drops a packet.
    ready: std::collections::VecDeque<EncodedPacket>,
}

fn make_rav1e_context(config: CodecConfig) -> Result<rav1e::Context<u8>, ScreenShareError> {
    use rav1e::prelude::*;
    let mut enc = rav1e::EncoderConfig::with_speed_preset(10);
    enc.width = config.width as usize;
    enc.height = config.height as usize;
    enc.bit_depth = 8;
    enc.chroma_sampling = ChromaSampling::Cs420;
    enc.time_base = Rational::new(1, config.target_fps as u64);
    enc.bitrate = config.target_bitrate_bps as i32;
    enc.low_latency = true;
    enc.speed_settings.rdo_lookahead_frames = 1;
    enc.speed_settings.scene_detection_mode = SceneDetectionSpeed::None;
    enc.set_key_frame_interval(config.keyframe_interval, config.keyframe_interval);
    let cfg = rav1e::Config::new()
        .with_encoder_config(enc)
        .with_threads(1);
    cfg.new_context::<u8>().map_err(fail)
}

impl Av1Encoder {
    pub fn new(config: CodecConfig) -> Result<Self, ScreenShareError> {
        let config = config.validate()?;
        Ok(Self {
            ctx: make_rav1e_context(config)?,
            config,
            generation: 0,
            sequence: 0,
            frames_since_keyframe: 0,
            keyframe_requested: true,
            shutdown: false,
            pending: std::collections::VecDeque::new(),
            ready: std::collections::VecDeque::new(),
        })
    }
    pub fn default_profile() -> Result<Self, ScreenShareError> {
        Self::new(CodecConfig::default())
    }

    fn ensure_running(&self) -> Result<(), ScreenShareError> {
        if self.shutdown {
            return Err(ScreenShareError::new("encoder is shut down"));
        }
        Ok(())
    }
}

impl VideoEncoder for Av1Encoder {
    fn configure(&mut self, config: CodecConfig) -> Result<(), ScreenShareError> {
        self.ensure_running()?;
        let config = config.validate()?;
        self.ctx = make_rav1e_context(config)?;
        self.config = config;
        self.generation += 1;
        self.frames_since_keyframe = 0;
        self.keyframe_requested = true;
        self.pending.clear();
        self.ready.clear();
        Ok(())
    }

    fn encode(&mut self, frame: &CapturedFrame) -> Result<EncodedPacket, ScreenShareError> {
        use rav1e::prelude::*;
        self.ensure_running()?;
        if frame.width == 0 || frame.height == 0 {
            return Err(ScreenShareError::new("frame dimensions must be non-zero"));
        }
        let rgb = scale_rgb(
            &rgba_to_rgb(frame)?,
            frame.width,
            frame.height,
            self.config.width,
            self.config.height,
        );
        let (y, u, v) = rgb_to_i420(&rgb, self.config.width, self.config.height);
        let mut rav_frame = self.ctx.new_frame();
        rav_frame.planes[0].copy_from_raw_u8(&y, self.config.width as usize, 1);
        rav_frame.planes[1].copy_from_raw_u8(&u, (self.config.width / 2) as usize, 1);
        rav_frame.planes[2].copy_from_raw_u8(&v, (self.config.width / 2) as usize, 1);
        rav_frame.planes[0].pad(self.config.width as usize, self.config.height as usize);
        rav_frame.planes[1].pad(self.config.width as usize, self.config.height as usize);
        rav_frame.planes[2].pad(self.config.width as usize, self.config.height as usize);

        let force_key =
            self.keyframe_requested || self.frames_since_keyframe >= self.config.keyframe_interval;
        self.keyframe_requested = false;
        if force_key {
            let params = FrameParameters {
                frame_type_override: FrameTypeOverride::Key,
                ..Default::default()
            };
            self.ctx.send_frame((rav_frame, params)).map_err(fail)?;
        } else {
            self.ctx.send_frame(rav_frame).map_err(fail)?;
        }
        // Record the frame we just submitted; its packet will be emitted on a
        // later call (rav1e holds one frame in the RDO lookahead).
        self.pending.push_back((frame.timestamp_us, self.sequence));
        self.sequence += 1;

        // Drain every packet rav1e has ready into our ready-queue. rav1e's
        // low-latency pipeline holds several frames in its RDO lookahead
        // (measured: ~4 frames at speed preset 10), so the first few calls
        // produce no packet at all, then each call produces one packet per
        // submitted frame. Each packet corresponds to the OLDEST pending frame
        // (rav1e emits in submission order in low-latency mode), so pop_front
        // keeps timestamp/sequence aligned. The ready-queue absorbs any burst
        // so no packet is dropped; this method returns exactly one.
        loop {
            let packet = match self.ctx.receive_packet() {
                Ok(packet) => packet,
                Err(EncoderStatus::NeedMoreData) => break,
                Err(EncoderStatus::Encoded) => continue,
                Err(e) => return Err(fail(e)),
            };
            let Some((timestamp_us, sequence)) = self.pending.pop_front() else {
                return Err(ScreenShareError::new("av1: packet without a pending frame"));
            };
            let keyframe = packet.frame_type.all_intra();
            if keyframe {
                self.frames_since_keyframe = 0;
            } else {
                self.frames_since_keyframe += 1;
            }
            self.ready.push_back(EncodedPacket {
                timestamp_us,
                encode_timestamp_us: now_micros(),
                sequence,
                keyframe,
                config_generation: self.generation,
                width: self.config.width,
                height: self.config.height,
                bytes: packet.data,
            });
        }
        self.ready.pop_front().ok_or_else(|| {
            ScreenShareError::new(
                "av1 encoder warming up (packet for the previous frame not ready)",
            )
        })
    }

    fn force_keyframe(&mut self) {
        self.keyframe_requested = true;
    }

    fn is_keyframe_pending(&self) -> bool {
        self.keyframe_requested
    }

    fn reconfigure_bitrate(&mut self, bitrate_bps: u32) -> Result<(), ScreenShareError> {
        self.ensure_running()?;
        if bitrate_bps == 0 {
            return Err(ScreenShareError::new("bitrate must be non-zero"));
        }
        if bitrate_bps == self.config.target_bitrate_bps {
            return Ok(());
        }
        // rav1e has no live bitrate setter; rebuild the context with the same
        // resolution/fps and the new bitrate, exactly like the OpenH264 path.
        let mut next = self.config;
        next.target_bitrate_bps = bitrate_bps;
        self.ctx = make_rav1e_context(next)?;
        self.config = next;
        self.frames_since_keyframe = 0;
        self.keyframe_requested = true;
        self.pending.clear();
        self.ready.clear();
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), ScreenShareError> {
        self.shutdown = true;
        Ok(())
    }

    fn metadata(&self) -> CodecMetadata {
        CodecMetadata {
            codec: CodecKind::Av1,
            config: self.config,
            generation: self.generation,
        }
    }
}

/// AV1 decoder backed by rav1d (BSD-2-Clause) through its C ABI.
///
/// Mirrors [`OpenH264Decoder`]: non-keyframes are dropped until an
/// independently decodable unit arrives, and a config-generation change
/// recreates the decoder.
///
/// `Send` safety: `Dav1dContext` is an opaque `RawArc` pointer whose pointee
/// (`Rav1dContext`) is declared `Send + Sync` by rav1d itself (see
/// `src/internal.rs`); the raw-pointer wrapper is not auto-`Send` only
/// because `NonNull` is conservatively `!Send` in current Rust. The decoder
/// is used from a single decode task, matching rav1d's own contract.
#[allow(missing_debug_implementations)]
pub struct Av1Decoder {
    ctx: Option<rav1d::include::dav1d::dav1d::Dav1dContext>,
    metadata: CodecMetadata,
    waiting_for_keyframe: bool,
    shutdown: bool,
}

// SAFETY: the wrapped `Rav1dContext` is `Send + Sync` (rav1d's own unsafe
// impls); the decoder serialises all access through `decode`/`reset` and is
// driven from one task, so moving the opaque context between threads is
// sound. See the struct docs above.
unsafe impl Send for Av1Decoder {}

impl Av1Decoder {
    pub fn new(config: CodecConfig) -> Result<Self, ScreenShareError> {
        let config = config.validate()?;
        let ctx = open_rav1d()?;
        Ok(Self {
            ctx: Some(ctx),
            metadata: CodecMetadata {
                codec: CodecKind::Av1,
                config,
                generation: 0,
            },
            waiting_for_keyframe: false,
            shutdown: false,
        })
    }
    pub fn default_profile() -> Result<Self, ScreenShareError> {
        Self::new(CodecConfig::default())
    }

    fn close(&mut self) {
        if let Some(ctx) = self.ctx.take() {
            unsafe {
                let mut slot = Some(ctx);
                rav1d::src::lib::dav1d_close(Some(std::ptr::NonNull::from(&mut slot)));
            }
        }
    }
}

/// Open a fresh rav1d context with 8-bit I420 output.
fn open_rav1d() -> Result<rav1d::include::dav1d::dav1d::Dav1dContext, ScreenShareError> {
    use rav1d::include::dav1d::dav1d::Dav1dSettings;
    let mut settings = unsafe { std::mem::MaybeUninit::<Dav1dSettings>::uninit() };
    unsafe {
        rav1d::src::lib::dav1d_default_settings(std::ptr::NonNull::from(&mut settings).cast());
    }
    let mut settings = unsafe { settings.assume_init() };
    // Deterministic single-thread decode; the decoder is used from one task.
    settings.n_threads = 1;
    settings.max_frame_delay = 1;
    let mut ctx: Option<rav1d::include::dav1d::dav1d::Dav1dContext> = None;
    let result = unsafe {
        rav1d::src::lib::dav1d_open(
            Some(std::ptr::NonNull::from(&mut ctx)),
            Some(std::ptr::NonNull::from(&settings)),
        )
    };
    if result.0 != 0 {
        return Err(ScreenShareError::new("rav1d: dav1d_open failed"));
    }
    ctx.ok_or_else(|| ScreenShareError::new("rav1d: open returned no context"))
}

impl Drop for Av1Decoder {
    fn drop(&mut self) {
        self.close();
    }
}

impl VideoDecoder for Av1Decoder {
    fn decode(&mut self, frame: &EncodedFrame) -> Result<Option<CapturedFrame>, ScreenShareError> {
        use rav1d::include::dav1d::headers::DAV1D_PIXEL_LAYOUT_I420;
        use rav1d::include::dav1d::picture::Dav1dPicture;
        if self.shutdown {
            return Err(ScreenShareError::new("decoder is shut down"));
        }
        if frame.bytes.is_empty() || (self.waiting_for_keyframe && !frame.keyframe) {
            return Ok(None);
        }
        if frame.config_generation != self.metadata.generation {
            self.metadata.generation = frame.config_generation;
            self.metadata.config.width = frame.width;
            self.metadata.config.height = frame.height;
            self.close();
            self.ctx = Some(open_rav1d()?);
            self.waiting_for_keyframe = !frame.keyframe;
            if self.waiting_for_keyframe {
                return Ok(None);
            }
        }
        let ctx = self
            .ctx
            .ok_or_else(|| ScreenShareError::new("rav1d: no context"))?;

        // Wrap the packet bytes in a Dav1dData owned by the decoder.
        let mut data: rav1d::include::dav1d::data::Dav1dData = Default::default();
        let ptr = unsafe {
            rav1d::src::lib::dav1d_data_create(
                Some(std::ptr::NonNull::from(&mut data)),
                frame.bytes.len(),
            )
        };
        if ptr.is_null() {
            return Err(ScreenShareError::new("rav1d: data allocation failed"));
        }
        unsafe {
            std::ptr::copy_nonoverlapping(frame.bytes.as_ptr(), ptr, frame.bytes.len());
        }
        let result = unsafe {
            rav1d::src::lib::dav1d_send_data(Some(ctx), Some(std::ptr::NonNull::from(&mut data)))
        };
        if result.0 != 0 {
            // EAGAIN: the decoder still holds a prior buffer. Our send path is
            // one packet in → one picture out, so this only happens on a
            // decode that needed more data; the pipeline's keyframe request
            // recovers.
            unsafe { rav1d::src::lib::dav1d_data_unref(Some(std::ptr::NonNull::from(&mut data))) };
            return Ok(None);
        }

        let mut picture: Dav1dPicture = Default::default();
        let result = unsafe {
            rav1d::src::lib::dav1d_get_picture(
                Some(ctx),
                Some(std::ptr::NonNull::from(&mut picture)),
            )
        };
        if result.0 != 0 {
            // EAGAIN: no picture ready yet; needs more data.
            return Ok(None);
        }

        let width = picture.p.w as u32;
        let height = picture.p.h as u32;
        if width == 0
            || height == 0
            || picture.p.layout != DAV1D_PIXEL_LAYOUT_I420
            || picture.p.bpc != 8
        {
            unsafe {
                rav1d::src::lib::dav1d_picture_unref(Some(std::ptr::NonNull::from(&mut picture)))
            };
            return Err(ScreenShareError::new("rav1d: unsupported picture layout"));
        }
        let y_stride = picture.stride[0] as usize;
        let uv_stride = picture.stride[1] as usize;
        let y_len = y_stride * height as usize;
        let uv_len = uv_stride * (height as usize / 2);
        let y_ptr = picture.data[0].map(|p| p.as_ptr() as *const u8);
        let u_ptr = picture.data[1].map(|p| p.as_ptr() as *const u8);
        let v_ptr = picture.data[2].map(|p| p.as_ptr() as *const u8);
        let (Some(y_ptr), Some(u_ptr), Some(v_ptr)) = (y_ptr, u_ptr, v_ptr) else {
            unsafe {
                rav1d::src::lib::dav1d_picture_unref(Some(std::ptr::NonNull::from(&mut picture)))
            };
            return Err(ScreenShareError::new("rav1d: missing planes"));
        };
        let y = unsafe { std::slice::from_raw_parts(y_ptr, y_len) }.to_vec();
        let u = unsafe { std::slice::from_raw_parts(u_ptr, uv_len) }.to_vec();
        let v = unsafe { std::slice::from_raw_parts(v_ptr, uv_len) }.to_vec();
        unsafe {
            rav1d::src::lib::dav1d_picture_unref(Some(std::ptr::NonNull::from(&mut picture)))
        };

        let rgba = i420_to_rgba(&y, &u, &v, width, height);
        Ok(Some(CapturedFrame {
            timestamp_us: frame.timestamp_us,
            width,
            height,
            pixel_format: PixelFormat::Rgba8,
            stride: width * 4,
            pixels: rgba,
            gpu_handle: None,
            dirty_region: None,
            cursor: None,
        }))
    }

    fn metadata(&self) -> CodecMetadata {
        self.metadata
    }

    fn reset(&mut self) -> Result<(), ScreenShareError> {
        self.close();
        self.ctx = Some(open_rav1d()?);
        self.waiting_for_keyframe = true;
        Ok(())
    }
}

/// Build the concrete encoder for a negotiated codec name.
///
/// `codec_name` is the lowercase wire name from `Hello.codecs` /
/// `ScreenShareOffer.codecs` (e.g. "h264", "h264_vaapi" or "av1"). Unknown
/// names are a clean rejection — the caller falls back to H.264 (which is
/// always in the advertised list) before this is reached. Hardware kinds
/// (`h264_vaapi`) fall back to OpenH264 on any init failure; `h264_mf`
/// returns a typed unavailable error in this build.
pub fn create_encoder(
    codec_name: &str,
    config: CodecConfig,
) -> Result<Box<dyn VideoEncoder>, ScreenShareError> {
    let kind = CodecKind::from_wire_name(codec_name)
        .ok_or_else(|| ScreenShareError::new(format!("unsupported codec: {codec_name}")))?;
    create_encoder_for(kind, config)
}

/// Build the concrete decoder for a negotiated codec name (see
/// [`create_encoder`]).
pub fn create_decoder(
    codec_name: &str,
    config: CodecConfig,
) -> Result<Box<dyn VideoDecoder>, ScreenShareError> {
    match CodecKind::from_name(codec_name) {
        Some(CodecKind::H264) => Ok(Box::new(OpenH264Decoder::new(config)?)),
        Some(CodecKind::Av1) => Ok(Box::new(Av1Decoder::new(config)?)),
        // h264_vaapi / h264_mf are encoder-side wire names; decoding is
        // always the software baseline (the viewer never needs to know the
        // encoder backend).
        Some(other) => Err(ScreenShareError::new(format!(
            "unsupported decoder codec: {}",
            other.wire_name()
        ))),
        None => Err(ScreenShareError::new(format!(
            "unsupported codec: {codec_name}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(width: u32, height: u32) -> CodecConfig {
        CodecConfig {
            width,
            height,
            target_fps: 30,
            target_bitrate_bps: 400_000,
            keyframe_interval: 4,
            max_queue_depth: 2,
            quality_profile: QualityProfile::Balanced,
        }
    }
    fn pattern(width: u32, height: u32, timestamp_us: u64) -> CapturedFrame {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[x as u8, y as u8, (x ^ y) as u8, 255]);
            }
        }
        CapturedFrame::cpu(timestamp_us, width, height, PixelFormat::Rgba8, pixels).unwrap()
    }
    #[test]
    fn synthetic_pattern_round_trips_with_bounded_metadata() {
        let cfg = config(32, 24);
        let source = pattern(32, 24, 1000);
        let mut encoder = OpenH264Encoder::new(cfg).unwrap();
        let encoded = encoder.encode(&source).unwrap();
        assert!(encoded.keyframe && encoded.sequence == 0 && !encoded.bytes.is_empty());
        let mut decoder = OpenH264Decoder::new(cfg).unwrap();
        let decoded = decoder.decode(&encoded).unwrap().unwrap();
        assert_eq!((decoded.width, decoded.height), (32, 24));
        assert_eq!(decoded.pixels.len(), source.pixels.len());
        assert_ne!(
            decoded.pixels.iter().fold(0u64, |sum, b| sum + *b as u64),
            0
        );
    }
    #[test]
    fn factory_rejects_unwired_hardware_kinds_with_typed_error() {
        // Windows Media Foundation (h264_mf) is documented but not wired in
        // this build: requesting it must produce a typed
        // HardwareAccelerationUnavailable error, never a silent software
        // encode (BORU-SS-34 fallback contract).
        let error = match create_encoder_for(CodecKind::H264Mf, config(32, 24)) {
            Err(error) => error,
            Ok(_) => panic!("h264_mf must not silently succeed in this build"),
        };
        assert_eq!(
            error.kind(),
            crate::screen_share::ScreenShareErrorKind::HardwareAccelerationUnavailable
        );
        // Unknown wire names never map to a codec kind (negotiation rejects
        // them rather than guessing).
        assert_eq!(CodecKind::from_wire_name("vp8"), None);
        assert_eq!(CodecKind::from_wire_name(""), None);
    }
    #[test]
    fn request_reset_and_reconfigure_are_explicit() {
        let mut encoder = OpenH264Encoder::new(config(16, 16)).unwrap();
        encoder.encode(&pattern(16, 16, 0)).unwrap();
        encoder.request_keyframe();
        assert!(encoder.encode(&pattern(16, 16, 1)).unwrap().keyframe);
        encoder.reconfigure(config(24, 16)).unwrap();
        assert_eq!(encoder.metadata().generation, 1);
        let frame = encoder.encode(&pattern(24, 16, 2)).unwrap();
        assert!(frame.keyframe);
        let mut decoder = OpenH264Decoder::new(config(24, 16)).unwrap();
        decoder.reset().unwrap();
        assert!(decoder.decode(&frame).unwrap().is_some());
    }
    #[test]
    fn static_screen_still_produces_decodable_frames_every_tick() {
        // Regression: skip_frames(true) made the encoder emit NO decodable
        // data for a static screen after the first keyframe — the viewer
        // froze on the first frame ("it shows a screen I was on
        // previously"). With skipping disabled every captured frame must
        // yield a non-empty P-frame that decodes.
        let cfg = config(64, 48);
        let mut encoder = OpenH264Encoder::new(cfg).unwrap();
        let mut decoder = OpenH264Decoder::new(cfg).unwrap();
        // Identical pixels on every tick = static screen (timestamps still
        // advance by one frame period, exactly like the X11 capture source).
        let first = encoder.encode(&pattern(64, 48, 0)).unwrap();
        assert!(first.keyframe && !first.bytes.is_empty());
        assert!(
            decoder.decode(&first).unwrap().is_some(),
            "keyframe must decode"
        );
        let mut decoded = 1;
        for tick in 1..=5 {
            let encoded = encoder
                .encode(&pattern(64, 48, tick * 33_333))
                .unwrap_or_else(|e| panic!("tick {tick}: {e}"));
            assert!(
                !encoded.bytes.is_empty(),
                "static frame {tick} must not be skipped"
            );
            if decoder.decode(&encoded).unwrap().is_some() {
                decoded += 1;
            }
        }
        assert_eq!(decoded, 6, "every static frame must decode");
    }
    #[test]
    fn force_keyframe_controls_the_next_access_unit() {
        let mut encoder = OpenH264Encoder::new(config(32, 24)).unwrap();
        let first = encoder.encode(&pattern(32, 24, 0)).unwrap();
        assert!(first.keyframe, "first unit is a keyframe");
        let second = encoder.encode(&pattern(32, 24, 33_333)).unwrap();
        assert!(!second.keyframe, "subsequent unit is a delta frame");
        encoder.force_keyframe();
        let third = encoder.encode(&pattern(32, 24, 66_666)).unwrap();
        assert!(
            third.keyframe,
            "force_keyframe must make the next unit a keyframe"
        );
    }
    #[test]
    fn reconfigure_bitrate_keeps_resolution_and_stays_decodable() {
        let cfg = config(32, 24);
        let mut encoder = OpenH264Encoder::new(cfg).unwrap();
        let mut decoder = OpenH264Decoder::new(cfg).unwrap();
        let first = encoder.encode(&pattern(32, 24, 0)).unwrap();
        assert!(decoder.decode(&first).unwrap().is_some());
        let gen_before = encoder.metadata().generation;
        encoder.reconfigure_bitrate(1_200_000).unwrap();
        assert_eq!(
            encoder.metadata().generation,
            gen_before,
            "bitrate change must not bump config generation"
        );
        assert_eq!(
            (
                encoder.metadata().config.width,
                encoder.metadata().config.height
            ),
            (32, 24)
        );
        let next = encoder.encode(&pattern(32, 24, 33_333)).unwrap();
        assert!(
            next.keyframe,
            "bitrate reconfigure forces a keyframe for re-sync"
        );
        assert!(
            decoder.decode(&next).unwrap().is_some(),
            "stream must stay decodable after bitrate change"
        );
        // Same-bitrate reconfigure is a no-op.
        encoder.reconfigure_bitrate(1_200_000).unwrap();
        assert_eq!(encoder.metadata().config.target_bitrate_bps, 1_200_000);
    }
    #[test]
    fn configure_changes_resolution_without_session_restart() {
        let mut encoder = OpenH264Encoder::new(config(32, 24)).unwrap();
        encoder.encode(&pattern(32, 24, 0)).unwrap();
        encoder.configure(config(48, 32)).unwrap();
        assert_eq!(
            encoder.metadata().generation,
            1,
            "resolution change bumps generation"
        );
        let frame = encoder.encode(&pattern(48, 32, 33_333)).unwrap();
        assert_eq!((frame.width, frame.height), (48, 32));
        assert!(frame.keyframe, "post-configure unit is a keyframe");
        // A decoder that follows the generation change decodes the new geometry.
        let mut decoder = OpenH264Decoder::new(config(48, 32)).unwrap();
        assert!(decoder.decode(&frame).unwrap().is_some());
    }
    #[test]
    fn shutdown_is_idempotent_and_blocks_further_use() {
        let mut encoder = OpenH264Encoder::new(config(16, 16)).unwrap();
        encoder.shutdown().unwrap();
        assert!(encoder.shutdown().is_ok(), "shutdown is idempotent");
        assert!(encoder.encode(&pattern(16, 16, 0)).is_err());
        assert!(encoder.configure(config(32, 24)).is_err());
        assert!(encoder.reconfigure_bitrate(500_000).is_err());
    }
    #[test]
    fn trait_contract_back_compat_aliases_delegate_to_five_ops() {
        // A mock that records only the five PDF operations proves the default
        // aliases (request_keyframe / reconfigure / reset) delegate to them
        // without touching OpenH264 at all.
        #[derive(Default)]
        struct MockEncoder {
            configured: Vec<CodecConfig>,
            keyframes: u32,
            bitrates: Vec<u32>,
            shutdowns: u32,
            config: CodecConfig,
        }
        impl VideoEncoder for MockEncoder {
            fn configure(&mut self, config: CodecConfig) -> Result<(), ScreenShareError> {
                self.config = config;
                self.configured.push(config);
                Ok(())
            }
            fn encode(&mut self, frame: &CapturedFrame) -> Result<EncodedPacket, ScreenShareError> {
                Ok(EncodedPacket {
                    timestamp_us: frame.timestamp_us,
                    encode_timestamp_us: frame.timestamp_us,
                    sequence: 0,
                    keyframe: true,
                    config_generation: 0,
                    width: self.config.width,
                    height: self.config.height,
                    bytes: vec![1],
                })
            }
            fn force_keyframe(&mut self) {
                self.keyframes += 1;
            }
            fn reconfigure_bitrate(&mut self, bitrate_bps: u32) -> Result<(), ScreenShareError> {
                self.bitrates.push(bitrate_bps);
                Ok(())
            }
            fn shutdown(&mut self) -> Result<(), ScreenShareError> {
                self.shutdowns += 1;
                Ok(())
            }
            fn metadata(&self) -> CodecMetadata {
                CodecMetadata {
                    codec: CodecKind::H264,
                    config: self.config,
                    generation: 0,
                }
            }
        }
        let mut encoder = MockEncoder::default();
        encoder.request_keyframe();
        assert_eq!(
            encoder.keyframes, 1,
            "request_keyframe delegates to force_keyframe"
        );
        encoder.reconfigure(config(16, 16)).unwrap();
        assert_eq!(
            encoder.configured.len(),
            1,
            "reconfigure delegates to configure"
        );
        encoder.reconfigure_bitrate(300_000).unwrap();
        assert_eq!(encoder.bitrates, vec![300_000]);
        encoder.reset().unwrap();
        assert_eq!(
            encoder.configured.len(),
            2,
            "reset reconfigures with the active config"
        );
        encoder.shutdown().unwrap();
        encoder.shutdown().unwrap();
        assert_eq!(encoder.shutdowns, 2, "shutdown is idempotent");
    }

    #[test]
    fn every_encode_stamps_an_encode_stage_timestamp() {
        // PDF Task 7.2: capture and encode stages both carry timestamps so
        // end-to-end latency is measurable. The encoder stamps on the same
        // SystemTime clock real PipeWire captures use, so the delta between
        // the two timestamps is the capture→encode stage latency.
        let cfg = config(32, 24);
        let mut encoder = OpenH264Encoder::new(cfg).unwrap();
        let first = encoder.encode(&pattern(32, 24, 1000)).unwrap();
        assert!(
            first.encode_timestamp_us > 0,
            "encode stage must carry a timestamp"
        );
        assert!(
            first.encode_timestamp_us >= first.timestamp_us,
            "encode happens at or after capture on the same clock"
        );
        // Timestamps are monotonic across encodes.
        let second = encoder.encode(&pattern(32, 24, 33_333)).unwrap();
        assert!(second.encode_timestamp_us >= first.encode_timestamp_us);
        // The capture timestamp is preserved unchanged for latency math.
        assert_eq!(second.timestamp_us, 33_333);
    }

    #[test]
    fn quality_profile_round_trips_through_wire_value() {
        for profile in [
            QualityProfile::Balanced,
            QualityProfile::LowLatency,
            QualityProfile::HighQuality,
        ] {
            assert_eq!(QualityProfile::from_u8(profile.as_u8()), Some(profile));
            assert!(!profile.name().is_empty());
        }
        assert_eq!(
            QualityProfile::from_u8(9),
            None,
            "unknown wire value must be rejected"
        );
        assert_eq!(QualityProfile::default(), QualityProfile::Balanced);
    }

    #[test]
    fn target_profiles_expose_720p30_and_1080p30() {
        let p720 = CodecConfig::profile_720p30();
        assert_eq!((p720.width, p720.height, p720.target_fps), (1280, 720, 30));
        assert_eq!(p720.target_bitrate_bps, TARGET_720P30_BITRATE_BPS);
        let p1080 = CodecConfig::profile_1080p30();
        assert_eq!(
            (p1080.width, p1080.height, p1080.target_fps),
            (1920, 1080, 30)
        );
        assert_eq!(p1080.target_bitrate_bps, TARGET_1080P30_BITRATE_BPS);
        // Both are valid encoder configs (validation passes).
        assert!(p720.validate().is_ok());
        assert!(p1080.validate().is_ok());
    }

    #[test]
    fn every_quality_profile_constructs_and_encodes_decodable_frames() {
        for profile in [
            QualityProfile::LowLatency,
            QualityProfile::Balanced,
            QualityProfile::HighQuality,
        ] {
            let cfg = CodecConfig {
                quality_profile: profile,
                ..config(32, 24)
            };
            let mut encoder = OpenH264Encoder::new(cfg).unwrap();
            let mut decoder = OpenH264Decoder::new(cfg).unwrap();
            let first = encoder.encode(&pattern(32, 24, 0)).unwrap();
            assert!(
                first.keyframe && !first.bytes.is_empty(),
                "{profile:?} keyframe must encode"
            );
            assert!(
                decoder.decode(&first).unwrap().is_some(),
                "{profile:?} keyframe must decode"
            );
            let second = encoder.encode(&pattern(32, 24, 33_333)).unwrap();
            assert!(
                !second.bytes.is_empty(),
                "{profile:?} delta frame must encode"
            );
            assert!(
                decoder.decode(&second).unwrap().is_some(),
                "{profile:?} delta frame must decode"
            );
        }
    }

    /// PDF Phase 11 Media matrix — 720p30: full encode → decode round trip
    /// at the documented 720p30 target profile (1280x720 @ 30 fps). Every
    /// frame must encode into a non-empty access unit, decode back to the
    /// same geometry, and the capture timestamps must advance by one frame
    /// period (33.3 ms) exactly as a real 30 fps capture source would.
    #[test]
    fn media_round_trip_720p30() {
        let cfg = CodecConfig::profile_720p30();
        let mut encoder = OpenH264Encoder::new(cfg).unwrap();
        let mut decoder = OpenH264Decoder::new(cfg).unwrap();
        let mut decoded = 0;
        for i in 0..3 {
            let frame = pattern(TARGET_720P30_WIDTH, TARGET_720P30_HEIGHT, i * 33_333);
            let encoded = encoder.encode(&frame).unwrap();
            assert!(!encoded.bytes.is_empty(), "720p30 frame {i} must encode");
            assert_eq!(
                (encoded.width, encoded.height),
                (TARGET_720P30_WIDTH, TARGET_720P30_HEIGHT)
            );
            assert_eq!(
                encoded.timestamp_us,
                i * 33_333,
                "capture timestamp preserved"
            );
            assert_eq!(encoded.sequence, i as u64, "sequence advances per frame");
            if i == 0 {
                assert!(encoded.keyframe, "first 720p30 unit is a keyframe");
            } else {
                assert!(
                    !encoded.keyframe,
                    "subsequent 720p30 units are delta frames"
                );
            }
            let out = decoder
                .decode(&encoded)
                .unwrap()
                .expect("720p30 frame decodes");
            assert_eq!(
                (out.width, out.height),
                (TARGET_720P30_WIDTH, TARGET_720P30_HEIGHT)
            );
            assert_eq!(
                out.pixels.len(),
                (TARGET_720P30_WIDTH * TARGET_720P30_HEIGHT * 4) as usize
            );
            decoded += 1;
        }
        assert_eq!(decoded, 3, "every 720p30 frame must decode");
    }

    /// PDF Phase 11 Media matrix — 1080p30: full encode → decode round trip
    /// at the documented 1080p30 target profile (1920x1080 @ 30 fps).
    #[test]
    fn media_round_trip_1080p30() {
        let cfg = CodecConfig::profile_1080p30();
        let mut encoder = OpenH264Encoder::new(cfg).unwrap();
        let mut decoder = OpenH264Decoder::new(cfg).unwrap();
        let mut decoded = 0;
        for i in 0..3 {
            let frame = pattern(TARGET_1080P30_WIDTH, TARGET_1080P30_HEIGHT, i * 33_333);
            let encoded = encoder.encode(&frame).unwrap();
            assert!(!encoded.bytes.is_empty(), "1080p30 frame {i} must encode");
            assert_eq!(
                (encoded.width, encoded.height),
                (TARGET_1080P30_WIDTH, TARGET_1080P30_HEIGHT)
            );
            assert_eq!(encoded.timestamp_us, i * 33_333);
            if i == 0 {
                assert!(encoded.keyframe, "first 1080p30 unit is a keyframe");
            } else {
                assert!(!encoded.keyframe);
            }
            let out = decoder
                .decode(&encoded)
                .unwrap()
                .expect("1080p30 frame decodes");
            assert_eq!(
                (out.width, out.height),
                (TARGET_1080P30_WIDTH, TARGET_1080P30_HEIGHT)
            );
            assert_eq!(
                out.pixels.len(),
                (TARGET_1080P30_WIDTH * TARGET_1080P30_HEIGHT * 4) as usize
            );
            decoded += 1;
        }
        assert_eq!(decoded, 3, "every 1080p30 frame must decode");
    }

    /// PDF Phase 11 Media matrix — keyframe recovery. The viewer lost frames
    /// 1..=4 (network drop); on its KeyframeRequest the host forces a
    /// keyframe and the SAME decoder instance recovers immediately — no
    /// session restart, no encoder reset.
    #[test]
    fn keyframe_recovery_after_dropped_frames() {
        let cfg = config(64, 48);
        let mut encoder = OpenH264Encoder::new(cfg).unwrap();
        let mut decoder = OpenH264Decoder::new(cfg).unwrap();
        let first = encoder.encode(&pattern(64, 48, 0)).unwrap();
        assert!(first.keyframe);
        assert!(
            decoder.decode(&first).unwrap().is_some(),
            "keyframe decodes"
        );
        // Frames 1..=4 are dropped by the viewer (never decoded).
        for i in 1..=4 {
            let _ = encoder.encode(&pattern(64, 48, i * 33_333)).unwrap();
        }
        // The viewer's KeyframeRequest becomes force_keyframe on the host.
        encoder.force_keyframe();
        let recovery = encoder.encode(&pattern(64, 48, 5 * 33_333)).unwrap();
        assert!(
            recovery.keyframe,
            "recovery frame must be independently decodable"
        );
        let recovered = decoder
            .decode(&recovery)
            .unwrap()
            .expect("recovery frame decodes");
        assert_eq!((recovered.width, recovered.height), (64, 48));
        assert_ne!(
            recovered.pixels.iter().fold(0u64, |sum, b| sum + *b as u64),
            0,
            "recovered frame carries real pixels"
        );
    }

    /// PDF Phase 11 Media matrix — long-running share. A two-minute share at
    /// 30 fps (3600 frames) through the real capture source
    /// (capture → encode → decode) must stay healthy: every frame encodes
    /// and decodes, timestamps and sequence numbers advance monotonically,
    /// and the stream remains fully decodable start to finish.
    #[test]
    fn long_running_share_remains_healthy() {
        use crate::screen_share::capture::{ScreenCapture, TestPatternCapture};
        const WIDTH: u32 = 160;
        const HEIGHT: u32 = 90;
        const FRAMES: u64 = 3600; // 2 minutes at 30 fps
        let cfg = CodecConfig {
            width: WIDTH,
            height: HEIGHT,
            target_fps: 30,
            keyframe_interval: 60,
            ..config(WIDTH, HEIGHT)
        };
        let mut capture = TestPatternCapture::new(WIDTH, HEIGHT).unwrap();
        let mut encoder = OpenH264Encoder::new(cfg).unwrap();
        let mut decoder = OpenH264Decoder::new(cfg).unwrap();
        let mut decoded = 0u64;
        let mut last_timestamp: Option<u64> = None;
        for i in 0..FRAMES {
            let frame = capture.capture().unwrap().expect("capture frame");
            assert_eq!((frame.width, frame.height), (WIDTH, HEIGHT));
            if let Some(last) = last_timestamp {
                assert!(
                    frame.timestamp_us > last,
                    "capture timestamps advance monotonically (frame {i})"
                );
            }
            last_timestamp = Some(frame.timestamp_us);
            let encoded = encoder
                .encode(&frame)
                .unwrap_or_else(|e| panic!("frame {i}: {e}"));
            assert!(!encoded.bytes.is_empty(), "frame {i} must encode");
            assert_eq!(encoded.sequence, i, "sequence advances monotonically");
            assert_eq!(encoded.timestamp_us, frame.timestamp_us);
            let out = decoder
                .decode(&encoded)
                .unwrap_or_else(|e| panic!("decode {i}: {e}"));
            assert!(out.is_some(), "frame {i} must decode");
            decoded += 1;
        }
        assert_eq!(decoded, FRAMES, "every frame of a 2-minute share decodes");
    }

    #[test]
    fn codec_config_applies_capture_config_encode_knobs() {
        let capture = CaptureConfig {
            target_fps: 24,
            target_bitrate_bps: 3_000_000,
            keyframe_interval: 30,
            quality_profile: QualityProfile::HighQuality,
            ..CaptureConfig::default()
        };
        let codec = CodecConfig::from_capture_config(&capture, 1280, 720);
        assert_eq!(codec.width, 1280);
        assert_eq!(codec.height, 720);
        assert_eq!(codec.target_fps, 24);
        assert_eq!(codec.target_bitrate_bps, 3_000_000);
        assert_eq!(codec.keyframe_interval, 30);
        assert_eq!(codec.quality_profile, QualityProfile::HighQuality);
        // Default capture config drives the default codec values.
        let defaults = CodecConfig::from_capture_config(&CaptureConfig::default(), 640, 360);
        assert_eq!(defaults.target_bitrate_bps, DEFAULT_BITRATE_BPS);
        assert_eq!(defaults.keyframe_interval, DEFAULT_KEYFRAME_INTERVAL);
        assert_eq!(defaults.quality_profile, QualityProfile::Balanced);
    }

    #[test]
    fn codec_kind_name_mapping_is_bidirectional_and_case_insensitive() {
        for (kind, name) in [(CodecKind::H264, "h264"), (CodecKind::Av1, "av1")] {
            assert_eq!(kind.name(), name);
            assert_eq!(CodecKind::from_name(name), Some(kind));
            assert_eq!(CodecKind::from_name(&name.to_uppercase()), Some(kind));
        }
        assert_eq!(CodecKind::from_name("vp8"), None, "unknown codec rejects");
        assert_eq!(CodecKind::from_name(""), None);
    }

    /// rav1e's low-latency pipeline holds several frames in its RDO lookahead
    /// before emitting the first packet (measured: ~4 frames at speed preset
    /// 10). Feed frames and return every packet that emerges; the first few
    /// calls produce nothing (the encoder returns a warming-up error the host
    /// logs and continues).
    fn feed(encoder: &mut Av1Encoder, count: u64, start_us: u64) -> Vec<EncodedPacket> {
        let mut packets = Vec::new();
        for i in 0..count {
            if let Ok(packet) = encoder.encode(&pattern(32, 24, start_us + i * 33_333)) {
                packets.push(packet);
            }
        }
        packets
    }

    #[test]
    fn av1_synthetic_pattern_round_trips_with_bounded_metadata() {
        let cfg = config(32, 24);
        let mut encoder = Av1Encoder::new(cfg).unwrap();
        let mut decoder = Av1Decoder::new(cfg).unwrap();
        // Feed 8 frames: the fixed lookahead swallows the first four, so we
        // get packets for frames 0..3.
        let packets = feed(&mut encoder, 8, 0);
        assert_eq!(packets.len(), 4, "one packet per frame after warm-up");
        assert!(!packets[0].bytes.is_empty(), "keyframe must encode");
        let keyframes = packets.iter().filter(|p| p.keyframe).count();
        assert_eq!(keyframes, 1, "exactly one keyframe in a 4-frame run");
        let mut decoded = 0;
        for packet in &packets {
            if let Some(decoded_frame) = decoder.decode(packet).unwrap() {
                assert_eq!((decoded_frame.width, decoded_frame.height), (32, 24));
                assert_eq!(decoded_frame.pixels.len(), (32 * 24 * 4) as usize);
                assert_ne!(
                    decoded_frame
                        .pixels
                        .iter()
                        .fold(0u64, |sum, b| sum + *b as u64),
                    0,
                    "decoded pixels must be non-zero"
                );
                decoded += 1;
            }
        }
        assert!(decoded >= 1, "at least the keyframe must decode");
    }

    #[test]
    fn av1_force_keyframe_controls_the_next_access_unit() {
        let cfg = config(32, 24);
        let mut encoder = Av1Encoder::new(cfg).unwrap();
        // Warm up and consume the first four packets: [key, delta, delta, delta].
        let first_batch = feed(&mut encoder, 8, 0);
        assert_eq!(first_batch.len(), 4);
        assert!(first_batch[0].keyframe, "first unit is a keyframe");
        assert!(!first_batch[1].keyframe, "subsequent unit is a delta frame");
        // force_keyframe marks the NEXT submitted frame; its packet emerges
        // after the fixed lookahead (5 more calls: 4 deltas then the key).
        encoder.force_keyframe();
        let forced = feed(&mut encoder, 5, 200_000);
        assert_eq!(forced.len(), 5);
        assert!(
            forced.last().unwrap().keyframe,
            "force_keyframe must make the next unit a keyframe"
        );
    }

    #[test]
    fn av1_reconfigure_bitrate_keeps_resolution_and_stays_decodable() {
        let cfg = config(32, 24);
        let mut encoder = Av1Encoder::new(cfg).unwrap();
        let mut decoder = Av1Decoder::new(cfg).unwrap();
        let first_batch = feed(&mut encoder, 8, 0);
        assert!(!first_batch.is_empty());
        assert!(decoder.decode(&first_batch[0]).unwrap().is_some());
        let gen_before = encoder.metadata().generation;
        encoder.reconfigure_bitrate(1_200_000).unwrap();
        assert_eq!(
            encoder.metadata().generation,
            gen_before,
            "bitrate change must not bump config generation"
        );
        assert_eq!(
            (
                encoder.metadata().config.width,
                encoder.metadata().config.height
            ),
            (32, 24)
        );
        // The rebuild resets the lookahead, so the next few calls warm up
        // again; the first packet after the rebuild is a fresh keyframe.
        let next_batch = feed(&mut encoder, 8, 400_000);
        assert!(!next_batch.is_empty());
        assert!(
            next_batch[0].keyframe,
            "bitrate reconfigure forces a keyframe for re-sync"
        );
        assert!(
            decoder.decode(&next_batch[0]).unwrap().is_some(),
            "stream must stay decodable after bitrate change"
        );
        encoder.reconfigure_bitrate(1_200_000).unwrap();
        assert_eq!(encoder.metadata().config.target_bitrate_bps, 1_200_000);
    }

    #[test]
    fn av1_configure_changes_resolution_without_session_restart() {
        let mut encoder = Av1Encoder::new(config(32, 24)).unwrap();
        // Warm-up on the old resolution.
        let _ = feed(&mut encoder, 4, 0);
        encoder.configure(config(48, 32)).unwrap();
        assert_eq!(
            encoder.metadata().generation,
            1,
            "resolution change bumps generation"
        );
        // Rebuild resets the lookahead: feed enough to get a packet at the
        // new resolution.
        let packets = feed(&mut encoder, 8, 0);
        assert!(!packets.is_empty());
        let frame = &packets[0];
        assert_eq!((frame.width, frame.height), (48, 32));
        assert!(frame.keyframe, "post-configure unit is a keyframe");
    }

    #[test]
    fn create_encoder_and_decoder_dispatch_on_codec_name() {
        let cfg = config(32, 24);
        let enc_h264 = create_encoder("h264", cfg).unwrap();
        assert_eq!(enc_h264.metadata().codec, CodecKind::H264);
        let enc_av1 = create_encoder("av1", cfg).unwrap();
        assert_eq!(enc_av1.metadata().codec, CodecKind::Av1);
        let dec_h264 = create_decoder("h264", cfg).unwrap();
        assert_eq!(dec_h264.metadata().codec, CodecKind::H264);
        let dec_av1 = create_decoder("av1", cfg).unwrap();
        assert_eq!(dec_av1.metadata().codec, CodecKind::Av1);
        // Unknown codec names are a clean rejection (callers fall back to
        // H.264 before this point).
        assert!(create_encoder("vp9", cfg).is_err());
        assert!(create_decoder("vp9", cfg).is_err());
    }
}
