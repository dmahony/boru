//! Codec boundary for screen-frame encoders and decoders.

use super::{capture::CapturedFrame, ScreenShareError};

/// Encoded screen frame passed to a transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    /// Presentation timestamp copied from capture.
    pub timestamp_us: u64,
    /// Codec payload.
    pub bytes: Vec<u8>,
}

/// Encodes captured frames without prescribing a codec.
pub trait VideoEncoder: Send {
    /// Encode one captured frame.
    fn encode(&mut self, frame: &CapturedFrame) -> Result<EncodedFrame, ScreenShareError>;
}

/// Decodes transport frames for a viewer.
pub trait VideoDecoder: Send {
    /// Decode one encoded frame.
    fn decode(&mut self, frame: &EncodedFrame) -> Result<CapturedFrame, ScreenShareError>;
}

/// Combined codec boundary useful for simple implementations and tests.
pub trait ScreenShareCodec: VideoEncoder + VideoDecoder {}
impl<T: VideoEncoder + VideoDecoder> ScreenShareCodec for T {}
