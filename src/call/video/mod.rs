//! Independent live-call video media pipeline.
//!
//! This namespace owns camera frames, negotiated codec access units, call
//! packets, reassembly, and eventual live rendering. It is deliberately
//! separate from [`crate::video_playback`] and [`crate::streaming_server`],
//! which handle completed file attachments through `iced_video_player` and
//! local HTTP streaming. Live H.264 must never be routed through those paths.
//!
//! The module is a skeleton for the capture/codec/reassembly tasks. It does
//! not open cameras, decode media, or alter attachment playback behavior.

pub mod capture;
pub mod codec;
pub mod packet;
pub mod pipeline;
pub mod reassembly;

pub use codec::{EncodedFrame, VideoCodec};
pub use packet::{VideoPacket, MAX_VIDEO_PAYLOAD_BYTES};
pub use pipeline::LiveVideoPipeline;
