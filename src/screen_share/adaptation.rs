//! Congestion-driven quality control for screen sharing.
//!
//! The controller has no transport-specific assumptions: direct and relayed
//! connections are treated identically through local pipeline observations.
//!
//! Also hosts the frame-pacing state (PDF Task 7.2): a bounded latest-frame
//! queue between capture and encode that prefers dropping obsolete frames over
//! building latency, caps queue length, and records drop counters.
#![allow(missing_docs)]

use std::collections::VecDeque;

use super::{capture::CapturedFrame, codec::{CodecConfig, DEFAULT_HEIGHT, DEFAULT_WIDTH}, stats::ScreenShareStatsSnapshot, ScreenShareError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityDecision {
    pub config: CodecConfig,
    pub changed: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct AdaptiveQuality {
    base: CodecConfig,
    current: CodecConfig,
    level: u8,
    congested: u8,
    stable: u8,
}

impl AdaptiveQuality {
    pub fn new(base: CodecConfig) -> Self { Self { base, current: base, level: 0, congested: 0, stable: 0 } }
    pub fn config(&self) -> CodecConfig { self.current }
    pub fn level(&self) -> u8 { self.level }
    pub fn update(&mut self, stats: ScreenShareStatsSnapshot) -> QualityDecision {
        let pressure = stats.bytes_in_flight > 512 * 1024
            || stats.late_drops >= 2
            || stats.frame_age_us > 250_000
            || stats.decode_errors > 0;
        if pressure {
            self.congested = self.congested.saturating_add(1);
            self.stable = 0;
            if self.congested >= 3 { self.level = (self.level + 1).min(3); self.congested = 0; }
        } else {
            self.congested = 0;
            self.stable = self.stable.saturating_add(1);
            if self.stable >= 8 { self.level = self.level.saturating_sub(1); self.stable = 0; }
        }
        let next = config_for_level(self.base, self.level);
        let changed = next != self.current;
        self.current = next;
        QualityDecision { config: next, changed }
    }
}

/// Monotonic drop/pacing counters recorded by [`PacingController`]
/// (PDF Task 7.2). Exposed for BORU-SS-28 metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PacingCounters {
    /// Captured frames handed to the queue.
    pub captured: u64,
    /// Frames popped for encoding (the newest when the pipeline fell behind).
    pub encoded: u64,
    /// Frames dropped because the queue was full (obsolete older frames).
    pub dropped_queue_full: u64,
    /// Frames dropped because a newer frame superseded them at pop time.
    pub dropped_obsolete: u64,
}

/// Bounded latest-frame queue between capture and encoding.
///
/// Implements the PDF Task 7.2 pacing policy: when the encoder or network
/// falls behind, obsolete frames are dropped instead of building latency.
/// The queue holds at most `capacity` frames; a push onto a full queue drops
/// the OLDEST frame (counted as `dropped_queue_full`), and `pop_latest`
/// returns only the newest frame, discarding older stale frames (counted as
/// `dropped_obsolete`).
#[derive(Debug)]
pub struct PacingController {
    frames: VecDeque<CapturedFrame>,
    capacity: usize,
    counters: PacingCounters,
}

impl PacingController {
    /// Create a queue that retains at most `capacity` frames.
    pub fn new(capacity: usize) -> Result<Self, ScreenShareError> {
        if capacity == 0 {
            return Err(ScreenShareError::new("pacing capacity must be non-zero"));
        }
        Ok(Self {
            frames: VecDeque::with_capacity(capacity.min(16)),
            capacity,
            counters: PacingCounters::default(),
        })
    }

    /// Maximum number of frames this queue can hold.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Current number of queued frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the queue holds no frames.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Push one captured frame. When the queue is full the OLDEST frame is
    /// dropped to make room (latest-frame-wins) and counted; returns `true`
    /// when that happened.
    pub fn push(&mut self, frame: CapturedFrame) -> bool {
        self.counters.captured = self.counters.captured.saturating_add(1);
        let dropped_oldest = if self.frames.len() >= self.capacity {
            self.frames.pop_front();
            self.counters.dropped_queue_full = self.counters.dropped_queue_full.saturating_add(1);
            true
        } else {
            false
        };
        self.frames.push_back(frame);
        dropped_oldest
    }

    /// Take the NEWEST queued frame, discarding older stale frames. This is
    /// the latest-frame strategy: when the encoder or network fell behind,
    /// every queued frame except the newest is obsolete.
    pub fn pop_latest(&mut self) -> Option<CapturedFrame> {
        let frame = self.frames.pop_back()?;
        let stale = self.frames.len() as u64;
        self.frames.clear();
        self.counters.dropped_obsolete = self.counters.dropped_obsolete.saturating_add(stale);
        self.counters.encoded = self.counters.encoded.saturating_add(1);
        Some(frame)
    }

    /// Discard every queued frame without encoding (e.g. on reconnection).
    /// Obsolete frames are counted as dropped.
    pub fn clear(&mut self) {
        let stale = self.frames.len() as u64;
        self.frames.clear();
        self.counters.dropped_obsolete = self.counters.dropped_obsolete.saturating_add(stale);
    }

    /// Record frames the pipeline skipped because it fell behind.
    ///
    /// A capture loop with `MissedTickBehavior::Skip` coalesces missed ticks
    /// when the previous encode/send round exceeded one frame period; those
    /// frames were implicitly dropped rather than queued (the latest-frame
    /// strategy). They are counted as obsolete so drop pressure is visible to
    /// metrics (BORU-SS-28).
    pub fn note_missed_frames(&mut self, count: u64) {
        self.counters.dropped_obsolete = self.counters.dropped_obsolete.saturating_add(count);
    }

    /// Monotonic pacing counters for diagnostics/metrics (BORU-SS-28).
    pub fn counters(&self) -> PacingCounters {
        self.counters
    }
}

fn config_for_level(base: CodecConfig, level: u8) -> CodecConfig {
    let mut config = base;
    match level {
        1 => config.target_bitrate_bps = base.target_bitrate_bps.saturating_mul(65) / 100,
        2 => { config.target_bitrate_bps = base.target_bitrate_bps.saturating_mul(45) / 100; config.target_fps = (base.target_fps / 2).max(5); }
        3 => { config.target_bitrate_bps = base.target_bitrate_bps.saturating_mul(30) / 100; config.target_fps = (base.target_fps / 2).max(5); config.width = (base.width / 2).max(2) & !1; config.height = (base.height / 2).max(2) & !1; }
        _ => {}
    }
    config.width = config.width.min(DEFAULT_WIDTH).max(2) & !1;
    config.height = config.height.min(DEFAULT_HEIGHT).max(2) & !1;
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stats(backlog: u64, age: u64, late: u64) -> ScreenShareStatsSnapshot {
        ScreenShareStatsSnapshot { sender_fps: 30, encoded_fps: 30, dropped_capture_frames: 0, encode_time_us: 0, bitrate_bps: 0, bytes_in_flight: backlog, media_resets: 0, receiver_fps: 30, decode_time_us: 0, late_drops: late, frame_age_us: age, decoded_frames: 0, rendered_frames: 0, decode_errors: 0 }
    }
    #[test]
    fn sustained_pressure_steps_bitrate_then_fps_then_resolution() {
        let base = CodecConfig::default();
        let mut quality = AdaptiveQuality::new(base);
        for _ in 0..3 { assert!(!quality.update(stats(1024 * 1024, 0, 0)).changed || quality.level() == 1); }
        assert_eq!(quality.level(), 1); assert!(quality.config().target_bitrate_bps < base.target_bitrate_bps);
        for _ in 0..3 { quality.update(stats(1024 * 1024, 300_000, 2)); }
        assert_eq!(quality.level(), 2); assert!(quality.config().target_fps < base.target_fps);
        for _ in 0..3 { quality.update(stats(1024 * 1024, 300_000, 2)); }
        assert_eq!(quality.level(), 3); assert!(quality.config().width < base.width);
    }
    #[test]
    fn recovery_is_gradual_and_hysteretic() {
        let base = CodecConfig::default(); let mut quality = AdaptiveQuality::new(base);
        for _ in 0..3 { quality.update(stats(1024 * 1024, 300_000, 2)); }
        assert_eq!(quality.level(), 1);
        for _ in 0..7 { quality.update(stats(0, 0, 0)); }
        assert_eq!(quality.level(), 1);
        quality.update(stats(0, 0, 0)); assert_eq!(quality.level(), 0);
        assert_eq!(quality.config(), base);
    }

    // ---- PacingController (PDF Task 7.2) ----

    fn pacing_frame(timestamp_us: u64) -> CapturedFrame {
        CapturedFrame::cpu(timestamp_us, 1, 1, super::super::capture::PixelFormat::Bgra8, vec![0; 4]).unwrap()
    }

    #[test]
    fn pacing_queue_cap_is_enforced() {
        let mut pacing = PacingController::new(2).unwrap();
        assert!(PacingController::new(0).is_err(), "capacity must be non-zero");
        assert!(!pacing.push(pacing_frame(1)));
        assert!(!pacing.push(pacing_frame(2)));
        // Third push onto a full queue drops the OLDEST (seq 1), not the newest.
        assert!(pacing.push(pacing_frame(3)), "push onto a full queue must drop the oldest");
        assert_eq!(pacing.len(), 2);
        let counters = pacing.counters();
        assert_eq!(counters.captured, 3);
        assert_eq!(counters.dropped_queue_full, 1);
    }

    #[test]
    fn pacing_drop_counter_increments_on_overflow() {
        let mut pacing = PacingController::new(3).unwrap();
        for ts in 1..=10 {
            pacing.push(pacing_frame(ts));
        }
        let counters = pacing.counters();
        assert_eq!(counters.captured, 10);
        assert_eq!(counters.dropped_queue_full, 7, "7 pushes overflowed the cap-3 queue");
        assert_eq!(pacing.len(), 3);
    }

    #[test]
    fn pacing_latest_frame_wins_under_lag() {
        // Encoder/network falls behind: multiple captures accumulate, but only
        // the newest frame is encoded; the rest are obsolete and dropped.
        let mut pacing = PacingController::new(4).unwrap();
        for ts in 1..=4 {
            pacing.push(pacing_frame(ts));
        }
        let newest = pacing.pop_latest().unwrap();
        assert_eq!(newest.timestamp_us, 4, "newest frame must be selected");
        assert!(pacing.is_empty());
        let counters = pacing.counters();
        assert_eq!(counters.encoded, 1);
        assert_eq!(counters.dropped_obsolete, 3, "3 older frames were obsolete");
    }

    #[test]
    fn pacing_empty_pop_and_clear_are_counted() {
        let mut pacing = PacingController::new(2).unwrap();
        assert!(pacing.pop_latest().is_none(), "empty pop returns None");
        pacing.push(pacing_frame(1));
        pacing.push(pacing_frame(2));
        pacing.clear();
        assert_eq!(pacing.counters().dropped_obsolete, 2, "clear counts queued frames as dropped");
        assert_eq!(pacing.counters().encoded, 0);
    }
}
