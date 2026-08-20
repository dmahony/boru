//! Independent runtime gates for roadmap features.
//!
//! Gates are deliberately additive: the default keeps every existing path
//! enabled, while an operator can disable one risky feature without changing
//! the others. Consumers should check the relevant gate immediately before
//! starting optional work and otherwise retain their existing fallback path.

use serde::{Deserialize, Serialize};

/// Independent runtime switches for optional roadmap features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeatureGates {
    /// Enable privacy-safe map rendering and map updates.
    pub map: bool,
    /// Enable the screen-share transport (the chat path is unaffected).
    pub screen_share_transport: bool,
    /// Enable presence announcements and presence presentation.
    pub presence: bool,
    /// Enable file-transfer offers and downloads.
    pub file_transfer: bool,
}

impl Default for FeatureGates {
    fn default() -> Self {
        Self {
            map: true,
            screen_share_transport: true,
            presence: true,
            file_transfer: true,
        }
    }
}

impl FeatureGates {
    /// Load independent overrides from `BORU_FEATURE_*` environment variables.
    ///
    /// Only explicit false values (`0`, `false`, or `off`, case-insensitive)
    /// disable a gate. Missing or unrecognised values preserve the default,
    /// so an unrelated setting can never silently disable a feature.
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            map: env_enabled("BORU_FEATURE_MAP", defaults.map),
            screen_share_transport: env_enabled(
                "BORU_FEATURE_SCREEN_SHARE_TRANSPORT",
                defaults.screen_share_transport,
            ),
            presence: env_enabled("BORU_FEATURE_PRESENCE", defaults.presence),
            file_transfer: env_enabled("BORU_FEATURE_FILE_TRANSFER", defaults.file_transfer),
        }
    }

    /// Return whether the named feature is enabled.
    pub fn is_enabled(self, feature: FeatureGate) -> bool {
        match feature {
            FeatureGate::Map => self.map,
            FeatureGate::ScreenShareTransport => self.screen_share_transport,
            FeatureGate::Presence => self.presence,
            FeatureGate::FileTransfer => self.file_transfer,
        }
    }
}

/// A feature controlled by [`FeatureGates`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureGate {
    /// Privacy-safe map.
    Map,
    /// Screen-share transport.
    ScreenShareTransport,
    /// Presence.
    Presence,
    /// File transfer.
    FileTransfer,
}

fn env_enabled(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value)
            if matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off"
            ) =>
        {
            false
        }
        Ok(value)
            if matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "on"
            ) =>
        {
            true
        }
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_keep_all_existing_paths_enabled() {
        let gates = FeatureGates::default();
        assert!(gates.map && gates.screen_share_transport && gates.presence && gates.file_transfer);
    }

    #[test]
    fn gates_are_independent_in_serialized_config() {
        let gates = FeatureGates {
            map: false,
            screen_share_transport: true,
            presence: false,
            file_transfer: true,
        };
        let json = serde_json::to_string(&gates).unwrap();
        let roundtrip: FeatureGates = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip, gates);
        assert!(!roundtrip.is_enabled(FeatureGate::Map));
        assert!(roundtrip.is_enabled(FeatureGate::ScreenShareTransport));
        assert!(!roundtrip.is_enabled(FeatureGate::Presence));
        assert!(roundtrip.is_enabled(FeatureGate::FileTransfer));
    }

    #[test]
    fn unknown_environment_values_preserve_defaults() {
        // Keep this test deterministic without mutating process-global env.
        assert!(env_enabled("BORU_FEATURE_GATE_NOT_SET", true));
        assert!(!env_enabled("BORU_FEATURE_GATE_NOT_SET", false));
    }
}
