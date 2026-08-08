//! Bounded reassembly for live-call video datagrams.
//!
//! Video is latency-sensitive: incomplete access units are discarded after a
//! short deadline rather than being retained until a missing fragment arrives.
//! All peer-controlled sizes are checked before payloads are copied.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use super::packet::VideoPacket;
use crate::call::media::{
    validate_fragment_meta, FragmentMeta, MediaDatagram, MediaDatagramError, FLAG_KEYFRAME,
    MAX_ENCODED_VIDEO_FRAME_BYTES,
};
use crate::call::CallId;

/// Maximum number of incomplete frames retained by one reassembler.
pub const MAX_INCOMPLETE_VIDEO_FRAMES: usize = 10;
/// Deadline for an incomplete frame.
pub const VIDEO_REASSEMBLY_TIMEOUT: Duration = Duration::from_millis(200);

/// Result of attempting to assemble live video packets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReassemblyResult {
    /// No complete access unit is available yet.
    Pending,
    /// A complete encoded access unit is ready for the live decoder.
    Complete(Vec<u8>),
}

/// Compatibility admission facade for callers that only need the hard-limit
/// gate. Full byte assembly is provided by [`VideoReassembler`].
#[derive(Debug, Default)]
pub struct FragmentAdmission {
    reassembler: VideoReassembler,
}

impl FragmentAdmission {
    /// Create an admission gate using the protocol hard limits.
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit a legacy single-packet value without retaining its completed bytes.
    pub fn admit_at(
        &mut self,
        packet: &VideoPacket,
        now: Instant,
    ) -> Result<(), MediaDatagramError> {
        self.reassembler.push_at(packet.clone(), now).map(|_| ())
    }

    /// Admit a parsed datagram before handing it to a decoder.
    pub fn admit_datagram_at(
        &mut self,
        packet: &MediaDatagram,
        now: Instant,
    ) -> Result<(), MediaDatagramError> {
        self.reassembler.push_datagram_at(packet, now).map(|_| ())
    }

    /// Return the number of currently incomplete frames.
    pub fn incomplete_frames(&self) -> usize {
        self.reassembler.incomplete_frames()
    }
}

type FrameKey = (CallId, u32, u32); // call, track, frame/sequence id

#[derive(Debug)]
struct IncompleteFrame {
    fragment_count: u16,
    fragments: Vec<Option<Vec<u8>>>,
    received_fragments: usize,
    total_bytes: usize,
    first_seen: Instant,
    deadline: Instant,
    keyframe: bool,
    timestamp: u32,
}

/// Bounded live-video fragment reassembler.
#[derive(Debug)]
pub struct VideoReassembler {
    frames: HashMap<FrameKey, IncompleteFrame>,
    /// Completed/expired keys prevent a delayed fragment from resurrecting an
    /// old frame. The queue is bounded, so this cannot become a memory sink.
    retired: HashSet<FrameKey>,
    retired_order: VecDeque<FrameKey>,
    max_incomplete_frames: usize,
    timeout: Duration,
    packet_count: usize,
}

impl Default for VideoReassembler {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoReassembler {
    /// Create an empty reassembler using the protocol hard limits.
    pub fn new() -> Self {
        Self {
            frames: HashMap::new(),
            retired: HashSet::new(),
            retired_order: VecDeque::new(),
            max_incomplete_frames: MAX_INCOMPLETE_VIDEO_FRAMES,
            timeout: VIDEO_REASSEMBLY_TIMEOUT,
            packet_count: 0,
        }
    }

    /// Create a reassembler with a synthetic timeout, useful for deterministic tests.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout,
            ..Self::new()
        }
    }

    /// Observe a legacy single-packet video value.
    pub fn push(&mut self, packet: VideoPacket) -> Result<ReassemblyResult, MediaDatagramError> {
        self.push_at(packet, Instant::now())
    }

    /// Testable single-packet form with an explicit local arrival time.
    pub fn push_at(
        &mut self,
        packet: VideoPacket,
        now: Instant,
    ) -> Result<ReassemblyResult, MediaDatagramError> {
        let datagram = MediaDatagram {
            kind: crate::call::media::MediaKind::Video,
            flags: if packet.keyframe { FLAG_KEYFRAME } else { 0 },
            call_id: packet.call_id,
            track_id: 1,
            sequence: packet.sequence,
            timestamp: packet.timestamp,
            fragment_index: 0,
            fragment_count: 1,
            payload: packet.payload,
        };
        self.push_datagram_at(&datagram, now)
    }

    /// Accept one parsed video fragment and assemble it when all fragments arrive.
    pub fn push_datagram(
        &mut self,
        packet: &MediaDatagram,
    ) -> Result<ReassemblyResult, MediaDatagramError> {
        self.push_datagram_at(packet, Instant::now())
    }

    /// Testable datagram form with an explicit arrival time.
    pub fn push_datagram_at(
        &mut self,
        packet: &MediaDatagram,
        now: Instant,
    ) -> Result<ReassemblyResult, MediaDatagramError> {
        validate_fragment_meta(FragmentMeta {
            fragment_index: packet.fragment_index,
            fragment_count: packet.fragment_count,
            payload_len: packet.payload.len(),
        })?;
        self.expire_at(now);

        let key = (packet.call_id, packet.track_id, packet.sequence);
        if self.retired.contains(&key) {
            return Err(MediaDatagramError::FragmentExpired);
        }

        if let Some(frame) = self.frames.get_mut(&key) {
            if frame.deadline <= now {
                self.frames.remove(&key);
                self.retire(key);
                return Err(MediaDatagramError::FragmentExpired);
            }
            if frame.fragment_count != packet.fragment_count {
                return Err(MediaDatagramError::InvalidFragmentCount);
            }
            let slot = &mut frame.fragments[packet.fragment_index as usize];
            if slot.is_some() {
                // Retransmitted fragments are harmless and must not inflate the
                // cumulative frame budget.
                self.packet_count = self.packet_count.saturating_add(1);
                return Ok(ReassemblyResult::Pending);
            }
            let new_total = frame.total_bytes.saturating_add(packet.payload.len());
            if new_total > MAX_ENCODED_VIDEO_FRAME_BYTES {
                return Err(MediaDatagramError::EncodedFrameTooLarge {
                    advertised: new_total,
                    maximum: MAX_ENCODED_VIDEO_FRAME_BYTES,
                });
            }
            *slot = Some(packet.payload.clone());
            frame.total_bytes = new_total;
            frame.received_fragments += 1;
            self.packet_count = self.packet_count.saturating_add(1);
            if frame.received_fragments != frame.fragment_count as usize {
                return Ok(ReassemblyResult::Pending);
            }
        } else {
            if self.frames.len() >= self.max_incomplete_frames {
                return Err(MediaDatagramError::TooManyIncompleteFrames {
                    maximum: self.max_incomplete_frames,
                });
            }
            let count = packet.fragment_count as usize;
            let mut fragments = (0..count).map(|_| None).collect::<Vec<_>>();
            fragments[packet.fragment_index as usize] = Some(packet.payload.clone());
            self.frames.insert(
                key,
                IncompleteFrame {
                    fragment_count: packet.fragment_count,
                    fragments,
                    received_fragments: 1,
                    total_bytes: packet.payload.len(),
                    first_seen: now,
                    deadline: now + self.timeout,
                    keyframe: packet.flags & FLAG_KEYFRAME != 0,
                    timestamp: packet.timestamp,
                },
            );
            self.packet_count = self.packet_count.saturating_add(1);
            if packet.fragment_count == 1 {
                let frame = self
                    .frames
                    .remove(&key)
                    .expect("single-fragment frame exists");
                self.retire(key);
                return Ok(ReassemblyResult::Complete(
                    frame.fragments.into_iter().flatten().flatten().collect(),
                ));
            }
            return Ok(ReassemblyResult::Pending);
        }

        let frame = self.frames.remove(&key).expect("completed frame exists");
        let _ = (frame.first_seen, frame.keyframe, frame.timestamp);
        let bytes = frame.fragments.into_iter().flatten().flatten().collect();
        self.retire(key);
        Ok(ReassemblyResult::Complete(bytes))
    }

    /// Discard all frames whose deadline has passed and return the count removed.
    pub fn expire(&mut self) -> usize {
        self.expire_at(Instant::now())
    }

    /// Deterministic form of [`Self::expire`].
    pub fn expire_at(&mut self, now: Instant) -> usize {
        let expired = self
            .frames
            .iter()
            .filter_map(|(key, frame)| (frame.deadline <= now).then_some(*key))
            .collect::<Vec<_>>();
        for key in &expired {
            self.frames.remove(key);
            self.retire(*key);
        }
        expired.len()
    }

    fn retire(&mut self, key: FrameKey) {
        if self.retired.insert(key) {
            self.retired_order.push_back(key);
            while self.retired_order.len() > self.max_incomplete_frames * 2 {
                if let Some(old) = self.retired_order.pop_front() {
                    self.retired.remove(&old);
                }
            }
        }
    }

    /// Number of live packets observed by this instance.
    pub const fn packet_count(&self) -> usize {
        self.packet_count
    }

    /// Number of incomplete frames currently retained.
    pub fn incomplete_frames(&self) -> usize {
        self.frames.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::media::{MediaKind, MAX_VIDEO_FRAGMENTS_PER_FRAME};

    fn frame(
        call_id: CallId,
        track_id: u32,
        id: u32,
        payload: &[u8],
        index: u16,
        count: u16,
    ) -> MediaDatagram {
        MediaDatagram {
            kind: MediaKind::Video,
            flags: if id == 7 { FLAG_KEYFRAME } else { 0 },
            call_id,
            track_id,
            sequence: id,
            timestamp: id + 100,
            fragment_index: index,
            fragment_count: count,
            payload: payload.to_vec(),
        }
    }

    fn collect_in_order(
        reassembler: &mut VideoReassembler,
        packets: &[MediaDatagram],
        now: Instant,
    ) -> Vec<u8> {
        let mut result = ReassemblyResult::Pending;
        for packet in packets {
            result = reassembler.push_datagram_at(packet, now).unwrap();
        }
        match result {
            ReassemblyResult::Complete(bytes) => bytes,
            ReassemblyResult::Pending => panic!("frame did not complete"),
        }
    }

    #[test]
    fn fragments_in_order_are_assembled() {
        let call = CallId::generate();
        let packets = vec![
            frame(call, 1, 1, b"ab", 0, 3),
            frame(call, 1, 1, b"cd", 1, 3),
            frame(call, 1, 1, b"ef", 2, 3),
        ];
        assert_eq!(
            collect_in_order(&mut VideoReassembler::new(), &packets, Instant::now()),
            b"abcdef"
        );
    }

    #[test]
    fn reversed_and_random_order_are_assembled() {
        let call = CallId::generate();
        let packets = vec![
            frame(call, 1, 2, b"ef", 2, 3),
            frame(call, 1, 2, b"ab", 0, 3),
            frame(call, 1, 2, b"cd", 1, 3),
        ];
        assert_eq!(
            collect_in_order(&mut VideoReassembler::new(), &packets, Instant::now()),
            b"abcdef"
        );
    }

    #[test]
    fn missing_fragment_stays_pending() {
        let call = CallId::generate();
        let mut r = VideoReassembler::new();
        assert_eq!(
            r.push_datagram(&frame(call, 1, 3, b"ab", 0, 3)).unwrap(),
            ReassemblyResult::Pending
        );
        assert_eq!(
            r.push_datagram(&frame(call, 1, 3, b"ef", 2, 3)).unwrap(),
            ReassemblyResult::Pending
        );
    }

    #[test]
    fn duplicate_fragment_is_ignored() {
        let call = CallId::generate();
        let mut r = VideoReassembler::new();
        let p = frame(call, 1, 4, b"ab", 0, 2);
        r.push_datagram(&p).unwrap();
        r.push_datagram(&p).unwrap();
        assert_eq!(
            r.push_datagram(&frame(call, 1, 4, b"cd", 1, 2)).unwrap(),
            ReassemblyResult::Complete(b"abcd".to_vec())
        );
    }

    #[test]
    fn late_fragment_after_completion_is_rejected() {
        let call = CallId::generate();
        let mut r = VideoReassembler::new();
        r.push_datagram(&frame(call, 1, 5, b"ab", 0, 1)).unwrap();
        assert_eq!(
            r.push_datagram(&frame(call, 1, 5, b"ab", 0, 1)),
            Err(MediaDatagramError::FragmentExpired)
        );
    }

    #[test]
    fn interleaved_frames_and_keyframe_loss_are_independent() {
        let call = CallId::generate();
        let mut r = VideoReassembler::new();
        r.push_datagram(&frame(call, 1, 6, b"a", 0, 2)).unwrap();
        r.push_datagram(&frame(call, 1, 7, b"x", 0, 2)).unwrap();
        assert_eq!(
            r.push_datagram(&frame(call, 1, 6, b"b", 1, 2)).unwrap(),
            ReassemblyResult::Complete(b"ab".to_vec())
        );
        assert_eq!(r.incomplete_frames(), 1);
    }

    #[test]
    fn oversized_and_excess_fragment_advertisements_are_rejected() {
        let call = CallId::generate();
        let mut r = VideoReassembler::new();
        let oversized = frame(
            call,
            1,
            8,
            &vec![0; MAX_ENCODED_VIDEO_FRAME_BYTES + 1],
            0,
            1,
        );
        assert!(matches!(
            r.push_datagram(&oversized),
            Err(MediaDatagramError::EncodedFrameTooLarge { .. })
        ));
        let excessive = frame(call, 1, 9, b"x", 0, MAX_VIDEO_FRAGMENTS_PER_FRAME + 1);
        assert!(matches!(
            r.push_datagram(&excessive),
            Err(MediaDatagramError::TooManyFragments { .. })
        ));
    }

    #[test]
    fn timeout_discards_incomplete_frame_and_rejects_late_fragment() {
        let start = Instant::now();
        let call = CallId::generate();
        let mut r = VideoReassembler::with_timeout(Duration::from_millis(5));
        r.push_datagram_at(&frame(call, 1, 10, b"a", 0, 2), start)
            .unwrap();
        assert_eq!(r.expire_at(start + Duration::from_millis(6)), 1);
        assert_eq!(
            r.push_datagram_at(
                &frame(call, 1, 10, b"b", 1, 2),
                start + Duration::from_millis(6)
            ),
            Err(MediaDatagramError::FragmentExpired)
        );
        assert_eq!(r.incomplete_frames(), 0);
    }

    /// Deterministic xorshift PRNG so randomized trials reproduce on failure.
    struct XorShift(u64);

    impl XorShift {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() as usize) % n
        }
    }

    #[test]
    fn randomized_permutations_always_assembled_with_bounded_state() {
        // Property-style: many trials, random fragment counts and random
        // payloads, each delivered in a fresh random permutation. Every
        // complete permutation must reassemble to exactly the original
        // bytes, and the reassembler must retain no incomplete frames after
        // each trial (bounded allocations).
        let mut rng = XorShift(0xC0FFEE_0123_4567_89);
        for trial in 0..64u32 {
            let call = CallId::generate();
            let frame_id = trial + 100;
            // Fragment count varies 1..=8; payload length varies 1..=96 so
            // both single- and multi-fragment frames are exercised. Cap the
            // count at the payload length so every fragment is non-empty.
            let payload: Vec<u8> = (0..(1 + rng.below(96))).map(|_| rng.next() as u8).collect();
            let count = (1 + rng.below(8)).min(payload.len());
            // Split the payload into `count` contiguous fragments with
            // proportional boundaries so uneven splits stay in range.
            let mut fragments: Vec<MediaDatagram> = (0..count)
                .map(|i| {
                    let start = i * payload.len() / count;
                    let end = (i + 1) * payload.len() / count;
                    frame(
                        call,
                        1,
                        frame_id,
                        &payload[start..end],
                        i as u16,
                        count as u16,
                    )
                })
                .collect();
            // Random permutation (Fisher-Yates).
            for i in (1..fragments.len()).rev() {
                let j = rng.below(i + 1);
                fragments.swap(i, j);
            }

            let mut reassembler = VideoReassembler::new();
            let mut result = ReassemblyResult::Pending;
            for datagram in &fragments {
                result = reassembler
                    .push_datagram_at(datagram, Instant::now())
                    .expect("randomized fragments must be admitted");
            }
            assert_eq!(
                result,
                ReassemblyResult::Complete(payload.clone()),
                "trial {trial}: {count} fragments must reassemble to the original payload"
            );
            assert_eq!(
                reassembler.incomplete_frames(),
                0,
                "trial {trial}: no incomplete frames may remain after a complete frame"
            );
        }
    }
}
