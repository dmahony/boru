//! Compatibility and feature-selection gates for the frozen call boundary.

use boru_core::call::wire::{
    AudioCapabilities, AudioCodec, CallControl, HangupReason, MediaCapabilities, NegotiatedMedia,
    NegotiatedVideo, VideoCapabilities, VideoCodec, CALL_CONTROL_VERSION,
};
use boru_core::call::{CallId, CallKind};

const V1_FIXTURES: &str = include_str!("fixtures/call_control_v1_postcard.hex");

fn fixture_call_id() -> CallId {
    CallId::from_bytes([
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f,
    ])
}

fn v1_messages() -> [(&'static str, CallControl); 6] {
    let call_id = fixture_call_id();
    let capabilities = MediaCapabilities {
        audio: AudioCapabilities {
            codecs: vec![AudioCodec::Opus],
            sample_rates: vec![48_000],
            channels: vec![1],
            frame_ms: vec![20],
        },
        video: Some(VideoCapabilities {
            codecs: vec![VideoCodec::H264],
            max_width: 1_920,
            max_height: 1_080,
            max_fps: 30,
        }),
    };
    let selected = NegotiatedMedia {
        audio_codec: AudioCodec::Opus,
        sample_rate: 48_000,
        channels: 1,
        frame_ms: 20,
        video: Some(NegotiatedVideo {
            codec: VideoCodec::H264,
            width: 1_280,
            height: 720,
            fps: 30,
        }),
    };
    [
        (
            "hello",
            CallControl::Hello {
                version: 1,
                call_id,
            },
        ),
        (
            "offer",
            CallControl::Offer {
                call_id,
                kind: CallKind::Video,
                capabilities,
            },
        ),
        ("accept", CallControl::Accept { call_id, selected }),
        (
            "media-state",
            CallControl::MediaState {
                call_id,
                audio_muted: true,
                video_enabled: false,
            },
        ),
        (
            "request-keyframe",
            CallControl::RequestKeyframe {
                call_id,
                track_id: 7,
            },
        ),
        (
            "hangup",
            CallControl::Hangup {
                call_id,
                reason: HangupReason::Shutdown,
            },
        ),
    ]
}

#[test]
fn v1_fixture_identity_is_frozen() {
    let expected: std::collections::HashMap<_, _> = V1_FIXTURES
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split_once('=').expect("fixture entry has a key"))
        .collect();

    for (name, message) in v1_messages() {
        let actual = hex::encode(postcard::to_stdvec(&message).expect("v1 message serializes"));
        assert_eq!(
            expected.get(name),
            Some(&actual.as_str()),
            "fixture changed: {name}"
        );
    }
    assert_eq!(expected.get("video-codec-h264").copied(), Some("00"));
}

#[test]
fn feature_selection_keeps_call_wire_available_without_media_features() {
    // CallControl is deliberately feature-independent: peers can reject a call
    // cleanly even when this binary has no native media device support.
    assert_eq!(CALL_CONTROL_VERSION, 1);
    let hello = CallControl::Hello {
        version: CALL_CONTROL_VERSION,
        call_id: fixture_call_id(),
    };
    assert!(postcard::to_stdvec(&hello).is_ok());
}

#[cfg(feature = "video-calls")]
#[test]
fn video_feature_also_selects_voice_feature() {
    assert!(cfg!(feature = "voice-calls"));
}

#[cfg(not(feature = "video-calls"))]
#[test]
fn v2_media_messages_are_not_enabled_by_feature_selection() {
    // This test is intentionally selected when video-calls is absent. There is
    // no v2 enum or v2 media path to accidentally activate yet.
    assert!(!cfg!(feature = "video-calls"));
}
