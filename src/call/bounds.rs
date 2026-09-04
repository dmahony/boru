//! Shared bounds for peer-controlled call negotiation values.
//!
//! These checks run immediately after decoding a control message and before
//! negotiated values are used to construct media state. The control-frame
//! limit remains the outer allocation guard; these tighter limits prevent a
//! valid-sized frame from carrying pathological capability lists or settings.

use super::wire::{
    AudioCapabilities, MediaCapabilities, MediaCapabilitiesV2, NegotiatedMedia, NegotiatedMediaV2,
    NegotiatedVideo, ReceiverReport, VideoTrackConfig,
};

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
/// Maximum number of v2 codec capability entries.
pub const MAX_VIDEO_CODEC_CAPABILITIES: usize = 8;
/// Maximum v2 video bitrate, in bits per second.
pub const MAX_VIDEO_BITRATE_BPS: u32 = 8_000_000;
/// Maximum v2 receiver report counter accepted from a peer.
pub const MAX_RECEIVER_REPORT_COUNTER: u32 = 1_000_000_000;
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

/// Validate v2 video capabilities before negotiating or allocating media.
pub fn validate_video_capabilities_v2(value: &MediaCapabilitiesV2) -> Result<(), &'static str> {
    validate_audio_capabilities(&value.audio)?;
    if let Some(video) = &value.video {
        if video.codecs.is_empty() || video.codecs.len() > MAX_VIDEO_CODEC_CAPABILITIES {
            return Err("v2 video codec capability count out of bounds");
        }
        for capability in &video.codecs {
            if capability.max_width == 0 || capability.max_height == 0 {
                return Err("v2 video dimensions must be non-zero");
            }
            if capability.max_width > MAX_VIDEO_WIDTH || capability.max_height > MAX_VIDEO_HEIGHT {
                return Err("v2 video resolution out of bounds");
            }
            if capability.max_fps == 0 || capability.max_fps > MAX_VIDEO_FPS {
                return Err("v2 video frame rate out of bounds");
            }
            if capability.max_bitrate_bps == 0 || capability.max_bitrate_bps > MAX_VIDEO_BITRATE_BPS
            {
                return Err("v2 video bitrate out of bounds");
            }
        }
    }
    Ok(())
}

/// Validate v2 selected media before constructing a media pipeline.
pub fn validate_negotiated_media_v2(value: &NegotiatedMediaV2) -> Result<(), &'static str> {
    validate_negotiated_media(&NegotiatedMedia {
        audio_codec: value.audio_codec,
        sample_rate: value.sample_rate,
        channels: value.channels,
        frame_ms: value.frame_ms,
        video: None,
    })?;
    if let Some(video) = &value.video {
        if video.track_id == 0 {
            return Err("v2 track id must be non-zero");
        }
        if video.width == 0
            || video.height == 0
            || video.width > MAX_VIDEO_WIDTH
            || video.height > MAX_VIDEO_HEIGHT
        {
            return Err("v2 negotiated video resolution out of bounds");
        }
        if video.fps == 0 || video.fps > MAX_VIDEO_FPS {
            return Err("v2 negotiated video frame rate out of bounds");
        }
        if video.bitrate_bps == 0 || video.bitrate_bps > MAX_VIDEO_BITRATE_BPS {
            return Err("v2 negotiated video bitrate out of bounds");
        }
    }
    Ok(())
}

/// Validate a v2 track configuration before allocating encoder/capture state.
pub fn validate_video_track_config(value: &VideoTrackConfig) -> Result<(), &'static str> {
    if value.track_id == 0 {
        return Err("v2 track id must be non-zero");
    }
    if value.width == 0
        || value.height == 0
        || value.width > MAX_VIDEO_WIDTH
        || value.height > MAX_VIDEO_HEIGHT
    {
        return Err("v2 track resolution out of bounds");
    }
    if value.fps == 0 || value.fps > MAX_VIDEO_FPS {
        return Err("v2 track frame rate out of bounds");
    }
    if value.keyframe_interval == 0 || value.keyframe_interval > value.fps.saturating_mul(10) {
        return Err("v2 keyframe interval out of bounds");
    }
    Ok(())
}

/// Validate receiver feedback without permitting counter/bandwidth abuse.
pub fn validate_receiver_report(value: &ReceiverReport) -> Result<(), &'static str> {
    if value.track_id == 0 {
        return Err("v2 track id must be non-zero");
    }
    if value.received_packets > MAX_RECEIVER_REPORT_COUNTER
        || value.lost_packets > MAX_RECEIVER_REPORT_COUNTER
    {
        return Err("receiver report counter out of bounds");
    }
    if value.estimated_bitrate_bps > MAX_VIDEO_BITRATE_BPS {
        return Err("receiver report bitrate out of bounds");
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
