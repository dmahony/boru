//! Named quality presets selected from the connection path (LAN/direct vs relay).
//!
//! BORU-SS-39 (PDF Phase 14, T14): when the host starts a session it reads
//! the iroh selected path kind (`PathKind::{Direct,Relay}` from
//! `transport.rs`) and picks an initial quality preset — a direct/LAN path
//! can afford higher bitrate/fps, a relayed path gets a conservative ceiling
//! so a public relay is never saturated. The user can override the preset at
//! any time, and mid-session Direct↔Relay path switches feed `AdaptiveQuality`
//! as a ceiling signal (conservative, never a sudden jump — see
//! `AdaptiveQuality::set_ceiling`/`override_ceiling`).
//!
//! Presets map onto the existing [`QualityProfile`] knob (codec complexity /
//! QP range) plus relative bitrate/fps multipliers, so they scale with
//! whatever base rate the capture session negotiated (the default 4 Mbps /
//! 30 fps, or a future negotiated profile).
#![allow(missing_docs)]

use super::codec::{CodecConfig, QualityProfile};
use super::transport::PathKind;

/// How aggressively the stream should target quality versus latency.
///
/// `LanHigh` favours crispness on fast direct/LAN paths; `Balanced` is the
/// existing default; `RelayConservative` favours latency and bandwidth
/// frugality over a relayed path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QualityPreset {
    /// Direct/LAN path: high bitrate, full frame rate, crispest encode.
    LanHigh,
    /// Default balanced profile (the existing `CodecConfig::default`).
    #[default]
    Balanced,
    /// Relay path: conservative bitrate/fps ceiling, lowest-latency encode.
    RelayConservative,
}

impl QualityPreset {
    /// Stable human/machine-readable name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::LanHigh => "lan-high",
            Self::Balanced => "balanced",
            Self::RelayConservative => "relay-conservative",
        }
    }

    /// Stable wire/API value: 0 = Balanced, 1 = LanHigh, 2 = RelayConservative.
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Balanced => 0,
            Self::LanHigh => 1,
            Self::RelayConservative => 2,
        }
    }

    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Balanced),
            1 => Some(Self::LanHigh),
            2 => Some(Self::RelayConservative),
            _ => None,
        }
    }

    /// Select the initial preset from the connection path kind. A direct
    /// path is assumed to be LAN-class (high headroom); a relayed path gets
    /// the conservative ceiling; unknown (not yet selected) falls back to
    /// the balanced default.
    pub const fn for_path(kind: PathKind) -> Self {
        match kind {
            PathKind::Direct => Self::LanHigh,
            PathKind::Relay => Self::RelayConservative,
            PathKind::Unknown => Self::Balanced,
        }
    }

    /// The quality/latency profile this preset maps onto (codec complexity
    /// and QP range — see [`QualityProfile`]).
    pub const fn quality_profile(self) -> QualityProfile {
        match self {
            Self::LanHigh => QualityProfile::HighQuality,
            Self::Balanced => QualityProfile::Balanced,
            Self::RelayConservative => QualityProfile::LowLatency,
        }
    }

    /// Apply the preset's rate/profile ceiling to a config, relative to the
    /// config's current rates. The capture geometry (width/height/fps) is
    /// left intact — only the bitrate target, the fps ceiling and the
    /// quality profile move.
    ///
    /// - `LanHigh`: 2× bitrate (capped at 12 Mbps), full frame rate,
    ///   `HighQuality` encode.
    /// - `Balanced`: identity.
    /// - `RelayConservative`: 50% bitrate (floor 500 kbps), fps capped at
    ///   20, `LowLatency` encode.
    pub fn apply_to_config(self, config: &mut CodecConfig) {
        match self {
            Self::LanHigh => {
                config.target_bitrate_bps = (config.target_bitrate_bps * 2).clamp(1, 12_000_000);
            }
            Self::Balanced => {}
            Self::RelayConservative => {
                config.target_bitrate_bps = (config.target_bitrate_bps / 2).max(500_000);
                config.target_fps = config.target_fps.min(20);
            }
        }
        config.quality_profile = self.quality_profile();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_kind_selects_initial_preset() {
        assert_eq!(
            QualityPreset::for_path(PathKind::Direct),
            QualityPreset::LanHigh
        );
        assert_eq!(
            QualityPreset::for_path(PathKind::Relay),
            QualityPreset::RelayConservative
        );
        assert_eq!(
            QualityPreset::for_path(PathKind::Unknown),
            QualityPreset::Balanced
        );
    }

    #[test]
    fn lan_preset_raises_bitrate_and_keeps_capture_fps() {
        let mut config = CodecConfig::default();
        QualityPreset::LanHigh.apply_to_config(&mut config);
        assert_eq!(config.quality_profile, QualityProfile::HighQuality);
        assert!(config.target_bitrate_bps > CodecConfig::default().target_bitrate_bps);
        // The frame rate is the capture's choice; the preset never invents
        // frames the capture backend does not produce.
        assert_eq!(config.target_fps, CodecConfig::default().target_fps);
    }

    #[test]
    fn relay_preset_is_conservative() {
        let mut config = CodecConfig::default();
        QualityPreset::RelayConservative.apply_to_config(&mut config);
        assert_eq!(config.quality_profile, QualityProfile::LowLatency);
        assert!(config.target_bitrate_bps < CodecConfig::default().target_bitrate_bps);
        assert!(config.target_fps <= CodecConfig::default().target_fps);
        // A tiny base is never driven to zero.
        let mut tiny = CodecConfig::default();
        tiny.target_bitrate_bps = 100_000;
        QualityPreset::RelayConservative.apply_to_config(&mut tiny);
        assert!(tiny.target_bitrate_bps >= 500_000);
    }

    #[test]
    fn balanced_preset_is_identity() {
        let base = CodecConfig::default();
        let mut config = base;
        QualityPreset::Balanced.apply_to_config(&mut config);
        assert_eq!(config, base);
    }

    #[test]
    fn preset_wire_values_round_trip() {
        for preset in [
            QualityPreset::LanHigh,
            QualityPreset::Balanced,
            QualityPreset::RelayConservative,
        ] {
            assert_eq!(QualityPreset::from_u8(preset.as_u8()), Some(preset));
        }
        assert_eq!(QualityPreset::from_u8(99), None);
    }

    #[test]
    fn preset_application_is_relative_to_reference_rates() {
        // The same presets applied to a 4 Mbps base: LAN doubles, relay
        // halves — the ordering holds for any base the capture negotiated.
        let mut lan = CodecConfig::default();
        QualityPreset::LanHigh.apply_to_config(&mut lan);
        let mut relay = CodecConfig::default();
        QualityPreset::RelayConservative.apply_to_config(&mut relay);
        assert!(lan.target_bitrate_bps > relay.target_bitrate_bps);
        assert_eq!(lan.target_bitrate_bps, 8_000_000);
        assert_eq!(relay.target_bitrate_bps, 2_000_000);
    }

    #[test]
    fn presets_never_touch_capture_geometry() {
        let mut config = CodecConfig {
            width: 1280,
            height: 720,
            target_fps: 15,
            target_bitrate_bps: 4_000_000,
            keyframe_interval: 60,
            max_queue_depth: 2,
            quality_profile: QualityProfile::Balanced,
        };
        QualityPreset::LanHigh.apply_to_config(&mut config);
        assert_eq!((config.width, config.height), (1280, 720));
        assert_eq!(config.target_fps, 15);
        assert_eq!(config.keyframe_interval, 60);
        assert_eq!(config.max_queue_depth, 2);
    }
}
