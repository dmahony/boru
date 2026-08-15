//! Congestion-driven quality control for screen sharing.
//!
//! The controller has no transport-specific assumptions: direct and relayed
//! connections are treated identically through local pipeline observations.
//!
//! Adaptive quality (PDF Task 7.3) tracks send-queue depth, measured
//! throughput, RTT when available, encode time, and dropped frames; it
//! reduces bitrate/fps/resolution gradually under *sustained* congestion
//! (hysteresis), recovers conservatively after a stable period, and honours
//! an explicit lower-quality request from the viewer ([`ViewerQualityRequest`]).
//!
//! Also hosts the frame-pacing state (PDF Task 7.2): a bounded latest-frame
//! queue between capture and encode that prefers dropping obsolete frames over
//! building latency, caps queue length, and records drop counters.
#![allow(missing_docs)]

use std::collections::VecDeque;

use super::{capture::CapturedFrame, codec::{CodecConfig, DEFAULT_HEIGHT, DEFAULT_WIDTH}, stats::ScreenShareStatsSnapshot, ScreenShareError};

/// Congestion thresholds. All are conservative: a single noisy statistics
/// sample must persist for several control-loop ticks before quality moves.
/// Recovery requires roughly three times as many clean ticks as congestion
/// requires dirty ones, so quality rises slowly and falls quickly.
const QUEUE_FRAMES_PRESSURE: u64 = 2;
const QUEUE_BYTES_PRESSURE: u64 = 512 * 1024;
const RTT_PRESSURE_US: u64 = 250_000;
const FRAME_AGE_PRESSURE_US: u64 = 250_000;
const CONGESTED_TICKS_TO_STEP: u8 = 3;
const STABLE_TICKS_TO_STEP: u8 = 8;
const MAX_LEVEL: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityDecision {
    pub config: CodecConfig,
    pub changed: bool,
}

/// A manual lower-quality request from the viewer (PDF Task 7.3). Carried by
/// the versioned `QualityUpdate` protocol message; the host clamps the
/// controller to this ceiling and adaptive recovery never exceeds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewerQualityRequest {
    /// Requested bitrate ceiling in bits per second.
    pub target_bitrate_bps: u32,
    /// Maximum acceptable frame rate.
    pub max_frame_rate: u16,
    /// Relative scale of the encoded resolution, 1..=100 (100 = full).
    pub scale_factor: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct AdaptiveQuality {
    base: CodecConfig,
    current: CodecConfig,
    level: u8,
    congested: u8,
    stable: u8,
    viewer_request: Option<ViewerQualityRequest>,
    last_dropped_frames: u64,
    last_late_drops: u64,
}

impl AdaptiveQuality {
    pub fn new(base: CodecConfig) -> Self {
        Self { base, current: base, level: 0, congested: 0, stable: 0,
            viewer_request: None, last_dropped_frames: 0, last_late_drops: 0 }
    }
    pub fn config(&self) -> CodecConfig { self.current }
    pub fn level(&self) -> u8 { self.level }
    pub fn viewer_request(&self) -> Option<ViewerQualityRequest> { self.viewer_request }

    /// Apply the viewer's manual lower-quality request (QualityUpdate path).
    ///
    /// The request becomes a hard ceiling: the current config is clamped to it
    /// immediately and the adaptive ladder resets so recovery can never raise
    /// quality above the requested mode. `clear_viewer_request` removes the
    /// ceiling.
    pub fn apply_viewer_request(&mut self, request: ViewerQualityRequest) -> QualityDecision {
        self.viewer_request = Some(request);
        // Start from the requested ceiling: level 0 (best adaptive step)
        // clamped to the request. Congestion can still push below it.
        self.level = 0;
        self.congested = 0;
        self.stable = 0;
        self.last_dropped_frames = 0;
        self.last_late_drops = 0;
        self.recompute()
    }

    /// Remove the viewer's manual quality ceiling and resume full adaptive
    /// behaviour from the best quality step.
    pub fn clear_viewer_request(&mut self) -> QualityDecision {
        self.viewer_request = None;
        self.level = 0;
        self.congested = 0;
        self.stable = 0;
        self.last_dropped_frames = 0;
        self.last_late_drops = 0;
        self.recompute()
    }

    /// Adopt a new *capture* geometry (e.g. a real portal source negotiated a
    /// different size after streaming started). The base resolution follows
    /// the capture so the adaptive ladder and any active viewer ceiling scale
    /// with the source; the current level is preserved.
    pub fn set_capture_geometry(&mut self, width: u32, height: u32) -> QualityDecision {
        let width = width.max(2) & !1;
        let height = height.max(2) & !1;
        if width == self.base.width && height == self.base.height {
            return QualityDecision { config: self.current, changed: false };
        }
        self.base.width = width;
        self.base.height = height;
        self.recompute()
    }

    /// One control-loop tick. Feed it a fresh stats snapshot periodically
    /// (e.g. every 25 encoded frames). Returns the next codec config.
    pub fn update(&mut self, stats: ScreenShareStatsSnapshot) -> QualityDecision {
        // Drops are cumulative counters; use the delta since the last tick so
        // a single burst is visible while an old value never sticks forever.
        let dropped_delta = stats.dropped_frames.saturating_sub(self.last_dropped_frames);
        let late_drops_delta = stats.late_drops.saturating_sub(self.last_late_drops);
        self.last_dropped_frames = stats.dropped_frames;
        self.last_late_drops = stats.late_drops;

        let frame_period_us = (1_000_000 / self.current.target_fps.max(1)) as u64;
        let pressure = stats.send_queue_depth >= QUEUE_FRAMES_PRESSURE
            || stats.bytes_in_flight > QUEUE_BYTES_PRESSURE
            || (stats.measured_throughput_bps > 0
                && stats.measured_throughput_bps >= self.current.target_bitrate_bps as u64
                && stats.send_queue_depth >= 1)
            || (stats.rtt_us > 0 && stats.rtt_us > RTT_PRESSURE_US)
            || (stats.encode_time_avg_us > 0 && stats.encode_time_avg_us > frame_period_us)
            || dropped_delta > 0
            || late_drops_delta > 0
            || stats.frame_age_us > FRAME_AGE_PRESSURE_US
            || stats.decode_errors > 0;
        if pressure {
            self.congested = self.congested.saturating_add(1);
            self.stable = 0;
            if self.congested >= CONGESTED_TICKS_TO_STEP { self.level = (self.level + 1).min(MAX_LEVEL); self.congested = 0; }
        } else {
            self.congested = 0;
            self.stable = self.stable.saturating_add(1);
            if self.stable >= STABLE_TICKS_TO_STEP { self.level = self.level.saturating_sub(1); self.stable = 0; }
        }
        self.recompute()
    }

    fn recompute(&mut self) -> QualityDecision {
        let next = self.effective_config();
        let changed = next != self.current;
        self.current = next;
        QualityDecision { config: next, changed }
    }

    /// Adaptive step at the current level, clamped to the viewer's manual
    /// request ceiling when one is active.
    fn effective_config(&self) -> CodecConfig {
        let mut config = config_for_level(self.base, self.level);
        if let Some(request) = &self.viewer_request {
            let scale = (request.scale_factor as u32).clamp(1, 100);
            let width = ((self.base.width * scale) / 100).max(2) & !1;
            let height = ((self.base.height * scale) / 100).max(2) & !1;
            config.target_bitrate_bps = config.target_bitrate_bps.min(request.target_bitrate_bps);
            config.target_fps = config.target_fps.min(request.max_frame_rate as u32).max(1);
            config.width = config.width.min(width).max(2) & !1;
            config.height = config.height.min(height).max(2) & !1;
        }
        config.width = config.width.min(DEFAULT_WIDTH).max(2) & !1;
        config.height = config.height.min(DEFAULT_HEIGHT).max(2) & !1;
        config
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
        ScreenShareStatsSnapshot { sender_fps: 30, encoded_fps: 30, dropped_capture_frames: 0, skipped_frames: 0, encode_time_us: 0, bitrate_bps: 0, bytes_in_flight: backlog, media_resets: 0, receiver_fps: 30, decode_time_us: 0, late_drops: late, frame_age_us: age, decoded_frames: 0, rendered_frames: 0, decode_errors: 0, keyframe_requests: 0, send_queue_depth: 0, measured_throughput_bps: 0, encode_time_avg_us: 0, rtt_us: 0, dropped_frames: 0 }
    }
    fn base_stats() -> ScreenShareStatsSnapshot { stats(0, 0, 0) }
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

    // ---- PDF Task 7.3: adaptive quality signals ----

    #[test]
    fn queue_depth_pressure_steps_down() {
        let base = CodecConfig::default();
        let mut quality = AdaptiveQuality::new(base);
        // Send queue backing up (2+ frames waiting in the media channel) is
        // congestion even when no bytes are in flight yet.
        for _ in 0..3 {
            let mut s = base_stats();
            s.send_queue_depth = 2;
            quality.update(s);
        }
        assert_eq!(quality.level(), 1);
        assert!(quality.config().target_bitrate_bps < base.target_bitrate_bps);
    }

    #[test]
    fn rtt_pressure_steps_down_when_available() {
        let base = CodecConfig::default();
        let mut quality = AdaptiveQuality::new(base);
        // Elevated RTT (when measured) is a congestion signal.
        for _ in 0..3 {
            let mut s = base_stats();
            s.rtt_us = 300_000;
            quality.update(s);
        }
        assert_eq!(quality.level(), 1);
    }

    #[test]
    fn encode_time_pressure_steps_down_when_cpu_bound() {
        let base = CodecConfig::default();
        let mut quality = AdaptiveQuality::new(base);
        // Encode takes longer than one frame period (33ms at 30fps): the
        // encoder cannot keep up, so the controller reduces load.
        for _ in 0..3 {
            let mut s = base_stats();
            s.encode_time_avg_us = 40_000;
            quality.update(s);
        }
        assert_eq!(quality.level(), 1);
        assert!(quality.config().target_fps < base.target_fps || quality.config().target_bitrate_bps < base.target_bitrate_bps);
    }

    #[test]
    fn throughput_saturation_is_pressure_only_with_queue_growth() {
        let base = CodecConfig::default();
        let mut quality = AdaptiveQuality::new(base);
        // Measured throughput at/above the target bitrate with a growing
        // queue signals a saturated link.
        for _ in 0..3 {
            let mut s = base_stats();
            s.measured_throughput_bps = base.target_bitrate_bps as u64;
            s.send_queue_depth = 1;
            quality.update(s);
        }
        assert_eq!(quality.level(), 1);
    }

    #[test]
    fn dropped_frame_burst_is_pressure_but_does_not_stick() {
        let base = CodecConfig::default();
        let mut quality = AdaptiveQuality::new(base);
        // A burst of dropped frames (delta each tick) is pressure...
        let mut dropped = 5u64;
        for _ in 0..3 {
            let mut s = base_stats();
            dropped += 1;
            s.dropped_frames = dropped;
            quality.update(s);
        }
        assert_eq!(quality.level(), 1);
        // ...but an old cumulative value with no new drops is NOT pressure:
        // the counter stays flat and the controller recovers conservatively.
        let mut s = base_stats();
        s.dropped_frames = dropped;
        for _ in 0..8 { quality.update(s); }
        assert_eq!(quality.level(), 0);
        assert_eq!(quality.config(), base);
    }

    #[test]
    fn manual_viewer_request_is_honored_as_a_ceiling() {
        let base = CodecConfig::default();
        let mut quality = AdaptiveQuality::new(base);
        let request = ViewerQualityRequest { target_bitrate_bps: 1_000_000, max_frame_rate: 10, scale_factor: 50 };
        let decision = quality.apply_viewer_request(request);
        assert!(decision.changed, "a lower-quality request must change the config immediately");
        assert_eq!(quality.viewer_request(), Some(request));
        let ceiling = decision.config;
        assert_eq!(ceiling.target_bitrate_bps, 1_000_000);
        assert_eq!(ceiling.target_fps, 10);
        assert_eq!(ceiling.width, (base.width / 2) & !1);
        assert_eq!(ceiling.height, (base.height / 2) & !1);
        // Even a long stable period never raises quality above the request.
        for _ in 0..32 { quality.update(base_stats()); }
        assert_eq!(quality.config(), ceiling);
        assert_eq!(quality.level(), 0);
        // Clearing the request restores full adaptive behaviour.
        let cleared = quality.clear_viewer_request();
        assert!(cleared.changed);
        assert_eq!(quality.viewer_request(), None);
        assert_eq!(quality.config(), base);
    }

    #[test]
    fn congestion_still_reduces_below_a_viewer_ceiling() {
        let base = CodecConfig::default();
        let mut quality = AdaptiveQuality::new(base);
        // A generous ceiling (3 Mbps on a 4 Mbps base) still allows adaptive
        // reduction below it under sustained congestion.
        let request = ViewerQualityRequest { target_bitrate_bps: 3_000_000, max_frame_rate: 30, scale_factor: 100 };
        let decision = quality.apply_viewer_request(request);
        assert_eq!(decision.config.target_bitrate_bps, 3_000_000);
        for _ in 0..3 { quality.update(stats(1024 * 1024, 0, 0)); }
        assert_eq!(quality.level(), 1);
        assert!(quality.config().target_bitrate_bps < 3_000_000, "congestion reduces below the ceiling");
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
