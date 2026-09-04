//! Versioned, reliable control messages exchanged during a call.
#![allow(missing_docs)]

use serde::{Deserialize, Serialize};
use std::fmt;

use super::bounds::{
    validate_capabilities, validate_negotiated_media, validate_negotiated_media_v2,
    validate_receiver_report, validate_video_capabilities_v2, validate_video_track_config,
};
use super::{CallId, CallKind};

/// Current call-control protocol version.
pub const CALL_CONTROL_VERSION: u16 = 1;
/// Version namespace used by the appended v2 control messages.
pub const CALL_CONTROL_V2_VERSION: u16 = 2;

/// Maximum postcard payload in one length-prefixed call-control frame.
pub const MAX_CALL_CONTROL_FRAME_SIZE: usize = 64 * 1024;

/// Errors returned while encoding or decoding a call-control frame.
#[derive(Debug, PartialEq, Eq)]
pub enum CallControlFrameError {
    /// The declared payload is larger than the protocol limit.
    FrameTooLarge {
        /// Number of bytes declared by the peer.
        declared: usize,
        /// Maximum payload accepted by this protocol.
        maximum: usize,
    },
    /// The input ended before the four-byte length prefix was complete.
    TruncatedLength {
        /// Number of prefix bytes received.
        actual: usize,
    },
    /// The input ended before the declared payload was complete.
    TruncatedPayload {
        /// Number of bytes declared by the peer.
        declared: usize,
        /// Number of payload bytes received.
        actual: usize,
    },
    /// The postcard payload could not be decoded.
    Deserialize(postcard::Error),
    /// The control message could not be encoded as postcard.
    Serialize(postcard::Error),
    /// A decoded message contained a peer-controlled value outside protocol bounds.
    InvalidValue(&'static str),
}

impl fmt::Display for CallControlFrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameTooLarge { declared, maximum } => {
                write!(
                    formatter,
                    "call-control frame is {declared} bytes (maximum {maximum})"
                )
            }
            Self::TruncatedLength { actual } => {
                write!(
                    formatter,
                    "truncated call-control length prefix ({actual} bytes)"
                )
            }
            Self::TruncatedPayload { declared, actual } => write!(
                formatter,
                "truncated call-control payload (declared {declared} bytes, got {actual})"
            ),
            Self::Deserialize(error) => write!(formatter, "invalid call-control postcard: {error}"),
            Self::Serialize(error) => {
                write!(formatter, "could not encode call-control postcard: {error}")
            }
            Self::InvalidValue(reason) => write!(formatter, "invalid call-control value: {reason}"),
        }
    }
}

impl std::error::Error for CallControlFrameError {}

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

/// Audio capabilities advertised by a call participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioCapabilities {
    /// Audio codecs supported in preference order.
    pub codecs: Vec<AudioCodec>,
    /// Supported audio sample rates in Hz.
    pub sample_rates: Vec<u32>,
    /// Supported audio channel counts.
    pub channels: Vec<u8>,
    /// Supported audio frame durations in milliseconds.
    pub frame_ms: Vec<u16>,
}

/// Video capabilities advertised by a call participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoCapabilities {
    /// Video codecs supported in preference order.
    pub codecs: Vec<VideoCodec>,
    /// Maximum video width in pixels.
    pub max_width: u32,
    /// Maximum video height in pixels.
    pub max_height: u32,
    /// Maximum video frame rate.
    pub max_fps: u32,
}

/// Capabilities advertised by a call participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaCapabilities {
    /// Audio capabilities for this participant.
    pub audio: AudioCapabilities,
    /// `None` advertises a voice-only call.
    pub video: Option<VideoCapabilities>,
}

/// Video parameters selected for an established call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegotiatedVideo {
    /// Selected video codec.
    pub codec: VideoCodec,
    /// Selected video width in pixels.
    pub width: u32,
    /// Selected video height in pixels.
    pub height: u32,
    /// Selected video frame rate.
    pub fps: u32,
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
    /// Selected video parameters, when video is enabled.
    pub video: Option<NegotiatedVideo>,
}

/// A bounded codec option advertised by a v2 video participant.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoCodecCapability {
    pub codec: VideoCodec,
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u32,
    pub max_bitrate_bps: u32,
}

/// Video capabilities advertised by a v2 participant, in preference order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoCapabilitiesV2 {
    pub codecs: Vec<VideoCodecCapability>,
}

/// Media capabilities used by the v2 offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaCapabilitiesV2 {
    pub audio: AudioCapabilities,
    pub video: Option<VideoCapabilitiesV2>,
}

/// Video parameters selected for a v2 call, including its media track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegotiatedVideoV2 {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
    pub track_id: u32,
}

/// Media parameters selected by a v2 accept message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegotiatedMediaV2 {
    pub audio_codec: AudioCodec,
    pub sample_rate: u32,
    pub channels: u8,
    pub frame_ms: u16,
    pub video: Option<NegotiatedVideoV2>,
}

/// Runtime configuration for one negotiated v2 video track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoTrackConfig {
    pub track_id: u32,
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub keyframe_interval: u32,
}

/// Acknowledges a reliable v2 control message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallControlAck {
    pub message_id: u64,
}

/// Bounded receiver feedback for one v2 video track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverReport {
    pub track_id: u32,
    pub highest_sequence: u32,
    pub received_packets: u32,
    pub lost_packets: u32,
    pub estimated_bitrate_bps: u32,
}

/// Why a v2 call fell back to the v1 media path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FallbackReason {
    Unsupported,
    InvalidConfiguration,
    Congestion,
    EncoderUnavailable,
}

/// Generate a non-zero v2 track identifier from the OS CSPRNG.
pub fn generate_track_id() -> u32 {
    loop {
        let mut bytes = [0; 4];
        getrandom::fill(&mut bytes).expect("OS CSPRNG unavailable for video track id");
        let id = u32::from_be_bytes(bytes);
        if id != 0 {
            return id;
        }
    }
}

/// Return the deliberately small version-1 media capability set.
pub fn v1_defaults() -> MediaCapabilities {
    MediaCapabilities {
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
    }
}

/// Select media common to both participants using local preference order.
pub fn negotiate(local: &MediaCapabilities, remote: &MediaCapabilities) -> Option<NegotiatedMedia> {
    validate_capabilities(local).ok()?;
    validate_capabilities(remote).ok()?;
    let audio_codec = local
        .audio
        .codecs
        .iter()
        .find(|codec| remote.audio.codecs.contains(codec))
        .copied()?;
    let sample_rate = local
        .audio
        .sample_rates
        .iter()
        .find(|rate| remote.audio.sample_rates.contains(rate))
        .copied()?;
    let channels = local
        .audio
        .channels
        .iter()
        .find(|channels| remote.audio.channels.contains(channels))
        .copied()?;
    let frame_ms = local
        .audio
        .frame_ms
        .iter()
        .find(|frame_ms| remote.audio.frame_ms.contains(frame_ms))
        .copied()?;

    let video = match (&local.video, &remote.video) {
        (None, _) | (_, None) => None,
        (Some(local), Some(remote)) => {
            let codec = local
                .codecs
                .iter()
                .find(|codec| remote.codecs.contains(codec))
                .copied()?;
            Some(NegotiatedVideo {
                codec,
                width: local.max_width.min(remote.max_width),
                height: local.max_height.min(remote.max_height),
                fps: local.max_fps.min(remote.max_fps),
            })
        }
    };

    Some(NegotiatedMedia {
        audio_codec,
        sample_rate,
        channels,
        frame_ms,
        video,
    })
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
    /// Requests a new media generation for an established call.
    Reconnect { call_id: CallId, generation: u64 },
    /// Ends the call with a safe protocol reason.
    Hangup {
        call_id: CallId,
        reason: HangupReason,
    },
    /// v2 offer with bounded, per-codec video capabilities.
    OfferV2 {
        call_id: CallId,
        kind: CallKind,
        capabilities: MediaCapabilitiesV2,
    },
    /// v2 accept with an explicitly identified negotiated video track.
    AcceptV2 {
        call_id: CallId,
        selected: NegotiatedMediaV2,
    },
    /// Changes the configuration of one v2 video track.
    VideoTrackConfig {
        call_id: CallId,
        config: VideoTrackConfig,
    },
    /// Acknowledges a v2 control message.
    Ack {
        call_id: CallId,
        ack: CallControlAck,
    },
    /// Reports bounded receive statistics for a v2 video track.
    ReceiverReport {
        call_id: CallId,
        report: ReceiverReport,
    },
    /// Requests fallback from v2 to the compatible v1 path.
    Fallback {
        call_id: CallId,
        reason: FallbackReason,
    },
}

/// Encode a call-control message as a big-endian length-prefixed frame.
pub fn encode_call_control(control: &CallControl) -> Result<Vec<u8>, CallControlFrameError> {
    let payload = postcard::to_stdvec(control).map_err(CallControlFrameError::Serialize)?;
    if payload.len() > MAX_CALL_CONTROL_FRAME_SIZE {
        return Err(CallControlFrameError::FrameTooLarge {
            declared: payload.len(),
            maximum: MAX_CALL_CONTROL_FRAME_SIZE,
        });
    }

    let length = payload.len() as u32;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decode one complete big-endian length-prefixed call-control frame.
///
/// The declared length is checked before the payload is sliced or deserialized;
/// callers that read from a stream must perform the same check before allocating
/// a payload buffer.
pub fn decode_call_control(frame: &[u8]) -> Result<CallControl, CallControlFrameError> {
    if frame.len() < 4 {
        return Err(CallControlFrameError::TruncatedLength {
            actual: frame.len(),
        });
    }

    let declared = u32::from_be_bytes(frame[..4].try_into().expect("length checked")) as usize;
    if declared > MAX_CALL_CONTROL_FRAME_SIZE {
        return Err(CallControlFrameError::FrameTooLarge {
            declared,
            maximum: MAX_CALL_CONTROL_FRAME_SIZE,
        });
    }

    let payload = &frame[4..];
    if payload.len() != declared {
        return Err(CallControlFrameError::TruncatedPayload {
            declared,
            actual: payload.len(),
        });
    }

    let control = postcard::from_bytes(payload).map_err(CallControlFrameError::Deserialize)?;
    match &control {
        CallControl::Offer { capabilities, .. } => {
            validate_capabilities(capabilities).map_err(CallControlFrameError::InvalidValue)?;
        }
        CallControl::Accept { selected, .. } => {
            validate_negotiated_media(selected).map_err(CallControlFrameError::InvalidValue)?;
        }
        CallControl::ReceiverReport { report, .. } => {
            validate_receiver_report(report).map_err(CallControlFrameError::InvalidValue)?;
        }
        CallControl::VideoTrackConfig { config, .. } => {
            validate_video_track_config(config).map_err(CallControlFrameError::InvalidValue)?;
        }
        CallControl::OfferV2 { capabilities, .. } => {
            validate_video_capabilities_v2(capabilities)
                .map_err(CallControlFrameError::InvalidValue)?;
        }
        CallControl::AcceptV2 { selected, .. } => {
            validate_negotiated_media_v2(selected).map_err(CallControlFrameError::InvalidValue)?;
        }
        _ => {}
    }
    Ok(control)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities() -> MediaCapabilities {
        MediaCapabilities {
            audio: AudioCapabilities {
                codecs: vec![AudioCodec::Opus],
                sample_rates: vec![48_000],
                channels: vec![1],
                frame_ms: vec![20],
            },
            video: Some(VideoCapabilities {
                codecs: vec![VideoCodec::H264],
                max_width: 1920,
                max_height: 1080,
                max_fps: 30,
            }),
        }
    }

    fn selected() -> NegotiatedMedia {
        NegotiatedMedia {
            audio_codec: AudioCodec::Opus,
            sample_rate: 48_000,
            channels: 1,
            frame_ms: 20,
            video: Some(NegotiatedVideo {
                codec: VideoCodec::H264,
                width: 1280,
                height: 720,
                fps: 30,
            }),
        }
    }

    fn offer_with_sample_rates(count: usize) -> CallControl {
        let mut capabilities = capabilities();
        capabilities.audio.sample_rates = vec![0; count];
        CallControl::Offer {
            call_id: CallId::generate(),
            kind: CallKind::Video,
            capabilities,
        }
    }

    #[test]
    fn control_frame_at_limit_is_encoded_but_pathological_list_is_rejected() {
        let mut low = 0;
        let mut high = MAX_CALL_CONTROL_FRAME_SIZE + 1;
        while low + 1 < high {
            let count = (low + high) / 2;
            let candidate = offer_with_sample_rates(count);
            let size = postcard::to_stdvec(&candidate).unwrap().len();
            if size <= MAX_CALL_CONTROL_FRAME_SIZE {
                low = count;
            } else {
                high = count;
            }
        }

        let original = offer_with_sample_rates(low);
        let frame = encode_call_control(&original).expect("frame at the limit should encode");
        assert_eq!(
            u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize,
            MAX_CALL_CONTROL_FRAME_SIZE
        );
        assert_eq!(frame.len(), 4 + MAX_CALL_CONTROL_FRAME_SIZE);
        assert_eq!(
            decode_call_control(&frame),
            Err(CallControlFrameError::InvalidValue(
                "too many audio sample rates"
            ))
        );
    }

    #[test]
    fn oversized_declared_length_is_rejected_before_payload_processing() {
        let mut frame = vec![0; 4];
        frame[..4].copy_from_slice(&((MAX_CALL_CONTROL_FRAME_SIZE as u32) + 1).to_be_bytes());
        let error = decode_call_control(&frame).unwrap_err();
        assert_eq!(
            error,
            CallControlFrameError::FrameTooLarge {
                declared: MAX_CALL_CONTROL_FRAME_SIZE + 1,
                maximum: MAX_CALL_CONTROL_FRAME_SIZE,
            }
        );
    }

    #[test]
    fn truncated_length_prefix_is_rejected() {
        assert_eq!(
            decode_call_control(&[0, 0, 0]).unwrap_err(),
            CallControlFrameError::TruncatedLength { actual: 3 }
        );
    }

    #[test]
    fn malformed_postcard_is_rejected_cleanly() {
        let error = decode_call_control(&[0, 0, 0, 1, 0xff]).unwrap_err();
        assert!(matches!(error, CallControlFrameError::Deserialize(_)));
    }

    #[test]
    fn v1_defaults_advertise_opus_and_h264() {
        let defaults = v1_defaults();
        assert_eq!(defaults.audio.codecs, vec![AudioCodec::Opus]);
        assert_eq!(defaults.audio.sample_rates, vec![48_000]);
        assert_eq!(defaults.audio.channels, vec![1]);
        assert_eq!(defaults.audio.frame_ms, vec![20]);
        assert_eq!(
            defaults.video.as_ref().unwrap().codecs,
            vec![VideoCodec::H264]
        );
    }

    #[test]
    fn negotiate_picks_v1_audio() {
        let negotiated = negotiate(&v1_defaults(), &v1_defaults()).expect("v1 media must match");
        assert_eq!(negotiated.audio_codec, AudioCodec::Opus);
        assert_eq!(negotiated.sample_rate, 48_000);
        assert_eq!(negotiated.channels, 1);
        assert_eq!(negotiated.frame_ms, 20);
    }

    #[test]
    fn negotiate_returns_none_without_common_audio_codec() {
        let mut remote = v1_defaults();
        remote.audio.codecs.clear();
        assert!(negotiate(&v1_defaults(), &remote).is_none());
    }

    #[test]
    fn negotiate_omits_video_for_voice_only_offer() {
        let mut voice = v1_defaults();
        voice.video = None;
        let negotiated = negotiate(&voice, &v1_defaults()).expect("audio still matches");
        assert_eq!(negotiated.video, None);
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

    #[test]
    fn every_control_variant_round_trips_through_frame_functions() {
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
            let frame = encode_call_control(&original).expect("control message should encode");
            let decoded = decode_call_control(&frame).expect("frame should decode");
            assert_eq!(original, decoded);
        }
    }

    #[test]
    fn truncated_payload_is_rejected() {
        // Declared length says 5 bytes, only 2 are present.
        let frame = [0, 0, 0, 5, 0x01, 0x02];
        assert_eq!(
            decode_call_control(&frame).unwrap_err(),
            CallControlFrameError::TruncatedPayload {
                declared: 5,
                actual: 2,
            }
        );
    }

    #[test]
    fn invalid_enum_discriminant_is_rejected_cleanly() {
        // Encode a valid message, then corrupt the first payload byte so the
        // postcard discriminant names a variant that does not exist.
        let original = CallControl::KeepAlive {
            call_id: CallId::generate(),
        };
        let mut frame = encode_call_control(&original).expect("control message should encode");
        frame[4] = 0xff;
        assert!(matches!(
            decode_call_control(&frame).unwrap_err(),
            CallControlFrameError::Deserialize(_)
        ));
    }

    #[test]
    fn unsupported_version_hello_decodes_at_wire_layer_and_has_typed_semantic_reject() {
        // The wire layer is deliberately version-agnostic: `Hello` carries the
        // version as data and postcard decodes any u16. Version negotiation is
        // a semantic check performed after decode; its typed rejection is
        // `RejectReason::UnsupportedVersion`. This test pins that contract so
        // a future wire-level version gate cannot silently change it.
        let hello = CallControl::Hello {
            version: CALL_CONTROL_VERSION + 1,
            call_id: CallId::generate(),
        };
        let frame = encode_call_control(&hello).expect("unsupported-version Hello should encode");
        let decoded = decode_call_control(&frame).expect("wire layer must decode any version");
        assert_eq!(decoded, hello);
        match decoded {
            CallControl::Hello { version, .. } => {
                assert_eq!(version, CALL_CONTROL_VERSION + 1);
            }
            other => panic!("expected Hello, got {other:?}"),
        }

        // The typed semantic rejection exists and round-trips on the wire.
        let reject = CallControl::Reject {
            call_id: CallId::generate(),
            reason: RejectReason::UnsupportedVersion,
        };
        let frame = encode_call_control(&reject).expect("reject should encode");
        assert_eq!(decode_call_control(&frame).unwrap(), reject);
    }

    #[test]
    fn mismatched_call_ids_round_trip_independently() {
        // Two messages carrying different call identities must decode without
        // interference; each keeps its own id (semantic comparison is the
        // caller's job after decode).
        let first_id = CallId::generate();
        let second_id = CallId::generate();
        assert_ne!(first_id, second_id);

        let first = CallControl::Ringing { call_id: first_id };
        let second = CallControl::Ringing { call_id: second_id };
        let first_decoded =
            decode_call_control(&encode_call_control(&first).unwrap()).expect("first decodes");
        let second_decoded =
            decode_call_control(&encode_call_control(&second).unwrap()).expect("second decodes");
        match (first_decoded, second_decoded) {
            (CallControl::Ringing { call_id: a }, CallControl::Ringing { call_id: b }) => {
                assert_eq!(a, first_id);
                assert_eq!(b, second_id);
                assert_ne!(a, b);
            }
            other => panic!("expected two Ringing messages, got {other:?}"),
        }
    }

    #[test]
    fn v2_control_variants_round_trip_and_track_ids_are_nonzero() {
        let id = fixture_call_id();
        let video = VideoCodecCapability {
            codec: VideoCodec::H264,
            max_width: 640,
            max_height: 360,
            max_fps: 24,
            max_bitrate_bps: 800_000,
        };
        let messages = [
            CallControl::OfferV2 {
                call_id: id,
                kind: CallKind::Video,
                capabilities: MediaCapabilitiesV2 {
                    audio: capabilities().audio,
                    video: Some(VideoCapabilitiesV2 {
                        codecs: vec![video],
                    }),
                },
            },
            CallControl::AcceptV2 {
                call_id: id,
                selected: NegotiatedMediaV2 {
                    audio_codec: AudioCodec::Opus,
                    sample_rate: 48_000,
                    channels: 1,
                    frame_ms: 20,
                    video: Some(NegotiatedVideoV2 {
                        codec: VideoCodec::H264,
                        width: 640,
                        height: 360,
                        fps: 24,
                        bitrate_bps: 600_000,
                        track_id: 9,
                    }),
                },
            },
            CallControl::VideoTrackConfig {
                call_id: id,
                config: VideoTrackConfig {
                    track_id: 9,
                    codec: VideoCodec::H264,
                    width: 640,
                    height: 360,
                    fps: 24,
                    keyframe_interval: 48,
                },
            },
            CallControl::Ack {
                call_id: id,
                ack: CallControlAck { message_id: 4 },
            },
            CallControl::ReceiverReport {
                call_id: id,
                report: ReceiverReport {
                    track_id: 9,
                    highest_sequence: 10,
                    received_packets: 10,
                    lost_packets: 1,
                    estimated_bitrate_bps: 500_000,
                },
            },
            CallControl::Fallback {
                call_id: id,
                reason: FallbackReason::Congestion,
            },
        ];
        assert_ne!(generate_track_id(), 0);
        for original in messages {
            let frame = encode_call_control(&original).expect("v2 message should encode");
            assert_eq!(decode_call_control(&frame).unwrap(), original);
        }
    }

    #[test]
    fn v2_zero_track_id_is_rejected_before_media_use() {
        let message = CallControl::VideoTrackConfig {
            call_id: fixture_call_id(),
            config: VideoTrackConfig {
                track_id: 0,
                codec: VideoCodec::H264,
                width: 640,
                height: 360,
                fps: 24,
                keyframe_interval: 48,
            },
        };
        let frame = encode_call_control(&message).unwrap();
        assert_eq!(
            decode_call_control(&frame),
            Err(CallControlFrameError::InvalidValue(
                "v2 track id must be non-zero"
            ))
        );
    }

    #[test]
    fn v2_codec_capability_bound_is_enforced() {
        let mut codecs = Vec::new();
        for _ in 0..=super::super::bounds::MAX_VIDEO_CODEC_CAPABILITIES {
            codecs.push(VideoCodecCapability {
                codec: VideoCodec::H264,
                max_width: 640,
                max_height: 360,
                max_fps: 24,
                max_bitrate_bps: 800_000,
            });
        }
        let message = CallControl::OfferV2 {
            call_id: fixture_call_id(),
            kind: CallKind::Video,
            capabilities: MediaCapabilitiesV2 {
                audio: capabilities().audio,
                video: Some(VideoCapabilitiesV2 { codecs }),
            },
        };
        let frame = encode_call_control(&message).unwrap();
        assert_eq!(
            decode_call_control(&frame),
            Err(CallControlFrameError::InvalidValue(
                "v2 video codec capability count out of bounds"
            ))
        );
    }

    #[test]
    fn v1_control_fixtures_are_stable_and_h264_is_discriminant_zero() {
        let expected = [
            "0001000102030405060708090a0b0c0d0e0f",
            "01000102030405060708090a0b0c0d0e0f0101000180f70201010114010100800fb8081e",
            "03000102030405060708090a0b0c0d0e0f0080f70201140100800ad0051e",
            "06000102030405060708090a0b0c0d0e0f0100",
            "07000102030405060708090a0b0c0d0e0f07",
            "0a000102030405060708090a0b0c0d0e0f06",
        ];
        for (message, expected_hex) in v1_fixture_messages().iter().zip(expected) {
            let payload = postcard::to_stdvec(message).expect("fixture should serialize");
            assert_eq!(hex::encode(payload), expected_hex);
        }
        assert_eq!(postcard::to_stdvec(&VideoCodec::H264).unwrap(), [0]);
        assert_eq!(CALL_CONTROL_VERSION, 1);
    }
}
