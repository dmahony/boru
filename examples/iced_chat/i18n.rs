//! Lightweight key-based internationalization (i18n) for the Boru UI.
//!
//! # Design
//!
//! - Locale files are JSON dictionaries keyed by translation keys, stored in
//!   a dedicated `locales/` directory (see [`locales_dir()`]).
//! - English (`en.json`) is embedded into the binary with `include_str!` so
//!   the app always has a complete default, even when no locale directory is
//!   present at runtime.
//! - Additional locales are loaded at runtime from the locale directory
//!   (searched in order: `BORU_LOCALE_DIR` env var, `<data_dir>/locales/`,
//!   and the repo `locales/` dir when running from a source tree). Dropping
//!   a new `<code>.json` file into that directory adds the language without
//!   changing any application source code.
//! - The active locale is chosen from (in order): the `--locale` CLI value
//!   passed to [`init`], the `BORU_LOCALE` environment variable, or `en`.
//! - Lookup order for a key: active locale → English → the key itself.
//!
//! # Usage
//!
//! Any UI component can call the free functions [`t`] / [`t_args`]:
//!
//! ```ignore
//! use crate::i18n::{t, t_args};
//! text(t("sidebar.chats"))
//! text(t_args("chat.header.members", &[("count", &count.to_string())]))
//! ```
//!
//! New components adopt the pattern with zero wiring — just call `t()`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Default locale code used when nothing else selects one.
pub const DEFAULT_LOCALE: &str = "en";

/// The environment variable that selects the active locale at startup.
pub const LOCALE_ENV: &str = "BORU_LOCALE";

/// The environment variable that overrides where locale files are searched.
pub const LOCALE_DIR_ENV: &str = "BORU_LOCALE_DIR";

/// Embedded English dictionary — the guaranteed-complete default.
const EMBEDDED_EN: &str = include_str!("locales/en.json");

/// Global translations provider, initialized once by [`init`].
static TRANSLATIONS: OnceLock<Translations> = OnceLock::new();

/// A loaded set of translations for one active locale, with an English
/// fallback layer.
pub struct Translations {
    /// Active locale code, e.g. `"en"`, `"fr"`.
    locale: String,
    /// Active locale dictionary (may be partial).
    active: HashMap<String, String>,
    /// English dictionary (complete) — fallback layer.
    english: HashMap<String, String>,
    /// All locale codes available from the search dirs (including `en`).
    available: Vec<String>,
}

impl Translations {
    /// Build translations for `locale` by searching `dirs` (in order) for a
    /// `<locale>.json` file. Always falls back to embedded English.
    pub fn load(locale: &str, dirs: &[PathBuf]) -> Self {
        let english = parse_locale_json(EMBEDDED_EN);
        let mut available = vec![DEFAULT_LOCALE.to_string()];
        let mut active = HashMap::new();

        for dir in dirs {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("json") {
                        continue;
                    }
                    let Some(code) = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string())
                    else {
                        continue;
                    };
                    if code == locale {
                        if let Ok(json) = std::fs::read_to_string(&path) {
                            active = parse_locale_json(&json);
                        }
                    }
                    if code != DEFAULT_LOCALE && !available.contains(&code) {
                        available.push(code);
                    }
                }
            }
        }
        available.sort();
        if !available.contains(&locale.to_string()) {
            available.push(locale.to_string());
        }

        Self {
            locale: locale.to_string(),
            active,
            english,
            available,
        }
    }

    /// English-only provider (used before `init`, e.g. in tests).
    pub fn english() -> Self {
        Self {
            locale: DEFAULT_LOCALE.to_string(),
            active: HashMap::new(),
            english: parse_locale_json(EMBEDDED_EN),
            available: vec![DEFAULT_LOCALE.to_string()],
        }
    }

    /// The active locale code.
    pub fn locale(&self) -> &str {
        &self.locale
    }

    /// Locale codes available from the search dirs (including `en`).
    pub fn available_locales(&self) -> &[String] {
        &self.available
    }

    /// Translate `key` with `{name}` placeholder substitution.
    ///
    /// Lookup order: active locale → English → the key itself.
    pub fn t(&self, key: &str) -> String {
        if let Some(v) = self.active.get(key) {
            return v.clone();
        }
        if let Some(v) = self.english.get(key) {
            return v.clone();
        }
        key.to_string()
    }

    /// Translate `key` and substitute `{name}` placeholders with the values
    /// from `args` (each `("name", "value")` pair).
    pub fn t_args(&self, key: &str, args: &[(&str, &str)]) -> String {
        let template = if let Some(v) = self.active.get(key) {
            v.clone()
        } else if let Some(v) = self.english.get(key) {
            v.clone()
        } else {
            key.to_string()
        };
        if args.is_empty() {
            return template;
        }
        let mut out = String::with_capacity(template.len() + args.len() * 8);
        let mut rest = template.as_str();
        while let Some(start) = rest.find('{') {
            out.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let Some(end) = after.find('}') else {
                out.push_str(&rest[start..]);
                rest = "";
                break;
            };
            let name = &after[..end];
            match args.iter().find(|(n, _)| *n == name) {
                Some((_, value)) => out.push_str(value),
                None => out.push_str(&rest[start..=start + end + 1]),
            }
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        out
    }
}

/// Parse a JSON translation dictionary into a string map.
fn parse_locale_json(json: &str) -> HashMap<String, String> {
    serde_json::from_str(json).unwrap_or_default()
}

/// Initialize the global translations provider. Call once during startup,
/// before any view code runs. `locale_override` is the CLI `--locale` value
/// (may be `None`); `data_dir` is the app data directory used to find a
/// runtime `locales/` subdirectory.
///
/// Idempotent: a second call (e.g. from a test) is ignored.
pub fn init(locale_override: Option<&str>, data_dir: Option<&Path>) {
    let locale = locale_override
        .map(|s| s.to_string())
        .or_else(|| std::env::var(LOCALE_ENV).ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_LOCALE.to_string());

    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var(LOCALE_DIR_ENV) {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(data_dir) = data_dir {
        dirs.push(data_dir.join("locales"));
    }
    // Repo-relative `locales/` when running from a source tree (dev/test).
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        dirs.push(PathBuf::from(manifest).join("locales"));
    }

    let _ = TRANSLATIONS.set(Translations::load(&locale, &dirs));
}

/// Access the global provider; falls back to English when uninitialized.
fn provider() -> &'static Translations {
    TRANSLATIONS.get_or_init(Translations::english)
}

/// Translate a key using the active locale (fallback: English → key).
pub fn t(key: &str) -> String {
    provider().t(key)
}

/// Translate a key with `{name}` placeholder substitution.
pub fn t_args(key: &str, args: &[(&str, &str)]) -> String {
    provider().t_args(key, args)
}

/// Active locale code.
pub fn locale() -> &'static str {
    provider().locale()
}

/// Locale codes available from the search dirs (including `en`).
pub fn available_locales() -> Vec<String> {
    provider().available_locales().to_vec()
}

/// The default search directories for locale files, mirroring [`init`]'s
/// resolution (used by tests and tooling).
pub fn locales_dir() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var(LOCALE_DIR_ENV) {
        dirs.push(PathBuf::from(dir));
    }
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        dirs.push(PathBuf::from(manifest).join("locales"));
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_is_complete_and_serves_as_default() {
        let tr = Translations::english();
        // The English dictionary is non-trivial and covers core keys.
        assert!(!tr.english.is_empty(), "embedded en.json is empty");
        assert!(tr.english.contains_key("app.splash.tagline"));
        assert!(tr.english.contains_key("sidebar.chats"));
    }

    #[test]
    fn missing_active_key_falls_back_to_english() {
        // Build an active locale whose dictionary is missing a key that
        // English has.
        let en = parse_locale_json(EMBEDDED_EN);
        let tr = Translations {
            locale: "xx".to_string(),
            active: HashMap::new(),
            english: en,
            available: vec!["xx".to_string(), DEFAULT_LOCALE.to_string()],
        };
        assert_eq!(tr.t("sidebar.chats"), "Chats");
    }

    #[test]
    fn missing_key_in_both_returns_key() {
        let tr = Translations::english();
        assert_eq!(tr.t("no.such.key.anywhere"), "no.such.key.anywhere");
    }

    #[test]
    fn active_locale_overrides_english() {
        let mut active = HashMap::new();
        active.insert("sidebar.chats".to_string(), "Discussions".to_string());
        let tr = Translations {
            locale: "fr".to_string(),
            active,
            english: parse_locale_json(EMBEDDED_EN),
            available: vec!["fr".to_string(), DEFAULT_LOCALE.to_string()],
        };
        assert_eq!(tr.t("sidebar.chats"), "Discussions");
    }

    #[test]
    fn interpolation_substitutes_placeholders() {
        let tr = Translations::english();
        let out = tr.t_args(
            "chat.header.members",
            &[("count", &"3".to_string())],
        );
        assert_eq!(out, "3 members");
    }

    #[test]
    fn interpolation_keeps_unknown_placeholder() {
        let tr = Translations::english();
        let out = tr.t_args(
            "chat.header.members",
            &[("other", &"x".to_string())],
        );
        assert_eq!(out, "{count} members");
    }

    #[test]
    fn global_falls_back_to_english_before_init() {
        // In tests the provider may already be initialized by another test;
        // assert that whichever provider is active still resolves core keys.
        assert_eq!(t("sidebar.chats"), "Chats");
        assert!(!available_locales().is_empty());
    }

    #[test]
    fn init_is_idempotent() {
        init(Some("en"), None);
        init(Some("fr"), None);
        // Second call ignored; still resolves.
        assert_eq!(t("sidebar.chats"), "Chats");
    }
}
