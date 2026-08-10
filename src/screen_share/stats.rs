//! Local-only screen-share pipeline statistics.
//!
//! Counters are monotonic and snapshots derive rates from a monotonic clock. No
//! peer identity, addresses, or payloads are retained or exported.
#![allow(missing_docs)]

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenShareStatsSnapshot {
    pub sender_fps: u32,
    pub encoded_fps: u32,
    pub dropped_capture_frames: u64,
    pub encode_time_us: u64,
    pub bitrate_bps: u64,
    pub bytes_in_flight: u64,
    pub media_resets: u64,
    pub receiver_fps: u32,
    pub decode_time_us: u64,
    pub late_drops: u64,
    pub frame_age_us: u64,
    pub decoded_frames: u64,
    pub rendered_frames: u64,
    pub decode_errors: u64,
}

#[derive(Debug)]
pub struct ScreenShareStats {
    started: Instant,
    last_snapshot: Instant,
    captured: u64,
    encoded: u64,
    decoded: u64,
    rendered: u64,
    dropped_capture_frames: u64,
    late_drops: u64,
    decode_errors: u64,
    encode_time_us: u64,
    decode_time_us: u64,
    bytes_sent: u64,
    bytes_in_flight: u64,
    media_resets: u64,
    frame_age_us: u64,
}

impl Default for ScreenShareStats {
    fn default() -> Self { Self::new() }
}

impl ScreenShareStats {
    pub fn new() -> Self {
        let now = Instant::now();
        Self { started: now, last_snapshot: now, captured: 0, encoded: 0, decoded: 0,
            rendered: 0, dropped_capture_frames: 0, late_drops: 0, decode_errors: 0,
            encode_time_us: 0, decode_time_us: 0, bytes_sent: 0, bytes_in_flight: 0,
            media_resets: 0, frame_age_us: 0 }
    }
    pub fn observe_capture(&mut self) { self.captured = self.captured.saturating_add(1); }
    pub fn observe_capture_drop(&mut self) { self.dropped_capture_frames = self.dropped_capture_frames.saturating_add(1); }
    pub fn observe_encode(&mut self, elapsed: Duration) { self.encoded = self.encoded.saturating_add(1); self.encode_time_us = self.encode_time_us.saturating_add(elapsed.as_micros() as u64); }
    pub fn observe_send(&mut self, bytes: usize) { self.bytes_sent = self.bytes_sent.saturating_add(bytes as u64); }
    pub fn observe_send_delay(&mut self, elapsed: Duration) {
        self.frame_age_us = self.frame_age_us.max(elapsed.as_micros().min(u64::MAX as u128) as u64);
    }
    pub fn set_bytes_in_flight(&mut self, bytes: u64) { self.bytes_in_flight = bytes; }
    pub fn observe_receive(&mut self, timestamp_us: u64, now: Instant) {
        self.frame_age_us = now
            .saturating_duration_since(self.started)
            .as_micros()
            .saturating_sub(timestamp_us as u128)
            .min(u64::MAX as u128) as u64;
    }
    pub fn observe_decode(&mut self, elapsed: Duration, error: bool) {
        self.decoded = self.decoded.saturating_add(1);
        self.decode_time_us = self.decode_time_us.saturating_add(elapsed.as_micros() as u64);
        if error { self.decode_errors = self.decode_errors.saturating_add(1); }
    }
    pub fn observe_render(&mut self) { self.rendered = self.rendered.saturating_add(1); }
    pub fn observe_late_drop(&mut self) { self.late_drops = self.late_drops.saturating_add(1); }
    pub fn observe_media_reset(&mut self) { self.media_resets = self.media_resets.saturating_add(1); }
    pub fn snapshot(&mut self) -> ScreenShareStatsSnapshot {
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_snapshot).max(Duration::from_millis(1));
        let seconds = elapsed.as_secs_f64();
        self.last_snapshot = now;
        ScreenShareStatsSnapshot {
            sender_fps: (self.captured as f64 / seconds).round() as u32,
            encoded_fps: (self.encoded as f64 / seconds).round() as u32,
            dropped_capture_frames: self.dropped_capture_frames,
            encode_time_us: self.encode_time_us,
            bitrate_bps: (self.bytes_sent as f64 * 8.0 / now.saturating_duration_since(self.started).as_secs_f64().max(0.001)) as u64,
            bytes_in_flight: self.bytes_in_flight,
            media_resets: self.media_resets,
            receiver_fps: (self.decoded as f64 / seconds).round() as u32,
            decode_time_us: self.decode_time_us,
            late_drops: self.late_drops,
            frame_age_us: self.frame_age_us,
            decoded_frames: self.decoded,
            rendered_frames: self.rendered,
            decode_errors: self.decode_errors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_contains_pipeline_stages_and_monotonic_counters() {
        let mut stats = ScreenShareStats::new();
        stats.observe_capture(); stats.observe_capture_drop(); stats.observe_encode(Duration::from_micros(12));
        stats.observe_send(1_000); stats.set_bytes_in_flight(55); stats.observe_receive(0, Instant::now());
        stats.observe_decode(Duration::from_micros(8), false); stats.observe_render(); stats.observe_late_drop(); stats.observe_media_reset();
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.dropped_capture_frames, 1);
        assert_eq!(snapshot.bytes_in_flight, 55);
        assert_eq!(snapshot.late_drops, 1);
        assert_eq!(snapshot.media_resets, 1);
        assert!(snapshot.encode_time_us >= 12 && snapshot.decode_time_us >= 8);
    }
}
