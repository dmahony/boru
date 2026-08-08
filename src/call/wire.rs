//! Versioned, reliable control messages exchanged during a call.

use serde::{Deserialize, Serialize};

use super::{CallId, CallKind};

/// Current call-control protocol version.
pub const CALL_CONTROL_VERSION: u16 = 1;

/// Audio codecs supported by the initial call protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioCodec {
    /// Opus audio codec.
    Opus,
}

/// Video codecs supported by the initial call protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoCodec {
    /// H.264 video codec.
    H264,
}

/// Capabilities advertised by a call participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaCapabilities {
    /// Audio codecs supported in preference order.
    pub audio_codecs: Vec<AudioCodec>,
    /// Supported audio sample rates in Hz.
    pub sample_rates: Vec<u32>,
    /// Supported audio channel counts.
    pub channels: Vec<u8>,
    /// Supported audio frame durations in milliseconds.
    pub frame_ms: Vec<u16>,
    /// Video codecs supported in preference order.
    pub video_codecs: Vec<VideoCodec>,
    /// Maximum video width in pixels.
    pub max_width: u32,
    /// Maximum video height in pixels.
    pub max_height: u32,
    /// Maximum video frame rate.
    pub max_fps: u16,
}

/// Media parameters selected for an established call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegotiatedMedia {
    /// Selected audio codec.
    pub audio_codec: AudioCodec,
    /// Selected audio sample rate in Hz.
    pub sample_rate: u32,
    /// Selected audio channel count.
    pub channels: u8,
    /// Selected audio frame duration in milliseconds.
    pub frame_ms: u16,
    /// Selected video codec, when video is enabled.
    pub video_codec: Option<VideoCodec>,
    /// Selected video width in pixels.
    pub width: u32,
    /// Selected video height in pixels.
    pub height: u32,
    /// Selected video frame rate.
    pub fps: u16,
}

/// Safe, protocol-defined reason for rejecting a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    /// The callee declined the call.
    Declined,
    /// The callee is already in another call.
    Busy,
    /// The caller is blocked.
    Blocked,
    /// The caller is not authorized.
    Unauthorized,
    /// The peer requires an unsupported protocol version.
    UnsupportedVersion,
    /// No common audio codec exists.
    NoCommonAudioCodec,
    /// No common video codec exists.
    NoCommonVideoCodec,
    /// No audio input/output device is available.
    NoAudioDevice,
    /// No camera is available.
    NoCamera,
    /// Device permission was denied.
    PermissionDenied,
    /// The protocol state is invalid.
    ProtocolError,
}

/// Safe, protocol-defined reason for ending a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HangupReason {
    /// The local user ended the call.
    LocalHangup,
    /// The remote user ended the call.
    RemoteHangup,
    /// The transport connection was lost.
    ConnectionLost,
    /// The protocol state is invalid.
    ProtocolError,
    /// Authorization was revoked.
    AuthorizationRevoked,
    /// A local media device failed.
    DeviceError,
    /// The application is shutting down.
    Shutdown,
    /// Negotiation did not complete in time.
    NegotiationTimeout,
}

/// Reliable control message for call setup, state, and teardown.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallControl {
    /// Initiates protocol version and call identity negotiation.
    Hello { version: u16, call_id: CallId },
    /// Advertises the desired call kind and media capabilities.
    Offer {
        call_id: CallId,
        kind: CallKind,
        capabilities: MediaCapabilities,
    },
    /// Indicates that the callee is alerting the user.
    Ringing { call_id: CallId },
    /// Accepts the call with selected media parameters.
    Accept {
        call_id: CallId,
        selected: NegotiatedMedia,
    },
    /// Rejects the call with a safe protocol reason.
    Reject {
        call_id: CallId,
        reason: RejectReason,
    },
    /// Indicates that the callee is unavailable.
    Busy { call_id: CallId },
    /// Reports local mute/camera state.
    MediaState {
        call_id: CallId,
        audio_muted: bool,
        video_enabled: bool,
    },
    /// Requests an intra frame for a video track.
    RequestKeyframe { call_id: CallId, track_id: u32 },
    /// Keeps the reliable control stream alive.
    KeepAlive { call_id: CallId },
    /// Ends the call with a safe protocol reason.
    Hangup {
        call_id: CallId,
        reason: HangupReason,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities() -> MediaCapabilities {
        MediaCapabilities {
            audio_codecs: vec![AudioCodec::Opus],
            sample_rates: vec![48_000],
            channels: vec![1],
            frame_ms: vec![20],
            video_codecs: vec![VideoCodec::H264],
            max_width: 1920,
            max_height: 1080,
            max_fps: 30,
        }
    }

    fn selected() -> NegotiatedMedia {
        NegotiatedMedia {
            audio_codec: AudioCodec::Opus,
            sample_rate: 48_000,
            channels: 1,
            frame_ms: 20,
            video_codec: Some(VideoCodec::H264),
            width: 1280,
            height: 720,
            fps: 30,
        }
    }

    #[test]
    fn every_control_variant_round_trips_through_postcard() {
        let id = CallId::generate();
        let messages = vec![
            CallControl::Hello {
                version: CALL_CONTROL_VERSION,
                call_id: id,
            },
            CallControl::Offer {
                call_id: id,
                kind: CallKind::Video,
                capabilities: capabilities(),
            },
            CallControl::Ringing { call_id: id },
            CallControl::Accept {
                call_id: id,
                selected: selected(),
            },
            CallControl::Reject {
                call_id: id,
                reason: RejectReason::Declined,
            },
            CallControl::Busy { call_id: id },
            CallControl::MediaState {
                call_id: id,
                audio_muted: true,
                video_enabled: false,
            },
            CallControl::RequestKeyframe {
                call_id: id,
                track_id: 7,
            },
            CallControl::KeepAlive { call_id: id },
            CallControl::Hangup {
                call_id: id,
                reason: HangupReason::Shutdown,
            },
        ];

        for original in messages {
            let encoded = postcard::to_stdvec(&original).expect("control message should serialize");
            let decoded: CallControl =
                postcard::from_bytes(&encoded).expect("control message should deserialize");
            assert_eq!(original, decoded);
        }
    }
}
