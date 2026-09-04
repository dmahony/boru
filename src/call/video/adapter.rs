//! Adapter between camera pipeline values and neutral realtime video frames.
//!
//! The call pipeline retains ownership of capture, packetization, and session
//! policy; only representation conversion belongs here.

use super::capture::CapturedFrame;
use crate::realtime_video::{self, VideoFrame};

/// Convert the camera's packed RGB capture representation.
pub fn frame_to_realtime(
    frame: &CapturedFrame,
    width: u32,
    height: u32,
) -> Result<VideoFrame, realtime_video::CodecError> {
    VideoFrame::packed(
        frame.timestamp_us,
        width,
        height,
        realtime_video::PixelFormat::Rgb8,
        frame.data.clone(),
    )
}

/// Return tightly packed RGBA pixels for preview or a codec implementation.
pub fn rgba_pixels(frame: &VideoFrame) -> Result<Vec<u8>, realtime_video::CodecError> {
    realtime_video::to_rgba8(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn translates_camera_rgb_capture() {
        let source = CapturedFrame {
            timestamp_us: 11,
            data: vec![1, 2, 3],
        };
        let frame = frame_to_realtime(&source, 1, 1).unwrap();
        assert_eq!(rgba_pixels(&frame).unwrap(), vec![1, 2, 3, 255]);
    }
}
