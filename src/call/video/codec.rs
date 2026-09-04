//! Codec-independent boundary for live-call video.
//!
//! The public frame types intentionally contain only owned bytes and video
//! dimensions.  OpenH264 is an implementation detail and can be replaced by a
//! hardware H.264 or AV1 implementation without changing call signalling.

use anyhow::{anyhow, Result};

use super::config::{VideoConfig, VideoProfile};

/// Typed failures from codec lifecycle operations.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    Backend(String),
    InvalidConfiguration(String),
    AlreadyShutdown,
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backend(error) => write!(f, "codec backend error: {error}"),
            Self::InvalidConfiguration(error) => write!(f, "invalid codec configuration: {error}"),
            Self::AlreadyShutdown => f.write_str("codec is already shut down"),
        }
    }
}

impl std::error::Error for CodecError {}

/// Capabilities exposed without leaking a vendor backend type.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecCapabilities {
    pub codec: VideoCodec,
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u32,
    pub hardware_accelerated: bool,
}

/// Owned metadata associated with an encoded access unit.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrameMetadata {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub timestamp_us: u64,
    pub keyframe: bool,
}

/// Live camera profile: 640x360 (360p), 24 frames per second, and a
/// bitrate deliberately centered in the requested 400–800 kbps range.
pub const VIDEO_WIDTH: u32 = VideoProfile::Q0.config().width;
/// Height of the live camera profile in pixels.
pub const VIDEO_HEIGHT: u32 = VideoProfile::Q0.config().height;
/// Frame rate of the live camera profile.
pub const VIDEO_FRAMES_PER_SECOND: u32 = VideoProfile::Q0.config().fps;
/// Target bitrate for the live camera profile, in bits per second.
pub const VIDEO_TARGET_BITRATE_BPS: u32 = VideoProfile::Q0.config().bitrate.target_bps;
/// Maximum interval between periodic keyframes, in encoded frames.
pub const VIDEO_KEYFRAME_INTERVAL_FRAMES: u64 = VideoProfile::Q0.config().keyframe_interval as u64;

/// Codec negotiated for a live video track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    /// H.264/AVC elementary stream.
    H264,
}

/// A raw RGB video frame presented to an encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawVideoFrame {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Monotonic presentation timestamp in microseconds.
    pub timestamp_us: u64,
    /// Packed RGB8 bytes, one pixel per three bytes, row-major.
    pub rgb: Vec<u8>,
}

impl RawVideoFrame {
    fn validate(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 || self.width % 2 != 0 || self.height % 2 != 0 {
            return Err(anyhow!("video dimensions must be non-zero even values"));
        }
        let expected = self.width as usize * self.height as usize * 3;
        if self.rgb.len() != expected {
            return Err(anyhow!(
                "RGB frame has {} bytes, expected {expected}",
                self.rgb.len()
            ));
        }
        Ok(())
    }
}

/// An encoded H.264 access unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedVideoFrame {
    /// Codec that produced the access unit.
    pub codec: VideoCodec,
    /// Encoded frame width in pixels.
    pub width: u32,
    /// Encoded frame height in pixels.
    pub height: u32,
    /// Presentation timestamp copied from the raw frame.
    pub timestamp_us: u64,
    /// Whether this access unit is independently decodable.
    pub keyframe: bool,
    /// Owned codec bytes.
    pub bytes: Vec<u8>,
}

impl EncodedVideoFrame {
    /// Return an owned, codec-neutral description of this access unit.
    pub fn metadata(&self) -> VideoFrameMetadata {
        VideoFrameMetadata {
            codec: self.codec,
            width: self.width,
            height: self.height,
            timestamp_us: self.timestamp_us,
            keyframe: self.keyframe,
        }
    }
}

/// A decoded RGB8 video frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedVideoFrame {
    /// Decoded frame width in pixels.
    pub width: u32,
    /// Decoded frame height in pixels.
    pub height: u32,
    /// Packed RGB8 bytes, row-major.
    pub bytes: Vec<u8>,
}

/// Codec-independent video encoder interface.
#[allow(missing_docs)]
pub trait VideoEncoder: Send {
    fn configure(&mut self, _config: VideoConfig) -> std::result::Result<(), CodecError> {
        Ok(())
    }
    /// Empty output is valid while a backend warms up.
    fn encode(
        &mut self,
        frame: &RawVideoFrame,
    ) -> std::result::Result<Vec<EncodedVideoFrame>, CodecError>;
    /// Request an intra frame.
    fn force_keyframe(&mut self) {}
    fn reconfigure(&mut self, config: VideoConfig) -> std::result::Result<(), CodecError> {
        self.configure(config)
    }
    fn metadata(&self) -> VideoConfig {
        VideoProfile::Q0.config()
    }
    fn capabilities(&self) -> CodecCapabilities {
        CodecCapabilities {
            codec: VideoCodec::H264,
            max_width: crate::call::bounds::MAX_VIDEO_WIDTH,
            max_height: crate::call::bounds::MAX_VIDEO_HEIGHT,
            max_fps: crate::call::bounds::MAX_VIDEO_FPS,
            hardware_accelerated: false,
        }
    }
    fn reset(&mut self) -> std::result::Result<(), CodecError> {
        Ok(())
    }
    fn shutdown(&mut self) -> std::result::Result<(), CodecError> {
        Ok(())
    }
    fn request_keyframe(&mut self) {
        self.force_keyframe();
    }
}

/// Codec-independent video decoder interface.
#[allow(missing_docs)]
pub trait VideoDecoder: Send {
    fn configure(&mut self, config: VideoConfig) -> std::result::Result<(), CodecError>;
    /// Empty output is valid while a backend warms up or buffers input.
    fn decode(&mut self, frame: &[u8]) -> std::result::Result<Vec<DecodedVideoFrame>, CodecError>;
    fn metadata(&self) -> VideoConfig {
        VideoProfile::Q0.config()
    }
    fn capabilities(&self) -> CodecCapabilities {
        CodecCapabilities {
            codec: VideoCodec::H264,
            max_width: crate::call::bounds::MAX_VIDEO_WIDTH,
            max_height: crate::call::bounds::MAX_VIDEO_HEIGHT,
            max_fps: crate::call::bounds::MAX_VIDEO_FPS,
            hardware_accelerated: false,
        }
    }
    fn reset(&mut self) -> std::result::Result<(), CodecError> {
        Ok(())
    }
    fn shutdown(&mut self) -> std::result::Result<(), CodecError> {
        Ok(())
    }
}

/// OpenH264-backed H.264 encoder.
#[allow(missing_debug_implementations)]
pub struct OpenH264Encoder {
    encoder: openh264::encoder::Encoder,
    keyframe_requested: bool,
    frames_since_keyframe: u64,
    config: VideoConfig,
    shutdown: bool,
}

impl OpenH264Encoder {
    fn make_encoder(
        config: VideoConfig,
    ) -> std::result::Result<openh264::encoder::Encoder, CodecError> {
        use openh264::encoder::{
            BitRate, Complexity, EncoderConfig, IntraFramePeriod, RateControlMode, UsageType,
        };
        let settings = EncoderConfig::new()
            .bitrate(BitRate::from_bps(config.bitrate.target_bps))
            .max_frame_rate(openh264::encoder::FrameRate::from_hz(config.fps as f32))
            .rate_control_mode(RateControlMode::Bitrate)
            .usage_type(UsageType::CameraVideoRealTime)
            .complexity(Complexity::Low)
            .skip_frames(true)
            .scene_change_detect(false)
            .background_detection(false)
            .long_term_reference(false)
            .intra_frame_period(IntraFramePeriod::from_num_frames(config.keyframe_interval));
        openh264::encoder::Encoder::with_api_config(openh264::OpenH264API::from_source(), settings)
            .map_err(|error| CodecError::Backend(error.to_string()))
    }

    /// Create an encoder using OpenH264's default settings.
    pub fn new() -> Result<Self> {
        let video_config = VideoProfile::Q0.config();
        Ok(Self {
            encoder: Self::make_encoder(video_config).map_err(|error| anyhow!(error))?,
            // The first frame is explicitly forced below rather than relying
            // on OpenH264's implicit initial IDR behavior.
            keyframe_requested: true,
            frames_since_keyframe: 0,
            config: video_config,
            shutdown: false,
        })
    }
}

impl VideoEncoder for OpenH264Encoder {
    fn configure(&mut self, config: VideoConfig) -> std::result::Result<(), CodecError> {
        if self.shutdown {
            return Err(CodecError::AlreadyShutdown);
        }
        config
            .validate()
            .map_err(|e| CodecError::InvalidConfiguration(e.to_string()))?;
        let changes = config.changes_from(self.config);
        if !changes.any() {
            return Ok(());
        }
        if changes.geometry || changes.codec {
            return Err(CodecError::InvalidConfiguration(
                "geometry and codec changes require video-call v2".to_string(),
            ));
        }
        // Build before swapping so backend failures preserve the old encoder.
        let replacement = Self::make_encoder(config)?;
        self.encoder = replacement;
        self.config = config;
        self.frames_since_keyframe = 0;
        self.keyframe_requested = true;
        Ok(())
    }

    fn encode(
        &mut self,
        frame: &RawVideoFrame,
    ) -> std::result::Result<Vec<EncodedVideoFrame>, CodecError> {
        if self.shutdown {
            return Err(CodecError::AlreadyShutdown);
        }
        frame
            .validate()
            .map_err(|e| CodecError::Backend(e.to_string()))?;
        if self.keyframe_requested
            || self.frames_since_keyframe >= self.config.keyframe_interval as u64
        {
            self.encoder.force_intra_frame();
            self.keyframe_requested = false;
        }
        let source = openh264::formats::RgbSliceU8::new(
            &frame.rgb,
            (frame.width as usize, frame.height as usize),
        );
        let yuv = openh264::formats::YUVBuffer::from_rgb_source(source);
        let stream = self
            .encoder
            .encode_at(
                &yuv,
                openh264::Timestamp::from_millis(frame.timestamp_us / 1_000),
            )
            .map_err(|e| CodecError::Backend(e.to_string()))?;
        let bytes = stream.to_vec();
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        let keyframe = matches!(
            stream.frame_type(),
            openh264::encoder::FrameType::IDR | openh264::encoder::FrameType::I
        );
        if keyframe {
            self.frames_since_keyframe = 0;
        } else {
            self.frames_since_keyframe = self.frames_since_keyframe.saturating_add(1);
        }
        Ok(vec![EncodedVideoFrame {
            codec: VideoCodec::H264,
            width: frame.width,
            height: frame.height,
            timestamp_us: frame.timestamp_us,
            keyframe,
            bytes: stream.to_vec(),
        }])
    }

    fn force_keyframe(&mut self) {
        self.keyframe_requested = true;
    }
    fn metadata(&self) -> VideoConfig {
        self.config
    }
    fn capabilities(&self) -> CodecCapabilities {
        CodecCapabilities {
            codec: VideoCodec::H264,
            max_width: crate::call::bounds::MAX_VIDEO_WIDTH,
            max_height: crate::call::bounds::MAX_VIDEO_HEIGHT,
            max_fps: crate::call::bounds::MAX_VIDEO_FPS,
            hardware_accelerated: false,
        }
    }
    fn reset(&mut self) -> std::result::Result<(), CodecError> {
        if self.shutdown {
            return Err(CodecError::AlreadyShutdown);
        }
        self.frames_since_keyframe = 0;
        self.keyframe_requested = true;
        Ok(())
    }
    fn shutdown(&mut self) -> std::result::Result<(), CodecError> {
        if self.shutdown {
            return Err(CodecError::AlreadyShutdown);
        }
        self.shutdown = true;
        Ok(())
    }
}

/// OpenH264-backed H.264 decoder.
#[allow(missing_debug_implementations)]
pub struct OpenH264Decoder {
    decoder: openh264::decoder::Decoder,
    config: VideoConfig,
    shutdown: bool,
}

impl OpenH264Decoder {
    /// Create a decoder using OpenH264's default settings.
    pub fn new() -> Result<Self> {
        Ok(Self {
            decoder: openh264::decoder::Decoder::new()?,
            config: VideoProfile::Q0.config(),
            shutdown: false,
        })
    }
}

impl VideoDecoder for OpenH264Decoder {
    fn configure(&mut self, config: VideoConfig) -> std::result::Result<(), CodecError> {
        config
            .validate()
            .map_err(|e| CodecError::InvalidConfiguration(e.to_string()))?;
        self.config = config;
        Ok(())
    }
    fn decode(&mut self, frame: &[u8]) -> std::result::Result<Vec<DecodedVideoFrame>, CodecError> {
        if self.shutdown {
            return Err(CodecError::AlreadyShutdown);
        }
        if frame.is_empty() {
            return Ok(Vec::new());
        }
        let Some(yuv) = self
            .decoder
            .decode(frame)
            .map_err(|e| CodecError::Backend(e.to_string()))?
        else {
            return Ok(Vec::new());
        };
        use openh264::formats::YUVSource;
        let (width, height) = yuv.dimensions();
        let mut bytes = vec![0; yuv.rgb8_len()];
        yuv.write_rgb8(&mut bytes);
        Ok(vec![DecodedVideoFrame {
            width: width as u32,
            height: height as u32,
            bytes,
        }])
    }
    fn metadata(&self) -> VideoConfig {
        self.config
    }
    fn capabilities(&self) -> CodecCapabilities {
        CodecCapabilities {
            codec: VideoCodec::H264,
            max_width: crate::call::bounds::MAX_VIDEO_WIDTH,
            max_height: crate::call::bounds::MAX_VIDEO_HEIGHT,
            max_fps: crate::call::bounds::MAX_VIDEO_FPS,
            hardware_accelerated: false,
        }
    }
    fn reset(&mut self) -> std::result::Result<(), CodecError> {
        if self.shutdown {
            return Err(CodecError::AlreadyShutdown);
        }
        Ok(())
    }
    fn shutdown(&mut self) -> std::result::Result<(), CodecError> {
        if self.shutdown {
            return Err(CodecError::AlreadyShutdown);
        }
        self.shutdown = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OpenH264Decoder, OpenH264Encoder, RawVideoFrame, VideoDecoder, VideoEncoder,
        VIDEO_FRAMES_PER_SECOND, VIDEO_KEYFRAME_INTERVAL_FRAMES, VIDEO_TARGET_BITRATE_BPS,
    };
    use crate::call::video::VideoProfile;

    fn frame(timestamp_us: u64) -> RawVideoFrame {
        RawVideoFrame {
            width: 16,
            height: 16,
            timestamp_us,
            rgb: (0..16 * 16 * 3).map(|value| value as u8).collect(),
        }
    }

    #[test]
    fn openh264_encode_decode_round_trip() {
        let mut encoder = OpenH264Encoder::new().expect("encoder");
        let encoded = encoder
            .encode(&frame(33_000))
            .expect("encoded frame")
            .into_iter()
            .next()
            .expect("access unit");
        assert!(!encoded.bytes.is_empty());
        assert!(encoded.keyframe);

        let mut decoder = OpenH264Decoder::new().expect("decoder");
        let decoded = decoder
            .decode(&encoded.bytes)
            .expect("decoded frame")
            .into_iter()
            .next()
            .expect("picture available");
        assert_eq!((decoded.width, decoded.height), (16, 16));
        assert_eq!(decoded.bytes.len(), 16 * 16 * 3);
    }

    #[test]
    fn request_keyframe_forces_next_access_unit_to_be_intra() {
        let mut encoder = OpenH264Encoder::new().expect("encoder");
        let _ = encoder.encode(&frame(0)).expect("first frame");
        encoder.request_keyframe();
        let encoded = encoder
            .encode(&frame(33_000))
            .expect("keyframe")
            .into_iter()
            .next()
            .expect("access unit");
        assert!(encoded.keyframe);
    }

    #[test]
    fn profile_has_realtime_camera_defaults() {
        assert_eq!(VIDEO_FRAMES_PER_SECOND, 24);
        assert_eq!(VIDEO_TARGET_BITRATE_BPS, 600_000);
        assert_eq!(VIDEO_KEYFRAME_INTERVAL_FRAMES, 48);
    }

    #[test]
    fn periodic_keyframe_is_emitted_within_two_seconds() {
        let mut encoder = OpenH264Encoder::new().expect("encoder");
        assert!(
            encoder
                .encode(&frame(0))
                .expect("first frame")
                .into_iter()
                .next()
                .expect("access unit")
                .keyframe
        );

        let mut periodic = None;
        for index in 1..=VIDEO_KEYFRAME_INTERVAL_FRAMES {
            let encoded = encoder
                .encode(&frame(index * 1_000_000 / VIDEO_FRAMES_PER_SECOND as u64))
                .expect("encoded frame")
                .into_iter()
                .next()
                .expect("access unit");
            if encoded.keyframe {
                periodic = Some(encoded.timestamp_us);
                break;
            }
        }
        let timestamp = periodic.expect("periodic keyframe");
        assert!(timestamp > 0);
        assert!(timestamp <= 2_000_000);
    }

    #[test]
    fn empty_input_does_not_create_a_decoded_frame() {
        let mut decoder = OpenH264Decoder::new().expect("decoder");
        assert!(decoder.decode(&[]).expect("empty input").is_empty());
    }

    #[test]
    fn rate_and_fps_reconfigure_rebuilds_and_forces_keyframe() {
        let mut encoder = OpenH264Encoder::new().expect("encoder");
        let mut config = VideoProfile::Q0.config();
        config.bitrate.target_bps = 300_000;
        config.bitrate.peak_bps = 400_000;
        config.fps = 20;
        config.keyframe_interval = 40;
        encoder.configure(config).expect("reconfigure");
        assert_eq!(encoder.metadata(), config);
        let encoded = encoder
            .encode(&frame(33_000))
            .expect("encoded frame")
            .into_iter()
            .next()
            .expect("access unit");
        assert!(encoded.keyframe);
    }

    #[test]
    fn identical_configuration_is_a_no_op_and_geometry_is_v2_gated() {
        let mut encoder = OpenH264Encoder::new().expect("encoder");
        let config = VideoProfile::Q0.config();
        encoder.configure(config).expect("identical config");
        let mut geometry = config;
        geometry.width = 480;
        assert!(matches!(
            encoder.configure(geometry),
            Err(super::CodecError::InvalidConfiguration(_))
        ));
        assert_eq!(encoder.metadata(), config);
    }

    #[test]
    fn bitrate_downshift_and_recovery_remain_keyframed_and_decodable() {
        let mut encoder = OpenH264Encoder::new().expect("encoder");
        let mut decoder = OpenH264Decoder::new().expect("decoder");
        let high = VideoProfile::Q0.config();
        let mut low = high;
        low.bitrate.target_bps = 300_000;
        low.bitrate.peak_bps = 400_000;

        for (index, config) in [high, low, high].into_iter().enumerate() {
            encoder.configure(config).expect("reconfigure");
            let encoded = encoder
                .encode(&frame(index as u64 * 33_000))
                .expect("encoded frame")
                .into_iter()
                .next()
                .expect("access unit");
            assert!(encoded.keyframe, "reconfiguration {index} must force IDR");
            assert!(
                decoder
                    .decode(&encoded.bytes)
                    .expect("decode")
                    .into_iter()
                    .next()
                    .is_some(),
                "reconfiguration {index} must remain decodable"
            );
        }
    }
}
