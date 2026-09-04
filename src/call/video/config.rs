//! Validated source of truth for adaptive live-call video configuration.
#![allow(missing_docs)]

use std::time::Duration;

use super::codec::VideoCodec;
use crate::call::bounds::{MAX_VIDEO_FPS, MAX_VIDEO_HEIGHT, MAX_VIDEO_WIDTH};

/// Target and peak bitrate limits for one video profile, in bits per second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoRateEnvelope {
    pub target_bps: u32,
    pub peak_bps: u32,
}

/// Encoder implementation generation used by a video configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EncoderGeneration {
    Software,
    Hardware,
}

/// Temporal/spatial scalability mode requested from an encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScalabilityMode {
    None,
    L1T1,
}

/// Reason a controller selected a different video configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdaptationReason {
    Initial,
    Congestion,
    Recovery,
    SevereCongestion,
    PathCeiling,
    AudioReserve,
    Manual,
}

/// Complete validated configuration shared by capture, codec, packetization,
/// and adaptation code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub keyframe_interval: u32,
    pub bitrate: VideoRateEnvelope,
    pub codec: VideoCodec,
    pub scalability: ScalabilityMode,
    pub encoder_generation: EncoderGeneration,
}

/// Named quality level in the adaptive profile ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VideoProfile {
    Q0,
    Q1,
    Q2,
    Q3,
    Q4,
}

/// Which properties differ between two video configurations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VideoConfigChanges {
    pub bitrate: bool,
    pub cadence: bool,
    pub geometry: bool,
    pub codec: bool,
}

impl VideoConfigChanges {
    pub const fn any(self) -> bool {
        self.bitrate || self.cadence || self.geometry || self.codec
    }
}

impl VideoRateEnvelope {
    pub const fn new(target_bps: u32, peak_bps: u32) -> Self {
        Self {
            target_bps,
            peak_bps,
        }
    }
}

impl VideoConfig {
    pub fn changes_from(self, previous: Self) -> VideoConfigChanges {
        VideoConfigChanges {
            bitrate: self.bitrate.target_bps != previous.bitrate.target_bps
                || self.bitrate.peak_bps != previous.bitrate.peak_bps,
            cadence: self.fps != previous.fps
                || self.keyframe_interval != previous.keyframe_interval,
            geometry: self.width != previous.width || self.height != previous.height,
            codec: self.codec != previous.codec
                || self.scalability != previous.scalability
                || self.encoder_generation != previous.encoder_generation,
        }
    }

    pub fn validate(self) -> Result<(), &'static str> {
        if self.width == 0 || self.height == 0 || self.width % 2 != 0 || self.height % 2 != 0 {
            return Err("video dimensions must be non-zero even values");
        }
        if self.width > MAX_VIDEO_WIDTH || self.height > MAX_VIDEO_HEIGHT {
            return Err("video resolution out of bounds");
        }
        if self.fps == 0 || self.fps > MAX_VIDEO_FPS {
            return Err("video frame rate out of bounds");
        }
        if self.keyframe_interval == 0 || self.keyframe_interval > self.fps.saturating_mul(10) {
            return Err("video keyframe interval out of bounds");
        }
        if self.bitrate.target_bps == 0 || self.bitrate.peak_bps < self.bitrate.target_bps {
            return Err("video bitrate envelope is invalid");
        }
        Ok(())
    }

    pub const fn frame_interval(self) -> Duration {
        Duration::from_nanos(1_000_000_000 / self.fps as u64)
    }

    pub const fn paused(self) -> bool {
        self.fps == 0
    }
}

impl VideoProfile {
    pub const fn config(self) -> VideoConfig {
        let (width, height, fps, target_bps, peak_bps) = match self {
            Self::Q0 => (640, 360, 24, 600_000, 800_000),
            Self::Q1 => (640, 360, 20, 450_000, 600_000),
            Self::Q2 => (480, 270, 15, 300_000, 400_000),
            Self::Q3 => (320, 180, 10, 160_000, 220_000),
            // Paused is represented by a zero cadence; it is intentionally
            // not passed to VideoConfig::validate as an active encoder config.
            Self::Q4 => (640, 360, 0, 1, 1),
        };
        VideoConfig {
            width,
            height,
            fps,
            keyframe_interval: if fps == 0 { 1 } else { fps * 2 },
            bitrate: VideoRateEnvelope::new(target_bps, peak_bps),
            codec: VideoCodec::H264,
            scalability: ScalabilityMode::None,
            encoder_generation: EncoderGeneration::Software,
        }
    }

    pub const fn is_paused(self) -> bool {
        matches!(self, Self::Q4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_adaptive_ladder() {
        assert_eq!(
            VideoProfile::Q0.config().bitrate,
            VideoRateEnvelope::new(600_000, 800_000)
        );
        assert_eq!(
            (
                VideoProfile::Q1.config().width,
                VideoProfile::Q1.config().fps
            ),
            (640, 20)
        );
        assert_eq!(
            (
                VideoProfile::Q2.config().width,
                VideoProfile::Q2.config().height
            ),
            (480, 270)
        );
        assert_eq!(
            VideoProfile::Q3.config().bitrate,
            VideoRateEnvelope::new(160_000, 220_000)
        );
        assert!(VideoProfile::Q4.is_paused());
    }

    #[test]
    fn active_profiles_validate_and_degrade_monotonically() {
        for profile in [
            VideoProfile::Q0,
            VideoProfile::Q1,
            VideoProfile::Q2,
            VideoProfile::Q3,
        ] {
            assert!(profile.config().validate().is_ok());
        }
        for pair in [
            (VideoProfile::Q0, VideoProfile::Q1),
            (VideoProfile::Q1, VideoProfile::Q2),
            (VideoProfile::Q2, VideoProfile::Q3),
        ] {
            let (better, worse) = (pair.0.config(), pair.1.config());
            assert!(worse.bitrate.target_bps <= better.bitrate.target_bps);
            assert!(worse.bitrate.peak_bps <= better.bitrate.peak_bps);
            assert!(worse.fps <= better.fps);
            assert!(worse.width * worse.height <= better.width * better.height);
        }
    }

    #[test]
    fn changes_identify_each_domain() {
        let q0 = VideoProfile::Q0.config();
        let q1 = VideoProfile::Q1.config();
        let changes = q1.changes_from(q0);
        assert!(changes.bitrate && changes.cadence && !changes.geometry && !changes.codec);
        assert!(!q0.changes_from(q0).any());
    }
}
