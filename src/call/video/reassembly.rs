//! Admission and reassembly boundary for live-call video packets.
//!
//! This module owns the resource budget for peer-supplied fragments. Actual
//! byte assembly and decoding are deliberately separate: hostile metadata is
//! rejected before a frame-sized buffer can be allocated.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::packet::VideoPacket;
use crate::call::media::{
    validate_fragment_meta, FragmentMeta, MediaDatagram, MediaDatagramError,
    MAX_ENCODED_VIDEO_FRAME_BYTES,
};
use crate::call::CallId;

/// Maximum number of incomplete frames retained by one reassembler.
pub const MAX_INCOMPLETE_VIDEO_FRAMES: usize = 10;
/// Deadline for an incomplete frame. Short enough to avoid stale memory.
pub const VIDEO_REASSEMBLY_TIMEOUT: Duration = Duration::from_millis(200);

/// Result of attempting to assemble live video packets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReassemblyResult {
    /// No complete access unit is available yet.
    Pending,
    /// A complete encoded access unit is ready for the live decoder.
    Complete(Vec<u8>),
}

#[derive(Debug, Clone, Copy)]
struct IncompleteFrame {
    fragment_count: u16,
    received_bytes: usize,
    deadline: Instant,
}

/// Bounded admission tracker used before a reassembly buffer is allocated.
#[derive(Debug)]
pub struct FragmentAdmission {
    frames: HashMap<(CallId, u32, u32), IncompleteFrame>,
    max_incomplete_frames: usize,
    timeout: Duration,
}

impl Default for FragmentAdmission {
    fn default() -> Self {
        Self::new()
    }
}

impl FragmentAdmission {
    /// Create an admission tracker with the protocol hard limits.
    pub fn new() -> Self {
        Self {
            frames: HashMap::new(),
            max_incomplete_frames: MAX_INCOMPLETE_VIDEO_FRAMES,
            timeout: VIDEO_REASSEMBLY_TIMEOUT,
        }
    }

    /// Validate metadata and reserve one bounded frame slot.
    pub fn admit_at(
        &mut self,
        packet: &VideoPacket,
        now: Instant,
    ) -> Result<(), MediaDatagramError> {
        // This gate runs before any future frame buffer allocation. The
        // product check is conservative because the final fragment may be
        // smaller, but it prevents oversized peer advertisements outright.
        validate_fragment_meta(FragmentMeta {
            fragment_index: 0,
            fragment_count: 1,
            payload_len: packet.payload.len(),
        })?;
        let key = (packet.call_id, packet.sequence, packet.timestamp);
        self.frames.retain(|_, frame| frame.deadline > now);

        if let Some(frame) = self.frames.get_mut(&key) {
            if frame.fragment_count != 1 {
                return Err(MediaDatagramError::InvalidFragmentCount);
            }
            if frame.received_bytes.saturating_add(packet.payload.len())
                > MAX_ENCODED_VIDEO_FRAME_BYTES
            {
                return Err(MediaDatagramError::EncodedFrameTooLarge {
                    advertised: frame.received_bytes.saturating_add(packet.payload.len()),
                    maximum: MAX_ENCODED_VIDEO_FRAME_BYTES,
                });
            }
            frame.received_bytes += packet.payload.len();
            return Ok(());
        }
        if self.frames.len() >= self.max_incomplete_frames {
            return Err(MediaDatagramError::TooManyIncompleteFrames {
                maximum: self.max_incomplete_frames,
            });
        }
        self.frames.insert(
            key,
            IncompleteFrame {
                fragment_count: 1,
                received_bytes: packet.payload.len(),
                deadline: now + self.timeout,
            },
        );
        Ok(())
    }

    /// Validate a peer datagram using its advertised fragment count before
    /// any reassembly storage is allocated.
    pub fn admit_datagram_at(
        &mut self,
        packet: &MediaDatagram,
        now: Instant,
    ) -> Result<(), MediaDatagramError> {
        validate_fragment_meta(FragmentMeta {
            fragment_index: packet.fragment_index,
            fragment_count: packet.fragment_count,
            payload_len: packet.payload.len(),
        })?;
        let key = (packet.call_id, packet.track_id, packet.sequence);
        self.frames.retain(|_, frame| frame.deadline > now);
        if let Some(frame) = self.frames.get_mut(&key) {
            if frame.fragment_count != packet.fragment_count {
                return Err(MediaDatagramError::InvalidFragmentCount);
            }
            if frame.received_bytes.saturating_add(packet.payload.len())
                > MAX_ENCODED_VIDEO_FRAME_BYTES
            {
                return Err(MediaDatagramError::EncodedFrameTooLarge {
                    advertised: frame.received_bytes.saturating_add(packet.payload.len()),
                    maximum: MAX_ENCODED_VIDEO_FRAME_BYTES,
                });
            }
            frame.received_bytes += packet.payload.len();
            return Ok(());
        }
        if self.frames.len() >= self.max_incomplete_frames {
            return Err(MediaDatagramError::TooManyIncompleteFrames {
                maximum: self.max_incomplete_frames,
            });
        }
        self.frames.insert(
            key,
            IncompleteFrame {
                fragment_count: packet.fragment_count,
                received_bytes: packet.payload.len(),
                deadline: now + self.timeout,
            },
        );
        Ok(())
    }

    /// Return the number of currently admitted incomplete frames.
    pub fn incomplete_frames(&self) -> usize {
        self.frames.len()
    }
}

/// Per-call live packet reassembler admission boundary.
#[derive(Debug, Default)]
pub struct VideoReassembler {
    admission: FragmentAdmission,
    packet_count: usize,
}

impl VideoReassembler {
    /// Create an empty live-media reassembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe one live packet, rejecting hostile metadata before allocation.
    pub fn push(&mut self, packet: VideoPacket) -> Result<ReassemblyResult, MediaDatagramError> {
        self.push_at(packet, Instant::now())
    }

    /// Testable form of [`Self::push`] with an explicit local arrival time.
    pub fn push_at(
        &mut self,
        packet: VideoPacket,
        now: Instant,
    ) -> Result<ReassemblyResult, MediaDatagramError> {
        self.admission.admit_at(&packet, now)?;
        self.packet_count = self.packet_count.saturating_add(1);
        Ok(ReassemblyResult::Pending)
    }

    /// Admit one parsed peer datagram through the same pre-allocation gate
    /// used by the eventual byte reassembler.
    pub fn push_datagram_at(
        &mut self,
        packet: &MediaDatagram,
        now: Instant,
    ) -> Result<ReassemblyResult, MediaDatagramError> {
        self.admission.admit_datagram_at(packet, now)?;
        self.packet_count = self.packet_count.saturating_add(1);
        Ok(ReassemblyResult::Pending)
    }

    /// Number of live packets observed by this instance.
    pub const fn packet_count(&self) -> usize {
        self.packet_count
    }

    /// Number of frame slots currently held for incomplete frames.
    pub fn incomplete_frames(&self) -> usize {
        self.admission.incomplete_frames()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::video::codec::VideoCodec;

    fn packet(sequence: u32, payload_len: usize) -> VideoPacket {
        VideoPacket::new(
            CallId::generate(),
            VideoCodec::H264,
            sequence,
            sequence,
            false,
            vec![0; payload_len],
        )
        .expect("test packet is within sender limit")
    }

    #[test]
    fn oversized_advertised_frame_is_rejected_before_allocation() {
        let mut reassembler = VideoReassembler::new();
        let datagram = MediaDatagram {
            kind: crate::call::media::MediaKind::Video,
            flags: 0,
            call_id: CallId::generate(),
            track_id: 1,
            sequence: 1,
            timestamp: 1,
            fragment_index: 0,
            fragment_count: 1,
            payload: vec![0; MAX_ENCODED_VIDEO_FRAME_BYTES + 1],
        };
        let result = reassembler
            .admission
            .admit_datagram_at(&datagram, Instant::now());
        assert!(matches!(
            result,
            Err(MediaDatagramError::EncodedFrameTooLarge { .. })
        ));
        assert_eq!(reassembler.incomplete_frames(), 0);
    }

    #[test]
    fn too_many_incomplete_frames_are_rejected() {
        let mut reassembler = VideoReassembler::new();
        for sequence in 0..MAX_INCOMPLETE_VIDEO_FRAMES as u32 {
            reassembler.push(packet(sequence, 1)).unwrap();
        }
        let result = reassembler.push(packet(999, 1));
        assert_eq!(
            result,
            Err(MediaDatagramError::TooManyIncompleteFrames {
                maximum: MAX_INCOMPLETE_VIDEO_FRAMES
            })
        );
    }

    #[test]
    fn expired_frame_slot_is_removed_before_admission() {
        let mut reassembler = VideoReassembler::new();
        let start = Instant::now();
        reassembler.push_at(packet(1, 1), start).unwrap();
        assert_eq!(reassembler.incomplete_frames(), 1);
        reassembler
            .push_at(packet(2, 1), start + VIDEO_REASSEMBLY_TIMEOUT)
            .unwrap();
        assert_eq!(reassembler.incomplete_frames(), 1);
    }

    #[test]
    fn fragment_limit_is_hard_bounded() {
        assert!(matches!(
            validate_fragment_meta(FragmentMeta {
                fragment_index: 0,
                fragment_count: crate::call::media::MAX_VIDEO_FRAGMENTS_PER_FRAME + 1,
                payload_len: 1,
            }),
            Err(MediaDatagramError::TooManyFragments { .. })
        ));
        assert_eq!(MAX_ENCODED_VIDEO_FRAME_BYTES, 2 * 1024 * 1024);
    }
}
