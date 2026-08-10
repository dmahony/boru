//! Congestion-driven quality control for screen sharing.
//!
//! The controller has no transport-specific assumptions: direct and relayed
//! connections are treated identically through local pipeline observations.
#![allow(missing_docs)]

use super::{codec::{CodecConfig, DEFAULT_HEIGHT, DEFAULT_WIDTH}, stats::ScreenShareStatsSnapshot};

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
}
