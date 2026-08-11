//! Replaceable codec boundary and the initial low-latency H.264 implementation.
#![allow(missing_docs)]

use super::{capture::{CapturedFrame, PixelFormat}, ScreenShareError};

pub const DEFAULT_WIDTH: u32 = 1920;
pub const DEFAULT_HEIGHT: u32 = 1080;
pub const DEFAULT_FPS: u32 = 30;
pub const DEFAULT_BITRATE_BPS: u32 = 4_000_000;
pub const DEFAULT_KEYFRAME_INTERVAL: u64 = 60;
pub const DEFAULT_QUEUE_CAPACITY: usize = 2;

/// Encoder/decoder configuration negotiated for a screen stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecConfig {
    pub width: u32,
    pub height: u32,
    pub target_fps: u32,
    pub target_bitrate_bps: u32,
    pub keyframe_interval: u64,
    pub max_queue_depth: usize,
}

impl Default for CodecConfig {
    fn default() -> Self {
        Self { width: DEFAULT_WIDTH, height: DEFAULT_HEIGHT, target_fps: DEFAULT_FPS,
            target_bitrate_bps: DEFAULT_BITRATE_BPS, keyframe_interval: DEFAULT_KEYFRAME_INTERVAL,
            max_queue_depth: DEFAULT_QUEUE_CAPACITY }
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecMetadata { pub codec: CodecKind, pub config: CodecConfig, pub generation: u64 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecKind { H264 }

/// Encoded screen frame passed to a transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    pub timestamp_us: u64,
    pub sequence: u64,
    pub keyframe: bool,
    pub config_generation: u64,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

pub trait VideoEncoder: Send {
    fn encode(&mut self, frame: &CapturedFrame) -> Result<EncodedFrame, ScreenShareError>;
    fn metadata(&self) -> CodecMetadata;
    fn request_keyframe(&mut self);
    fn reconfigure(&mut self, config: CodecConfig) -> Result<(), ScreenShareError>;
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
}

impl OpenH264Encoder {
    pub fn new(config: CodecConfig) -> Result<Self, ScreenShareError> {
        let config = config.validate()?;
        Ok(Self { encoder: make_encoder(config)?, config, generation: 0, sequence: 0,
            frames_since_keyframe: 0, keyframe_requested: true })
    }
    pub fn default_profile() -> Result<Self, ScreenShareError> { Self::new(CodecConfig::default()) }
}

fn make_encoder(config: CodecConfig) -> Result<openh264::encoder::Encoder, ScreenShareError> {
    use openh264::encoder::{BitRate, Complexity, EncoderConfig, IntraFramePeriod, RateControlMode, UsageType};
    let settings = EncoderConfig::new().bitrate(BitRate::from_bps(config.target_bitrate_bps))
        .max_frame_rate(openh264::encoder::FrameRate::from_hz(config.target_fps as f32))
        .rate_control_mode(RateControlMode::Bitrate).usage_type(UsageType::CameraVideoRealTime)
        .complexity(Complexity::Low).skip_frames(false).scene_change_detect(false)
        .background_detection(false).long_term_reference(false)
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
    fn encode(&mut self, frame: &CapturedFrame) -> Result<EncodedFrame, ScreenShareError> {
        if frame.width == 0 || frame.height == 0 { return Err(ScreenShareError::new("frame dimensions must be non-zero")); }
        let rgb = scale_rgb(&rgba_to_rgb(frame)?, frame.width, frame.height, self.config.width, self.config.height);
        if self.keyframe_requested || self.frames_since_keyframe >= self.config.keyframe_interval { self.encoder.force_intra_frame(); self.keyframe_requested = false; }
        let source = openh264::formats::RgbSliceU8::new(&rgb, (self.config.width as usize, self.config.height as usize));
        let yuv = openh264::formats::YUVBuffer::from_rgb_source(source);
        let stream = self.encoder.encode_at(&yuv, openh264::Timestamp::from_millis(frame.timestamp_us / 1_000)).map_err(fail)?;
        let keyframe = matches!(stream.frame_type(), openh264::encoder::FrameType::IDR | openh264::encoder::FrameType::I);
        if keyframe { self.frames_since_keyframe = 0; } else { self.frames_since_keyframe += 1; }
        let encoded = EncodedFrame { timestamp_us: frame.timestamp_us, sequence: self.sequence, keyframe,
            config_generation: self.generation, width: self.config.width, height: self.config.height, bytes: stream.to_vec() };
        self.sequence += 1;
        Ok(encoded)
    }
    fn metadata(&self) -> CodecMetadata { CodecMetadata { codec: CodecKind::H264, config: self.config, generation: self.generation } }
    fn request_keyframe(&mut self) { self.keyframe_requested = true; }
    fn reconfigure(&mut self, config: CodecConfig) -> Result<(), ScreenShareError> {
        let config = config.validate()?;
        self.encoder = make_encoder(config)?; self.config = config; self.generation += 1;
        self.frames_since_keyframe = 0; self.keyframe_requested = true; Ok(())
    }
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
        Ok(Some(CapturedFrame { timestamp_us: frame.timestamp_us, width: width as u32, height: height as u32, pixel_format: PixelFormat::Rgba8, pixels: rgba, gpu_handle: None }))
    }
    fn metadata(&self) -> CodecMetadata { self.metadata }
    fn reset(&mut self) -> Result<(), ScreenShareError> { self.decoder = openh264::decoder::Decoder::new().map_err(fail)?; self.waiting_for_keyframe = true; Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(width: u32, height: u32) -> CodecConfig { CodecConfig { width, height, target_fps: 30, target_bitrate_bps: 400_000, keyframe_interval: 4, max_queue_depth: 2 } }
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
        // advance, exactly like the X11 capture source does).
        let first = encoder.encode(&pattern(64, 48, 0)).unwrap();
        assert!(first.keyframe && !first.bytes.is_empty());
        let mut decoded = 0;
        for tick in 1..=5 {
            let encoded = encoder.encode(&pattern(64, 48, tick)).unwrap();
            assert!(!encoded.bytes.is_empty(), "static frame {tick} must not be skipped");
            if decoder.decode(&encoded).unwrap().is_some() {
                decoded += 1;
            }
        }
        assert_eq!(decoded, 5, "every static frame must decode");
    }
}
