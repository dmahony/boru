//! Wire packet types for the live-call video media track.
//!
//! These packets are call media, not attachment metadata. They are never
//! handed to `streaming_server`, `video_playback`, or `iced_video_player`.

use super::codec::{EncodedVideoFrame, VideoCodec};
use crate::call::media::{
    payload_capacity, MediaDatagram, MediaDatagramError, MediaKind, FLAG_KEYFRAME,
};
use crate::call::CallId;
use super::super::bounds::validate_video_track_config;
use super::super::wire::{ReceiverReport, VideoTrackConfig};
use std::time::{Duration, Instant};

#[cfg(feature = "net")]
use iroh::endpoint::Connection;

/// Maximum encoded access-unit payload accepted by the live packet layer.
pub const MAX_VIDEO_PAYLOAD_BYTES: usize = 256 * 1024;
/// Time allowed for a sender to receive a track-configuration acknowledgement.
pub const TRACK_CONFIG_ACK_TIMEOUT: Duration = Duration::from_secs(2);

/// State machine for an acknowledged v2 video track.
#[derive(Debug, Clone)]
pub struct VideoTrackGeneration {
    config: VideoTrackConfig,
    acknowledged: bool,
    awaiting_keyframe: bool,
    sent_at: Instant,
}

impl VideoTrackGeneration {
    /// Start a new sender generation. A new track id is required by callers.
    pub fn new_sender(config: VideoTrackConfig, now: Instant) -> Option<Self> {
        validate_video_track_config(&config).ok()?;
        Some(Self {
            config,
            acknowledged: false,
            awaiting_keyframe: true,
            sent_at: now,
        })
    }

    /// Validate and stage a receiver-side configuration before admitting media.
    pub fn receive_config(config: VideoTrackConfig, now: Instant) -> Option<Self> {
        Self::new_sender(config, now)
    }

    /// Acknowledge the staged configuration.
    pub fn acknowledge(&mut self) { self.acknowledged = true; }

    /// Whether this generation's acknowledgement has expired.
    pub fn ack_expired(&self, now: Instant) -> bool {
        !self.acknowledged && now.saturating_duration_since(self.sent_at) >= TRACK_CONFIG_ACK_TIMEOUT
    }

    /// Admit a packet only for this track, and require an acknowledged keyframe.
    pub fn admit(&mut self, packet: &MediaDatagram) -> bool {
        if packet.track_id != self.config.track_id || !self.acknowledged {
            return false;
        }
        if self.awaiting_keyframe {
            if packet.flags & FLAG_KEYFRAME == 0 { return false; }
            self.awaiting_keyframe = false;
        }
        true
    }

    /// Access the negotiated configuration.
    pub const fn config(&self) -> &VideoTrackConfig { &self.config }
}

/// Content-free receive counters, emitted as bounded deltas.
#[derive(Debug, Default, Clone)]
pub struct ReceiverReportDelta {
    track_id: u32,
    highest_sequence: u32,
    received_packets: u32,
    lost_packets: u32,
    estimated_bitrate_bps: u32,
    reported_received: u32,
    reported_lost: u32,
}

impl ReceiverReportDelta {
    /// Create counters for one track.
    pub const fn new(track_id: u32) -> Self {
        Self {
            track_id,
            highest_sequence: 0,
            received_packets: 0,
            lost_packets: 0,
            estimated_bitrate_bps: 0,
            reported_received: 0,
            reported_lost: 0,
        }
    }
    /// Record a received packet and its observed loss count.
    pub fn observe(&mut self, sequence: u32, lost: u32, bitrate_bps: u32) {
        self.highest_sequence = sequence;
        self.received_packets = self.received_packets.saturating_add(1);
        self.lost_packets = self.lost_packets.saturating_add(lost);
        self.estimated_bitrate_bps = bitrate_bps;
    }
    /// Take a report containing only counters since the previous report.
    pub fn take(&mut self) -> ReceiverReport {
        let report = ReceiverReport { track_id: self.track_id, highest_sequence: self.highest_sequence, received_packets: self.received_packets.saturating_sub(self.reported_received), lost_packets: self.lost_packets.saturating_sub(self.reported_lost), estimated_bitrate_bps: self.estimated_bitrate_bps };
        self.reported_received = self.received_packets;
        self.reported_lost = self.lost_packets;
        report
    }
}

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
        if capacity == 0 {
            return Err(MediaDatagramError::DatagramTooSmall {
                maximum: max_datagram_size,
                header: crate::call::media::MEDIA_HEADER_SIZE,
            });
        }
        if frame.bytes.is_empty() {
            return Err(MediaDatagramError::EmptyPayload);
        }
        if frame.bytes.len() > MAX_VIDEO_PAYLOAD_BYTES {
            return Err(MediaDatagramError::EncodedFrameTooLarge {
                advertised: frame.bytes.len(),
                maximum: MAX_VIDEO_PAYLOAD_BYTES,
            });
        }

        let fragment_count = frame.bytes.len().div_ceil(capacity);
        let fragment_count =
            u16::try_from(fragment_count).map_err(|_| MediaDatagramError::FragmentCountOverflow)?;
        if fragment_count > crate::call::media::MAX_VIDEO_FRAGMENTS_PER_FRAME {
            return Err(MediaDatagramError::TooManyFragments {
                count: fragment_count,
                maximum: crate::call::media::MAX_VIDEO_FRAGMENTS_PER_FRAME,
            });
        }
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
    use crate::call::wire::VideoTrackConfig;

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

    fn datagram(track_id: u32, flags: u16, sequence: u32) -> MediaDatagram {
        MediaDatagram {
            kind: MediaKind::Video,
            flags,
            call_id: CallId::from_bytes([7; 16]),
            track_id,
            sequence,
            timestamp: sequence,
            fragment_index: 0,
            fragment_count: 1,
            payload: vec![1],
        }
    }

    fn config(track_id: u32) -> VideoTrackConfig {
        VideoTrackConfig {
            track_id,
            codec: crate::call::wire::VideoCodec::H264,
            width: 640,
            height: 360,
            fps: 30,
            keyframe_interval: 60,
        }
    }

    #[test]
    fn reorder_before_config_and_late_old_track_are_rejected() {
        let now = Instant::now();
        let mut track = VideoTrackGeneration::receive_config(config(9), now).unwrap();
        assert!(!track.admit(&datagram(9, FLAG_KEYFRAME, 0)), "ack is required first");
        track.acknowledge();
        assert!(!track.admit(&datagram(8, FLAG_KEYFRAME, 0)), "old track is stale");
        assert!(!track.admit(&datagram(9, 0, 1)), "delta before keyframe is rejected");
        assert!(track.admit(&datagram(9, FLAG_KEYFRAME, 2)));
        assert!(track.admit(&datagram(9, 0, 3)));
    }

    #[test]
    fn unacknowledged_track_times_out() {
        let now = Instant::now();
        let track = VideoTrackGeneration::new_sender(config(11), now).unwrap();
        assert!(!track.ack_expired(now + TRACK_CONFIG_ACK_TIMEOUT - Duration::from_millis(1)));
        assert!(track.ack_expired(now + TRACK_CONFIG_ACK_TIMEOUT));
    }

    #[test]
    fn receiver_report_is_a_content_free_delta() {
        let mut reports = ReceiverReportDelta::new(9);
        reports.observe(4, 2, 123_000);
        let first = reports.take();
        assert_eq!((first.track_id, first.received_packets, first.lost_packets), (9, 1, 2));
        assert_eq!(reports.take().received_packets, 0);
        reports.observe(5, 1, 124_000);
        let second = reports.take();
        assert_eq!((second.highest_sequence, second.received_packets, second.lost_packets), (5, 1, 1));
    }
}
