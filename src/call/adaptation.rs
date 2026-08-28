//! Congestion adaptation for live calls.
//!
//! Audio is deliberately kept alive at every adaptation level. Video is the
//! elastic part of a call: its bitrate is reduced first, then frame rate, and
//! finally resolution. Decisions are emitted only after hysteresis thresholds
//! are met so one noisy statistics sample cannot make quality flap.

use super::manager::CallStats;

/// The video dimensions selected by the adaptation controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoResolution {
    /// Horizontal pixel count.
    pub width: u32,
    /// Vertical pixel count.
    pub height: u32,
}

/// Audio adaptation. The bitrate remains in the voice-safe 16–40 kbps band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioAdaptation {
    /// Target Opus bitrate in kilobits per second.
    pub bitrate_kbps: u32,
}

/// Video hint ordered from least to most aggressive degradation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoAdaptationHint {
    /// Target video bitrate in kilobits per second.
    pub bitrate_kbps: u32,
    /// Target video frame rate.
    pub fps: u32,
    /// Target encoded frame dimensions.
    pub resolution: VideoResolution,
}

/// The complete decision for one statistics snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdaptationDecision {
    /// Audio must remain available, even at the most congested level.
    pub audio: AudioAdaptation,
    /// Video quality is reduced before audio is impaired.
    pub video: VideoAdaptationHint,
}

impl Default for AdaptationDecision {
    fn default() -> Self {
        Self {
            audio: AudioAdaptation { bitrate_kbps: 32 },
            video: VideoAdaptationHint {
                bitrate_kbps: 2_500,
                fps: 30,
                resolution: VideoResolution {
                    width: 1280,
                    height: 720,
                },
            },
        }
    }
}

/// Congestion controller with two-sample worsening and three-sample recovery
/// hysteresis. `level` is the degradation order: bitrate, fps, resolution.
#[derive(Debug, Clone, Copy, Default)]
pub struct AdaptationController {
    decision: AdaptationDecision,
    level: u8,
    congested_samples: u8,
    healthy_samples: u8,
    previous: Option<CallStats>,
}

impl AdaptationController {
    /// Evaluate one cumulative statistics snapshot.
    pub fn update(&mut self, stats: CallStats) -> AdaptationDecision {
        let pressure = self.pressure(stats);
        self.previous = Some(stats);
        if pressure > 0 {
            self.healthy_samples = 0;
            self.congested_samples = self.congested_samples.saturating_add(1);
            if self.congested_samples >= 2 {
                self.level = self.level.saturating_add(1).min(3);
                self.congested_samples = 0;
            }
        } else {
            self.congested_samples = 0;
            self.healthy_samples = self.healthy_samples.saturating_add(1);
            if self.healthy_samples >= 3 {
                self.level = self.level.saturating_sub(1);
                self.healthy_samples = 0;
            }
        }
        self.decision = decision_for_level(self.level);
        self.decision
    }

    /// Current decision, useful when applying it to a newly-created encoder.
    pub const fn decision(&self) -> AdaptationDecision {
        self.decision
    }

    /// Current degradation level: 0 is normal, 1 bitrate, 2 fps, 3 resolution.
    pub const fn level(&self) -> u8 {
        self.level
    }

    fn pressure(&self, current: CallStats) -> u8 {
        let Some(previous) = self.previous else {
            return 0;
        };
        let audio_loss = current.audio_packets_lost > previous.audio_packets_lost;
        let audio_underrun = current.audio_playback_underruns > previous.audio_playback_underruns;
        let video_drop = current.video_packets_dropped > previous.video_packets_dropped
            || current.video_frames_dropped > previous.video_frames_dropped;
        let bitrate_drop = current.estimated_send_bitrate > 0
            && previous.estimated_send_bitrate > 0
            && current.estimated_send_bitrate < previous.estimated_send_bitrate * 85 / 100;
        u8::from(audio_loss || audio_underrun || video_drop || bitrate_drop)
    }
}

fn decision_for_level(level: u8) -> AdaptationDecision {
    let mut decision = AdaptationDecision::default();
    match level {
        1 => {
            decision.audio.bitrate_kbps = 24;
            decision.video.bitrate_kbps = 1_500;
        }
        2 => {
            decision.audio.bitrate_kbps = 20;
            decision.video.bitrate_kbps = 1_000;
            decision.video.fps = 15;
        }
        3 => {
            decision.audio.bitrate_kbps = 16;
            decision.video.bitrate_kbps = 700;
            decision.video.fps = 15;
            decision.video.resolution = VideoResolution {
                width: 640,
                height: 360,
            };
        }
        _ => {}
    }
    decision
}

#[cfg(test)]
mod tests {
    use super::*;

    fn congested(stats: &mut CallStats) {
        stats.audio_packets_lost += 1;
        stats.video_frames_dropped += 1;
    }

    #[test]
    fn congestion_preserves_audio_and_degrades_video_in_order() {
        let mut controller = AdaptationController::default();
        let mut stats = CallStats::default();
        assert_eq!(controller.update(stats), AdaptationDecision::default());

        congested(&mut stats);
        assert_eq!(controller.update(stats).video.bitrate_kbps, 2_500);
        congested(&mut stats);
        let bitrate = controller.update(stats);
        assert_eq!(bitrate.audio.bitrate_kbps, 24);
        assert_eq!(bitrate.video.fps, 30);
        assert_eq!(bitrate.video.resolution.width, 1280);

        congested(&mut stats);
        controller.update(stats);
        congested(&mut stats);
        let fps = controller.update(stats);
        assert_eq!(fps.audio.bitrate_kbps, 20);
        assert_eq!(fps.video.fps, 15);
        assert_eq!(fps.video.resolution.width, 1280);

        congested(&mut stats);
        controller.update(stats);
        congested(&mut stats);
        let resolution = controller.update(stats);
        assert_eq!(resolution.audio.bitrate_kbps, 16);
        assert_eq!(resolution.video.resolution.width, 640);
        assert!(resolution.audio.bitrate_kbps >= 16);
    }

    #[test]
    fn hysteresis_prevents_oscillation() {
        let mut controller = AdaptationController::default();
        let mut stats = CallStats::default();
        controller.update(stats);
        congested(&mut stats);
        controller.update(stats);
        congested(&mut stats);
        controller.update(stats);
        assert_eq!(controller.level(), 1);

        // Two healthy samples are insufficient to recover.
        controller.update(stats);
        controller.update(stats);
        assert_eq!(controller.level(), 1);
        controller.update(stats);
        assert_eq!(controller.level(), 0);
    }
}
