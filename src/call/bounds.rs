//! Shared bounds for peer-controlled call negotiation values.
//!
//! These checks run immediately after decoding a control message and before
//! negotiated values are used to construct media state. The control-frame
//! limit remains the outer allocation guard; these tighter limits prevent a
//! valid-sized frame from carrying pathological capability lists or settings.

use super::wire::{AudioCapabilities, MediaCapabilities, NegotiatedMedia, NegotiatedVideo};

/// Maximum entries in any peer-advertised capability list.
pub const MAX_CAPABILITY_LIST_ENTRIES: usize = 8;
/// Maximum sample rate accepted from a peer, in Hz.
pub const MAX_AUDIO_SAMPLE_RATE: u32 = 192_000;
/// Maximum audio channels accepted from a peer.
pub const MAX_AUDIO_CHANNELS: u8 = 8;
/// Maximum audio frame duration accepted from a peer, in milliseconds.
pub const MAX_AUDIO_FRAME_MS: u16 = 120;
/// Maximum frame rate accepted from a peer.
pub const MAX_VIDEO_FPS: u32 = 60;
/// Maximum negotiated video width in version 1, in pixels.
pub const MAX_VIDEO_WIDTH: u32 = 1_920;
/// Maximum negotiated video height in version 1, in pixels.
pub const MAX_VIDEO_HEIGHT: u32 = 1_080;

/// Validate an advertised audio capability set without allocating.
pub fn validate_audio_capabilities(value: &AudioCapabilities) -> Result<(), &'static str> {
    if value.codecs.len() > MAX_CAPABILITY_LIST_ENTRIES {
        return Err("too many audio codecs");
    }
    if value.sample_rates.len() > MAX_CAPABILITY_LIST_ENTRIES {
        return Err("too many audio sample rates");
    }
    if value.channels.len() > MAX_CAPABILITY_LIST_ENTRIES {
        return Err("too many audio channel counts");
    }
    if value.frame_ms.len() > MAX_CAPABILITY_LIST_ENTRIES {
        return Err("too many audio frame durations");
    }
    if value
        .sample_rates
        .iter()
        .any(|rate| *rate == 0 || *rate > MAX_AUDIO_SAMPLE_RATE)
    {
        return Err("audio sample rate out of bounds");
    }
    if value
        .channels
        .iter()
        .any(|channels| *channels == 0 || *channels > MAX_AUDIO_CHANNELS)
    {
        return Err("audio channel count out of bounds");
    }
    if value
        .frame_ms
        .iter()
        .any(|frame_ms| *frame_ms == 0 || *frame_ms > MAX_AUDIO_FRAME_MS)
    {
        return Err("audio frame duration out of bounds");
    }
    Ok(())
}

/// Validate all peer-advertised media capabilities.
pub fn validate_capabilities(value: &MediaCapabilities) -> Result<(), &'static str> {
    validate_audio_capabilities(&value.audio)?;
    if let Some(video) = &value.video {
        if video.codecs.len() > MAX_CAPABILITY_LIST_ENTRIES {
            return Err("too many video codecs");
        }
        if video.max_width == 0 || video.max_height == 0 {
            return Err("video dimensions must be non-zero");
        }
        if video.max_width > MAX_VIDEO_WIDTH || video.max_height > MAX_VIDEO_HEIGHT {
            return Err("video resolution out of bounds");
        }
        if video.max_fps == 0 || video.max_fps > MAX_VIDEO_FPS {
            return Err("video frame rate out of bounds");
        }
    }
    Ok(())
}

/// Validate peer-selected media before activating a call runtime.
pub fn validate_negotiated_media(value: &NegotiatedMedia) -> Result<(), &'static str> {
    if value.sample_rate == 0 || value.sample_rate > MAX_AUDIO_SAMPLE_RATE {
        return Err("negotiated audio sample rate out of bounds");
    }
    if value.channels == 0 || value.channels > MAX_AUDIO_CHANNELS {
        return Err("negotiated audio channel count out of bounds");
    }
    if value.frame_ms == 0 || value.frame_ms > MAX_AUDIO_FRAME_MS {
        return Err("negotiated audio frame duration out of bounds");
    }
    if let Some(video) = &value.video {
        validate_negotiated_video(video)?;
    }
    Ok(())
}

fn validate_negotiated_video(value: &NegotiatedVideo) -> Result<(), &'static str> {
    if value.width == 0 || value.height == 0 {
        return Err("negotiated video dimensions must be non-zero");
    }
    if value.width > MAX_VIDEO_WIDTH || value.height > MAX_VIDEO_HEIGHT {
        return Err("negotiated video resolution out of bounds");
    }
    if value.fps == 0 || value.fps > MAX_VIDEO_FPS {
        return Err("negotiated video frame rate out of bounds");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::wire::{AudioCodec, VideoCapabilities, VideoCodec};

    fn audio() -> AudioCapabilities {
        AudioCapabilities {
            codecs: vec![AudioCodec::Opus],
            sample_rates: vec![48_000],
            channels: vec![1],
            frame_ms: vec![20],
        }
    }

    #[test]
    fn oversized_capability_lists_are_rejected() {
        let mut value = audio();
        value.sample_rates = vec![48_000; MAX_CAPABILITY_LIST_ENTRIES + 1];
        assert_eq!(
            validate_audio_capabilities(&value),
            Err("too many audio sample rates")
        );
    }

    #[test]
    fn invalid_audio_values_are_rejected() {
        let mut value = audio();
        value.sample_rates = vec![MAX_AUDIO_SAMPLE_RATE + 1];
        assert_eq!(
            validate_audio_capabilities(&value),
            Err("audio sample rate out of bounds")
        );
    }

    #[test]
    fn invalid_video_fps_is_rejected() {
        let value = MediaCapabilities {
            audio: audio(),
            video: Some(VideoCapabilities {
                codecs: vec![VideoCodec::H264],
                max_width: 640,
                max_height: 480,
                max_fps: MAX_VIDEO_FPS + 1,
            }),
        };
        assert_eq!(
            validate_capabilities(&value),
            Err("video frame rate out of bounds")
        );
    }

    #[test]
    fn video_resolution_above_cap_is_rejected() {
        let value = audio();
        let capabilities = MediaCapabilities {
            audio: value,
            video: Some(VideoCapabilities {
                codecs: vec![VideoCodec::H264],
                max_width: MAX_VIDEO_WIDTH + 1,
                max_height: MAX_VIDEO_HEIGHT,
                max_fps: 60,
            }),
        };
        assert_eq!(
            validate_capabilities(&capabilities),
            Err("video resolution out of bounds")
        );
    }
}
