//! Fixed-size audio framing and RTP-style media clock progression.

use std::time::Duration;

/// Duration of one audio frame in milliseconds.
pub const FRAME_MS: u32 = 20;
/// Sample rate used by the internal audio pipeline.
pub const SAMPLE_RATE: u32 = 48_000;
/// Number of mono samples in one audio frame.
pub const SAMPLES_PER_FRAME: usize = (SAMPLE_RATE as usize / 1_000) * FRAME_MS as usize;
/// Duration of one audio frame.
pub const FRAME_DURATION: Duration = Duration::from_millis(FRAME_MS as u64);

/// Sequence number and sample-clock timestamp for produced audio frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSeq {
    /// Sequence number of the most recently produced frame.
    pub sequence: u32,
    /// Sample-clock timestamp of the most recently produced frame.
    pub timestamp: u32,
}

impl AudioSeq {
    /// Construct a counter at an explicit sequence and timestamp.
    pub const fn new(sequence: u32, timestamp: u32) -> Self {
        Self {
            sequence,
            timestamp,
        }
    }

    /// Advance to and return the next frame's sequence and timestamp.
    ///
    /// Both fields use wrapping arithmetic because media clocks are finite
    /// 32-bit protocol values. After `u32::MAX`, the next value is zero; the
    /// comparison logic at the receiver must therefore use serial-number
    /// arithmetic rather than ordinary unbounded integer ordering.
    pub fn next_frame(&mut self) -> (u32, u32) {
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(SAMPLES_PER_FRAME as u32);
        (self.sequence, self.timestamp)
    }
}

impl Default for AudioSeq {
    fn default() -> Self {
        Self::new(0, 0)
    }
}

/// Whether `candidate` is newer than `reference` under wrapping serial-number
/// arithmetic (RFC 1982 style for 32-bit counters).
///
/// Media clocks wrap at `u32::MAX`; a plain `>` comparison is wrong across the
/// boundary (e.g. `0 > u32::MAX` is false even though `0` follows
/// `u32::MAX`).  With serial-number arithmetic, `candidate` is newer iff the
/// forward distance `candidate.wrapping_sub(reference)` is non-zero and less
/// than half the counter range (2^31).  Distances of exactly half the range
/// are ambiguous and report "not newer" to stay consistent.
pub const fn sequence_newer_than(candidate: u32, reference: u32) -> bool {
    candidate != reference && candidate.wrapping_sub(reference) < (1 << 31)
}

/// Whether `candidate` is older than `reference` (the inverse of
/// [`sequence_newer_than`] for non-equal values).
pub const fn sequence_older_than(candidate: u32, reference: u32) -> bool {
    reference != candidate && reference.wrapping_sub(candidate) < (1 << 31)
}

#[cfg(test)]
mod tests {
    use super::{AudioSeq, FRAME_DURATION, FRAME_MS, SAMPLES_PER_FRAME, SAMPLE_RATE};
    use std::time::Duration;

    #[test]
    fn frame_constants_describe_20ms_at_48khz() {
        assert_eq!(FRAME_MS, 20);
        assert_eq!(SAMPLE_RATE, 48_000);
        assert_eq!(SAMPLES_PER_FRAME, 960);
        assert_eq!(FRAME_DURATION, Duration::from_millis(20));
    }

    #[test]
    fn next_frame_increments_sequence_and_sample_timestamp() {
        let mut clock = AudioSeq::default();
        assert_eq!(clock.next_frame(), (1, 960));
        assert_eq!(clock.next_frame(), (2, 1_920));
    }

    #[test]
    fn sequence_and_timestamp_wrap_independently() {
        let mut clock = AudioSeq::new(u32::MAX - 2, u32::MAX - 959);
        assert_eq!(clock.next_frame(), (u32::MAX - 1, 0));
        assert_eq!(clock.next_frame(), (u32::MAX, 960));
        assert_eq!(clock.next_frame(), (0, 1_920));
        assert_eq!(clock.next_frame(), (1, 2_880));
        assert_eq!(clock.next_frame(), (2, 3_840));
    }

    #[test]
    fn sequence_increments_wrap_across_the_u32_boundary() {
        let mut clock = AudioSeq::new(0xffff_fffd, 0);
        let mut expected = 0xffff_fffdu32;
        for _ in 0..6 {
            let (seq, _) = clock.next_frame();
            expected = expected.wrapping_add(1);
            assert_eq!(seq, expected);
        }
        // The full progression around the boundary:
        // fffffffd -> fffffffe -> ffffffff -> 0 -> 1 -> 2
        let mut clock2 = AudioSeq::new(0xffff_fffd, 0);
        assert_eq!(clock2.next_frame(), (0xffff_fffe, 960));
        assert_eq!(clock2.next_frame(), (0xffff_ffff, 1_920));
        assert_eq!(clock2.next_frame(), (0, 2_880));
        assert_eq!(clock2.next_frame(), (1, 3_840));
        assert_eq!(clock2.next_frame(), (2, 4_800));
    }

    #[test]
    fn newer_than_orders_frames_across_the_wrap_boundary() {
        use super::{sequence_newer_than, sequence_older_than};

        // Normal ordering away from the boundary.
        assert!(sequence_newer_than(10, 5));
        assert!(!sequence_newer_than(5, 10));
        assert!(sequence_older_than(5, 10));
        assert!(!sequence_older_than(10, 5));

        // Across the wrap: 0 follows u32::MAX, so 0 is newer than u32::MAX.
        assert!(sequence_newer_than(0, u32::MAX));
        assert!(sequence_newer_than(1, u32::MAX));
        assert!(sequence_newer_than(1, 0xffff_fffd));
        assert!(!sequence_newer_than(u32::MAX, 0));
        assert!(sequence_older_than(u32::MAX, 0));

        // Full boundary walk: each successor is newer than its predecessor
        // around fffffffd -> fffffffe -> ffffffff -> 0 -> 1 -> 2.
        let walk = [0xffff_fffd, 0xffff_fffe, 0xffff_ffff, 0, 1, 2];
        for (i, a) in walk.iter().enumerate() {
            for (j, b) in walk.iter().enumerate() {
                if i > j {
                    assert!(
                        sequence_newer_than(*a, *b),
                        "{a:#x} should be newer than {b:#x}"
                    );
                    assert!(sequence_older_than(*b, *a));
                } else if i < j {
                    assert!(
                        !sequence_newer_than(*a, *b),
                        "{a:#x} should be older than {b:#x}"
                    );
                } else {
                    assert!(!sequence_newer_than(*a, *b), "equal values are never newer");
                    assert!(!sequence_older_than(*a, *b), "equal values are never older");
                }
            }
        }
    }

    #[test]
    fn newer_than_handles_short_forward_arcs_after_wrap() {
        use super::sequence_newer_than;

        // A receiver that saw 0xfffffffd then receives 2 (a 5-step forward
        // jump across the boundary) must treat 2 as newer.
        assert!(sequence_newer_than(2, 0xffff_fffd));

        // Half-range ambiguity: exactly 2^31 forward is neither newer nor
        // older by design (RFC 1982).
        let mid = u32::MAX / 2 + 1;
        assert!(!sequence_newer_than(mid, 0));
        assert!(!sequence_newer_than(0, mid));
    }
}
