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
}
