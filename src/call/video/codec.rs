//! Codec boundary for live-call video.
//!
//! Encoding/decoding is deliberately not implemented here. In particular,
//! this module does not invoke GStreamer or `iced_video_player`; live H.264
//! will be transported as call media and rendered by a future call widget.

/// Codec negotiated for a live video track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    /// H.264/AVC elementary stream carried by the call media protocol.
    H264,
}

/// Encoded access unit produced by a live-call encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    /// Codec used for the encoded bytes.
    pub codec: VideoCodec,
    /// Presentation timestamp in microseconds.
    pub timestamp_us: u64,
    /// Whether this access unit can begin independent decoding.
    pub keyframe: bool,
    /// Encoded elementary-stream bytes owned by the live pipeline.
    pub data: Vec<u8>,
}

/// Live video codec boundary reserved for the codec implementation task.
pub trait VideoEncoder: Send {
    /// Encode one raw frame into a live-call access unit.
    fn encode(&mut self, frame: crate::call::video::capture::CapturedFrame)
        -> Option<EncodedFrame>;
}
