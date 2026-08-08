//! Bounded, deadline-driven jitter buffering for live Opus audio.
//!
//! This is intentionally not an ordered reliable queue. Packets that miss their
//! deadline are reported as PLC opportunities, while packets from a new call
//! or a large sequence discontinuity replace the old playout stream.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::call::CallId;

/// Default initial delay: 75 ms, or just under four 20 ms audio frames.
pub const DEFAULT_JITTER_DELAY: Duration = Duration::from_millis(75);
/// Audio frame duration used by the initial voice profile.
pub const AUDIO_FRAME_DURATION: Duration = Duration::from_millis(20);
/// Minimum adaptive playout target.
pub const MIN_JITTER_DELAY: Duration = Duration::from_millis(40);
/// Maximum adaptive playout target.
pub const MAX_JITTER_DELAY: Duration = Duration::from_millis(200);
const JITTER_STEP: Duration = Duration::from_millis(5);
const JITTER_HYSTERESIS_MS: u64 = 4;
/// Hard upper bound on retained encoded packets.
pub const MAX_BUFFERED_AUDIO_PACKETS: usize = 64;
const MAX_DISCONTINUITY: u32 = MAX_BUFFERED_AUDIO_PACKETS as u32 * 4;

/// Smoothed arrival-jitter estimator and bounded playout target controller.
#[derive(Debug, Clone, Copy)]
pub struct JitterAdaptation {
    estimate_ms: u64,
    target: Duration,
    last_arrival: Option<Instant>,
    last_sequence: Option<u32>,
}

impl Default for JitterAdaptation {
    fn default() -> Self {
        Self::new(DEFAULT_JITTER_DELAY)
    }
}

impl JitterAdaptation {
    /// Create an estimator with a target clamped to the supported range.
    pub fn new(target: Duration) -> Self {
        Self {
            estimate_ms: 0,
            target: target.clamp(MIN_JITTER_DELAY, MAX_JITTER_DELAY),
            last_arrival: None,
            last_sequence: None,
        }
    }

    /// Observe an in-order packet. Reordered packets do not perturb the estimate.
    pub fn observe(&mut self, sequence: u32, arrival: Instant, frame_duration: Duration) {
        let in_order = self
            .last_sequence
            .is_none_or(|last| sequence.wrapping_sub(last) < 0x8000_0000);
        if !in_order {
            return;
        }
        if let Some(last) = self.last_sequence {
            // Losses and discontinuities are not arrival-jitter samples.
            if sequence.wrapping_sub(last) != 1 {
                self.last_arrival = Some(arrival);
                self.last_sequence = Some(sequence);
                return;
            }
        }
        if let Some(previous) = self.last_arrival {
            let actual_ms = arrival.saturating_duration_since(previous).as_millis() as u64;
            let expected_ms = frame_duration.as_millis() as u64;
            let deviation = actual_ms.abs_diff(expected_ms);
            // EWMA alpha=1/8: one late packet cannot cause a latency jump.
            self.estimate_ms = (self.estimate_ms * 7 + deviation) / 8;
            let desired = (MIN_JITTER_DELAY.as_millis() as u64)
                .saturating_add(self.estimate_ms.saturating_mul(2))
                .clamp(
                    MIN_JITTER_DELAY.as_millis() as u64,
                    MAX_JITTER_DELAY.as_millis() as u64,
                );
            let current = self.target.as_millis() as u64;
            if desired.abs_diff(current) > JITTER_HYSTERESIS_MS {
                let step = JITTER_STEP.as_millis() as u64;
                self.target = Duration::from_millis(if desired > current {
                    (current + step).min(desired)
                } else {
                    current.saturating_sub(step).max(desired)
                });
            }
        }
        self.last_arrival = Some(arrival);
        self.last_sequence = Some(sequence);
    }

    /// Smoothed absolute arrival-jitter estimate in milliseconds.
    pub const fn estimate_ms(&self) -> u64 {
        self.estimate_ms
    }
    /// Current bounded playout target.
    pub const fn target(&self) -> Duration {
        self.target
    }
}

/// A received encoded audio packet and its arrival metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferedAudioPacket {
    /// Call session that produced the packet.
    pub call_id: CallId,
    /// Wrapping media sequence number.
    pub sequence: u32,
    /// Codec/sample-clock timestamp.
    pub timestamp: u32,
    /// Monotonic time at which the packet was received.
    pub arrival: Instant,
    /// Encoded Opus payload.
    pub payload: Vec<u8>,
}

/// Result of a playout tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioPlayout {
    /// Decode and play this packet.
    Packet(BufferedAudioPacket),
    /// The packet at `sequence` missed its deadline; invoke PLC.
    Missing {
        /// Sequence number to conceal with packet-loss concealment.
        sequence: u32,
    },
}

/// True when `a` precedes `b` under 32-bit serial-number arithmetic.
pub fn seq_before(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) > 0x8000_0000
}

/// A bounded jitter buffer for one live audio call and track.
#[derive(Debug)]
pub struct AudioJitterBuffer {
    call_id: Option<CallId>,
    packets: BTreeMap<u32, BufferedAudioPacket>,
    expected_next: Option<u32>,
    /// Highest sequence already played or concealed (PLC). Used to reject
    /// stale packets that arrive after playout has drained, so a late
    /// duplicate cannot re-anchor the stream and replay old audio.
    played_through: Option<u32>,
    next_deadline: Option<Instant>,
    jitter_delay: Duration,
    adaptation: JitterAdaptation,
    frame_duration: Duration,
    dropped_packets: u64,
    missing_packets: u64,
}

impl Default for AudioJitterBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_JITTER_DELAY, AUDIO_FRAME_DURATION)
    }
}

impl AudioJitterBuffer {
    /// Create a buffer with explicit initial delay and frame duration.
    pub fn new(jitter_delay: Duration, frame_duration: Duration) -> Self {
        Self {
            call_id: None,
            packets: BTreeMap::new(),
            expected_next: None,
            played_through: None,
            next_deadline: None,
            jitter_delay,
            adaptation: JitterAdaptation::new(jitter_delay),
            frame_duration,
            dropped_packets: 0,
            missing_packets: 0,
        }
    }

    /// Return the call currently accepted by this buffer.
    pub fn call_id(&self) -> Option<CallId> {
        self.call_id
    }
    /// Number of encoded packets currently retained.
    pub fn len(&self) -> usize {
        self.packets.len()
    }
    /// Whether no packet is currently retained.
    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }
    /// Number of packets rejected because the buffer was full.
    pub const fn dropped_packets(&self) -> u64 {
        self.dropped_packets
    }
    /// Number of deadlines reported as missing for PLC.
    pub const fn missing_packets(&self) -> u64 {
        self.missing_packets
    }
    /// Deadline of the next expected sequence, if playout is active.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.next_deadline
    }

    /// Current adaptive target, bounded to 40–200 ms.
    pub const fn jitter_target(&self) -> Duration {
        self.adaptation.target()
    }
    /// Smoothed arrival-jitter estimate in milliseconds.
    pub const fn jitter_estimate_ms(&self) -> u64 {
        self.adaptation.estimate_ms()
    }

    /// Borrow a packet by sequence without removing it from the playout queue.
    /// This lets Opus FEC inspect the following packet while preserving its
    /// normal deadline and decode order.
    pub(crate) fn peek(&self, sequence: u32) -> Option<&BufferedAudioPacket> {
        self.packets.get(&sequence)
    }

    /// Start accepting packets for a call, discarding every previous packet.
    pub fn reset_for_call(&mut self, call_id: CallId) {
        self.call_id = Some(call_id);
        self.packets.clear();
        self.expected_next = None;
        self.played_through = None;
        self.next_deadline = None;
        self.adaptation = JitterAdaptation::new(self.jitter_delay);
    }

    /// Insert one packet without waiting. Returns false for stale, duplicate,
    /// wrong-call, discontinuous, or capacity-dropped packets.
    pub fn push(&mut self, packet: BufferedAudioPacket) -> bool {
        let call_id = match self.call_id {
            Some(call_id) if call_id == packet.call_id => call_id,
            Some(_) => return false,
            None => {
                self.call_id = Some(packet.call_id);
                packet.call_id
            }
        };
        debug_assert_eq!(call_id, packet.call_id);

        self.adaptation
            .observe(packet.sequence, packet.arrival, self.frame_duration);
        self.jitter_delay = self.adaptation.target();

        if self.packets.contains_key(&packet.sequence) {
            return false;
        }

        if let Some(expected) = self.expected_next {
            if seq_before(packet.sequence, expected) {
                // Before the first deadline, a packet just before the first
                // arrival is valid reordering. Once playout has advanced,
                // `expected_next` is never moved backwards.
                let behind = expected.wrapping_sub(packet.sequence);
                if !self.packets.is_empty() && behind <= MAX_BUFFERED_AUDIO_PACKETS as u32 {
                    self.expected_next = Some(packet.sequence);
                    self.next_deadline = Some(packet.arrival + self.jitter_delay);
                } else {
                    return false;
                }
            }
            let ahead = packet
                .sequence
                .wrapping_sub(self.expected_next.expect("expected sequence set"));
            if ahead > MAX_DISCONTINUITY {
                self.packets.clear();
                self.expected_next = Some(packet.sequence);
                self.next_deadline = Some(packet.arrival + self.jitter_delay);
            }
        } else {
            // The buffer drained (expected_next was reset). Only a packet
            // strictly newer than everything already played may restart
            // playout; a stale or duplicate packet for an already-played
            // sequence must be rejected.
            if let Some(played) = self.played_through {
                if !seq_before(played, packet.sequence) {
                    return false;
                }
            }
            self.expected_next = Some(packet.sequence);
            self.next_deadline = Some(packet.arrival + self.jitter_delay);
        }

        if self.packets.len() >= MAX_BUFFERED_AUDIO_PACKETS {
            self.dropped_packets += 1;
            return false;
        }
        self.packets.insert(packet.sequence, packet);
        true
    }

    /// Return the next packet when its deadline arrives, or mark it missing.
    /// Returns `None` when playout is not active or the deadline has not passed.
    pub fn pop_due(&mut self, now: Instant) -> Option<AudioPlayout> {
        let expected = self.expected_next?;
        let deadline = self.next_deadline?;
        if now < deadline {
            return None;
        }

        let result = match self.packets.remove(&expected) {
            Some(packet) => AudioPlayout::Packet(packet),
            None => {
                self.missing_packets += 1;
                AudioPlayout::Missing { sequence: expected }
            }
        };
        self.played_through = Some(expected);
        self.expected_next = Some(expected.wrapping_add(1));
        self.next_deadline = Some(deadline + self.frame_duration);

        // Do not keep generating PLC forever while the sender is silent. A
        // later packet starts a fresh short playout anchor.
        if self.packets.is_empty() {
            self.expected_next = None;
            self.next_deadline = None;
        }
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(call_id: CallId, sequence: u32, arrival: Instant) -> BufferedAudioPacket {
        BufferedAudioPacket {
            call_id,
            sequence,
            timestamp: sequence.wrapping_mul(960),
            arrival,
            payload: vec![sequence as u8],
        }
    }

    fn due(buffer: &mut AudioJitterBuffer, at: Instant) -> AudioPlayout {
        buffer.pop_due(at).expect("packet should be due")
    }

    #[test]
    fn jitter_estimate_and_target_are_smoothed_and_bounded() {
        let t = Instant::now();
        let mut adaptation = JitterAdaptation::default();
        adaptation.observe(1, t, AUDIO_FRAME_DURATION);
        adaptation.observe(2, t + Duration::from_millis(80), AUDIO_FRAME_DURATION);
        assert!(adaptation.estimate_ms() > 0);
        assert!(adaptation.target() >= MIN_JITTER_DELAY);
        assert!(adaptation.target() <= MAX_JITTER_DELAY);
        assert!(adaptation.target() <= DEFAULT_JITTER_DELAY + JITTER_STEP);
    }

    #[test]
    fn one_late_packet_does_not_jump_latency() {
        let t = Instant::now();
        let mut adaptation = JitterAdaptation::default();
        adaptation.observe(1, t, AUDIO_FRAME_DURATION);
        let before = adaptation.target();
        adaptation.observe(2, t + Duration::from_millis(200), AUDIO_FRAME_DURATION);
        assert!(adaptation.target() <= before + JITTER_STEP);
    }

    #[test]
    fn target_respects_bounds_after_extreme_jitter() {
        let t = Instant::now();
        let mut adaptation = JitterAdaptation::new(Duration::from_secs(10));
        adaptation.observe(1, t, AUDIO_FRAME_DURATION);
        for sequence in 2..200 {
            adaptation.observe(
                sequence,
                t + Duration::from_millis(sequence as u64 * 500),
                AUDIO_FRAME_DURATION,
            );
        }
        assert!(adaptation.target() >= MIN_JITTER_DELAY);
        assert!(adaptation.target() <= MAX_JITTER_DELAY);
    }

    #[test]
    fn ordered_playback() {
        let call = CallId::from_bytes([1; 16]);
        let t = Instant::now();
        let mut buffer = AudioJitterBuffer::default();
        assert!(buffer.push(packet(call, 1, t)));
        assert!(buffer.push(packet(call, 2, t + AUDIO_FRAME_DURATION)));
        assert!(
            matches!(due(&mut buffer, t + DEFAULT_JITTER_DELAY), AudioPlayout::Packet(p) if p.sequence == 1)
        );
        assert!(
            matches!(due(&mut buffer, t + DEFAULT_JITTER_DELAY + AUDIO_FRAME_DURATION), AudioPlayout::Packet(p) if p.sequence == 2)
        );
    }

    #[test]
    fn out_of_order_is_played_in_sequence_order() {
        let call = CallId::from_bytes([2; 16]);
        let t = Instant::now();
        let mut buffer = AudioJitterBuffer::default();
        assert!(buffer.push(packet(call, 2, t)));
        assert!(buffer.push(packet(call, 1, t)));
        assert!(
            matches!(due(&mut buffer, t + DEFAULT_JITTER_DELAY), AudioPlayout::Packet(p) if p.sequence == 1)
        );
        assert!(
            matches!(due(&mut buffer, t + DEFAULT_JITTER_DELAY + AUDIO_FRAME_DURATION), AudioPlayout::Packet(p) if p.sequence == 2)
        );
    }

    #[test]
    fn duplicates_are_ignored() {
        let call = CallId::from_bytes([3; 16]);
        let t = Instant::now();
        let mut buffer = AudioJitterBuffer::default();
        assert!(buffer.push(packet(call, 1, t)));
        assert!(!buffer.push(packet(call, 1, t)));
        assert_eq!(buffer.len(), 1);
    }

    #[test]
    fn loss_is_reported_for_plc() {
        let call = CallId::from_bytes([4; 16]);
        let t = Instant::now();
        let mut buffer = AudioJitterBuffer::default();
        assert!(buffer.push(packet(call, 1, t)));
        assert!(buffer.push(packet(call, 3, t)));
        assert!(
            matches!(due(&mut buffer, t + DEFAULT_JITTER_DELAY), AudioPlayout::Packet(p) if p.sequence == 1)
        );
        assert!(matches!(
            due(&mut buffer, t + DEFAULT_JITTER_DELAY + AUDIO_FRAME_DURATION),
            AudioPlayout::Missing { sequence: 2 }
        ));
        assert_eq!(buffer.missing_packets(), 1);
    }

    #[test]
    fn wraparound_uses_serial_ordering() {
        let call = CallId::from_bytes([5; 16]);
        let t = Instant::now();
        let mut buffer = AudioJitterBuffer::default();
        assert!(buffer.push(packet(call, u32::MAX, t)));
        assert!(buffer.push(packet(call, 0, t)));
        assert!(
            matches!(due(&mut buffer, t + DEFAULT_JITTER_DELAY), AudioPlayout::Packet(p) if p.sequence == u32::MAX)
        );
        assert!(
            matches!(due(&mut buffer, t + DEFAULT_JITTER_DELAY + AUDIO_FRAME_DURATION), AudioPlayout::Packet(p) if p.sequence == 0)
        );
    }

    #[test]
    fn large_gap_restarts_without_fake_losses() {
        let call = CallId::from_bytes([6; 16]);
        let t = Instant::now();
        let mut buffer = AudioJitterBuffer::default();
        assert!(buffer.push(packet(call, 1, t)));
        assert!(buffer.push(packet(call, 10_000, t + Duration::from_secs(1))));
        assert_eq!(buffer.missing_packets(), 0);
        assert!(
            matches!(due(&mut buffer, t + Duration::from_secs(1) + DEFAULT_JITTER_DELAY), AudioPlayout::Packet(p) if p.sequence == 10_000)
        );
    }

    #[test]
    fn capacity_is_hard_bounded() {
        let call = CallId::from_bytes([7; 16]);
        let t = Instant::now();
        let mut buffer = AudioJitterBuffer::default();
        for seq in 0..MAX_BUFFERED_AUDIO_PACKETS as u32 {
            assert!(buffer.push(packet(call, seq, t)));
        }
        assert!(!buffer.push(packet(call, MAX_BUFFERED_AUDIO_PACKETS as u32, t)));
        assert_eq!(buffer.len(), MAX_BUFFERED_AUDIO_PACKETS);
        assert_eq!(buffer.dropped_packets(), 1);
    }

    #[test]
    fn stale_call_packets_are_rejected_after_restart() {
        let old = CallId::from_bytes([8; 16]);
        let new = CallId::from_bytes([9; 16]);
        let t = Instant::now();
        let mut buffer = AudioJitterBuffer::default();
        assert!(buffer.push(packet(old, 1, t)));
        buffer.reset_for_call(new);
        assert!(!buffer.push(packet(old, 2, t)));
        assert!(buffer.push(packet(new, 1, t)));
        assert_eq!(buffer.len(), 1);
    }

    #[test]
    fn fully_ordered_four_frame_stream_plays_in_order() {
        // Scenario: 1 2 3 4
        let call = CallId::from_bytes([10; 16]);
        let t = Instant::now();
        let mut buffer = AudioJitterBuffer::default();
        for seq in 1..=4 {
            assert!(buffer.push(packet(call, seq, t)));
        }
        assert_eq!(buffer.len(), 4);
        for seq in 1..=4 {
            let offset = (seq - 1) as u32;
            assert!(
                matches!(
                    due(&mut buffer, t + DEFAULT_JITTER_DELAY + AUDIO_FRAME_DURATION * offset),
                    AudioPlayout::Packet(p) if p.sequence == seq
                ),
                "sequence {seq} should be due in order"
            );
        }
        assert_eq!(buffer.missing_packets(), 0);
        assert_eq!(buffer.dropped_packets(), 0);
    }

    #[test]
    fn three_hop_reorder_plays_in_sequence_order() {
        // Scenario: 1 3 2 4 (arrival order), playout must be 1 2 3 4.
        let call = CallId::from_bytes([11; 16]);
        let t = Instant::now();
        let mut buffer = AudioJitterBuffer::default();
        for seq in [1u32, 3, 2, 4] {
            assert!(buffer.push(packet(call, seq, t)));
        }
        for seq in 1..=4 {
            let offset = (seq - 1) as u32;
            assert!(
                matches!(
                    due(&mut buffer, t + DEFAULT_JITTER_DELAY + AUDIO_FRAME_DURATION * offset),
                    AudioPlayout::Packet(p) if p.sequence == seq
                ),
                "reordered stream must still play sequence {seq} in order"
            );
        }
        assert_eq!(buffer.missing_packets(), 0);
    }

    #[test]
    fn duplicate_mixed_stream_plays_each_sequence_once() {
        // Scenario: 1 2 4 4 2 3 1 (duplicates arrive out of order).
        let call = CallId::from_bytes([12; 16]);
        let t = Instant::now();
        let mut buffer = AudioJitterBuffer::default();
        for seq in [1u32, 2, 4, 4, 2, 3, 1] {
            let _ = buffer.push(packet(call, seq, t));
        }
        assert_eq!(buffer.len(), 4, "four unique sequences retained");
        for seq in 1..=4 {
            let offset = (seq - 1) as u32;
            assert!(
                matches!(
                    due(&mut buffer, t + DEFAULT_JITTER_DELAY + AUDIO_FRAME_DURATION * offset),
                    AudioPlayout::Packet(p) if p.sequence == seq
                ),
                "each unique sequence {seq} must be played exactly once, in order"
            );
        }
        assert_eq!(buffer.len(), 0);
        assert_eq!(buffer.missing_packets(), 0);
    }

    #[test]
    fn late_packet_after_playout_advances_is_rejected() {
        // A packet for an already-played sequence must not replay it.
        let call = CallId::from_bytes([13; 16]);
        let t = Instant::now();
        let mut buffer = AudioJitterBuffer::default();
        assert!(buffer.push(packet(call, 1, t)));
        // Play seq 1 at its deadline.
        assert!(matches!(
            due(&mut buffer, t + DEFAULT_JITTER_DELAY),
            AudioPlayout::Packet(p) if p.sequence == 1
        ));
        // A late duplicate of seq 1 arriving after playout must be rejected.
        assert!(!buffer.push(packet(
            call,
            1,
            t + DEFAULT_JITTER_DELAY + AUDIO_FRAME_DURATION
        )));
        assert_eq!(
            buffer.len(),
            0,
            "late duplicate must not re-enter the buffer"
        );
        // The drained buffer stays inactive (no endless PLC for silence);
        // a genuinely newer packet restarts playout cleanly.
        assert!(
            buffer
                .pop_due(t + DEFAULT_JITTER_DELAY + AUDIO_FRAME_DURATION)
                .is_none(),
            "playout stays inactive after drain until a newer packet arrives"
        );
        assert!(buffer.push(packet(
            call,
            2,
            t + DEFAULT_JITTER_DELAY + AUDIO_FRAME_DURATION
        )));
        // Seq 2's deadline is its arrival (t + 75 + 20) plus the 75 ms
        // jitter delay = t + 170 ms.
        assert!(matches!(
            due(&mut buffer, t + 2 * DEFAULT_JITTER_DELAY + AUDIO_FRAME_DURATION),
            AudioPlayout::Packet(p) if p.sequence == 2
        ));
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn buffer_does_not_grow_unboundedly_over_many_inserts() {
        // Feeding a long stream with periodic full-queue pressure must keep
        // the retained set at the hard bound, never beyond it.
        let call = CallId::from_bytes([14; 16]);
        let t = Instant::now();
        let mut buffer = AudioJitterBuffer::default();
        for seq in 0..(MAX_BUFFERED_AUDIO_PACKETS as u32 * 5) {
            let _ = buffer.push(packet(call, seq, t));
            assert!(
                buffer.len() <= MAX_BUFFERED_AUDIO_PACKETS,
                "retained packets must never exceed the hard bound"
            );
        }
        assert!(
            buffer.dropped_packets() > 0,
            "congestion must have dropped packets"
        );
    }
}
