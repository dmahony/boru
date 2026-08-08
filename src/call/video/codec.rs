//! Codec-independent boundary for live-call video.
//!
//! The public frame types intentionally contain only owned bytes and video
//! dimensions.  OpenH264 is an implementation detail and can be replaced by a
//! hardware H.264 or AV1 implementation without changing call signalling.

use anyhow::{anyhow, Result};

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
pub trait VideoEncoder: Send {
    /// Encode one raw frame.
    fn encode(&mut self, frame: &RawVideoFrame) -> Result<EncodedVideoFrame>;
    /// Request that the next encoded frame be intra-coded.
    fn request_keyframe(&mut self);
}

/// Codec-independent video decoder interface.
pub trait VideoDecoder: Send {
    /// Decode one access unit, returning `None` while the decoder buffers input.
    fn decode(&mut self, frame: &[u8]) -> Result<Option<DecodedVideoFrame>>;
}

/// OpenH264-backed H.264 encoder.
#[allow(missing_debug_implementations)]
pub struct OpenH264Encoder {
    encoder: openh264::encoder::Encoder,
    keyframe_requested: bool,
}

impl OpenH264Encoder {
    /// Create an encoder using OpenH264's default settings.
    pub fn new() -> Result<Self> {
        Ok(Self {
            encoder: openh264::encoder::Encoder::new()?,
            keyframe_requested: false,
        })
    }
}

impl VideoEncoder for OpenH264Encoder {
    fn encode(&mut self, frame: &RawVideoFrame) -> Result<EncodedVideoFrame> {
        frame.validate()?;
        if self.keyframe_requested {
            self.encoder.force_intra_frame();
            self.keyframe_requested = false;
        }

        let source = openh264::formats::RgbSliceU8::new(
            &frame.rgb,
            (frame.width as usize, frame.height as usize),
        );
        let yuv = openh264::formats::YUVBuffer::from_rgb_source(source);
        let stream = self.encoder.encode_at(
            &yuv,
            openh264::Timestamp::from_millis(frame.timestamp_us / 1_000),
        )?;
        let keyframe = matches!(
            stream.frame_type(),
            openh264::encoder::FrameType::IDR | openh264::encoder::FrameType::I
        );
        Ok(EncodedVideoFrame {
            codec: VideoCodec::H264,
            width: frame.width,
            height: frame.height,
            timestamp_us: frame.timestamp_us,
            keyframe,
            bytes: stream.to_vec(),
        })
    }

    fn request_keyframe(&mut self) {
        self.keyframe_requested = true;
    }
}

/// OpenH264-backed H.264 decoder.
#[allow(missing_debug_implementations)]
pub struct OpenH264Decoder {
    decoder: openh264::decoder::Decoder,
}

impl OpenH264Decoder {
    /// Create a decoder using OpenH264's default settings.
    pub fn new() -> Result<Self> {
        Ok(Self {
            decoder: openh264::decoder::Decoder::new()?,
        })
    }
}

impl VideoDecoder for OpenH264Decoder {
    fn decode(&mut self, frame: &[u8]) -> Result<Option<DecodedVideoFrame>> {
        if frame.is_empty() {
            return Ok(None);
        }
        let Some(yuv) = self.decoder.decode(frame)? else {
            return Ok(None);
        };
        use openh264::formats::YUVSource;

        let (width, height) = yuv.dimensions();
        let mut bytes = vec![0; yuv.rgb8_len()];
        yuv.write_rgb8(&mut bytes);
        Ok(Some(DecodedVideoFrame {
            width: width as u32,
            height: height as u32,
            bytes,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{OpenH264Decoder, OpenH264Encoder, RawVideoFrame, VideoDecoder, VideoEncoder};

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
        let encoded = encoder.encode(&frame(33_000)).expect("encoded frame");
        assert!(!encoded.bytes.is_empty());
        assert!(encoded.keyframe);

        let mut decoder = OpenH264Decoder::new().expect("decoder");
        let decoded = decoder
            .decode(&encoded.bytes)
            .expect("decoded frame")
            .expect("picture available");
        assert_eq!((decoded.width, decoded.height), (16, 16));
        assert_eq!(decoded.bytes.len(), 16 * 16 * 3);
    }

    #[test]
    fn request_keyframe_forces_next_access_unit_to_be_intra() {
        let mut encoder = OpenH264Encoder::new().expect("encoder");
        let _ = encoder.encode(&frame(0)).expect("first frame");
        encoder.request_keyframe();
        let encoded = encoder.encode(&frame(33_000)).expect("keyframe");
        assert!(encoded.keyframe);
    }

    #[test]
    fn empty_input_does_not_create_a_decoded_frame() {
        let mut decoder = OpenH264Decoder::new().expect("decoder");
        assert!(decoder.decode(&[]).expect("empty input").is_none());
    }
}
