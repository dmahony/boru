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
        }
    }
}

#[derive(Debug, Default)]
struct Accumulator {
    snapshot: CallStats,
    last_audio_sequence: Option<u32>,
    last_video_sequence: Option<u32>,
    jitter: Jitter,
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
            }
        }
        if previous.is_none_or(|previous| packet.sequence.wrapping_sub(previous) < 0x8000_0000) {
            *last = Some(packet.sequence);
        }
    }

    fn observe_malformed(&mut self) {
        self.snapshot.video_packets_dropped = self.snapshot.video_packets_dropped.saturating_add(1);
    }
    fn snapshot(&self) -> CallStats {
        self.snapshot
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
        self.accumulator.snapshot()
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
}
