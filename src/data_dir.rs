//! Data directory resolution with backward compatibility.
//!
//! Implements the documented priority order for determining the
//! application's persistent data directory:
//!
//! 1. `--data-dir` CLI flag (passed in as an override)
//! 2. `BORU_DATA_DIR` environment variable (new)
//! 3. `BORU_CHAT_DATA_DIR` environment variable (legacy fallback)
//! 4. Existing legacy data directory, auto-detected (if present and no
//!    new-style directory exists)
//! 5. `$XDG_DATA_HOME/boru` (new default)
//! 6. `$PWD/.boru` (new fallback)
//!
//! Existing installations keep using their original directory unless the
//! user explicitly opts in to the new paths via `--data-dir` or `BORU_DATA_DIR`.
//! Data is never automatically moved or deleted.

use std::path::PathBuf;

// ── Directory names ─────────────────────────────────────────────────────

/// New-style top-level data directory name (relative under XDG data home or cwd).
const NEW_DIR_NAME: &str = "boru";

/// Legacy top-level data directory name (relative under XDG data home or cwd).
const LEGACY_DIR_NAME: &str = "boru-chat";

/// Shared-folder subdirectory within the data directory.
pub const SHARED_DIR_NAME: &str = "shared";

// ── Environment variable names ─────────────────────────────────────────

/// New-style environment variable for the data directory.
pub const ENV_BORU_DATA_DIR: &str = "BORU_DATA_DIR";

/// Legacy environment variable for the data directory.
pub const ENV_BORU_CHAT_DATA_DIR: &str = "BORU_CHAT_DATA_DIR";

/// XDG data home environment variable.
const ENV_XDG_DATA_HOME: &str = "XDG_DATA_HOME";

/// HOME environment variable.
const ENV_HOME: &str = "HOME";

/// LOCALAPPDATA environment variable (Windows).
const ENV_LOCALAPPDATA: &str = "LOCALAPPDATA";

// ── Public API ──────────────────────────────────────────────────────────

/// Resolve the data directory according to the documented priority order.
///
/// Pass `cli_override` when `--data-dir` is supplied on the CLI;
/// pass `None` otherwise.
///
/// **Testability:** This function reads real environment variables and
/// filesystem state.  In tests, use temp directories and set env vars
/// to control behaviour.  The helper functions it delegates to
/// (`legacy_candidate_dirs`, `new_default_dir`, etc.) are individually
/// testable and deterministic given a fixed environment.
pub fn resolve_data_dir(cli_override: Option<PathBuf>) -> PathBuf {
    // 1. CLI override (highest priority)
    if let Some(dir) = cli_override {
        return dir;
    }

    // 2. New env var BORU_DATA_DIR
    if let Ok(val) = std::env::var(ENV_BORU_DATA_DIR) {
        return PathBuf::from(val);
    }

    // 3. Legacy env var BORU_CHAT_DATA_DIR (deprecated)
    if let Ok(val) = std::env::var(ENV_BORU_CHAT_DATA_DIR) {
        eprintln!(
            "warning: environment variable {} is deprecated, use {} instead",
            ENV_BORU_CHAT_DATA_DIR,
            ENV_BORU_DATA_DIR
        );
        return PathBuf::from(val);
    }

    // 4. Auto-detect legacy directory (existing installation) — deprecated
    let new_dir = new_default_dir();
    let new_dir_exists = new_dir.exists();

    for legacy in legacy_candidate_dirs() {
        if legacy.exists() {
            // If the new-style directory also exists, the user has
            // opted in — new takes precedence.
            if new_dir_exists {
                return new_dir;
            }
            eprintln!(
                "warning: using legacy data directory {:?} (deprecated). \
                 Set {} or use --data-dir to opt into the new location {:?}",
                legacy.display(),
                ENV_BORU_DATA_DIR,
                new_dir.display()
            );
            return legacy;
        }
    }

    // 5. New default ($XDG_DATA_HOME/boru or equivalent)
    // Return it even if it doesn't exist yet (fresh install).
    new_dir
}

/// Return the new-style default data directory (`$XDG_DATA_HOME/boru` or
/// `$HOME/.local/share/boru`), with a Windows fallback.
///
/// This is the directory that will be created for fresh installations.
pub fn new_default_dir() -> PathBuf {
    if let Some(val) = std::env::var_os(ENV_XDG_DATA_HOME) {
        return PathBuf::from(val).join(NEW_DIR_NAME);
    }
    if let Some(val) = std::env::var_os(ENV_HOME) {
        return PathBuf::from(val)
            .join(".local")
            .join("share")
            .join(NEW_DIR_NAME);
    }
    if let Some(val) = std::env::var_os(ENV_LOCALAPPDATA) {
        return PathBuf::from(val).join(NEW_DIR_NAME);
    }
    // Ultimate fallback — current working directory
    std::env::current_dir()
        .unwrap_or_default()
        .join(format!(".{}", NEW_DIR_NAME))
}

/// Return candidate legacy directories for auto-detection.
///
/// Does NOT consider env vars (those are handled separately in `resolve_data_dir`).
/// Returns directories that the old `get_data_dir` would have returned as
/// defaults (steps after `BORU_CHAT_DATA_DIR`).
pub fn legacy_candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(val) = std::env::var_os(ENV_XDG_DATA_HOME) {
        dirs.push(PathBuf::from(val).join(LEGACY_DIR_NAME));
    }
    if let Some(val) = std::env::var_os(ENV_HOME) {
        dirs.push(
            PathBuf::from(val)
                .join(".local")
                .join("share")
                .join(LEGACY_DIR_NAME),
        );
    }
    if let Some(val) = std::env::var_os(ENV_LOCALAPPDATA) {
        dirs.push(PathBuf::from(val).join(LEGACY_DIR_NAME));
    }
    // CWD-based fallback
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join(format!(".{}", LEGACY_DIR_NAME)));
    }

    // Deduplicate by path
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| {
        if seen.contains(d) {
            return false;
        }
        seen.insert(d.clone());
        true
    });

    dirs
}

/// Return the shared-folder path rooted at the resolved data directory.
pub fn shared_folder_path(cli_override: Option<PathBuf>) -> PathBuf {
    resolve_data_dir(cli_override).join(SHARED_DIR_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::fs;

    /// Serial execution lock for data-dir tests that mutate global env vars.
    /// Using `std::sync::Mutex` instead of depending on `serial_test` crate.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // ── helpers ────────────────────────────────────────────────────────

    /// Create a temporary directory and set up an environment scope.
    struct EnvScope {
        _temp_dir: tempfile::TempDir,
        old_envs: Vec<(String, Option<String>)>,
    }

    impl EnvScope {
        fn new() -> Self {
            let temp_dir = tempfile::tempdir().expect("temp dir");
            let mut scope = Self {
                _temp_dir: temp_dir,
                old_envs: Vec::new(),
            };
            scope.clear_env(&[
                ENV_BORU_DATA_DIR,
                ENV_BORU_CHAT_DATA_DIR,
                ENV_XDG_DATA_HOME,
                ENV_HOME,
                ENV_LOCALAPPDATA,
            ]);
            scope
        }

        fn clear_env(&mut self, names: &[&str]) {
            for name in names {
                let old = std::env::var(name).ok();
                std::env::remove_var(name);
                self.old_envs.push((name.to_string(), old));
            }
        }

        fn set_env(&mut self, name: &str, val: &str) {
            let old = std::env::var(name).ok();
            std::env::set_var(name, val);
            self.old_envs.push((name.to_string(), old));
        }

        fn path(&self) -> &Path {
            self._temp_dir.path()
        }

        fn new_dir(&self) -> PathBuf {
            self.path().join("new_data")
        }

        fn legacy_dir(&self) -> PathBuf {
            self.path().join("legacy_data")
        }
    }

    impl Drop for EnvScope {
        fn drop(&mut self) {
            for (name, old) in &self.old_envs {
                match old {
                    Some(v) => std::env::set_var(name, v),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    // ── tests ──────────────────────────────────────────────────────────

    #[test]
    fn test_cli_override_highest_priority() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        scope.set_env(ENV_BORU_DATA_DIR, scope.legacy_dir().to_str().unwrap());

        let result = resolve_data_dir(Some(scope.new_dir()));
        assert_eq!(result, scope.new_dir());
    }

    #[test]
    fn test_new_env_var() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        scope.set_env(ENV_BORU_DATA_DIR, scope.new_dir().to_str().unwrap());

        let result = resolve_data_dir(None);
        assert_eq!(result, scope.new_dir());
    }

    #[test]
    fn test_legacy_env_var_fallback() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        scope.set_env(ENV_BORU_CHAT_DATA_DIR, scope.legacy_dir().to_str().unwrap());

        let result = resolve_data_dir(None);
        assert_eq!(result, scope.legacy_dir());
    }

    #[test]
    fn test_legacy_env_outranks_new_default() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        scope.set_env(ENV_BORU_CHAT_DATA_DIR, scope.legacy_dir().to_str().unwrap());
        fs::create_dir_all(scope.new_dir()).unwrap();

        let result = resolve_data_dir(None);
        assert_eq!(result, scope.legacy_dir());
    }

    #[test]
    fn test_new_env_outranks_legacy_env() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        scope.set_env(ENV_BORU_DATA_DIR, scope.new_dir().to_str().unwrap());
        scope.set_env(ENV_BORU_CHAT_DATA_DIR, scope.legacy_dir().to_str().unwrap());

        let result = resolve_data_dir(None);
        assert_eq!(result, scope.new_dir());
    }

    #[test]
    fn test_auto_detect_legacy_dir() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        let home = scope.path().to_str().unwrap().to_string();
        scope.set_env(ENV_HOME, &home);
        let legacy = scope.path().join(".local").join("share").join("boru-chat");
        fs::create_dir_all(&legacy).unwrap();

        let result = resolve_data_dir(None);
        assert_eq!(result, legacy);
    }

    #[test]
    fn test_auto_detect_legacy_cwd_fallback() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        let cwd_dir = scope.path().join("cwd_work");
        fs::create_dir_all(&cwd_dir).unwrap();
        let legacy = cwd_dir.join(".boru-chat");
        fs::create_dir_all(&legacy).unwrap();

        let fake_home = scope.path().join("nonexistent_home");
        scope.set_env(ENV_HOME, fake_home.to_str().unwrap());

        let orig_cwd = std::env::current_dir().ok();
        std::env::set_current_dir(&cwd_dir).ok();

        let result = resolve_data_dir(None);

        if let Some(cwd) = orig_cwd {
            let _ = std::env::set_current_dir(cwd);
        }

        assert_eq!(result, legacy);
    }

    #[test]
    fn test_new_default_when_no_legacy() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        let home = scope.path().to_str().unwrap().to_string();
        scope.set_env(ENV_HOME, &home);

        let result = resolve_data_dir(None);
        let expected = scope.path().join(".local").join("share").join("boru");
        assert_eq!(result, expected);
    }

    #[test]
    fn test_both_dirs_exist_new_wins() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        let home = scope.path().to_str().unwrap().to_string();
        scope.set_env(ENV_HOME, &home);

        let legacy = scope.path().join(".local").join("share").join("boru-chat");
        fs::create_dir_all(&legacy).unwrap();
        let new = scope.path().join(".local").join("share").join("boru");
        fs::create_dir_all(&new).unwrap();

        let result = resolve_data_dir(None);
        assert_eq!(result, new);
    }

    #[test]
    fn test_legacy_xdg_detected() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        let xdg_path = scope.path().to_str().unwrap().to_string();
        scope.set_env(ENV_XDG_DATA_HOME, &xdg_path);
        scope.set_env(ENV_HOME, scope.path().join("home").to_str().unwrap());

        let legacy = scope.path().join("boru-chat");
        fs::create_dir_all(&legacy).unwrap();

        let result = resolve_data_dir(None);
        assert_eq!(result, legacy);
    }

    #[test]
    fn test_windows_localappdata() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        let appdata = scope.path().join("AppData").join("Local");
        scope.set_env(ENV_LOCALAPPDATA, appdata.to_str().unwrap());

        let expected = appdata.join("boru");
        let result = resolve_data_dir(None);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_new_default_dir_uses_xdg() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        scope.set_env(ENV_XDG_DATA_HOME, "/custom/xdg");

        let result = new_default_dir();
        assert_eq!(result, PathBuf::from("/custom/xdg/boru"));
    }

    #[test]
    fn test_new_default_dir_fallback_home() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        scope.set_env(ENV_HOME, "/home/testuser");

        let result = new_default_dir();
        assert_eq!(result, PathBuf::from("/home/testuser/.local/share/boru"));
    }

    #[test]
    fn test_legacy_candidate_dirs_xdg() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        scope.set_env(ENV_XDG_DATA_HOME, "/custom/xdg");

        let dirs = legacy_candidate_dirs();
        assert!(dirs.contains(&PathBuf::from("/custom/xdg/boru-chat")));
    }

    #[test]
    fn test_legacy_candidate_dirs_home() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        scope.set_env(ENV_HOME, "/home/testuser");

        let dirs = legacy_candidate_dirs();
        assert!(dirs.contains(&PathBuf::from(
            "/home/testuser/.local/share/boru-chat"
        )));
    }

    #[test]
    fn test_shared_folder_path_uses_resolved_dir() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        scope.set_env(ENV_BORU_DATA_DIR, scope.new_dir().to_str().unwrap());

        let result = shared_folder_path(None);
        assert_eq!(result, scope.new_dir().join(SHARED_DIR_NAME));
    }

    #[test]
    fn test_dedup_legacy_candidates() {
        let _lock = SERIAL.lock().unwrap();
        let mut scope = EnvScope::new();
        let home = scope.path().join("home");
        scope.set_env(ENV_HOME, home.to_str().unwrap());
        scope.set_env(
            ENV_XDG_DATA_HOME,
            home.join(".local").join("share").to_str().unwrap(),
        );

        let dirs = legacy_candidate_dirs();
        let expected = home.join(".local").join("share").join("boru-chat");
        let count = dirs.iter().filter(|d| *d == &expected).count();
        assert_eq!(count, 1, "duplicate should be deduplicated");
    }
}
