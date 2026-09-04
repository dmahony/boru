//! Adapter between screen-share capture values and neutral realtime video values.
//!
//! Transport and screen-share session policy remain in their existing modules;
//! this file only translates frame/configuration data at the boundary.

use super::{CapturedFrame, PixelFormat, ScreenShareError};
use crate::realtime_video::{self, CodecConfig, VideoFrame};

/// Convert a screen capture frame while preserving timestamp and row stride.
pub fn frame_to_realtime(frame: &CapturedFrame) -> Result<VideoFrame, ScreenShareError> {
    let format = match frame.pixel_format {
        PixelFormat::Rgba8 => realtime_video::PixelFormat::Rgba8,
        PixelFormat::Bgra8 => realtime_video::PixelFormat::Bgra8,
        PixelFormat::Gpu => {
            return Err(ScreenShareError::new(
                "GPU frames require a platform codec adapter",
            ))
        }
    };
    let expected = (frame.stride as usize)
        .checked_mul(frame.height as usize)
        .ok_or_else(|| ScreenShareError::new("frame dimensions overflow"))?;
    if frame.pixels.len() != expected {
        return Err(ScreenShareError::new(
            "screen capture payload does not match stride",
        ));
    }
    Ok(VideoFrame {
        timestamp_us: frame.timestamp_us,
        width: frame.width,
        height: frame.height,
        format,
        stride: frame.stride as usize,
        pixels: frame.pixels.clone(),
    })
}

/// Translate common configuration knobs without importing screen-share policy.
pub fn config_to_realtime(
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: u32,
    keyframe_interval: u64,
) -> Result<CodecConfig, ScreenShareError> {
    let config = CodecConfig {
        width,
        height,
        fps,
        bitrate_bps,
        keyframe_interval,
    };
    config
        .validate()
        .map_err(|error| ScreenShareError::new(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn translates_bgra_capture_without_changing_pixels() {
        let frame = CapturedFrame::cpu(9, 2, 2, PixelFormat::Bgra8, vec![1; 16]).unwrap();
        let neutral = frame_to_realtime(&frame).unwrap();
        assert_eq!(neutral.timestamp_us, 9);
        assert_eq!(neutral.pixels, frame.pixels);
        assert_eq!(neutral.format, realtime_video::PixelFormat::Bgra8);
    }
}
