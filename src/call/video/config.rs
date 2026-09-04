//! Bounded runtime configuration for one live video codec.
#![allow(missing_docs)]

use super::{VideoCodec, VIDEO_FRAMES_PER_SECOND, VIDEO_HEIGHT, VIDEO_WIDTH, VIDEO_TARGET_BITRATE_BPS};
use crate::call::bounds::{MAX_VIDEO_FPS, MAX_VIDEO_HEIGHT, MAX_VIDEO_WIDTH};

/// Encoder/decoder configuration for one track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoConfig {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
}

impl VideoConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.codec != VideoCodec::H264 { return Err("no local implementation for codec"); }
        if self.width == 0 || self.height == 0 || self.width > MAX_VIDEO_WIDTH || self.height > MAX_VIDEO_HEIGHT { return Err("video dimensions out of bounds"); }
        if self.fps == 0 || self.fps > MAX_VIDEO_FPS || self.bitrate_bps == 0 { return Err("video configuration out of bounds"); }
        Ok(())
    }
}

/// Stable quality profiles used when a backend has no explicit configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoProfile { Q0 }

impl VideoProfile {
    pub const fn config(self) -> VideoConfig {
        VideoConfig { codec: VideoCodec::H264, width: VIDEO_WIDTH, height: VIDEO_HEIGHT, fps: VIDEO_FRAMES_PER_SECOND, bitrate_bps: VIDEO_TARGET_BITRATE_BPS }
    }
}
