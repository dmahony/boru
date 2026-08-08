//! Independent live-call video media pipeline.
//!
//! This namespace owns camera frames, negotiated codec access units, call
//! packets, reassembly, and eventual live rendering. It is deliberately
//! separate from [`crate::video_playback`] and [`crate::streaming_server`],
//! which handle completed file attachments through `iced_video_player` and
//! local HTTP streaming. Live H.264 must never be routed through those paths.

pub mod capture;
pub mod codec;
pub mod packet;
pub mod pipeline;
pub mod reassembly;

pub use capture::{
    enumerate_cameras, select_default_camera, CameraCapture, CameraDevice, CameraError,
    CaptureConfig, CaptureSource, CapturedFrame,
};
pub use codec::{
    DecodedVideoFrame, EncodedVideoFrame, OpenH264Decoder, OpenH264Encoder, RawVideoFrame,
    VideoCodec, VideoDecoder, VideoEncoder, VIDEO_FRAMES_PER_SECOND, VIDEO_HEIGHT,
    VIDEO_KEYFRAME_INTERVAL_FRAMES, VIDEO_TARGET_BITRATE_BPS, VIDEO_WIDTH,
};
pub use packet::{VideoPacket, VideoPacketizer, MAX_VIDEO_PAYLOAD_BYTES};
pub use pipeline::{LiveVideoPipeline, LocalVideoPipeline};
