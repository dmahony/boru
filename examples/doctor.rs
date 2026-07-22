//! Boru install doctor / sanity-check.
//!
//! Runs a battery of checks against the local install to verify:
//!   - Data directory existence and permissions (0700 on Unix)
//!   - Secret key file existence, permissions (0600), and validity
//!   - Loadability of friends store, room store, room history, chat history
//!   - Compiled feature flags
//!
//! Usage:
//!   cargo run --example doctor                        # default data dir
//!   cargo run --example doctor -- --data-dir /custom  # custom path
//!   cargo run --example doctor -- --verbose           # verbose output
//!   cargo run --example doctor -- --json              # machine-readable JSON output

use std::{
    env,
    path::{Path, PathBuf},
    str::FromStr,
};

use clap::Parser;
use iroh::SecretKey;
use n0_error::Result;

use boru_chat::chat_history::ChatHistoryStore;
use boru_chat::friends::FriendsStore;
use boru_chat::room::RoomStore;
use boru_chat::room_history::RoomHistoryStore;

// ── CLI ─────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "doctor", about = "Check Boru install health")]
struct Args {
    /// Override the data directory to check (default: auto-detect).
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Enable verbose diagnostics (individual file detail).
    #[arg(long)]
    verbose: bool,

    /// Output machine-readable JSON instead of human-readable text.
    #[arg(long)]
    json: bool,

    /// Skip checks that require network connectivity or endpoint binding.
    #[arg(long)]
    offline: bool,
}

// ── Check result type ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum Severity {
    Pass,
    Skip,
    Warn,
    Fail,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Pass => write!(f, "PASS"),
            Severity::Skip => write!(f, "SKIP"),
            Severity::Warn => write!(f, "WARN"),
            Severity::Fail => write!(f, "FAIL"),
        }
    }
}

#[derive(Debug, Clone)]
struct Check {
    name: String,
    severity: Severity,
    message: String,
}

// ── Serde-friendly output ───────────────────────────────────────────────────

#[derive(Debug, serde::Serialize)]
struct DoctorReport {
    data_dir: String,
    checks: Vec<CheckReport>,
    summary: SummaryReport,
}

#[derive(Debug, serde::Serialize)]
struct CheckReport {
    name: String,
    status: String,
    message: String,
}

#[derive(Debug, serde::Serialize)]
struct SummaryReport {
    passed: usize,
    warnings: usize,
    failures: usize,
    skipped: usize,
    total: usize,
}

// ── Platform helpers ────────────────────────────────────────────────────────

#[cfg(unix)]
fn mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).ok().map(|m| m.permissions().mode())
}

#[cfg(not(unix))]
fn mode(_path: &Path) -> Option<u32> {
    None // permission bits not available on this platform
}

fn data_dir_mode_ok(mode: u32) -> bool {
    // 0700 or stricter (no world/group write/execute, owner has rwx)
    // Accept 0700, 0750, 0700...  Stricter-than-0700 also fine.
    #[cfg(unix)]
    {
        let owner = (mode >> 6) & 7; // rwx for owner
        let group = (mode >> 3) & 7; // rwx for group
        let world = mode & 7; // rwx for others
        owner >= 6 && group <= 5 && world == 0
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        true
    }
}

fn secret_key_mode_ok(mode: u32) -> bool {
    // 0600 or stricter (owner read/write, no group/other access)
    #[cfg(unix)]
    {
        let owner = (mode >> 6) & 7;
        let group = (mode >> 3) & 7;
        let world = mode & 7;
        owner >= 6 && group == 0 && world == 0
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        true
    }
}

fn get_data_dir() -> PathBuf {
    if let Ok(val) = env::var("BORU_CHAT_DATA_DIR") {
        return PathBuf::from(val);
    }
    if let Some(val) = env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(val).join("boru-chat");
    }
    if let Some(val) = env::var_os("HOME") {
        return PathBuf::from(val)
            .join(".local")
            .join("share")
            .join("boru-chat");
    }
    if let Some(val) = env::var_os("LOCALAPPDATA") {
        return PathBuf::from(val).join("boru-chat");
    }
    std::env::current_dir()
        .unwrap_or_default()
        .join(".boru-chat")
}

fn resolve_data_dir(override_dir: Option<PathBuf>) -> PathBuf {
    match override_dir {
        Some(d) => d,
        None => get_data_dir(),
    }
}

fn candidate_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(val) = env::var("BORU_CHAT_DATA_DIR") {
        dirs.push(PathBuf::from(val));
    }
    if let Some(val) = env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(val).join("boru-chat"));
    }
    if let Some(val) = env::var_os("HOME") {
        dirs.push(
            PathBuf::from(val)
                .join(".local")
                .join("share")
                .join("boru-chat"),
        );
    }

    // Deduplicate by canonical path where possible
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

// ── Individual check functions ──────────────────────────────────────────────

fn check_data_dir(data_dir: &Path) -> Check {
    let name = "data-directory".to_string();

    match data_dir.try_exists() {
        Ok(true) => { /* exists */ }
        Ok(false) => {
            return Check {
                name,
                severity: Severity::Warn,
                message: format!("data directory does not exist: {}", data_dir.display()),
            };
        }
        Err(e) => {
            return Check {
                name,
                severity: Severity::Fail,
                message: format!("cannot stat data directory {}: {e}", data_dir.display()),
            };
        }
    };

    if !data_dir.is_dir() {
        return Check {
            name,
            severity: Severity::Fail,
            message: format!(
                "data directory exists but is not a directory: {}",
                data_dir.display()
            ),
        };
    }

    #[cfg(unix)]
    {
        if let Some(m) = mode(data_dir) {
            if !data_dir_mode_ok(m) {
                return Check {
                    name,
                    severity: Severity::Warn,
                    message: format!(
                        "data directory permissions are {:03o}; recommend 0700: {}",
                        m & 0o777,
                        data_dir.display()
                    ),
                };
            }
        }
    }

    Check {
        name,
        severity: Severity::Pass,
        message: format!("{} ({:#06x})", data_dir.display(), {
            #[cfg(unix)]
            {
                mode(data_dir).unwrap_or(0) & 0o777
            }
            #[cfg(not(unix))]
            {
                "permissions N/A"
            }
        }),
    }
}

fn check_secret_key(data_dir: &Path) -> Check {
    let name = "secret-key".to_string();
    let path = data_dir.join("secret_key.txt");

    if !path.exists() {
        return Check {
            name,
            severity: Severity::Warn,
            message: format!(
                "secret key file not found at {} — will be generated on first run",
                path.display()
            ),
        };
    }

    if !path.is_file() {
        return Check {
            name,
            severity: Severity::Fail,
            message: format!(
                "secret key path exists but is not a file: {}",
                path.display()
            ),
        };
    }

    #[cfg(unix)]
    {
        if let Some(m) = mode(&path) {
            if !secret_key_mode_ok(m) {
                return Check {
                    name: name.clone(),
                    severity: Severity::Warn,
                    message: format!(
                        "secret key file permissions are {:03o}; recommend 0600: {}",
                        m & 0o777,
                        path.display()
                    ),
                };
            }
        }
    }

    // Try to parse the key
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            let trimmed = raw.trim();
            match SecretKey::from_str(trimmed) {
                Ok(key) => Check {
                    name,
                    severity: Severity::Pass,
                    message: format!("valid key (public: {}) at {}", key.public(), path.display()),
                },
                Err(e) => Check {
                    name,
                    severity: Severity::Fail,
                    message: format!("invalid secret key in {}: {e}", path.display()),
                },
            }
        }
        Err(e) => Check {
            name,
            severity: Severity::Fail,
            message: format!("cannot read secret key file {}: {e}", path.display()),
        },
    }
}

fn check_friends_store(data_dir: &Path) -> Check {
    let name = "friends-store".to_string();
    let path = data_dir.join("friends.json");

    if !path.exists() {
        return Check {
            name,
            severity: Severity::Pass,
            message: "no friends store file (empty list — OK)".to_string(),
        };
    }

    match FriendsStore::load(data_dir) {
        Ok(store) => Check {
            name,
            severity: Severity::Pass,
            message: format!("loaded OK ({} friend(s))", store.len()),
        },
        Err(e) => Check {
            name,
            severity: Severity::Fail,
            message: format!("failed to load friends store: {e}"),
        },
    }
}

fn check_room_store(data_dir: &Path) -> Check {
    let name = "room-store".to_string();
    let path = data_dir.join("room.json");

    if !path.exists() {
        return Check {
            name,
            severity: Severity::Pass,
            message: "no room store file (new room will be created — OK)".to_string(),
        };
    }

    match RoomStore::load(data_dir) {
        Ok(maybe) => match maybe {
            Some(store) => Check {
                name,
                severity: Severity::Pass,
                message: format!("loaded OK (topic: {})", store.topic),
            },
            None => Check {
                name,
                severity: Severity::Pass,
                message: "room file empty but loadable".to_string(),
            },
        },
        Err(e) => Check {
            name,
            severity: Severity::Fail,
            message: format!("failed to load room store: {e}"),
        },
    }
}

fn check_room_history(data_dir: &Path) -> Check {
    let name = "room-history".to_string();
    let path = data_dir.join("rooms.json");

    if !path.exists() {
        return Check {
            name,
            severity: Severity::Pass,
            message: "no room history file (empty — OK)".to_string(),
        };
    }

    match RoomHistoryStore::load(data_dir) {
        Ok(None) if !path.exists() => Check {
            name,
            severity: Severity::Pass,
            message: "removed legacy room history file; no rooms are retained".to_string(),
        },
        Ok(_) => Check {
            name,
            severity: Severity::Fail,
            message: "room history remains present after cleanup".to_string(),
        },
        Err(e) => Check {
            name,
            severity: Severity::Fail,
            message: format!("failed to load room history: {e}"),
        },
    }
}

fn check_chat_history(data_dir: &Path) -> Check {
    let name = "chat-history".to_string();
    let path = data_dir.join("chat_history.json");

    if !path.exists() {
        return Check {
            name,
            severity: Severity::Pass,
            message: "no chat history file (empty — OK)".to_string(),
        };
    }

    match ChatHistoryStore::load(data_dir) {
        Ok(None) if !path.exists() => Check {
            name,
            severity: Severity::Pass,
            message: "removed legacy chat history file; no messages are retained".to_string(),
        },
        Ok(_) => Check {
            name,
            severity: Severity::Fail,
            message: "chat history remains present after cleanup".to_string(),
        },
        Err(e) => Check {
            name,
            severity: Severity::Fail,
            message: format!("failed to load chat history: {e}"),
        },
    }
}

fn check_features() -> Check {
    let name = "compiled-features".to_string();
    let mut features: Vec<&str> = Vec::with_capacity(6);

    #[cfg(feature = "net")]
    features.push("net");
    #[cfg(feature = "metrics")]
    features.push("metrics");

    #[cfg(feature = "examples")]
    features.push("examples");
    #[cfg(feature = "gui")]
    features.push("gui");
    #[cfg(feature = "simulator")]
    features.push("simulator");
    #[cfg(feature = "test-utils")]
    features.push("test-utils");

    if features.is_empty() {
        features.push("(default features only)");
    }

    Check {
        name,
        severity: Severity::Pass,
        message: format!("[{}]", features.join(", ")),
    }
}

fn check_env_overrides() -> Check {
    let name = "environment".to_string();
    let mut hints = Vec::new();

    if let Ok(dir) = env::var("BORU_CHAT_DATA_DIR") {
        hints.push(format!("BORU_CHAT_DATA_DIR={dir}"));
    }
    if let Some(xdg) = env::var_os("XDG_DATA_HOME") {
        hints.push(format!("XDG_DATA_HOME={}", xdg.to_string_lossy()));
    }

    if hints.is_empty() {
        Check {
            name,
            severity: Severity::Pass,
            message: "no environment overrides".to_string(),
        }
    } else {
        Check {
            name,
            severity: Severity::Pass,
            message: format!("active overrides: {}", hints.join("; ")),
        }
    }
}

fn check_candidate_dirs() -> Check {
    let name = "candidate-directories".to_string();
    let candidates = candidate_data_dirs();
    if candidates.is_empty() {
        return Check {
            name,
            severity: Severity::Skip,
            message: "no candidate directories discovered".to_string(),
        };
    }
    let found: Vec<String> = candidates
        .iter()
        .map(|d| {
            if d.exists() {
                format!("{} (exists)", d.display())
            } else {
                format!("{} (absent)", d.display())
            }
        })
        .collect();
    Check {
        name,
        severity: Severity::Pass,
        message: format!("[{}]", found.join(", ")),
    }
}

// ── Runner ──────────────────────────────────────────────────────────────────

fn run_checks(data_dir: &Path, _verbose: bool, offline: bool) -> Vec<Check> {
    let checks = vec![
        check_env_overrides(),
        check_candidate_dirs(),
        check_data_dir(data_dir),
        check_secret_key(data_dir),
        check_friends_store(data_dir),
        check_room_store(data_dir),
        check_room_history(data_dir),
        check_chat_history(data_dir),
        check_features(),
    ];

    if !offline {
        // Network-reachable checks could go here (e.g. relay connectivity)
        // For now we skip them.
    }

    checks
}

// ── Output ──────────────────────────────────────────────────────────────────

fn format_human(checks: &[Check], data_dir: &Path) {
    let mut passed = 0;
    let mut warnings = 0;
    let mut failures = 0;
    let mut skipped = 0;

    println!("═══ Boru doctor ═══");
    println!("data dir: {}", data_dir.display());
    println!();

    for check in checks {
        let icon = match check.severity {
            Severity::Pass => {
                passed += 1;
                "[PASS]"
            }
            Severity::Skip => {
                skipped += 1;
                "[SKIP]"
            }
            Severity::Warn => {
                warnings += 1;
                "[WARN]"
            }
            Severity::Fail => {
                failures += 1;
                "[FAIL]"
            }
        };
        println!("  {icon} {}: {}", check.name, check.message);
    }

    println!();
    println!("═══ summary ═══");
    println!("  passed: {passed}");
    println!("  skipped: {skipped}");
    println!("  warnings: {warnings}");
    println!("  failures: {failures}");

    if failures > 0 {
        println!();
        eprintln!("❌ {failures} failure(s) detected — see above for details");
        std::process::exit(1);
    } else if warnings > 0 {
        println!();
        println!("⚠️  All checks passed with {warnings} warning(s)");
    } else {
        println!();
        println!("✓ All checks passed");
    }
}

fn format_json(checks: &[Check], data_dir: &Path) {
    let mut passed = 0;
    let mut warnings = 0;
    let mut failures = 0;
    let mut skipped = 0;

    for check in checks {
        match check.severity {
            Severity::Pass => passed += 1,
            Severity::Skip => skipped += 1,
            Severity::Warn => warnings += 1,
            Severity::Fail => failures += 1,
        }
    }

    let report = DoctorReport {
        data_dir: data_dir.display().to_string(),
        checks: checks
            .iter()
            .map(|c| CheckReport {
                name: c.name.clone(),
                status: c.severity.to_string().to_lowercase(),
                message: c.message.clone(),
            })
            .collect(),
        summary: SummaryReport {
            passed,
            warnings,
            failures,
            skipped,
            total: checks.len(),
        },
    };

    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("serialize report")
    );

    if failures > 0 {
        std::process::exit(1);
    }
}

// ── Main ────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let args = Args::parse();

    #[cfg(feature = "net")]
    let _gossip = (); // ensure net feature is available

    let data_dir = resolve_data_dir(args.data_dir);

    // The doctor runs synchronous checks (disk I/O only), no async runtime
    // needed.
    let checks = run_checks(&data_dir, args.verbose, args.offline);

    if args.json {
        format_json(&checks, &data_dir);
    } else {
        format_human(&checks, &data_dir);
    }

    Ok(())
}
