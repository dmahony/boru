//! Wire packet types for the live-call video media track.
//!
//! These packets are call media, not attachment metadata. They are never
//! handed to `streaming_server`, `video_playback`, or `iced_video_player`.

use super::codec::{EncodedVideoFrame, VideoCodec};
use crate::call::media::{
    payload_capacity, MediaDatagram, MediaDatagramError, MediaKind, FLAG_KEYFRAME,
};
use crate::call::CallId;

#[cfg(feature = "net")]
use iroh::endpoint::Connection;

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

/// Stateful packetizer for encoded live-video frames.
#[derive(Debug, Default)]
pub struct VideoPacketizer {
    next_frame_id: u32,
}

impl VideoPacketizer {
    /// Create a packetizer whose first frame has id zero.
    pub const fn new() -> Self {
        Self { next_frame_id: 0 }
    }

    /// Fragment one encoded frame using the negotiated datagram size.
    ///
    /// Capacity is calculated from the supplied connection maximum; no fixed
    /// Ethernet or QUIC MTU is assumed.
    pub fn fragment_frame(
        &mut self,
        call_id: CallId,
        track_id: u32,
        frame: &EncodedVideoFrame,
        max_datagram_size: usize,
    ) -> Result<Vec<MediaDatagram>, MediaDatagramError> {
        let capacity = payload_capacity(max_datagram_size)?;
        if frame.bytes.is_empty() {
            return Err(MediaDatagramError::EmptyPayload);
        }

        let fragment_count = frame.bytes.len().div_ceil(capacity);
        let fragment_count =
            u16::try_from(fragment_count).map_err(|_| MediaDatagramError::FragmentCountOverflow)?;
        let frame_id = self.next_frame_id;
        self.next_frame_id = self.next_frame_id.wrapping_add(1);
        let flags = if frame.keyframe { FLAG_KEYFRAME } else { 0 };
        let timestamp = frame.timestamp_us as u32;

        Ok(frame
            .bytes
            .chunks(capacity)
            .enumerate()
            .map(|(index, payload)| MediaDatagram {
                kind: MediaKind::Video,
                flags,
                call_id,
                track_id,
                sequence: frame_id,
                timestamp,
                fragment_index: index as u16,
                fragment_count,
                payload: payload.to_vec(),
            })
            .collect())
    }

    /// Fragment and encode one frame for direct QUIC datagram submission.
    #[cfg(feature = "net")]
    pub fn fragment_for_connection(
        &mut self,
        connection: &Connection,
        call_id: CallId,
        track_id: u32,
        frame: &EncodedVideoFrame,
    ) -> Result<Vec<Vec<u8>>, MediaDatagramError> {
        let maximum = connection
            .max_datagram_size()
            .ok_or(MediaDatagramError::DatagramsUnavailable)?;
        Ok(self
            .fragment_frame(call_id, track_id, frame, maximum)?
            .into_iter()
            .map(|fragment| fragment.encode())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::media::MEDIA_HEADER_SIZE;

    fn frame(bytes: usize, keyframe: bool) -> EncodedVideoFrame {
        EncodedVideoFrame {
            codec: VideoCodec::H264,
            width: 640,
            height: 360,
            timestamp_us: 123_456,
            keyframe,
            bytes: (0..bytes).map(|value| value as u8).collect(),
        }
    }

    #[test]
    fn large_frame_fragments_fit_negotiated_capacity() {
        let mut packetizer = VideoPacketizer::new();
        let maximum = 127;
        let fragments = packetizer
            .fragment_frame(CallId::generate(), 1, &frame(300, false), maximum)
            .expect("frame should fragment");

        assert_eq!(fragments.len(), 4);
        assert!(fragments
            .iter()
            .all(|fragment| fragment.encode().len() <= maximum));
        assert_eq!(fragments[0].payload.len(), maximum - MEDIA_HEADER_SIZE);
        assert_eq!(
            fragments[3].payload.len(),
            300 - 3 * (maximum - MEDIA_HEADER_SIZE)
        );
    }

    #[test]
    fn fragments_share_frame_metadata_and_keyframe_flag() {
        let call_id = CallId::generate();
        let mut packetizer = VideoPacketizer::new();
        let fragments = packetizer
            .fragment_frame(call_id, 7, &frame(200, true), 100)
            .expect("frame should fragment");

        assert_eq!(fragments.len(), 4);
        for (index, fragment) in fragments.iter().enumerate() {
            assert_eq!(fragment.kind, MediaKind::Video);
            assert_eq!(fragment.call_id, call_id);
            assert_eq!(fragment.track_id, 7);
            assert_eq!(fragment.sequence, 0);
            assert_eq!(fragment.timestamp, 123_456);
            assert_eq!(fragment.fragment_index, index as u16);
            assert_eq!(fragment.fragment_count, 4);
            assert_eq!(fragment.flags, FLAG_KEYFRAME);
        }
    }

    #[test]
    fn frame_ids_increase_once_per_frame_not_per_fragment() {
        let mut packetizer = VideoPacketizer::new();
        let first = packetizer
            .fragment_frame(CallId::generate(), 1, &frame(150, false), 100)
            .unwrap();
        let second = packetizer
            .fragment_frame(CallId::generate(), 1, &frame(1, false), 100)
            .unwrap();

        assert!(first.iter().all(|fragment| fragment.sequence == 0));
        assert_eq!(second[0].sequence, 1);
    }
}
