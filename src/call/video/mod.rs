//! Independent live-call video media pipeline.
//!
//! This namespace owns camera frames, negotiated codec access units, call
//! packets, reassembly, and eventual live rendering. It is deliberately
//! separate from [`crate::video_playback`] and [`crate::streaming_server`],
//! which handle completed file attachments through `iced_video_player` and
//! local HTTP streaming. Live H.264 must never be routed through those paths.

use std::sync::Arc;

use anyhow::Result;

use crate::call::bounds::{MAX_VIDEO_HEIGHT, MAX_VIDEO_WIDTH};

/// A decoded video frame ready for presentation.
///
/// The pixel buffer is reference counted deliberately: Iced messages and
/// events commonly clone frames while routing them between the media task and
/// the UI. Cloning this value therefore does not copy the pixel data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Packed RGBA8 pixels, row-major.
    pub rgba: Arc<[u8]>,
    /// Media timestamp in the negotiated timestamp units.
    pub timestamp: u64,
}

impl VideoFrame {
    /// Return the checked byte size of an RGBA8 frame.
    pub fn checked_rgba_len(width: u32, height: u32) -> Result<usize> {
        (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| anyhow::anyhow!("RGBA frame size overflow"))
    }

    /// Convert an RGB8 frame into the presentation format without unchecked
    /// allocation arithmetic. Dimensions are bounded before any allocation.
    pub fn from_rgb(width: u32, height: u32, rgb: &[u8], timestamp: u64) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(anyhow::anyhow!("RGBA dimensions must be non-zero"));
        }
        if width > MAX_VIDEO_WIDTH || height > MAX_VIDEO_HEIGHT {
            return Err(anyhow::anyhow!("RGBA resolution exceeds protocol bounds"));
        }
        let pixel_count = (width as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| anyhow::anyhow!("RGBA frame size overflow"))?;
        let expected_rgb = pixel_count
            .checked_mul(3)
            .ok_or_else(|| anyhow::anyhow!("RGB frame size overflow"))?;
        if rgb.len() != expected_rgb {
            return Err(anyhow::anyhow!(
                "RGB frame has {} bytes, expected {expected_rgb}",
                rgb.len()
            ));
        }
        let rgba_len = Self::checked_rgba_len(width, height)?;
        let mut rgba = Vec::with_capacity(rgba_len);
        for pixel in rgb.chunks_exact(3) {
            rgba.extend_from_slice(pixel);
            rgba.push(255);
        }
        Ok(Self {
            width,
            height,
            rgba: rgba.into(),
            timestamp,
        })
    }
}

/// The presentation state for live-call video.
///
/// These are slots, not queues: a newly published frame replaces the previous
/// one so a slow renderer cannot accumulate latency or unbounded memory.
#[derive(Debug, Default)]
pub struct VideoFrameSlots {
    /// Newest frame from the local preview/capture path.
    pub latest_local_frame: Option<VideoFrame>,
    /// Newest frame from the remote receive path.
    pub latest_remote_frame: Option<VideoFrame>,
}

impl VideoFrameSlots {
    /// Replace the local slot and return the frame that was displaced.
    pub fn replace_local(&mut self, frame: VideoFrame) -> Option<VideoFrame> {
        self.latest_local_frame.replace(frame)
    }

    /// Replace the remote slot and return the frame that was displaced.
    pub fn replace_remote(&mut self, frame: VideoFrame) -> Option<VideoFrame> {
        self.latest_remote_frame.replace(frame)
    }
}

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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{VideoFrame, VideoFrameSlots};

    fn frame(value: u8, timestamp: u64) -> VideoFrame {
        VideoFrame {
            width: 1,
            height: 1,
            rgba: Arc::from(vec![value; 4]),
            timestamp,
        }
    }

    #[test]
    fn frame_construction_preserves_metadata_and_pixels() {
        let frame = frame(7, 42);
        assert_eq!((frame.width, frame.height, frame.timestamp), (1, 1, 42));
        assert_eq!(&*frame.rgba, &[7, 7, 7, 7]);
    }

    #[test]
    fn frame_clone_shares_rgba_allocation() {
        let frame = frame(3, 9);
        let clone = frame.clone();
        assert!(Arc::ptr_eq(&frame.rgba, &clone.rgba));
    }

    #[test]
    fn rgba_size_overflow_is_reported_without_allocation() {
        assert!(VideoFrame::checked_rgba_len(u32::MAX, u32::MAX).is_err());
    }

    #[test]
    fn rgb_conversion_adds_opaque_alpha() {
        let frame = VideoFrame::from_rgb(1, 1, &[1, 2, 3], 7).expect("RGB conversion");
        assert_eq!(&*frame.rgba, &[1, 2, 3, 255]);
        assert_eq!(frame.timestamp, 7);
    }

    #[test]
    fn slots_replace_without_retaining_history() {
        let mut slots = VideoFrameSlots::default();
        assert!(slots.replace_local(frame(1, 1)).is_none());
        assert!(slots.replace_remote(frame(2, 2)).is_none());
        let displaced = slots.replace_remote(frame(3, 3)).expect("old remote");
        assert_eq!(displaced.timestamp, 2);
        assert_eq!(slots.latest_local_frame.as_ref().unwrap().timestamp, 1);
        assert_eq!(slots.latest_remote_frame.as_ref().unwrap().timestamp, 3);
    }
}
