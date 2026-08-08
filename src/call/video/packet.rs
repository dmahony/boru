//! Wire packet types for the live-call video media track.
//!
//! These packets are call media, not attachment metadata. They are never
//! handed to `streaming_server`, `video_playback`, or `iced_video_player`.

use super::codec::VideoCodec;
use crate::call::CallId;

/// Maximum encoded access-unit payload accepted by the live packet layer.
pub const MAX_VIDEO_PAYLOAD_BYTES: usize = 256 * 1024;

/// A bounded encoded video packet transported by a live call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPacket {
    /// Call session that owns this packet.
    pub call_id: CallId,
    /// Codec used for the payload.
    pub codec: VideoCodec,
    /// Wrapping media sequence number.
    pub sequence: u32,
    /// Codec timestamp, in microseconds modulo `u32::MAX`.
    pub timestamp: u32,
    /// Whether this packet starts an independently decodable frame.
    pub keyframe: bool,
    /// Encoded live-media payload.
    pub payload: Vec<u8>,
}

impl VideoPacket {
    /// Construct a packet, rejecting payloads outside the live-media bound.
    pub fn new(
        call_id: CallId,
        codec: VideoCodec,
        sequence: u32,
        timestamp: u32,
        keyframe: bool,
        payload: Vec<u8>,
    ) -> Option<Self> {
        (payload.len() <= MAX_VIDEO_PAYLOAD_BYTES).then_some(Self {
            call_id,
            codec,
            sequence,
            timestamp,
            keyframe,
            payload,
        })
    }

    /// Whether `candidate` is newer under the shared live-media serial clock.
    ///
    /// This deliberately uses the same helper as the live audio jitter path;
    /// attachment playback has its own file/HTTP lifecycle and is unaffected.
    pub const fn sequence_newer_than(candidate: u32, reference: u32) -> bool {
        crate::call::sequence_newer_than(candidate, reference)
    }
}
