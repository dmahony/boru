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
        match self { Self::Balanced => "balanced", Self::LowLatency => "low-latency", Self::HighQuality => "high-quality" }
    }
    /// Stable wire value: 0 = Balanced, 1 = LowLatency, 2 = HighQuality.
    pub const fn as_u8(self) -> u8 {
        match self { Self::Balanced => 0, Self::LowLatency => 1, Self::HighQuality => 2 }
    }
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value { 0 => Some(Self::Balanced), 1 => Some(Self::LowLatency), 2 => Some(Self::HighQuality), _ => None }
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
        Self { width: DEFAULT_WIDTH, height: DEFAULT_HEIGHT, target_fps: DEFAULT_FPS,
            target_bitrate_bps: DEFAULT_BITRATE_BPS, keyframe_interval: DEFAULT_KEYFRAME_INTERVAL,
            max_queue_depth: DEFAULT_QUEUE_CAPACITY, quality_profile: QualityProfile::Balanced }
    }
}

impl CodecConfig {
    fn validate(self) -> Result<Self, ScreenShareError> {
        if self.width == 0 || self.height == 0 || self.width % 2 != 0 || self.height % 2 != 0 {
            return Err(ScreenShareError::new("codec dimensions must be non-zero even values"));
        }
        if self.target_fps == 0 || self.target_bitrate_bps == 0 || self.keyframe_interval == 0 || self.max_queue_depth == 0 {
            return Err(ScreenShareError::new("codec rates and queue depth must be non-zero"));
        }
        Ok(self)
    }

    /// 720p @ 30 fps target profile (PDF Task 7.1).
    pub fn profile_720p30() -> Self {
        Self { width: TARGET_720P30_WIDTH, height: TARGET_720P30_HEIGHT, target_fps: 30,
            target_bitrate_bps: TARGET_720P30_BITRATE_BPS, ..Self::default() }
    }

    /// 1080p @ 30 fps target profile (PDF Task 7.1).
    pub fn profile_1080p30() -> Self {
        Self { width: TARGET_1080P30_WIDTH, height: TARGET_1080P30_HEIGHT, target_fps: 30,
            target_bitrate_bps: TARGET_1080P30_BITRATE_BPS, ..Self::default() }
    }

    /// Build the codec config from a capture-session config, so bitrate,
    /// frame rate, keyframe interval and quality profile all flow from the
    /// same CaptureConfig the capture backend was started with.
    pub fn from_capture_config(capture: &CaptureConfig, width: u32, height: u32) -> Self {
        Self {
            width, height,
            target_fps: capture.target_fps,
            target_bitrate_bps: capture.target_bitrate_bps,
            keyframe_interval: capture.keyframe_interval,
            quality_profile: capture.quality_profile,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecMetadata { pub codec: CodecKind, pub config: CodecConfig, pub generation: u64 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecKind { H264 }

/// One encoded access unit (a keyframe or delta frame) passed to a transport.
///
/// Carries the timestamp, sequence number, keyframe flag, and the encoder
/// generation/resolution it was produced with, so downstream consumers (the
/// decoder and the protocol layer) never need codec-specific types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedPacket {
    pub timestamp_us: u64,
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
    fn shutdown(&mut self) -> Result<(), ScreenShareError> { Ok(()) }

    /// Current codec metadata (codec kind, active config, generation).
    fn metadata(&self) -> CodecMetadata;

    /// Back-compat alias for [`Self::force_keyframe`].
    fn request_keyframe(&mut self) { self.force_keyframe(); }
    /// Back-compat alias for [`Self::configure`].
    fn reconfigure(&mut self, config: CodecConfig) -> Result<(), ScreenShareError> { self.configure(config) }
    /// Reset the encoder to its currently active config (forces a keyframe).
    fn reset(&mut self) -> Result<(), ScreenShareError> { self.reconfigure(self.metadata().config) }
}

/// After reset, non-keyframes are dropped until an independently decodable unit arrives.
pub trait VideoDecoder: Send {
    fn decode(&mut self, frame: &EncodedFrame) -> Result<Option<CapturedFrame>, ScreenShareError>;
    fn metadata(&self) -> CodecMetadata;
    fn reset(&mut self) -> Result<(), ScreenShareError>;
}

pub trait ScreenShareCodec: VideoEncoder + VideoDecoder {}
impl<T: VideoEncoder + VideoDecoder> ScreenShareCodec for T {}

fn fail(error: impl std::fmt::Display) -> ScreenShareError { ScreenShareError::new(error.to_string()) }

fn rgba_to_rgb(frame: &CapturedFrame) -> Result<Vec<u8>, ScreenShareError> {
    if !matches!(frame.pixel_format, PixelFormat::Bgra8 | PixelFormat::Rgba8) {
        return Err(ScreenShareError::new("H.264 requires a CPU BGRA8 or RGBA8 frame"));
    }
    let expected = frame.width.checked_mul(frame.height).and_then(|n| n.checked_mul(4))
        .ok_or_else(|| ScreenShareError::new("frame dimensions overflow"))? as usize;
    if frame.pixels.len() != expected { return Err(ScreenShareError::new("frame payload does not match dimensions")); }
    let mut rgb = Vec::with_capacity(expected / 4 * 3);
    for pixel in frame.pixels.chunks_exact(4) {
        if frame.pixel_format == PixelFormat::Bgra8 { rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]); }
        else { rgb.extend_from_slice(&pixel[..3]); }
    }
    Ok(rgb)
}

fn scale_rgb(rgb: &[u8], source_width: u32, source_height: u32, width: u32, height: u32) -> Vec<u8> {
    let mut result = vec![0; width as usize * height as usize * 3];
    for y in 0..height { for x in 0..width {
        let sx = x as usize * source_width as usize / width as usize;
        let sy = y as usize * source_height as usize / height as usize;
        let from = (sy * source_width as usize + sx) * 3;
        let to = (y as usize * width as usize + x as usize) * 3;
        result[to..to + 3].copy_from_slice(&rgb[from..from + 3]);
    }}
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
        Ok(Self { encoder: make_encoder(config)?, config, generation: 0, sequence: 0,
            frames_since_keyframe: 0, keyframe_requested: true, shutdown: false })
    }
    pub fn default_profile() -> Result<Self, ScreenShareError> { Self::new(CodecConfig::default()) }

    fn ensure_running(&self) -> Result<(), ScreenShareError> {
        if self.shutdown { return Err(ScreenShareError::new("encoder is shut down")); }
        Ok(())
    }
}

fn make_encoder(config: CodecConfig) -> Result<openh264::encoder::Encoder, ScreenShareError> {
    use openh264::encoder::{BitRate, Complexity, EncoderConfig, IntraFramePeriod, QpRange, RateControlMode, UsageType};
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
    let settings = EncoderConfig::new().bitrate(BitRate::from_bps(config.target_bitrate_bps))
        .max_frame_rate(openh264::encoder::FrameRate::from_hz(config.target_fps as f32))
        .rate_control_mode(RateControlMode::Bitrate).usage_type(UsageType::ScreenContentRealTime)
        .complexity(complexity).qp(QpRange::new(0, qp_max)).skip_frames(false)
        .scene_change_detect(false).background_detection(false).long_term_reference(false)
        .intra_frame_period(IntraFramePeriod::from_num_frames(config.keyframe_interval as u32));
    // NOTE: skip_frames MUST stay false. With skipping enabled, a static
    // screen (nobody interacting with the host) makes the encoder emit no
    // decodable data after the first keyframe — the viewer then freezes on
    // that first frame ("it shows a screen I was on previously"). Every
    // captured frame must yield a P-frame, even when the content is
    // unchanged; the periodic intra_frame_period keyframe keeps the stream
    // self-recovering. Interactive video is better served by always-encoded
    // frames than by bitrate-optimized silence.
    openh264::encoder::Encoder::with_api_config(openh264::OpenH264API::from_source(), settings).map_err(fail)
}

impl VideoEncoder for OpenH264Encoder {
    fn configure(&mut self, config: CodecConfig) -> Result<(), ScreenShareError> {
        self.ensure_running()?;
        let config = config.validate()?;
        self.encoder = make_encoder(config)?; self.config = config; self.generation += 1;
        self.frames_since_keyframe = 0; self.keyframe_requested = true; Ok(())
    }

    fn encode(&mut self, frame: &CapturedFrame) -> Result<EncodedPacket, ScreenShareError> {
        self.ensure_running()?;
        if frame.width == 0 || frame.height == 0 { return Err(ScreenShareError::new("frame dimensions must be non-zero")); }
        let rgb = scale_rgb(&rgba_to_rgb(frame)?, frame.width, frame.height, self.config.width, self.config.height);
        if self.keyframe_requested || self.frames_since_keyframe >= self.config.keyframe_interval { self.encoder.force_intra_frame(); self.keyframe_requested = false; }
        let source = openh264::formats::RgbSliceU8::new(&rgb, (self.config.width as usize, self.config.height as usize));
        // Fast path: `from_rgb8_source` uses the integer `write_yuv_scalar`
        // converter. `from_rgb_source` goes through the f32 per-pixel
        // `write_yuv_by_pixel` path, which is dramatically slower at HD
        // resolutions (measured ~40ms extra per 1080p frame).
        let yuv = openh264::formats::YUVBuffer::from_rgb8_source(source);
        let stream = self.encoder.encode_at(&yuv, openh264::Timestamp::from_millis(frame.timestamp_us / 1_000)).map_err(fail)?;
        let keyframe = matches!(stream.frame_type(), openh264::encoder::FrameType::IDR | openh264::encoder::FrameType::I);
        if keyframe { self.frames_since_keyframe = 0; } else { self.frames_since_keyframe += 1; }
        let encoded = EncodedPacket { timestamp_us: frame.timestamp_us, sequence: self.sequence, keyframe,
            config_generation: self.generation, width: self.config.width, height: self.config.height, bytes: stream.to_vec() };
        self.sequence += 1;
        Ok(encoded)
    }

    fn force_keyframe(&mut self) { self.keyframe_requested = true; }

    fn reconfigure_bitrate(&mut self, bitrate_bps: u32) -> Result<(), ScreenShareError> {
        self.ensure_running()?;
        if bitrate_bps == 0 { return Err(ScreenShareError::new("bitrate must be non-zero")); }
        if bitrate_bps == self.config.target_bitrate_bps { return Ok(()); }
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

    fn metadata(&self) -> CodecMetadata { CodecMetadata { codec: CodecKind::H264, config: self.config, generation: self.generation } }
}

#[allow(missing_debug_implementations)]
pub struct OpenH264Decoder { decoder: openh264::decoder::Decoder, metadata: CodecMetadata, waiting_for_keyframe: bool }

impl OpenH264Decoder {
    pub fn new(config: CodecConfig) -> Result<Self, ScreenShareError> {
        let config = config.validate()?;
        Ok(Self { decoder: openh264::decoder::Decoder::new().map_err(fail)?, metadata: CodecMetadata { codec: CodecKind::H264, config, generation: 0 }, waiting_for_keyframe: false })
    }
    pub fn default_profile() -> Result<Self, ScreenShareError> { Self::new(CodecConfig::default()) }
}

impl VideoDecoder for OpenH264Decoder {
    fn decode(&mut self, frame: &EncodedFrame) -> Result<Option<CapturedFrame>, ScreenShareError> {
        if frame.bytes.is_empty() || (self.waiting_for_keyframe && !frame.keyframe) { return Ok(None); }
        if frame.config_generation != self.metadata.generation {
            self.metadata.generation = frame.config_generation; self.metadata.config.width = frame.width; self.metadata.config.height = frame.height;
            self.decoder = openh264::decoder::Decoder::new().map_err(fail)?; self.waiting_for_keyframe = !frame.keyframe;
            if self.waiting_for_keyframe { return Ok(None); }
        }
        let Some(yuv) = self.decoder.decode(&frame.bytes).map_err(fail)? else { return Ok(None); };
        use openh264::formats::YUVSource;
        let (width, height) = yuv.dimensions(); let mut rgb = vec![0; yuv.rgb8_len()]; yuv.write_rgb8(&mut rgb);
        let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
        for pixel in rgb.chunks_exact(3) { rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]); }
        Ok(Some(CapturedFrame {
            timestamp_us: frame.timestamp_us,
            width: width as u32,
            height: height as u32,
            pixel_format: PixelFormat::Rgba8,
            stride: width as u32 * 4,
            pixels: rgba,
            gpu_handle: None,
            dirty_region: None,
        }))
    }
    fn metadata(&self) -> CodecMetadata { self.metadata }
    fn reset(&mut self) -> Result<(), ScreenShareError> { self.decoder = openh264::decoder::Decoder::new().map_err(fail)?; self.waiting_for_keyframe = true; Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(width: u32, height: u32) -> CodecConfig { CodecConfig { width, height, target_fps: 30, target_bitrate_bps: 400_000, keyframe_interval: 4, max_queue_depth: 2, quality_profile: QualityProfile::Balanced } }
    fn pattern(width: u32, height: u32, timestamp_us: u64) -> CapturedFrame {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height { for x in 0..width { pixels.extend_from_slice(&[x as u8, y as u8, (x ^ y) as u8, 255]); }}
        CapturedFrame::cpu(timestamp_us, width, height, PixelFormat::Rgba8, pixels).unwrap()
    }
    #[test]
    fn synthetic_pattern_round_trips_with_bounded_metadata() {
        let cfg = config(32, 24); let source = pattern(32, 24, 1000);
        let mut encoder = OpenH264Encoder::new(cfg).unwrap(); let encoded = encoder.encode(&source).unwrap();
        assert!(encoded.keyframe && encoded.sequence == 0 && !encoded.bytes.is_empty());
        let mut decoder = OpenH264Decoder::new(cfg).unwrap(); let decoded = decoder.decode(&encoded).unwrap().unwrap();
        assert_eq!((decoded.width, decoded.height), (32, 24)); assert_eq!(decoded.pixels.len(), source.pixels.len());
        assert_ne!(decoded.pixels.iter().fold(0u64, |sum, b| sum + *b as u64), 0);
    }
    #[test]
    fn request_reset_and_reconfigure_are_explicit() {
        let mut encoder = OpenH264Encoder::new(config(16, 16)).unwrap(); encoder.encode(&pattern(16, 16, 0)).unwrap();
        encoder.request_keyframe(); assert!(encoder.encode(&pattern(16, 16, 1)).unwrap().keyframe);
        encoder.reconfigure(config(24, 16)).unwrap(); assert_eq!(encoder.metadata().generation, 1);
        let frame = encoder.encode(&pattern(24, 16, 2)).unwrap(); assert!(frame.keyframe);
        let mut decoder = OpenH264Decoder::new(config(24, 16)).unwrap(); decoder.reset().unwrap(); assert!(decoder.decode(&frame).unwrap().is_some());
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
            assert!(!encoded.bytes.is_empty(), "static frame {tick} must not be skipped");
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
        assert!(third.keyframe, "force_keyframe must make the next unit a keyframe");
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
        assert_eq!(encoder.metadata().generation, gen_before, "bitrate change must not bump config generation");
        assert_eq!((encoder.metadata().config.width, encoder.metadata().config.height), (32, 24));
        let next = encoder.encode(&pattern(32, 24, 33_333)).unwrap();
        assert!(next.keyframe, "bitrate reconfigure forces a keyframe for re-sync");
        assert!(decoder.decode(&next).unwrap().is_some(), "stream must stay decodable after bitrate change");
        // Same-bitrate reconfigure is a no-op.
        encoder.reconfigure_bitrate(1_200_000).unwrap();
        assert_eq!(encoder.metadata().config.target_bitrate_bps, 1_200_000);
    }
    #[test]
    fn configure_changes_resolution_without_session_restart() {
        let mut encoder = OpenH264Encoder::new(config(32, 24)).unwrap();
        encoder.encode(&pattern(32, 24, 0)).unwrap();
        encoder.configure(config(48, 32)).unwrap();
        assert_eq!(encoder.metadata().generation, 1, "resolution change bumps generation");
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
                self.config = config; self.configured.push(config); Ok(())
            }
            fn encode(&mut self, frame: &CapturedFrame) -> Result<EncodedPacket, ScreenShareError> {
                Ok(EncodedPacket { timestamp_us: frame.timestamp_us, sequence: 0, keyframe: true,
                    config_generation: 0, width: self.config.width, height: self.config.height,
                    bytes: vec![1] })
            }
            fn force_keyframe(&mut self) { self.keyframes += 1; }
            fn reconfigure_bitrate(&mut self, bitrate_bps: u32) -> Result<(), ScreenShareError> {
                self.bitrates.push(bitrate_bps); Ok(())
            }
            fn shutdown(&mut self) -> Result<(), ScreenShareError> { self.shutdowns += 1; Ok(()) }
            fn metadata(&self) -> CodecMetadata {
                CodecMetadata { codec: CodecKind::H264, config: self.config, generation: 0 }
            }
        }
        let mut encoder = MockEncoder::default();
        encoder.request_keyframe();
        assert_eq!(encoder.keyframes, 1, "request_keyframe delegates to force_keyframe");
        encoder.reconfigure(config(16, 16)).unwrap();
        assert_eq!(encoder.configured.len(), 1, "reconfigure delegates to configure");
        encoder.reconfigure_bitrate(300_000).unwrap();
        assert_eq!(encoder.bitrates, vec![300_000]);
        encoder.reset().unwrap();
        assert_eq!(encoder.configured.len(), 2, "reset reconfigures with the active config");
        encoder.shutdown().unwrap();
        encoder.shutdown().unwrap();
        assert_eq!(encoder.shutdowns, 2, "shutdown is idempotent");
    }

    #[test]
    fn quality_profile_round_trips_through_wire_value() {
        for profile in [QualityProfile::Balanced, QualityProfile::LowLatency, QualityProfile::HighQuality] {
            assert_eq!(QualityProfile::from_u8(profile.as_u8()), Some(profile));
            assert!(!profile.name().is_empty());
        }
        assert_eq!(QualityProfile::from_u8(9), None, "unknown wire value must be rejected");
        assert_eq!(QualityProfile::default(), QualityProfile::Balanced);
    }

    #[test]
    fn target_profiles_expose_720p30_and_1080p30() {
        let p720 = CodecConfig::profile_720p30();
        assert_eq!((p720.width, p720.height, p720.target_fps), (1280, 720, 30));
        assert_eq!(p720.target_bitrate_bps, TARGET_720P30_BITRATE_BPS);
        let p1080 = CodecConfig::profile_1080p30();
        assert_eq!((p1080.width, p1080.height, p1080.target_fps), (1920, 1080, 30));
        assert_eq!(p1080.target_bitrate_bps, TARGET_1080P30_BITRATE_BPS);
        // Both are valid encoder configs (validation passes).
        assert!(p720.validate().is_ok());
        assert!(p1080.validate().is_ok());
    }

    #[test]
    fn every_quality_profile_constructs_and_encodes_decodable_frames() {
        for profile in [QualityProfile::LowLatency, QualityProfile::Balanced, QualityProfile::HighQuality] {
            let cfg = CodecConfig { quality_profile: profile, ..config(32, 24) };
            let mut encoder = OpenH264Encoder::new(cfg).unwrap();
            let mut decoder = OpenH264Decoder::new(cfg).unwrap();
            let first = encoder.encode(&pattern(32, 24, 0)).unwrap();
            assert!(first.keyframe && !first.bytes.is_empty(), "{profile:?} keyframe must encode");
            assert!(decoder.decode(&first).unwrap().is_some(), "{profile:?} keyframe must decode");
            let second = encoder.encode(&pattern(32, 24, 33_333)).unwrap();
            assert!(!second.bytes.is_empty(), "{profile:?} delta frame must encode");
            assert!(decoder.decode(&second).unwrap().is_some(), "{profile:?} delta frame must decode");
        }
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
}
