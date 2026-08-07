//! KLIPY (external GIF search) configuration and API-key security.
//!
//! Boru's external GIF search requires a provider API key.  This module is the
//! single seam through which that key is read, so the authentication approach
//! (environment variable today, secure store or OAuth tomorrow) can change
//! without touching the UI or the domain model.
//!
//! Security invariants (KLIPY-04):
//! - The key is never hardcoded or committed.  It is read at runtime from the
//!   `KLIPY_API_KEY` environment variable (empty/unset ⇒ external GIF search is
//!   disabled gracefully).
//! - `Debug`/`Display` output for this module never contains the raw key.
//! - Callers must never log the raw key or URLs that embed it.

/// Environment variable that supplies the KLIPY API key.
pub const KLIPY_API_KEY_ENV: &str = "KLIPY_API_KEY";

/// Placeholder shown instead of the raw key in any formatted output.
pub const REDACTED: &str = "<redacted>";

/// API-key configuration for external GIF search.
///
/// Constructed via [`KlipyConfig::from_env`] at the point of use.  The raw key
/// is stored only in memory and is never printed by this type.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct KlipyConfig {
    api_key: Option<String>,
}

impl KlipyConfig {
    /// Load configuration from the process environment.
    ///
    /// A missing, empty, or whitespace-only `KLIPY_API_KEY` yields an
    /// unconfigured config, which disables external GIF search gracefully.
    pub fn from_env() -> Self {
        Self::from_value(std::env::var(KLIPY_API_KEY_ENV).ok())
    }

    /// Build from an optional raw value.
    ///
    /// Testable core behind [`KlipyConfig::from_env`] so unit tests do not
    /// need to mutate the global process environment.
    pub fn from_value(raw: Option<String>) -> Self {
        let api_key = raw
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        Self { api_key }
    }

    /// Whether an API key is configured (external GIF search is available).
    pub fn is_configured(&self) -> bool {
        self.api_key.is_some()
    }

    /// The configured API key, if any.
    ///
    /// # Safety
    /// The returned value is a secret.  Do not log it, do not put it in error
    /// messages, and do not include it in anything that leaves the process.
    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// The name of the environment variable, for user-facing messages.
    pub fn env_var_name() -> &'static str {
        KLIPY_API_KEY_ENV
    }
}

impl std::fmt::Debug for KlipyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KlipyConfig")
            .field("api_key", &self.api_key.as_ref().map(|_| REDACTED))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_env_is_not_configured() {
        let cfg = KlipyConfig::from_value(None);
        assert!(!cfg.is_configured());
        assert_eq!(cfg.api_key(), None);
    }

    #[test]
    fn empty_and_whitespace_values_are_not_configured() {
        assert!(!KlipyConfig::from_value(Some(String::new())).is_configured());
        assert!(!KlipyConfig::from_value(Some("   ".to_string())).is_configured());
    }

    #[test]
    fn configured_value_is_trimmed_and_available() {
        let cfg = KlipyConfig::from_value(Some("  secret-key-123  ".to_string()));
        assert!(cfg.is_configured());
        assert_eq!(cfg.api_key(), Some("secret-key-123"));
    }

    #[test]
    fn debug_output_never_contains_the_key() {
        let cfg = KlipyConfig::from_value(Some("super-secret-value".to_string()));
        let debug = format!("{cfg:?}");
        assert!(
            !debug.contains("super-secret-value"),
            "Debug leaked the key: {debug}"
        );
        assert!(
            debug.contains(REDACTED),
            "Debug should show the redacted placeholder"
        );
    }

    #[test]
    fn env_var_name_matches_documentation() {
        assert_eq!(KlipyConfig::env_var_name(), "KLIPY_API_KEY");
    }
}
