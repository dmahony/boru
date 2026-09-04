//! Call-scoped media statistics and adaptation state.

#![allow(missing_docs)]

use std::time::{Duration, Instant};

use super::adaptation::{AdaptationController, AdaptationDecision};
use super::media::{MediaDatagram, MediaKind};

/// A low-frequency snapshot of local call media health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallStats {
    pub rtt: Duration,
    pub audio_packets_sent: u64,
    pub audio_packets_received: u64,
    pub audio_packets_lost: u64,
    pub audio_packets_late: u64,
    pub audio_jitter_ms: u64,
    pub audio_playback_underruns: u64,
    pub video_packets_sent: u64,
    pub video_packets_received: u64,
    pub video_packets_dropped: u64,
    pub video_frames_encoded: u64,
    pub video_frames_decoded: u64,
    pub video_frames_dropped: u64,
    pub keyframe_requests: u64,
    pub estimated_send_bitrate: u64,
    pub estimated_receive_bitrate: u64,
    /// Media bytes handed to the transport, including wire headers.
    pub bytes_sent: u64,
    /// Media bytes accepted from the transport, including wire headers.
    pub bytes_received: u64,
    /// Video packet sequence gaps observed at the receiver.
    pub video_packets_lost: u64,
    /// Incomplete video frames discarded after their deadline.
    pub video_frames_expired: u64,
    /// Incomplete video frames rejected because the bounded budget was full.
    pub video_reassembly_overflows: u64,
    /// Milliseconds represented by the latest measurement interval.
    pub measurement_interval_ms: u64,
}

impl Default for CallStats {
    fn default() -> Self {
        Self {
            rtt: Duration::ZERO,
            audio_packets_sent: 0,
            audio_packets_received: 0,
            audio_packets_lost: 0,
            audio_packets_late: 0,
            audio_jitter_ms: 0,
            audio_playback_underruns: 0,
            video_packets_sent: 0,
            video_packets_received: 0,
            video_packets_dropped: 0,
            video_frames_encoded: 0,
            video_frames_decoded: 0,
            video_frames_dropped: 0,
            keyframe_requests: 0,
            estimated_send_bitrate: 0,
            estimated_receive_bitrate: 0,
            bytes_sent: 0,
            bytes_received: 0,
            video_packets_lost: 0,
            video_frames_expired: 0,
            video_reassembly_overflows: 0,
            measurement_interval_ms: 0,
        }
    }
}

#[derive(Debug, Default)]
struct Accumulator {
    snapshot: CallStats,
    last_audio_sequence: Option<u32>,
    last_video_sequence: Option<u32>,
    jitter: Jitter,
    last_rate_at: Option<Instant>,
    last_rate_sent: u64,
    last_rate_received: u64,
}

#[derive(Debug, Default)]
struct Jitter {
    estimate_ms: u64,
    target_ms: u64,
    last_arrival: Option<Instant>,
    last_sequence: Option<u32>,
}

impl Jitter {
    fn observe(&mut self, sequence: u32, arrival: Instant) {
        if self.target_ms == 0 {
            self.target_ms = 75;
        }
        let in_order = self
            .last_sequence
            .is_none_or(|last| sequence.wrapping_sub(last) < 0x8000_0000);
        if !in_order {
            return;
        }
        if let Some(previous) = self.last_arrival {
            let deviation = arrival
                .saturating_duration_since(previous)
                .as_millis()
                .abs_diff(20) as u64;
            self.estimate_ms = (self.estimate_ms * 7 + deviation) / 8;
            let desired = (40 + self.estimate_ms.saturating_mul(2)).clamp(40, 200);
            if desired.abs_diff(self.target_ms) > 4 {
                self.target_ms = if desired > self.target_ms {
                    (self.target_ms + 5).min(desired)
                } else {
                    self.target_ms.saturating_sub(5).max(desired)
                };
            }
        }
        self.last_arrival = Some(arrival);
        self.last_sequence = Some(sequence);
    }
}

impl Accumulator {
    fn observe_received(&mut self, packet: &MediaDatagram, arrival: Instant) {
        self.snapshot.bytes_received = self
            .snapshot
            .bytes_received
            .saturating_add(packet.encode().len() as u64);
        let (last, previous) = match packet.kind {
            MediaKind::Audio => {
                self.snapshot.audio_packets_received =
                    self.snapshot.audio_packets_received.saturating_add(1);
                let previous = self.last_audio_sequence;
                self.jitter.observe(packet.sequence, arrival);
                self.snapshot.audio_jitter_ms = self.jitter.target_ms;
                (&mut self.last_audio_sequence, previous)
            }
            MediaKind::Video => {
                self.snapshot.video_packets_received =
                    self.snapshot.video_packets_received.saturating_add(1);
                let previous = self.last_video_sequence;
                (&mut self.last_video_sequence, previous)
            }
        };
        if let Some(previous) = previous {
            let delta = packet.sequence.wrapping_sub(previous);
            if delta == 0 || delta > 0x8000_0000 {
                match packet.kind {
                    MediaKind::Audio => {
                        self.snapshot.audio_packets_late =
                            self.snapshot.audio_packets_late.saturating_add(1)
                    }
                    MediaKind::Video => {
                        self.snapshot.video_packets_dropped =
                            self.snapshot.video_packets_dropped.saturating_add(1)
                    }
                }
            } else if packet.kind == MediaKind::Audio && delta > 1 {
                self.snapshot.audio_packets_lost = self
                    .snapshot
                    .audio_packets_lost
                    .saturating_add((delta - 1) as u64);
            } else if packet.kind == MediaKind::Video && delta > 1 {
                self.snapshot.video_packets_lost = self
                    .snapshot
                    .video_packets_lost
                    .saturating_add((delta - 1) as u64);
            }
        }
        if previous.is_none_or(|previous| packet.sequence.wrapping_sub(previous) < 0x8000_0000) {
            *last = Some(packet.sequence);
        }
    }

    fn observe_malformed(&mut self) {
        self.snapshot.video_packets_dropped = self.snapshot.video_packets_dropped.saturating_add(1);
    }
    fn observe_sent(&mut self, bytes: u64, _datagrams: u64) {
        self.snapshot.bytes_sent = self.snapshot.bytes_sent.saturating_add(bytes);
    }
    fn snapshot_at(&mut self, now: Instant) -> CallStats {
        let interval = self
            .last_rate_at
            .map(|at| now.saturating_duration_since(at))
            .unwrap_or(Duration::ZERO);
        if !interval.is_zero() {
            let seconds = interval.as_secs_f64();
            let sent_delta = self.snapshot.bytes_sent.saturating_sub(self.last_rate_sent);
            let received_delta = self
                .snapshot
                .bytes_received
                .saturating_sub(self.last_rate_received);
            let sent_rate = (sent_delta as f64 / seconds) as u64 * 8;
            let received_rate = (received_delta as f64 / seconds) as u64 * 8;
            self.snapshot.estimated_send_bitrate =
                ewma(self.snapshot.estimated_send_bitrate, sent_rate);
            self.snapshot.estimated_receive_bitrate =
                ewma(self.snapshot.estimated_receive_bitrate, received_rate);
            self.snapshot.measurement_interval_ms = interval.as_millis() as u64;
        }
        self.last_rate_at = Some(now);
        self.last_rate_sent = self.snapshot.bytes_sent;
        self.last_rate_received = self.snapshot.bytes_received;
        self.snapshot
    }
}

fn ewma(previous: u64, sample: u64) -> u64 {
    if previous == 0 {
        sample
    } else {
        (previous.saturating_mul(7) + sample) / 8
    }
}

/// Bounded, content-free receiver health report design for call protocol v2.
/// This type is deliberately not sent on the v1 wire path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiverReportV2 {
    pub interval_ms: u32,
    pub received_bytes: u32,
    pub received_packets: u32,
    pub video_gap_packets: u32,
    pub video_expired_frames: u16,
    pub video_decode_drops: u16,
    pub estimated_capacity_bps: u32,
    pub rtt_ms: u16,
}

impl ReceiverReportV2 {
    /// Hard upper bound for a report's encoded field values.
    pub const MAX_INTERVAL_MS: u32 = 10_000;

    pub fn from_stats(stats: CallStats) -> Option<Self> {
        (stats.measurement_interval_ms <= Self::MAX_INTERVAL_MS).then_some(Self {
            interval_ms: stats.measurement_interval_ms as u32,
            received_bytes: stats.bytes_received.min(u32::MAX as u64) as u32,
            received_packets: stats
                .audio_packets_received
                .saturating_add(stats.video_packets_received)
                .min(u32::MAX as u64) as u32,
            video_gap_packets: stats.video_packets_lost.min(u32::MAX as u64) as u32,
            video_expired_frames: stats.video_frames_expired.min(u16::MAX as u64) as u16,
            video_decode_drops: stats.video_frames_dropped.min(u16::MAX as u64) as u16,
            estimated_capacity_bps: stats.estimated_receive_bitrate.min(u32::MAX as u64) as u32,
            rtt_ms: stats.rtt.as_millis().min(u16::MAX as u128) as u16,
        })
    }
}

/// Mutable statistics and adaptation controller belonging to one call runtime.
#[derive(Debug, Default)]
pub struct CallStatsRuntime {
    accumulator: Accumulator,
    adaptation: AdaptationController,
}

impl CallStatsRuntime {
    pub fn observe_received(&mut self, packet: &MediaDatagram, arrival: Instant) {
        self.accumulator.observe_received(packet, arrival);
    }
    pub fn observe_malformed(&mut self) {
        self.accumulator.observe_malformed();
    }
    pub fn snapshot(&self) -> CallStats {
        self.accumulator.snapshot
    }
    pub fn snapshot_at(&mut self, now: Instant) -> CallStats {
        self.accumulator.snapshot_at(now)
    }
    pub fn observe_sent(&mut self, bytes: u64, datagrams: u64) {
        self.accumulator.observe_sent(bytes, datagrams);
    }
    pub fn observe_video_expired(&mut self, count: u64) {
        self.accumulator.snapshot.video_frames_expired = self
            .accumulator
            .snapshot
            .video_frames_expired
            .saturating_add(count);
    }
    pub fn observe_video_reassembly_overflow(&mut self) {
        self.accumulator.snapshot.video_reassembly_overflows = self
            .accumulator
            .snapshot
            .video_reassembly_overflows
            .saturating_add(1);
    }
    pub fn adaptation_decision(&self) -> AdaptationDecision {
        self.adaptation.decision()
    }
    pub fn update_adaptation(&mut self) -> (AdaptationDecision, bool) {
        let previous = self.adaptation.decision();
        let decision = self.adaptation.update(self.snapshot());
        (decision, decision != previous)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::CallId;

    fn packet(kind: MediaKind, sequence: u32) -> MediaDatagram {
        MediaDatagram {
            kind,
            flags: 0,
            call_id: CallId::from_bytes([7; 16]),
            track_id: 1,
            sequence,
            timestamp: sequence,
            fragment_index: 0,
            fragment_count: 1,
            payload: vec![1],
        }
    }

    #[test]
    fn independent_runtimes_do_not_share_counters() {
        let now = Instant::now();
        let mut first = CallStatsRuntime::default();
        let second = CallStatsRuntime::default();
        first.observe_received(&packet(MediaKind::Audio, 1), now);
        assert_eq!(first.snapshot().audio_packets_received, 1);
        assert_eq!(second.snapshot().audio_packets_received, 0);
    }

    #[test]
    fn default_runtime_is_a_reconnect_baseline() {
        let mut runtime = CallStatsRuntime::default();
        runtime.observe_received(&packet(MediaKind::Video, 1), Instant::now());
        assert_ne!(runtime.snapshot().video_packets_received, 0);
        runtime = CallStatsRuntime::default();
        assert_eq!(runtime.snapshot(), CallStats::default());
        assert_eq!(
            runtime.adaptation_decision(),
            AdaptationController::default().decision()
        );
    }

    #[test]
    fn one_second_rates_count_wire_bytes_and_use_ewma() {
        let start = Instant::now();
        let mut runtime = CallStatsRuntime::default();
        runtime.observe_sent(1_000, 2);
        runtime.observe_received(&packet(MediaKind::Video, 1), start);
        let first = runtime.snapshot_at(start);
        assert_eq!(first.bytes_sent, 1_000);
        assert_eq!(first.bytes_received, 41);
        assert_eq!(first.measurement_interval_ms, 0);

        runtime.observe_sent(2_000, 3);
        let second = runtime.snapshot_at(start + Duration::from_secs(1));
        assert_eq!(second.measurement_interval_ms, 1_000);
        assert_eq!(second.estimated_send_bitrate, 16_000);
        assert_eq!(second.estimated_receive_bitrate, 328);
    }

    #[test]
    fn video_sequence_gaps_and_report_are_bounded_and_content_free() {
        let start = Instant::now();
        let mut runtime = CallStatsRuntime::default();
        runtime.observe_received(&packet(MediaKind::Video, 10), start);
        runtime.observe_received(
            &packet(MediaKind::Video, 13),
            start + Duration::from_secs(1),
        );
        runtime.observe_video_expired(2);
        let stats = runtime.snapshot_at(start + Duration::from_secs(2));
        assert_eq!(stats.video_packets_lost, 2);
        assert_eq!(stats.video_frames_expired, 2);
        let report = ReceiverReportV2::from_stats(stats).expect("bounded report");
        assert_eq!(report.video_gap_packets, 2);
        assert_eq!(report.video_expired_frames, 2);
    }
}
