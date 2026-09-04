//! Measured, audio-first quality adaptation for live calls.
//!
//! The controller consumes cumulative call statistics and changes quality only
//! after sustained evidence.  Audio is reserved first; video is the elastic
//! portion of the path.  Path ceilings are hard upper bounds and recovery is
//! deliberately slower than degradation.
#![allow(missing_docs)]

use super::stats::CallStats;
use super::video::config::VideoProfile;

const CONGESTED_SAMPLES_TO_STEP: u8 = 3;
const HEALTHY_SAMPLES_TO_STEP: u8 = 8;
const AUDIO_RESERVE_KBPS: u64 = 40;
const SEVERE_RTT: std::time::Duration = std::time::Duration::from_millis(750);

/// The five fixed adaptive quality levels, from best to audio-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QualityLevel {
    Q0,
    Q1,
    Q2,
    Q3,
    Q4,
}

impl QualityLevel {
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Q0 => 0,
            Self::Q1 => 1,
            Self::Q2 => 2,
            Self::Q3 => 3,
            Self::Q4 => 4,
        }
    }
    pub const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Q1,
            2 => Self::Q2,
            3 => Self::Q3,
            4.. => Self::Q4,
            _ => Self::Q0,
        }
    }
}

/// The video dimensions selected by the adaptation controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoResolution {
    pub width: u32,
    pub height: u32,
}

/// Audio remains in a voice-safe bitrate band at every quality level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioAdaptation {
    pub bitrate_kbps: u32,
}

/// Video configuration projected from the fixed Q0-Q4 geometry ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoAdaptationHint {
    pub bitrate_kbps: u32,
    pub fps: u32,
    pub resolution: VideoResolution,
}

/// The path's maximum permitted video configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathCeiling {
    pub bitrate_kbps: u32,
    pub fps: u32,
    pub resolution: VideoResolution,
}

impl Default for PathCeiling {
    fn default() -> Self {
        Self {
            bitrate_kbps: u32::MAX,
            fps: u32::MAX,
            resolution: VideoResolution {
                width: u32::MAX,
                height: u32::MAX,
            },
        }
    }
}

/// Stable machine-readable explanation for the selected level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptationReason {
    Initial,
    Congestion,
    Recovery,
    Severe,
    PathCeiling,
    AudioReserve,
}

impl AdaptationReason {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Congestion => "congestion",
            Self::Recovery => "recovery",
            Self::Severe => "severe",
            Self::PathCeiling => "path_ceiling",
            Self::AudioReserve => "audio_reserve",
        }
    }
}

/// Result of one adaptation evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdaptationDecision {
    pub audio: AudioAdaptation,
    pub video: VideoAdaptationHint,
    pub level: QualityLevel,
    pub reason: AdaptationReason,
    pub reason_code: &'static str,
    /// True only when the effective media configuration changed.
    pub changed: bool,
}

impl Default for AdaptationDecision {
    fn default() -> Self {
        decision_for(
            QualityLevel::Q0,
            AdaptationReason::Initial,
            PathCeiling::default(),
            u32::MAX,
        )
    }
}

/// Stateful measured quality policy.
#[derive(Debug, Clone, Copy)]
pub struct AdaptationController {
    decision: AdaptationDecision,
    level: QualityLevel,
    congested_samples: u8,
    healthy_samples: u8,
    previous: Option<CallStats>,
    path_ceiling: PathCeiling,
}

impl Default for AdaptationController {
    fn default() -> Self {
        Self {
            decision: AdaptationDecision::default(),
            level: QualityLevel::Q0,
            congested_samples: 0,
            healthy_samples: 0,
            previous: None,
            path_ceiling: PathCeiling::default(),
        }
    }
}

impl AdaptationController {
    pub fn update(&mut self, stats: CallStats) -> AdaptationDecision {
        let severe = self.severe(stats);
        let pressure = severe || self.pressure(stats);
        let mut reason = self.decision.reason;
        if severe {
            self.level = QualityLevel::from_u8(self.level.as_u8().saturating_add(1));
            self.congested_samples = 0;
            self.healthy_samples = 0;
            reason = AdaptationReason::Severe;
        } else if pressure {
            self.healthy_samples = 0;
            self.congested_samples = self.congested_samples.saturating_add(1);
            if self.congested_samples >= CONGESTED_SAMPLES_TO_STEP {
                self.level = QualityLevel::from_u8(self.level.as_u8().saturating_add(1));
                self.congested_samples = 0;
                reason = AdaptationReason::Congestion;
            }
        } else {
            self.congested_samples = 0;
            self.healthy_samples = self.healthy_samples.saturating_add(1);
            if self.healthy_samples >= HEALTHY_SAMPLES_TO_STEP && self.level != QualityLevel::Q0 {
                self.level = QualityLevel::from_u8(self.level.as_u8().saturating_sub(1));
                self.healthy_samples = 0;
                reason = AdaptationReason::Recovery;
            }
        }
        self.previous = Some(stats);
        let reserve_limited = stats.estimated_send_bitrate > 0
            && stats.estimated_send_bitrate / 1000
                <= AUDIO_RESERVE_KBPS + self.effective_video_bitrate(stats) as u64;
        if reserve_limited && !pressure {
            reason = AdaptationReason::AudioReserve;
        }
        let path_limited = self.level_config().bitrate_kbps > self.path_ceiling.bitrate_kbps
            || self.level_config().fps > self.path_ceiling.fps;
        if path_limited {
            reason = AdaptationReason::PathCeiling;
        }
        let next = decision_for(
            self.level,
            reason,
            self.path_ceiling,
            self.configured_bitrate(stats),
        );
        let changed = next.audio != self.decision.audio || next.video != self.decision.video;
        self.decision = AdaptationDecision { changed, ..next };
        self.decision
    }

    pub const fn decision(&self) -> AdaptationDecision {
        self.decision
    }
    pub const fn level(&self) -> QualityLevel {
        self.level
    }
    pub const fn path_ceiling(&self) -> PathCeiling {
        self.path_ceiling
    }

    /// Replace the path ceiling. Lower ceilings clamp immediately; raising it
    /// only provides headroom and still requires normal recovery hysteresis.
    pub fn set_path_ceiling(&mut self, ceiling: PathCeiling) -> AdaptationDecision {
        let previous = self.decision;
        let lowered = ceiling.bitrate_kbps < self.path_ceiling.bitrate_kbps
            || ceiling.fps < self.path_ceiling.fps;
        self.path_ceiling = ceiling;
        let mut next = decision_for(self.level, AdaptationReason::PathCeiling, ceiling, u32::MAX);
        if !lowered {
            next = self.decision;
        }
        next.changed = next.video != previous.video || next.audio != previous.audio;
        self.decision = next;
        self.decision
    }

    fn level_config(&self) -> VideoAdaptationHint {
        hint_for(self.level)
    }
    fn configured_bitrate(&self, stats: CallStats) -> u32 {
        self.effective_video_bitrate(stats)
    }
    fn effective_video_bitrate(&self, stats: CallStats) -> u32 {
        let configured = self.level_config().bitrate_kbps;
        if stats.estimated_send_bitrate == 0 {
            return configured;
        }
        configured.min(
            stats
                .estimated_send_bitrate
                .saturating_div(1000)
                .saturating_sub(AUDIO_RESERVE_KBPS) as u32,
        )
    }
    fn pressure(&self, current: CallStats) -> bool {
        let previous = self.previous.unwrap_or_default();
        current.audio_packets_lost > previous.audio_packets_lost
            || current.audio_playback_underruns > previous.audio_playback_underruns
            || current.video_packets_dropped > previous.video_packets_dropped
            || current.video_frames_dropped > previous.video_frames_dropped
            || (current.estimated_send_bitrate > 0
                && previous.estimated_send_bitrate > 0
                && current.estimated_send_bitrate
                    < previous.estimated_send_bitrate.saturating_mul(85) / 100)
            || current.rtt > std::time::Duration::from_millis(250)
    }
    fn severe(&self, current: CallStats) -> bool {
        current.audio_playback_underruns > self.previous.map_or(0, |p| p.audio_playback_underruns)
            || current
                .audio_packets_lost
                .saturating_sub(self.previous.map_or(0, |p| p.audio_packets_lost))
                >= 3
            || current.video_frames_expired > self.previous.map_or(0, |p| p.video_frames_expired)
            || current.rtt >= SEVERE_RTT
    }
}

impl From<QualityLevel> for VideoProfile {
    fn from(level: QualityLevel) -> Self {
        match level {
            QualityLevel::Q0 => Self::Q0,
            QualityLevel::Q1 => Self::Q1,
            QualityLevel::Q2 => Self::Q2,
            QualityLevel::Q3 => Self::Q3,
            QualityLevel::Q4 => Self::Q4,
        }
    }
}

fn hint_for(level: QualityLevel) -> VideoAdaptationHint {
    let c = level_config(level);
    VideoAdaptationHint {
        bitrate_kbps: c.0,
        fps: c.1,
        resolution: VideoResolution {
            width: c.2,
            height: c.3,
        },
    }
}
fn level_config(level: QualityLevel) -> (u32, u32, u32, u32) {
    let c = VideoProfile::from(level).config();
    (c.bitrate.target_bps / 1000, c.fps, c.width, c.height)
}
fn decision_for(
    level: QualityLevel,
    reason: AdaptationReason,
    ceiling: PathCeiling,
    measured: u32,
) -> AdaptationDecision {
    let mut video = hint_for(level);
    video.bitrate_kbps = video
        .bitrate_kbps
        .min(ceiling.bitrate_kbps)
        .min(measured.max(1));
    video.fps = video
        .fps
        .min(ceiling.fps)
        .max(if level == QualityLevel::Q4 { 0 } else { 1 });
    video.resolution.width = video.resolution.width.min(ceiling.resolution.width);
    video.resolution.height = video.resolution.height.min(ceiling.resolution.height);
    let audio = AudioAdaptation {
        bitrate_kbps: match level {
            QualityLevel::Q0 => 40,
            QualityLevel::Q1 => 32,
            QualityLevel::Q2 => 24,
            QualityLevel::Q3 | QualityLevel::Q4 => 16,
        },
    };
    AdaptationDecision {
        audio,
        video,
        level,
        reason,
        reason_code: reason.code(),
        changed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pressure(s: &mut CallStats) {
        s.video_frames_dropped += 1;
    }
    #[test]
    fn q0_to_q4_uses_hysteresis_and_audio_reserve() {
        let mut c = AdaptationController::default();
        let mut s = CallStats::default();
        for expected in [
            QualityLevel::Q1,
            QualityLevel::Q2,
            QualityLevel::Q3,
            QualityLevel::Q4,
        ] {
            for _ in 0..3 {
                pressure(&mut s);
                c.update(s);
            }
            assert_eq!(c.level(), expected);
        }
        assert!(c.decision().audio.bitrate_kbps >= 16);
    }
    #[test]
    fn clean_samples_recover_slowly() {
        let mut c = AdaptationController::default();
        let mut s = CallStats::default();
        for _ in 0..6 {
            pressure(&mut s);
            c.update(s);
        }
        assert!(c.level() >= QualityLevel::Q1);
        let level = c.level();
        for _ in 0..7 {
            c.update(s);
        }
        assert_eq!(c.level(), level);
        c.update(s);
        assert!(c.level() < level);
    }
    #[test]
    fn identical_configuration_reports_unchanged() {
        let mut c = AdaptationController::default();
        let s = CallStats::default();
        assert!(!c.update(s).changed);
        assert!(!c.update(s).changed);
    }
    #[test]
    fn path_ceiling_clamps_video() {
        let mut c = AdaptationController::default();
        let d = c.set_path_ceiling(PathCeiling {
            bitrate_kbps: 100,
            fps: 5,
            resolution: VideoResolution {
                width: 320,
                height: 180,
            },
        });
        assert_eq!(d.video.bitrate_kbps, 100);
        assert_eq!(d.video.fps, 5);
        assert_eq!(d.video.resolution.width, 320);
    }
}
