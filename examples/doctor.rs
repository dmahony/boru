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

use clap::{Parser, Subcommand};
use iroh::SecretKey;
use n0_error::Result;

use boru_core::chat_history::ChatHistoryStore;
use boru_core::friends::FriendsStore;
use boru_core::room::RoomStore;
use boru_core::room_history::RoomHistoryStore;

// ── CLI ─────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "doctor", about = "Check Boru install health")]
struct Args {
    /// Optional subcommand (e.g. `health` for the live networking health
    /// view). When omitted, the install sanity checks run.
    #[command(subcommand)]
    command: Option<Command>,

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

#[derive(Subcommand, Debug)]
enum Command {
    /// BORU-CP-15: live networking health view (debug-only, needs `net`).
    ///
    /// Boots a real node, joins the internal discovery topic, probes each
    /// discovered peer's deterministic direct topic, and prints a per-peer
    /// health view with six separate indicators (Discovery, Endpoint,
    /// Direct Topic, Inbound Delivery, Outbound Delivery, Path) plus a
    /// stable copy-diagnostics block for side-by-side machine comparison.
    Health(HealthArgs),
}

#[derive(clap::Args, Debug)]
struct HealthArgs {
    /// Observation window in seconds (default 30).
    #[arg(long, default_value_t = 30)]
    duration: u64,

    /// Relay URL override (default: iroh default relay).
    #[arg(long)]
    relay: Option<String>,

    /// Disable relay entirely (LAN-only discovery).
    #[arg(long)]
    no_relay: bool,

    /// Bootstrap peer node ids to dial into the discovery mesh at startup
    /// (repeatable). On machine A run `--bootstrap <B-node-id>`; on machine
    /// B run `--bootstrap <A-node-id>`.
    #[arg(long)]
    bootstrap: Vec<String>,

    /// Data directory for the node identity (default: auto-detect).
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Print only the copy-diagnostics block (stable labels).
    #[arg(long)]
    copy: bool,

    /// Do not send direct-topic probes (report store state only).
    #[arg(long)]
    no_probe: bool,
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

fn resolve_data_dir(override_dir: Option<PathBuf>) -> PathBuf {
    boru_core::data_dir::resolve_data_dir(override_dir)
}

fn candidate_data_dirs() -> Vec<PathBuf> {
    boru_core::data_dir::legacy_candidate_dirs()
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

    if let Ok(dir) = env::var(boru_core::data_dir::ENV_BORU_DATA_DIR) {
        hints.push(format!("{}={dir}", boru_core::data_dir::ENV_BORU_DATA_DIR));
    }
    if let Ok(dir) = env::var(boru_core::data_dir::ENV_BORU_CHAT_DATA_DIR) {
        hints.push(format!(
            "{}={dir}",
            boru_core::data_dir::ENV_BORU_CHAT_DATA_DIR
        ));
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

    // BORU-CP-15: the `health` subcommand boots a real node and prints the
    // live networking health view. It is debug-only; it needs the `net`
    // feature and an async runtime.
    if let Some(Command::Health(health)) = args.command {
        return run_health(health).map_err(Into::into);
    }

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

// ── Health view (BORU-CP-15, PDF 5.3) ──────────────────────────────────────

/// Build a fresh tokio runtime for the health view.
#[cfg(feature = "net")]
fn run_health(args: HealthArgs) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime for health view");
    runtime.block_on(run_health_async(args))
}

/// Fallback when the `net` feature is off (should not happen: the doctor
/// example declares `required-features = ["net"]`).
#[cfg(not(feature = "net"))]
fn run_health(_args: HealthArgs) -> anyhow::Result<()> {
    anyhow::bail!("health view requires the `net` feature")
}

/// BORU-CP-15: the live networking health view.
///
/// Boots a real iroh endpoint + gossip actor, joins the internal discovery
/// topic via [`DiscoveryService`], probes each discovered peer's
/// deterministic direct topic, and prints the per-peer health view plus a
/// stable copy-diagnostics block. Run on two machines with matching
/// `--relay`/`--bootstrap` flags and diff the copy-diagnostics blocks side
/// by side: the labels are stable and sorted by peer id, so an asymmetric
/// A→B vs B→A failure is obvious.
#[cfg(feature = "net")]
async fn run_health_async(args: HealthArgs) -> anyhow::Result<()> {
    use std::collections::HashSet;
    use std::time::{Duration, Instant};

    use boru_core::control_plane::health::{
        build_health_rows, probe_direct_topic, render_copy_diagnostics, render_health_view,
    };
    use boru_core::discovery_service::{DiscoveryService, PeerUpdate};
    use boru_core::discovery_topic::discovery_topic;
    use boru_core::net::{Gossip, GOSSIP_ALPN};
    use boru_core::public_room::PublicNetwork;
    use iroh::address_lookup::memory::MemoryLookup;
    use iroh::endpoint::presets;
    use iroh::protocol::Router;
    use iroh::{Endpoint, PublicKey, RelayMode, RelayUrl};

    let data_dir = resolve_data_dir(args.data_dir.clone());
    let secret_key = load_or_generate_secret_key(&data_dir)?;
    let local_public = secret_key.public();

    // Endpoint with a shared in-memory address book (bootstrap peers dial by
    // node id; relay mode is configurable for LAN-only tests).
    let relay_mode = if args.no_relay {
        RelayMode::Disabled
    } else if let Some(url) = args.relay {
        let relay_url = RelayUrl::from_str(&url).expect("valid relay URL");
        RelayMode::Custom(relay_url.into())
    } else {
        RelayMode::Custom(
            RelayUrl::from_str("https://relay.iroh.network/")
                .expect("valid default relay URL")
                .into(),
        )
    };
    let memory = MemoryLookup::new();
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(secret_key.clone())
        .address_lookup(memory)
        .relay_mode(relay_mode)
        .bind()
        .await?;
    if !args.no_relay {
        match tokio::time::timeout(Duration::from_secs(15), endpoint.online()).await {
            Ok(()) => eprintln!("relay connection established"),
            Err(_) => eprintln!("relay online() timed out after 15s (continuing)"),
        }
    }
    eprintln!("node: {}", endpoint.id());

    let gossip = Gossip::builder().spawn(endpoint.clone());
    let router = Router::builder(endpoint.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();

    let bootstrap: Vec<PublicKey> = args
        .bootstrap
        .iter()
        .filter_map(|s| PublicKey::from_str(s).ok())
        .collect();
    let service = std::sync::Arc::new(
        DiscoveryService::start(
            &gossip,
            discovery_topic(PublicNetwork::Mainnet),
            bootstrap,
            local_public,
            secret_key,
        )
        .await?
        .with_endpoint(endpoint.clone()),
    );

    // Track peers we have already probed so each peer gets at most one
    // direct-topic probe (bounded resources; idempotence guardrail).
    let mut probed: HashSet<PublicKey> = HashSet::new();
    let started = Instant::now();
    let window = Duration::from_secs(args.duration.max(1));

    // Watch peer updates; probe each newly discovered peer's direct topic.
    let mut updates = service.peer_updates();
    let mut probe_tasks = tokio::task::JoinSet::new();
    loop {
        let elapsed = started.elapsed();
        if elapsed >= window {
            break;
        }
        tokio::select! {
            update = updates.recv() => {
                match update {
                    Ok(PeerUpdate::Seen { node_id, .. })
                    | Ok(PeerUpdate::Advertised { advertised: node_id, .. }) => {
                        if node_id != local_public && probed.insert(node_id) && !args.no_probe {
                            let gossip = gossip.clone();
                            let service = std::sync::Arc::clone(&service);
                            probe_tasks.spawn(async move {
                                probe_direct_topic(&gossip, &service, local_public, node_id).await
                            });
                        }
                    }
                    Ok(PeerUpdate::Expired { .. }) => {}
                    Err(_) => break,
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    }

    // Let in-flight probes settle briefly so their events land in the store.
    while probe_tasks.join_next().await.is_some() {}

    let uptime = started.elapsed();
    let rows = build_health_rows(&service.peer_diagnostics());
    let node = local_public.fmt_short().to_string();

    if args.copy {
        print!("{}", render_copy_diagnostics(&node, uptime, &rows));
    } else {
        print!("{}", render_health_view(&node, uptime, &rows));
        println!();
        println!("── copy-diagnostics ───────────────────────────────────────");
        print!("{}", render_copy_diagnostics(&node, uptime, &rows));
    }

    // Clean shutdown. All probe tasks have finished (JoinSet drained), so
    // the Arc should be unique — unwrap it to consume the service. Stop the
    // gossip actor before the router: the router's protocol-handler shutdown
    // already tells gossip to stop, so shutting gossip down later would
    // error with ActorDropped.
    if let Ok(service) = std::sync::Arc::try_unwrap(service) {
        service.shutdown().await;
    }
    let _ = gossip.shutdown().await;
    router.shutdown().await?;
    Ok(())
}

/// Load the node secret key from the data dir, generating one on first run
/// (same behaviour as the main app's `load_or_generate_secret_key_at`).
#[cfg(feature = "net")]
fn load_or_generate_secret_key(data_dir: &Path) -> anyhow::Result<SecretKey> {
    let path = data_dir.join("secret_key.txt");
    if path.exists() {
        let raw = std::fs::read_to_string(&path)?;
        let trimmed = raw.trim();
        if let Ok(key) = SecretKey::from_str(trimmed) {
            return Ok(key);
        }
        anyhow::bail!("invalid secret key in {}", path.display());
    }
    let key = SecretKey::generate();
    let key_str = data_encoding::HEXLOWER.encode(&key.to_bytes());
    std::fs::create_dir_all(data_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700));
    }
    std::fs::write(&path, format!("{key_str}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(key)
}
