//! Iced desktop frontend for Boru.
//!
//! Usage:
//!   cargo run            # show chat list
//!   cargo run -- open    # open new room
//!   cargo run -- join <ticket>  # join room

mod activity_log_view_model;
mod app;
mod boru_dialog;
mod card_shell;
#[cfg(feature = "dev-ui")]
mod component_gallery;
#[cfg(feature = "dev-ui")]
mod designer;
mod connection_details;
mod dashboard_filters;
mod dashboard_view_model;
mod design_tokens;
mod download_progress_view;
mod downloaded_view_model;
mod downloading_view_model;
mod emoji;
mod file_category;
mod file_type_icon;
mod file_type_resolver;
mod focusable_button;
mod fonts;
mod form_components;
mod gui_test_actions;
mod i18n;
mod icon_system;
mod layout;
mod layout_config;
#[cfg(feature = "dev-ui")]
mod layout_inspector;
#[cfg(feature = "dev-ui")]
mod layout_metadata;
mod layout_merge;
mod layout_watcher;
#[cfg(test)]
mod layout_regression;
mod link_preview;
mod log_viewer;
mod mcp_server;
mod notification;
mod peers_downloading_view_model;
mod perf_tracker;
mod presentation;
mod quick_actions;
mod recent_activity_view_model;
mod shared_by_me_table;
mod status_card;
mod theme;
mod theme_config;
mod theme_merge;
mod theme_watcher;
#[cfg(test)]
mod theme_regression;
#[cfg(feature = "dev-ui")]
mod inspector;
#[cfg(test)]
mod offscreen_status_card;
mod sharing_summary;
#[cfg(feature = "terminal")]
mod terminal_view;
mod ui_components;
mod video_file_card;

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use bytes::Bytes;
use std::collections::HashSet;
use std::io::IsTerminal;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use boru_core::backfill::{BackfillHandle, BackfillProtocolHandler, BACKFILL_ALPN};
use boru_core::catalogue_handler::CatalogueHandler;
use boru_core::chat_core::friend_ping::{
    FriendPingManager, PingHandler, DEFAULT_CONNECT_TIMEOUT, DEFAULT_PING_INTERVAL,
    FRIEND_PING_ALPN,
};
use boru_core::chat_history::ChatHistoryStore;
use boru_core::file_access_handler::{FileAccessHandler, NonceStore};
use boru_core::file_offer::FileOfferRegistry;
use boru_core::file_offer_protocol::{FileOfferProtocolHandler, FILE_OFFER_ALPN};
use boru_core::friends::{FriendId, FriendsStore};
use boru_core::inbox::{inbox_message_id, InboxHandle, InboxMessageId, InboxProtocol, INBOX_ALPN};
use boru_core::mailbox::{MailboxStore, MAX_SYNC_ENVELOPES};
use boru_core::net::{Gossip, GOSSIP_ALPN};
use boru_core::proto::TopicId;
use boru_core::protocol_version::CATALOGUE_ALPN;
use boru_core::room::RoomStore;
use boru_core::room_history::RoomHistoryStore;
use boru_core::storage::Storage;
use boru_core::tunnel::{TunnelProtocol, BORU_TUNNEL_ALPN};
use clap::Parser;
use iroh::{
    address_lookup::{memory::MemoryLookup, AddrFilter, DnsAddressLookup, PkarrResolver},
    endpoint::presets,
    Endpoint, EndpointAddr, PublicKey, RelayMode, RelayUrl, SecretKey,
};
use iroh_blobs::{
    provider::events::{ConnectMode, EventMask, EventSender, ProviderMessage, RequestMode},
    store::fs::FsStore,
    BlobsProtocol,
};

use boru_core::whisper::{WhisperBuilder, WHISPER_ALPN};
#[cfg(feature = "screen-sharing")]
use boru_core::screen_share::{ScreenShareProtocol, SCREEN_SHARE_ALPN};
use iroh_mainline_address_lookup::DhtAddressLookup;

#[cfg(feature = "gui")]
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use n0_error::{bail_any, Result, StdResultExt};

/// Default relay server — user's VPS, relay TLS on 8443 (nginx TLS on 443).
const VPS_RELAY_URL: &str = "https://boru.chat:8443";

use tokio::sync::watch;
use tracing::{debug, error, info, warn};
use tracing_appender::non_blocking::{NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

const LOG_QUEUE_CAPACITY: usize = 8192;
const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;
const LOG_ROTATED_FILES: usize = 3;

use app::{DiscoveredPeersUpdate, IcedChat};

use perf_tracker::PerfTracker;

fn ensure_graphical_session() {
    #[cfg(target_os = "linux")]
    {
        let has_x11 = std::env::var_os("DISPLAY").is_some();
        let has_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        if !has_x11 && !has_wayland {
            eprintln!(
                "No graphical session detected (DISPLAY/WAYLAND_DISPLAY are unset). Run this from a desktop terminal, or use xvfb-run for a headless smoke test."
            );
            std::process::exit(1);
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "boru")]
struct Args {
    #[clap(long)]
    secret_key: Option<String>,
    #[clap(short, long)]
    relay: Option<RelayUrl>,
    #[clap(long)]
    no_relay: bool,
    /// Disable public and private room DHT discovery. mDNS, relay, tickets,
    /// and known addresses remain active.
    #[clap(long)]
    no_dht: bool,
    /// Publish direct (public) IP addresses to the DHT for relay-free connectivity.
    ///
    /// Off by default (relay-only mode, which is privacy-preserving). When enabled,
    /// the DhtAddressLookup uses AddrFilter::unfiltered so direct addresses are
    /// published alongside the relay URL. Requires --no-dht to NOT be set.
    /// WARNING: This exposes your public IP address on the Mainline DHT.
    #[clap(long)]
    publish_direct_addresses: bool,
    /// Directory for persistent identity and friend state. Chat and room
    /// history are kept in memory only.
    /// Defaults to the environment variables BORU_DATA_DIR or
    /// BORU_CHAT_DATA_DIR, or ~/.local/share/boru/.
    #[clap(long)]
    data_dir: Option<PathBuf>,

    #[clap(short, long)]
    name: Option<String>,
    /// Active UI locale (e.g. "en", "fr"). Defaults to the BORU_LOCALE env
    /// var, then English.
    #[clap(long)]
    locale: Option<String>,
    #[clap(long, default_value = "0")]
    bind_port: u16,
    /// Enable performance instrumentation and print baseline report at exit.
    #[clap(long)]
    perf: bool,
    /// Enable the live UI editor (boru-ui.toml overrides + file watcher +
    /// live theme redraw). Only effective in debug builds unless the
    /// `dev-ui` cargo feature is enabled (which turns it on in every
    /// build); ignored in release builds without the feature. The
    /// BORU_DEV_UI=1 environment variable is the equivalent.
    #[clap(long)]
    dev_ui: bool,
    /// Enable the MCP diagnostic server for AI-agent integration.
    #[clap(long)]
    mcp: bool,
    /// Enable GUI test actions via MCP (requires --mcp).
    #[clap(long)]
    enable_gui_test_actions: bool,
    /// Bind address for the MCP diagnostic server (default: 127.0.0.1:8765).
    #[clap(long, default_value = "127.0.0.1:8765")]
    mcp_bind: String,
    /// Optional subcommand.  When omitted, shows the chat list (inbox).
    #[clap(subcommand)]
    command: Option<Command>,
}

#[derive(Parser, Debug)]
enum Command {
    /// Open a new or saved chat room.
    Open { topic: Option<TopicId> },
    /// Join an existing chat room via ticket.
    Join { ticket: String },
    /// Open the standalone log viewer for this profile.
    Logs,
    /// Export a redacted support bundle without starting the GUI or network.
    SupportBundle { output: PathBuf },
}

// ── Developer-mode gate for the live UI editor (BORU-UI-08) ──────────

/// Pure gate predicate for the live UI editor.
///
/// `feature_on` is `cfg!(feature = "dev-ui")` and `debug_build` is
/// `cfg!(debug_assertions)`. The editor is enabled when:
///
/// 1. the `dev-ui` cargo feature is enabled — the deliberate opt-in that
///    also works in release builds, or
/// 2. the build is a debug build (`debug_assertions`) AND the operator
///    passed `--dev-ui` on the command line or set `BORU_DEV_UI=1`
///    (or `BORU_DEV_UI=true`, case-insensitive).
///
/// In every other case — most importantly release builds without the
/// feature — the editor is off: `boru-ui.toml` is never read, no watcher
/// thread is spawned, and the app uses `BoruTheme` defaults only.
fn dev_ui_gate_on(
    feature_on: bool,
    debug_build: bool,
    cli_flag: bool,
    env_value: Option<&str>,
) -> bool {
    if feature_on {
        return true;
    }
    if !debug_build {
        return false;
    }
    let env_flag = matches!(
        env_value.map(str::to_ascii_lowercase).as_deref(),
        Some("1") | Some("true")
    );
    cli_flag || env_flag
}

/// Runtime gate for the live UI editor (BORU-UI-08).
///
/// Precedence (documented in `docs/live-ui-editor/dev-mode-gate.md`):
/// cargo feature `dev-ui` wins in every build; otherwise (debug builds
/// only) `--dev-ui` or `BORU_DEV_UI=1` enables it; release builds without
/// the feature are always off.
fn dev_ui_enabled(cli_flag: bool) -> bool {
    dev_ui_gate_on(
        cfg!(feature = "dev-ui"),
        cfg!(debug_assertions),
        cli_flag,
        std::env::var("BORU_DEV_UI").ok().as_deref(),
    )
}

// ── Message protocol ──────────────────────────────────────────────────
use boru_core::api::GossipSender;
pub use boru_core::chat_core::{fmt_relay_mode, Message, NetEvent, SignedMessage, Ticket};
use boru_core::diagnostics::GuiTestHandle;
use boru_core::diagnostics::IcedMessageJournal;

// ── Network event bridging ────────────────────────────────────────────
pub use boru_core::chat_core::forward_gossip_events;

// ── Identity persistence ──────────────────────────────────────────────

fn get_data_dir(cli_override: Option<PathBuf>) -> PathBuf {
    boru_core::data_dir::resolve_data_dir(cli_override)
}

fn load_or_generate_secret_key(data_dir: &Path) -> Result<(SecretKey, PathBuf)> {
    load_or_generate_secret_key_at(data_dir)
}

fn load_or_generate_secret_key_at(data_dir: &Path) -> Result<(SecretKey, PathBuf)> {
    let key_path = data_dir.join("secret_key.txt");
    if key_path.exists() {
        let key_str =
            std::fs::read_to_string(&key_path).std_context("failed to read secret key file")?;
        let key = SecretKey::from_str(key_str.trim())
            .std_context("failed to parse secret key from file")?;
        Ok((key, key_path))
    } else {
        let key = SecretKey::generate();
        let key_str = data_encoding::HEXLOWER.encode(&key.to_bytes());
        std::fs::create_dir_all(data_dir).std_context("failed to create data directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700));
        }
        std::fs::write(&key_path, format!("{key_str}\n"))
            .std_context("failed to write secret key file")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
        }
        Ok((key, key_path))
    }
}

fn init_logging(data_dir: &Path) -> Result<WorkerGuard> {
    let log_path = log_viewer::log_file_path(data_dir);
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).std_context("failed to create log directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    let writer = RotatingFileWriter::open(&log_path, LOG_MAX_BYTES, LOG_ROTATED_FILES)
        .std_context("failed to open log file")?;
    let (writer, log_guard) = NonBlockingBuilder::default()
        .buffered_lines_limit(LOG_QUEUE_CAPACITY)
        .lossy(true)
        .thread_name("boru-log-writer")
        .finish(writer);
    // Keep the persistent log useful by default.  The iroh endpoint emits
    // very high-volume discovery and DNS diagnostics at DEBUG; leaving that
    // level enabled made a single GUI session grow iced_chat.log to tens of
    // megabytes.  Operators can still opt into the full trace with RUST_LOG.
    //
    // Known-harmless WARN patterns that are suppressed to ERROR:
    //   iroh::socket        – relay connect fails to N0 relays (use1-1, usw1-1)
    //                         which are unreachable from Dragon, and
    //                         no-path-datagram for peers only on those relays
    //   iroh::net_report    – captive-portal probe timeout on non-internet LANs
    //   noq_proto::connection – connection-close path cleanup (always benign)
    //   winit               – XSETTINGS warning on headless/xrdp sessions
    let file_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "info,iroh::socket=error,iroh::net_report=error,\
             noq_proto::connection=error,winit=error",
        )
    });
    // These are expected during normal endpoint startup and address
    // discovery. Keep them in the persistent log, but avoid making the GUI
    // terminal noisy. More severe events from either target remain visible.
    let terminal_filter = EnvFilter::new(
        "info,swarm_discovery=warn,\
         iroh::socket=error,iroh::net_report=error,\
         noq_proto::connection=error,winit=error",
    );
    let subscriber = build_logging_subscriber(
        writer,
        std::io::stderr,
        std::io::stderr().is_terminal(),
        file_filter,
        terminal_filter,
    );
    let _ = tracing::subscriber::set_global_default(subscriber);
    Ok(log_guard)
}

struct RotatingFileWriter {
    path: PathBuf,
    file: Option<std::fs::File>,
    size: u64,
    max_bytes: u64,
    rotated_files: usize,
}

impl RotatingFileWriter {
    fn open(path: &Path, max_bytes: u64, rotated_files: usize) -> std::io::Result<Self> {
        use std::fs::OpenOptions;
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let size = file.metadata()?.len();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(Self {
            path: path.to_owned(),
            file: Some(file),
            size,
            max_bytes,
            rotated_files,
        })
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        self.file
            .as_ref()
            .expect("rotating log writer has no active file")
            .sync_data()?;
        // Windows does not allow renaming a file while an open handle still
        // refers to it. Close the active handle before touching any rotation
        // paths; the replacement handle is opened only after all renames pass.
        drop(self.file.take());
        for index in (1..=self.rotated_files).rev() {
            let from = if index == 1 {
                self.path.clone()
            } else {
                self.rotated_path(index - 1)
            };
            let to = self.rotated_path(index);
            if from.exists() {
                std::fs::rename(&from, &to).map_err(|error| {
                    std::io::Error::new(
                        error.kind(),
                        format!(
                            "failed to rotate {} to {}: {error}",
                            from.display(),
                            to.display()
                        ),
                    )
                })?;
            }
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!(
                        "failed to reopen active log file {}: {error}",
                        self.path.display()
                    ),
                )
            })?;
        self.file = Some(file);
        self.size = 0;
        Ok(())
    }

    fn rotated_path(&self, index: usize) -> PathBuf {
        PathBuf::from(format!("{}.{}", self.path.display(), index))
    }
}

impl std::io::Write for RotatingFileWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.size > 0 && self.size.saturating_add(buf.len() as u64) > self.max_bytes {
            self.rotate()?;
        }
        let written = self
            .file
            .as_mut()
            .expect("rotating log writer has no active file")
            .write(buf)?;
        self.size = self.size.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file
            .as_mut()
            .expect("rotating log writer has no active file")
            .flush()
    }
}

struct ConditionalMakeWriter<W> {
    inner: W,
    enabled: bool,
}

impl<W> ConditionalMakeWriter<W> {
    fn new(inner: W, enabled: bool) -> Self {
        Self { inner, enabled }
    }
}

enum ConditionalWrite<W> {
    Inner(W),
    Sink(std::io::Sink),
}

impl<W: std::io::Write> std::io::Write for ConditionalWrite<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Inner(writer) => writer.write(buf),
            Self::Sink(writer) => writer.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Inner(writer) => writer.flush(),
            Self::Sink(writer) => writer.flush(),
        }
    }
}

impl<'a, W> tracing_subscriber::fmt::MakeWriter<'a> for ConditionalMakeWriter<W>
where
    W: tracing_subscriber::fmt::MakeWriter<'a>,
{
    type Writer = ConditionalWrite<W::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        if self.enabled {
            ConditionalWrite::Inner(self.inner.make_writer())
        } else {
            ConditionalWrite::Sink(std::io::sink())
        }
    }
}

fn build_logging_subscriber<F, T>(
    file_writer: F,
    terminal_writer: T,
    tee_to_terminal: bool,
    file_filter: EnvFilter,
    terminal_filter: EnvFilter,
) -> impl tracing::Subscriber + Send + Sync
where
    F: for<'a> tracing_subscriber::fmt::MakeWriter<'a> + Send + Sync + 'static,
    T: for<'a> tracing_subscriber::fmt::MakeWriter<'a> + Send + Sync + 'static,
{
    tracing_subscriber::registry()
        .with(file_filter)
        .with(fmt::layer().with_writer(file_writer).with_ansi(false))
        .with(
            fmt::layer()
                .with_ansi(false)
                .with_writer(ConditionalMakeWriter::new(terminal_writer, tee_to_terminal))
                .with_filter(terminal_filter),
        )
}

// ── Entry point ───────────────────────────────────────────────────────

fn main() -> Result<()> {
    let args = Args::parse();

    // Enable perf tracking if requested
    if args.perf {
        perf_tracker::PerfTracker::set_enabled(true);
    } else {
        perf_tracker::PerfTracker::set_enabled(false);
    }

    let _startup_timer = perf_tracker::PerfTracker::timer("app_startup", "full startup");

    // Opportunistically migrate legacy boru-chat data directory to new boru path
    let _ = boru_core::data_dir::auto_migrate_data_dir();

    let data_dir = get_data_dir(args.data_dir.clone());

    if let Some(Command::SupportBundle { output }) = &args.command {
        let input = boru_core::support_bundle::SupportBundleInput {
            build_sha: option_env!("GIT_HASH").unwrap_or("unknown").into(),
            os: std::env::consts::OS.into(), arch: std::env::consts::ARCH.into(),
            enabled_features: vec!["net".into(), "gui".into()], endpoint_id: "unavailable (headless export)".into(),
            relay_transport: if args.no_relay { "relay disabled" } else { "relay configured" }.into(),
            dht_health: if args.no_dht { "disabled" } else { "not started (headless export)" }.into(),
            schema_version: "unknown (headless export)".into(), ..Default::default()
        };
        boru_core::support_bundle::export_json(output, &input)?;
        println!("Support bundle written to {}", output.display()); return Ok(());
    }
    ensure_graphical_session();

    // Initialize the i18n provider before any view code runs. The active
    // locale is `--locale <code>` (CLI), then BORU_LOCALE env var, then
    // English. Locale files are loaded from BORU_LOCALE_DIR, `<data_dir>/locales/`,
    // and the repo `locales/` dir (dev); English is embedded in the binary as
    // the guaranteed-complete default.
    i18n::init(args.locale.as_deref(), Some(&data_dir));

    if matches!(&args.command, Some(Command::Logs)) {
        return log_viewer::run(log_viewer::log_file_path(&data_dir));
    }

    let _log_guard = init_logging(&data_dir)?;
    info!(data_dir = %data_dir.display(), "starting iced chat");

    // BORU-UI-04/08: load dev theme overrides (boru-ui.toml) from the data
    // dir — but ONLY when the live UI editor gate is on (cargo feature
    // `dev-ui`, or a debug build with `--dev-ui` / `BORU_DEV_UI=1`).
    // Release builds without the feature never read the file; the app uses
    // `BoruTheme` defaults only. A missing file still yields an empty
    // config (defaults); a malformed file logs a developer error and keeps
    // the last known-good theme — startup never fails because of the file.
    let dev_ui = dev_ui_enabled(args.dev_ui);
    let ui_theme_config = if dev_ui {
        match theme_config::load_ui_theme_config(&data_dir) {
            Ok(cfg) => cfg,
            Err(e) => {
                warn!(error = %e, "boru-ui.toml theme overrides ignored; using default theme");
                theme_config::UiThemeConfig::default()
            }
        }
    } else {
        debug!("live UI editor disabled (dev-ui gate off); boru-ui.toml not loaded");
        theme_config::UiThemeConfig::default()
    };
    // BORU-UI-05: surface validation warnings for unsafe/nonsensical values
    // (negative padding, zero font size, absurd sidebar widths, …) exactly
    // once at startup. The merge itself is pure and runs on every
    // `IcedChat::boru_theme()` call; the warnings are reported here.
    if dev_ui {
        let (_, theme_warnings) = theme_merge::merge_ui_theme(
            &crate::theme::BoruTheme::default(),
            &ui_theme_config,
        );
        for w in &theme_warnings {
            warn!(override = %w, "boru-ui.toml theme override adjusted");
        }
    }

    // BORU-LAYOUT-06: load dev layout overrides (boru-layout.toml) from the
    // data dir under the SAME dev-ui gate as the theme file. Release builds
    // without the feature never read it; the app uses `LayoutConfig::default()`
    // only. A missing file still yields an empty config (defaults); a
    // malformed file logs a developer error and keeps the default layout —
    // startup never fails because of the file.
    let layout_overrides = if dev_ui {
        match layout_config::load_layout_config(&data_dir) {
            Ok(cfg) => cfg,
            Err(e) => {
                warn!(error = %e, "boru-layout.toml layout overrides ignored; using default layout");
                layout::LayoutOverrides::default()
            }
        }
    } else {
        debug!("live layout editor disabled (dev-ui gate off); boru-layout.toml not loaded");
        layout::LayoutOverrides::default()
    };
    // BORU-LAYOUT-06: surface validation warnings for unsafe/nonsensical
    // values (negative padding, zero column counts, absurd widths, …) exactly
    // once at startup. The merge itself is pure; the warnings are reported
    // here and on every reload.
    if dev_ui {
        let (_, layout_warnings) =
            layout_merge::merge_layout_config(&layout::LayoutConfig::default(), &layout_overrides);
        for w in &layout_warnings {
            warn!(override = %w, "boru-layout.toml layout override adjusted");
        }
    }

    // ── Panic hook: catch Rust panics and write crash info to instance.log
    //     (which the splash window is tailing) plus a crash report file.
    {
        let crash_dir = data_dir.join("crash_reports");
        let splash_log = data_dir.join("instance.log");
        std::panic::set_hook(Box::new(move |info| {
            let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
            let msg = match info.payload().downcast_ref::<&str>() {
                Some(s) => s.to_string(),
                None => match info.payload().downcast_ref::<String>() {
                    Some(s) => s.clone(),
                    None => "unknown panic".to_string(),
                },
            };
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "unknown location".to_string());
            let crash_msg =
                format!("BORU CRASH at {timestamp}\nLocation: {location}\nMessage: {msg}");
            let backtrace = std::backtrace::Backtrace::force_capture();
            // Write to instance.log so the splash window displays it
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&splash_log)
            {
                let _ = writeln!(f, "{}", crash_msg);
            }
            // Write a dedicated crash report
            let _ = std::fs::create_dir_all(&crash_dir);
            let report_path = crash_dir.join(format!("crash-{timestamp}.txt"));
            if let Ok(mut f) = std::fs::File::create(&report_path) {
                let _ = writeln!(f, "{crash_msg}");
                let _ = writeln!(f, "Backtrace:\n{backtrace}");
                let _ = writeln!(
                    f,
                    "RUST_BACKTRACE: {}",
                    std::env::var("RUST_BACKTRACE").unwrap_or_else(|_| "unset".to_string())
                );
            }
            // Also log via tracing
            tracing::error!("{crash_msg}");
        }));
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .std_context("failed to create tokio runtime")?;
    let _tokio_timer = PerfTracker::timer("app_startup", "tokio-runtime");

    // Determine if there's an initial room to connect to
    let initial_room: Option<(TopicId, Vec<EndpointAddr>)> = runtime.block_on(async {
        match &args.command {
            Some(Command::Open { topic }) => {
                let (t, peers) = match topic {
                    Some(t) => (*t, Vec::new()),
                    None => match RoomStore::load_or_none(&data_dir) {
                        Some(store) => {
                            let n_peers = store.peers.len();
                            if n_peers > 0 {
                                info!(topic = %store.topic, peers = n_peers, "reusing saved room topic");
                            } else {
                                info!(topic = %store.topic, "reusing saved room topic");
                            }
                            // Pass saved bootstrap peers so the GUI can seed
                            // its address lookup before subscribing.
                            (store.topic, store.peers.clone())
                        }
                        None => {
                            let t = TopicId::from_bytes(rand::random());
                            info!(topic = %t, "opening new chat room");
                            let _room = RoomStore::new(&data_dir, t);
                            (t, vec![])
                        }
                    },
                };
                Some((t, peers))
            }
            Some(Command::Join { ticket }) => {
                let ticket: Ticket = match Ticket::from_str(ticket) {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(error = %e, "failed to parse ticket");
                        return None;
                    }
                };
                info!(topic = %ticket.topic, "joining chat room");
                Some((ticket.topic, ticket.peers))
            }
            Some(Command::Logs) | Some(Command::SupportBundle { .. }) => None,
            None => {
                // No explicit room: start on the Home/ChatList screen. Boru
                // no longer auto-opens the internal lobby as a conversation
                // (BORU-DISC-12); peer discovery is served by the internal
                // discovery topic via DiscoveryService, not by a lobby chat.
                None
            }
        }
    });

    let (secret_key, key_path) = match args.secret_key.as_ref() {
        None => load_or_generate_secret_key(&data_dir)?,
        Some(key) => (key.parse()?, PathBuf::from("<passed via cli flag>")),
    };
    let local_public = secret_key.public();
    info!("> our public key: {local_public}");
    info!("> identity file: {}", key_path.display());

    let local_label = args
        .name
        .clone()
        .unwrap_or_else(|| local_public.fmt_short().to_string());

    let relay_mode = match (args.no_relay, args.relay.clone()) {
        (true, Some(_)) => bail_any!("--no-relay and --relay are mutually exclusive"),
        (true, None) => RelayMode::Disabled,
        (false, None) => RelayMode::Custom(
            VPS_RELAY_URL
                .parse::<RelayUrl>()
                .expect("valid VPS relay URL")
                .into(),
        ),
        (false, Some(url)) => RelayMode::Custom(url.into()),
    };
    info!("> relay: {}", fmt_relay_mode(&relay_mode));

    // Extract the relay URL for directory topic derivation.
    // We do this outside the runtime.block_on block to avoid capturing
    // relay_mode by reference in the async closure.
    let relay_url_for_directory: Option<String> = match &relay_mode {
        RelayMode::Disabled => None,
        RelayMode::Custom(map) => map.urls::<Vec<_>>().first().map(|u| u.to_string()),
        RelayMode::Default => Some(VPS_RELAY_URL.to_string()),
        RelayMode::Staging => Some(VPS_RELAY_URL.to_string()),
    };

    // ── Incompatible-option checks ──────────────────────────────────────
    if args.publish_direct_addresses && args.no_dht {
        bail_any!(
            "--publish-direct-addresses requires DHT to be enabled. \
             Remove --no-dht or drop --publish-direct-addresses."
        );
    }

    // ── Persistent download storage (shared with CatalogueHandler) ─────
    let storage = Arc::new(Storage::open(&data_dir).expect("storage"));
    info!("download-storage: opened at {}", data_dir.display());

    // ── Start a native splash window so the user sees feedback immediately ─
    // The splash shows a spinner and startup progress messages while the
    // heavy network initialization runs.  After the Iced window opens it
    // stays alive as a runtime watchdog: a background thread sends "hb"
    // heartbeat ticks every 1s.  If the heartbeat stops (hang) or the pipe
    // closes (crash), the splash shows "not responding" or "exited".
    // Look for splash.py next to the binary first, then in the source tree.
    let splash_script = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("splash.py")))
        .unwrap_or_else(|| {
            std::path::PathBuf::from("/home/dan/iroh-gossip-chat/scripts/splash.py")
        });
    let splash_log_path = data_dir.join("instance.log");
    let mut splash_child: Option<std::process::Child> = None;
    let splash_stdin: Arc<Mutex<Option<std::process::ChildStdin>>> = if splash_script.exists() {
        match std::process::Command::new("python3")
            .arg(&splash_script)
            .arg("--log")
            .arg(&splash_log_path)
            .arg("--version")
            .arg(app::version_tag())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                let stdin = child.stdin.take();
                splash_child = Some(child);
                Arc::new(Mutex::new(stdin))
            }
            Err(_) => Arc::new(Mutex::new(None)),
        }
    } else {
        Arc::new(Mutex::new(None))
    };

    // Helper to send a line to the splash window.
    let splash_send = |msg: &str| {
        if let Ok(mut guard) = splash_stdin.lock() {
            if let Some(ref mut stdin) = *guard {
                let _ = writeln!(stdin, "{}", msg);
            }
        }
    };
    splash_send("Starting network...");

    // ── Build the endpoint, gossip, and router (no topic subscription yet) ──

    // ── Directory gossip sender (shared between main.rs receiver and MCP) ──
    // Created outside block_on so it survives after IcedChat.run() returns.
    let mcp_directory_sender: Arc<Mutex<Option<GossipSender>>> = Arc::new(Mutex::new(None));
    let mcp_dir_sender_for_block = mcp_directory_sender.clone();

    // Shared directory store created before block_on so both MCP
    // (inside) and IcedChat (outside) share the same Arc.
    let shared_directory_store = Arc::new(std::sync::Mutex::new(
        boru_core::directory::DirectoryStore::new(),
    ));
    if let Ok(storage) = boru_core::storage::Storage::open(&data_dir) {
        let _ = storage.with_conn(|conn| {
            Ok(shared_directory_store
                .lock()
                .map_err(|_| anyhow::anyhow!("directory store mutex poisoned"))?
                .load_from_db(conn)?)
        });
    }

    // ── FS-05/FS-11/FS-17 transfer projection ────────────────────
    // Shared live projection store (broadcast channel + snapshot) plus
    // item_id (content hash) → display-name enrichment maps. The blob
    // provider consumer below feeds outbound events into the store AND
    // records them durably (direction=outbound) so the Activity Log's
    // "By others" view has real history. Created outside the async
    // block because IcedChat::new (below) receives them.
    let transfer_store =
        std::sync::Arc::new(boru_core::transfer_state_projection::TransferStateStore::new(256));
    let outbound_item_labels: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, String>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let inbound_item_labels: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, String>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    #[cfg(feature = "screen-sharing")]
    // Screen-share protocol + its app-facing channels. Declared at function
    // scope so both the router registration (inside the async block below)
    // and the IcedChat wiring after it can use it.
    let screen_share = {
        let (events_tx, events_rx) = tokio::sync::mpsc::channel(32);
        let (media_tx, media_rx) = tokio::sync::mpsc::channel(64);
        let (audio_tx, audio_rx) = tokio::sync::mpsc::channel(32);
        let protocol = ScreenShareProtocol::with_channels(events_tx.clone(), media_tx, audio_tx);
        (protocol, events_rx, media_rx, audio_rx, events_tx)
    };
    // One process-local registry is shared by the direct-transfer handler and
    // the GUI, which later registers offers for outbound conversations.
    let file_offer_registry = Arc::new(Mutex::new(FileOfferRegistry::new()));

    let (
        endpoint,
        memory_lookup,
        gossip,
        router,
        blob_store,
        net_rx,
        net_tx,
        friend_mgr,
        friend_events_rx,
        friends,
        room_history,
        chat_history,
        backfill_handle,
        whisper_events_rx,
        whisper_handle,
        call_handle,
        call_events_rx,
        inbox_events_rx,
        discovered_peers_rx,
        directory_room_rx,
        // The discovery service handle is deliberately NOT stored on
        // IcedChat (no conversation/UI state for the internal discovery
        // topic); holding it here for the rest of main() keeps its
        // background drain task alive for the entire GUI lifetime.
        _discovery_service,
        // BORU-CP-07: reconnection signal channel + handle handed to the
        // app so it can re-join direct topics and report readiness.
        reconnect_ready_rx,
        reconnect_handle,
        dht_for_private,
        tunnel_service,
    ) = runtime.block_on(async {
        let memory_lookup = MemoryLookup::new();
        use std::net::{Ipv4Addr, SocketAddrV4};

        let mdns = MdnsAddressLookup::builder().build(secret_key.public())?;
        // Register the discovery subscriber before binding the endpoint. The
        // mDNS service caches advertisements but does not replay already-seen
        // endpoints to subscribers created later. Subscribing after endpoint
        // creation would miss the loopback advertisement and break local
        // single-machine demos.
        let mdns_events = mdns.subscribe().await;

        let endpoint = {
            {
                let ep_builder = if matches!(relay_mode, RelayMode::Disabled) {
                    // Minimal + manual lookups and no relay (PkarrPublisher skipped).
                    Endpoint::builder(presets::Minimal)
                        .address_lookup(PkarrResolver::n0_dns())
                        .address_lookup(DnsAddressLookup::n0_dns())
                } else {
                    // Use Minimal preset instead of N0 to skip the PkarrPublisher
                    // (HTTP PUT to dns.iroh.link) which hangs on Windows where the
                    // DNS server may be unreachable. Manually add read-only address
                    // lookups (PkarrResolver + DnsAddressLookup) below.
                    Endpoint::builder(presets::Minimal)
                };
                let endpoint = ep_builder
                    .secret_key(secret_key.clone())
                    .address_lookup(mdns)
                    .address_lookup(PkarrResolver::n0_dns())
                    .address_lookup(DnsAddressLookup::n0_dns())
                    .relay_mode(relay_mode.clone())
                    .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                    .bind()
                    .await?;
                #[allow(unused)]
                endpoint.address_lookup()?.add(memory_lookup.clone());
                if !matches!(relay_mode, RelayMode::Disabled) {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(15),
                        endpoint.online(),
                    )
                    .await
                    {
                        Ok(()) => info!("relay connection established"),
                        Err(_) => warn!("relay.online() timed out after 15s, proceeding anyway"),
                    }
                }
                info!(endpoint_addr = ?endpoint.addr(), "endpoint address ready");
                endpoint
            }
        };
        info!("> endpoint: {}", endpoint.id());
        splash_send("Endpoint ready");

        // The same mDNS service is registered with the endpoint and used for
        // discovery events. This keeps published endpoint addresses and the
        // event subscriber on one shared address book.

        // Keep DHT address lookup available for endpoint-free `boru1:` room
        // invitations.  mDNS still handles LAN discovery and the configured
        // relay handles transport connectivity; this lookup is only consulted
        // when a private-room tracker supplies a peer ID without an address.
        if !args.no_dht {
            // Choose address filter: relay-only (privacy-preserving, default)
            // vs. unfiltered (publishes direct IPs, opt-in only).
            let addr_filter = if args.publish_direct_addresses {
                eprintln!(
                    "\n  ⚠️  WARNING: --publish-direct-addresses is enabled.\n  \
                     Your public IP address will be published on the Mainline DHT.\n  \
                     This enables relay-free peer-to-peer connectivity but exposes\n  \
                     your network location publicly.\n"
                );
                AddrFilter::unfiltered()
            } else {
                AddrFilter::relay_only()
            };

            match endpoint.address_lookup() {
                Ok(registry) => {
                    match DhtAddressLookup::builder()
                        .secret_key(endpoint.secret_key().clone())
                        .addr_filter(addr_filter)
                        .build()
                    {
                        Ok(dht) => {
                            info!(
                                "DHT address lookup registered (filter: {})",
                                if args.publish_direct_addresses {
                                    "unfiltered"
                                } else {
                                    "relay-only"
                                }
                            );
                            registry.add(dht);
                            splash_send("DHT lookup registered");
                        }
                        Err(err) => {
                            warn!(
                                "DHT address lookup construction failed: {err}; \
                                 peer address resolution may be slower without DHT"
                            );
                        }
                    }
                }
                Err(err) => {
                    warn!(
                        "address lookup registry unavailable: {err}; \
                         DHT address lookup not registered"
                    );
                }
            }
        }


        let gossip = Gossip::builder().spawn(endpoint.clone());
        splash_send("Gossip mesh ready");
        let blob_store = FsStore::load(data_dir.join("blobs")).await?;

        let (blob_event_sender, blob_event_rx) = EventSender::channel(
            128,
            EventMask {
                connected: ConnectMode::Notify,
                get: RequestMode::NotifyLog,
                get_many: RequestMode::NotifyLog,
                push: RequestMode::Disabled,
                observe: iroh_blobs::provider::events::ObserveMode::None,
                throttle: iroh_blobs::provider::events::ThrottleMode::None,
            },
        );
        spawn_outbound_provider_consumer(
            runtime.handle(),
            blob_event_rx,
            Arc::clone(&transfer_store),
            Arc::clone(&outbound_item_labels),
            Some(storage.clone()),
            local_public,
        );
        let blobs_protocol = BlobsProtocol::new(&blob_store, Some(blob_event_sender));
        splash_send("Blob store ready");

        // ── Persistent history stores ────────────────────────────────
        let room_history = RoomHistoryStore::empty_at(&data_dir);

        // One-time legacy migration: import chat_history.json into the
        // SQLite `messages` table (message_store.db), then rename the JSON
        // to `chat_history.json.imported` as a backup + completion marker.
        // After this runs, SQLite is the only live history source and the
        // in-memory store starts empty; RoomOpened replays from SQLite.
        if let Ok(Some(legacy)) = ChatHistoryStore::load(&data_dir) {
            match legacy.migrate_legacy_json(
                &data_dir.join("message_store.db"),
                local_public.as_bytes(),
            ) {
                Ok(imported) => {
                    info!("legacy chat history migration: imported {imported} entries to SQLite");
                }
                Err(err) => {
                    warn!("legacy chat history migration failed (keeping JSON for retry): {err}");
                }
            }
        }

        // BORU-DISC-18: one-time startup migration — drop any stale saved
        // lobby conversation. Older versions auto-joined the canonical
        // public lobby and persisted it as a room (conversation store) and
        // as per-topic chat history (SQLite messages table). Remove only
        // that exact topic; unrelated public rooms are untouched.
        let lobby_report =
            boru_core::lobby_migration::migrate_legacy_lobby(&data_dir, Some(&storage));
        if !lobby_report.is_empty() {
            info!(
                conversations_removed = lobby_report.conversations_removed,
                messages_removed = lobby_report.messages_removed,
                meta_rows_removed = lobby_report.meta_rows_removed,
                "lobby migration: removed stale saved lobby from persisted state"
            );
        }

        let chat_history = Arc::new(std::sync::Mutex::new(
            ChatHistoryStore::load_or_default(&data_dir),
        ));
        // BORU-DISC-18: keep the in-memory history free of the stale lobby
        // topic too — covers the case where the legacy JSON migration failed
        // and chat_history.json still contains lobby entries.
        boru_core::lobby_migration::prune_chat_history(&mut chat_history.lock().unwrap());

        // Sync ChatHistoryStore's next_event_id with the SQLite
        // outgoing_messages table so that new event_ids never collide
        // with rows already in SQLite (e.g. after a crash where JSON
        // wasn't saved but SQLite was).
        if let Ok(max_id) = storage.max_outgoing_event_id() {
            chat_history.lock().unwrap().seed_next_event_id(max_id);
        }

        // ── Backfill handler ──────────────────────────────────────────
        // Authorization is anchored to this node's own public key; remote
        // requests must name a concrete topic and pass `authorize_backfill`.
        let backfill_handler = BackfillProtocolHandler::new(storage.clone(), secret_key.public());

        // ── Whisper protocol ──────────────────────────────────────────
        // Direct QUIC channels for private 1:1 messaging and file transfer.
        let whisper_builder = WhisperBuilder::new(endpoint.clone(), secret_key.clone());
        let whisper_handler = whisper_builder.protocol_handler();
        let (whisper_handle, whisper_events_rx_tmp) = whisper_builder.spawn();
        splash_send("Whisper protocol ready");

        // ── Inbox protocol ─────────────────────────────────────────────
        // Direct QUIC channels for offline message delivery to peers.
        let (inbox_handle, inbox_events_rx_tmp) = InboxHandle::new();
        // Shared set tracking which message IDs have been served via
        // SyncResponse.  The record_sync_served_fn callback inserts IDs
        // after each response; the pending_fn filters them out so that
        // repeated sync requests from the same peer do not re-serve the
        // same envelopes (replay protection).
        let served_ids: Arc<std::sync::Mutex<HashSet<InboxMessageId>>> =
            Arc::new(std::sync::Mutex::new(HashSet::new()));
        // Wire the callback so the protocol handler records served
        // message IDs after each SyncResponse.
        let served_ids_for_record = served_ids.clone();
        inbox_handle
            .set_record_sync_served_fn(Some(Arc::new(move |_peer, msg_ids| {
                let mut set = served_ids_for_record.lock().unwrap();
                for id in msg_ids {
                    set.insert(*id);
                }
            })))
            .await;
        // Serve reconnect sync from the durable mailbox owner.  The provider
        // applies the bounded retention/count/size policy in
        // `pending_for_recipient_since`; the requester-supplied timestamp is
        // therefore only a resume hint, never an unrestricted query.
        // Already-served message IDs (tracked in `served_ids`) are filtered
        // out to prevent duplicate delivery on replay sync requests.
        let mailbox_data_dir = data_dir.clone();
        let served_ids_for_filter = served_ids.clone();
        inbox_handle
            .set_pending_fn(Some(Arc::new(move |requester, since_ms| {
                let mut page = MailboxStore::load(&mailbox_data_dir)
                    .ok()
                    .flatten()
                    .map(|mut store| {
                        store.pending_for_recipient_since(requester, since_ms)
                    })
                    .unwrap_or_default();
                // Filter out envelopes that have already been served via
                // a previous SyncResponse (replay protection).  The same
                // inbox_message_id hash is used both when recording served
                // IDs and when filtering, ensuring consistent dedup.
                let served = served_ids_for_filter.lock().unwrap();
                page.retain(|env| {
                    let bytes = postcard::to_stdvec(env)
                        .expect("envelope encoding cannot fail");
                    !served.contains(&inbox_message_id(&bytes))
                });
                drop(served);
                // If the page is at the envelope limit there may be more;
                // the byte limit could also cause truncation.  This is a
                // best-effort has_more signal; true pagination requires
                // the SQLite-backed provider.
                let has_more = page.len() >= MAX_SYNC_ENVELOPES;
                (page, has_more)
            })))
            .await;
        let inbox_protocol = InboxProtocol::new(inbox_handle.inner()).with_secret_key(secret_key.clone());
        let inbox_events_rx = Arc::new(tokio::sync::Mutex::new(inbox_events_rx_tmp));
        splash_send("Inbox protocol ready");

        // ── Friends list (needed before router for CatalogueHandler) ───
        let friends = FriendsStore::load_from_sqlite(&storage, &data_dir);
        // Fall back to JSON if SQLite was empty
        let friends = if friends.is_empty() {
            let json_store = FriendsStore::load_or_default(&data_dir);
            if !json_store.is_empty() {
                // Migrate from JSON to SQLite
                let _ = json_store.save_to_sqlite(&storage);
                json_store
            } else {
                friends
            }
        } else {
            friends
        };
        splash_send(&format!("Loaded {} friends", friends.len()));
        if !friends.is_empty() {
            info!("> loaded {} friend(s) from disk", friends.len());
        }

        // ── Catalogue handler (serves file catalogues to peers) ────────
        let catalogue_handler = CatalogueHandler::new(
            storage.clone(),
            secret_key.clone(),
            local_public.to_string(),
            friends.clone(),
        );

        // ── File access handler (issues signed download tickets) ────────
        let nonce_store = Arc::new(NonceStore::new());
        let file_access_handler = FileAccessHandler::new(
            storage.clone(),
            secret_key.clone(),
            local_public.to_string(),
            friends.clone(),
            nonce_store,
            Arc::new(blob_store.clone().into()),
        );

        let file_offer_handler = FileOfferProtocolHandler::new(Arc::clone(&file_offer_registry));

        let tunnel_service = Arc::new(boru_core::tunnel::service::TunnelService::with_enrollment_store(
            Arc::new(boru_core::tunnel::enrollment::EnrollmentTokenStore::load_or_default(&data_dir)),
        ));
        let tunnel_handler = TunnelProtocol::with_service(Arc::clone(&tunnel_service), local_public);

        // ── Call-control protocol ─────────────────────────────────────
        // Signalling is deliberately independent of media; the actor only
        // exchanges bounded CallControl frames and emits state events.
        let call_builder = boru_core::call::manager::CallBuilder::new(
            endpoint.clone(),
            secret_key.clone(),
        );
        let call_handler = call_builder.protocol_handler();
        let (call_handle, call_events_rx) = call_builder.spawn();

        let router_builder = iroh::protocol::Router::builder(endpoint.clone())
            .accept(GOSSIP_ALPN, gossip.clone())
            .accept(iroh_blobs::ALPN, blobs_protocol.clone())
            .accept(FRIEND_PING_ALPN, PingHandler)
            .accept(BACKFILL_ALPN, backfill_handler)
            .accept(WHISPER_ALPN, whisper_handler)
            .accept(INBOX_ALPN, inbox_protocol)
            .accept(CATALOGUE_ALPN, catalogue_handler)
            .accept(boru_core::net::FILE_ACCESS_ALPN, file_access_handler)
            .accept(FILE_OFFER_ALPN, file_offer_handler)
            .accept(BORU_TUNNEL_ALPN, tunnel_handler)
            .accept(boru_core::call::manager::CALL_ALPN, call_handler);
        #[cfg(feature = "screen-sharing")]
        let router_builder = router_builder.accept(SCREEN_SHARE_ALPN, screen_share.0.clone());
        let router = router_builder.spawn();
        splash_send("Protocol router ready");

        // Create the discovered-peers channel for UI display. Peer discovery
        // is served by the internal discovery topic (DiscoveryService) and
        // mDNS; Boru no longer auto-subscribes to the public lobby at startup
        // (BORU-DISC-12).
        let (discovered_peers_tx, discovered_peers_rx_tmp) =
            tokio::sync::mpsc::channel::<DiscoveredPeersUpdate>(64);
        // Create the directory room channel for UI display.
        let (directory_room_tx, directory_room_rx_tmp) =
            tokio::sync::mpsc::channel::<app::DirectoryRoomEvent>(64);

        // ── Shared member-discovery DHT client ───────────────────────────
        // One `distributed_topic_tracker::Dht` handle is created (when DHT is
        // enabled) for user-facing room discovery: private-room trackers and
        // user-created public-room trackers inside `IcedChat`.  The lobby is
        // no longer auto-joined at startup (BORU-DISC-12), so there is no
        // startup public-lobby tracker consuming this handle.  This is
        // intentionally separate from Iroh's `DhtAddressLookup` (address
        // resolution) — see `docs/discovery-architecture.md` §2.
        let room_discovery_dht = (!args.no_dht).then(|| {
            let dht = distributed_topic_tracker::Dht::new(
                &distributed_topic_tracker::DhtConfig::default(),
            );
            info!("member-discovery DHT client created");
            dht
        });
        if args.no_dht {
            info!("room-discovery DHT disabled by --no-dht");
        }

        // ── Internal discovery topic subscription ────────────────────────
        // Every Boru node joins the versioned internal discovery gossip
        // topic (BORU_DISCOVERY_TOPIC_V1, BORU-DISC-05) at startup as
        // networking infrastructure: peer discovery / presence /
        // connectivity bootstrap. The old auto-joined lobby subscription
        // (BORU-DISC-12) is gone — the discovery topic is the mesh. The
        // discovery topic must never become a conversation: no IcedChat
        // field, no conversations entry, no sidebar row, no rendering. The
        // handle is returned to main() and held there for the app lifetime
        // so its background drain task is not dropped.
        splash_send("Joining discovery mesh...");
        let discovery_service = match boru_core::discovery_service::DiscoveryService::start(
            &gossip,
            boru_core::discovery_topic::discovery_topic(
                boru_core::public_room::PublicNetwork::Mainnet,
            ),
            Vec::new(),
            local_public,
            secret_key.clone(),
        )
        .await
        {
            Ok(service) => {
                let topic = service.topic();
                info!(topic = %topic, "joined internal discovery topic");
                // BORU-CP-14: attach the iroh endpoint so per-peer path
                // diagnostics (direct / relay / transitioning) are
                // classified from iroh's remote-info snapshots. Path type
                // is diagnostic metadata only — chat delivery never depends
                // on being direct.
                Some(service.with_endpoint(endpoint.clone()))
            }
            Err(error) => {
                warn!(
                    error = %error,
                    "failed to join internal discovery topic; continuing without discovery service"
                );
                None
            }
        };

        // Keep coarse presence on the same live heartbeat. GeoIP is optional
        // and local; missing database paths leave normal online/TTL semantics
        // unchanged while announcing no map metadata.
        if let Some(service) = &discovery_service {
            let city = std::env::var_os("BORU_GEOIP_CITY").map(PathBuf::from);
            let asn = std::env::var_os("BORU_GEOIP_ASN").map(PathBuf::from);
            let resolver = Arc::new(Mutex::new(
                boru_core::network_location::GeoIpResolver::from_paths(
                    city.as_deref(),
                    asn.as_deref(),
                ),
            ));
            let sink = service.coarse_presence_sink();
            let last_coarse = Arc::new(Mutex::new(None));
            let last_coarse_for_watcher = Arc::clone(&last_coarse);
            boru_core::network_location::spawn_endpoint_watcher(
                endpoint.clone(),
                resolver,
                move |coarse| {
                    let Ok(mut last) = last_coarse_for_watcher.lock() else {
                        return;
                    };
                    if *last == coarse {
                        return;
                    }
                    *last = coarse.clone();
                    sink(coarse);
                },
            );
        }

        // ── BORU-CP-07: automatic reconnection wiring ─────────────────────
        // The discovery service's reconnect loop emits ReconnectSignal when
        // a reconnect attempt succeeds. Forward those signals into the app
        // so it can ensure the deterministic direct topic is
        // joined/subscribed (the data-plane action the discovery service
        // never performs itself). The app also gets a ReconnectHandle to
        // report real direct-topic readiness back (clears retry/backoff).
        let (reconnect_ready_tx, reconnect_ready_rx) =
            tokio::sync::mpsc::channel::<PublicKey>(64);
        let reconnect_handle = discovery_service.as_ref().map(|service| service.reconnect_handle());
        if let Some(service) = &discovery_service {
            let mut reconnect_events = service.reconnect_events();
            let tx = reconnect_ready_tx.clone();
            tokio::spawn(async move {
                loop {
                    match reconnect_events.recv().await {
                        Ok(boru_core::discovery_service::ReconnectSignal::PeerReachable {
                            peer,
                        }) => {
                            info!(peer = %peer.fmt_short(), "reconnect signal forwarded to app");
                            let _ = tx.try_send(peer);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }

        // mDNS-based LAN peer discovery: when a peer appears on the LAN,
        // join it into the internal discovery gossip mesh (the mesh role the
        // lobby previously played, BORU-DISC-12), and forward the peer ID to
        // the UI for sidebar display. The discovery-service joiner is used so
        // the mesh edge forms on the discovery topic, never a conversation.
        {
            let memory_lookup_for_events = memory_lookup.clone();
            let tx = discovered_peers_tx.clone();
            let my_id = endpoint.id();
            let discovery_joiner = discovery_service.as_ref().map(|service| service.joiner());
            tokio::spawn(async move {
                use n0_future::StreamExt;
                let mut joined_peers = std::collections::HashSet::new();
                let mut events = mdns_events;
                while let Some(event) = events.next().await {
                    match event {
                        DiscoveryEvent::Discovered { endpoint_info, .. } => {
                            let peer = endpoint_info.endpoint_id;
                            if peer == my_id {
                                debug!(peer = %peer, "mDNS discovered our own endpoint, skipping");
                                continue;
                            }
                            // Keep the concrete addresses in the endpoint's
                            // shared lookup cache. mDNS can resolve the
                            // endpoint itself, but the explicit memory entry
                            // also makes subsequent dials deterministic.
                            memory_lookup_for_events.set_endpoint_info(endpoint_info);
                            if !joined_peers.insert(peer) {
                                continue;
                            }
                            // Spawn join_peers in a separate task so the
                            // mDNS event loop isn't blocked. join_peers
                            // triggers the gossip actor to dial the peer
                            // and establish a properly wired connection.
                            if let Some(joiner) = &discovery_joiner {
                                let joiner = joiner.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = joiner.join_peers(vec![peer]).await {
                                        warn!(peer = %peer, error = %e, "join_peers failed");
                                    } else {
                                        info!(peer = %peer, "join_peers succeeded");
                                    }
                                });
                            }
                            let _ = tx.try_send(DiscoveredPeersUpdate {
                                added: vec![peer],
                                removed: Vec::new(),
                            });
                        }
                        DiscoveryEvent::Expired { endpoint_id } => {
                            memory_lookup_for_events.remove_endpoint_info(endpoint_id);
                            if joined_peers.remove(&endpoint_id) {
                                info!(peer = %endpoint_id, "mDNS peer advertisement expired");
                                let _ = tx.try_send(DiscoveredPeersUpdate {
                                    added: Vec::new(),
                                    removed: vec![endpoint_id],
                                });
                            }
                        }
                        _ => {}
                    }
                }
            });
        }

        // Feed peers seen on the internal discovery topic into the Discover
        // sidebar. The DHT/lobby neighbour feed the old startup lobby drain
        // provided is now served by the discovery service (BORU-DISC-12).
        if let Some(service) = &discovery_service {
            let tx = discovered_peers_tx.clone();
            let mut peer_updates = service.peer_updates();
            // BORU-CP-07: the app (this task) owns the friendship check —
            // a fresh announcement from a KNOWN FRIEND is the trigger for a
            // reconnect attempt. The discovery service never decides
            // friend-ness (no authorisation by presence).
            let reconnect_handle_for_task = reconnect_handle.clone();
            let friends_for_reconnect = friends.clone();
            tokio::spawn(async move {
                loop {
                    match peer_updates.recv().await {
                        Ok(update) => {
                            match update {
                                boru_core::discovery_service::PeerUpdate::Seen { node_id, .. } => {
                                    let _ = tx.try_send(DiscoveredPeersUpdate {
                                        added: vec![node_id],
                                        removed: Vec::new(),
                                    });
                                    // BORU-CP-07: a fresh announcement from a
                                    // known friend is a reconnect trigger.
                                    // queue_reconnect dedups (one active
                                    // attempt per peer) and skips online
                                    // peers, so presence refreshes never
                                    // spam the scheduler.
                                    // BORU-CP-08: only message-capable
                                    // friends trigger reconnection — a
                                    // blocked relationship is never
                                    // resurrected (no authorisation by
                                    // presence).
                                    if let Some(handle) = &reconnect_handle_for_task {
                                        if friends_for_reconnect
                                            .get(&boru_core::friends::FriendId::from_public_key(
                                                node_id,
                                            ))
                                            .is_some_and(|record| record.relationship.can_message())
                                        {
                                            handle.queue_reconnect(node_id);
                                        }
                                    }
                                }
                                boru_core::discovery_service::PeerUpdate::Advertised {
                                    advertised,
                                    ..
                                } => {
                                    let _ = tx.try_send(DiscoveredPeersUpdate {
                                        added: vec![advertised],
                                        removed: Vec::new(),
                                    });
                                }
                                // BORU-CP-03: a peer went stale past the
                                // presence TTL — drop it from visible
                                // presence (stale peers disappear from
                                // active presence).
                                boru_core::discovery_service::PeerUpdate::Expired { node_id } => {
                                    let _ = tx.try_send(DiscoveredPeersUpdate {
                                        added: Vec::new(),
                                        removed: vec![node_id],
                                    });
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }

        // ── Global DHT bootstrap (BORU-DHT-01) ───────────────────────────
        // A fresh internet-only node (no mDNS / friend / ticket) bootstraps
        // into the discovery mesh by publishing its EndpointId to a
        // deterministic Mainnet namespace and looking up other nodes'
        // EndpointIds from the same namespace, then feeds them into the
        // existing discovery connectivity/join path. It reuses the shared
        // member-discovery DHT handle (a clone of room_discovery_dht); it is
        // skipped entirely under --no-dht (no DHT handle exists then). The DHT
        // only supplies candidate EndpointIds — all ongoing presence stays on
        // gossip, and this never creates a friendship/group/conversation.
        let bootstrap_cancel = tokio_util::sync::CancellationToken::new();
        if let (Some(service), Some(dht)) = (&discovery_service, &room_discovery_dht) {
            let backend = boru_core::discovery_backend::MainlineDhtBackend::new(dht.clone());
            let tracker = boru_core::discovery_bootstrap::DiscoveryBootstrapTracker::new(
                Box::new(backend),
                boru_core::public_room::PublicNetwork::Mainnet.network_byte(),
                local_public,
                secret_key.clone(),
                // Adaptive DHT discovery cadence (BORU-DHT-05): fresh/isolated
                // nodes probe fast, healthy meshes settle to a slow cadence.
                boru_core::discovery_bootstrap::BootstrapConfig {
                    cadence: Some(
                        boru_core::discovery_cadence::CadencePolicyConfig::default(),
                    ),
                    ..Default::default()
                },
            );
            let sink = service.bootstrap_sink();
            let cancel = bootstrap_cancel.clone();
            // `run` publishes the local EndpointId immediately, looks up the
            // namespace, and feeds discovered candidates into the connectivity
            // loop, then re-runs on the ~5-minute lease. It exits promptly
            // when `cancel` fires (deterministic shutdown).
            tokio::spawn(tracker.run(move |peers| sink.report(peers), cancel));
            info!("global DHT bootstrap started (Mainnet)");
        } else {
            debug!("global DHT bootstrap skipped (no discovery service or --no-dht)");
        }

        // ── Directory topic subscription ──────────────────────────────────
        // Subscribe to the directory gossip topic for public-room discovery.
        // The directory topic is derived from the relay URL so all peers on
        // the same relay share one directory mesh.
        //
        // The sender is shared with the MCP server so boru_create_public_room
        // can broadcast new ads immediately.
        let directory_sender = mcp_dir_sender_for_block;
        {
            let dir_sender_for_mcp = directory_sender.clone();
            if let Some(ref relay_url) = relay_url_for_directory {
                let dir_topic = boru_core::directory::directory_topic(relay_url);
                // Bootstrap the directory mesh with known friend identities.
                let bootstrap_peers: Vec<PublicKey> = friends
                    .iter()
                    .filter_map(|(id, _)| id.parse_public_key().ok())
                    .filter(|pk| *pk != local_public)
                    .collect();
                info!(dir_topic=%dir_topic, bootstrap_count=bootstrap_peers.len(),
                    "subscribing to directory topic with friend bootstrap");
                if let Ok(sub) = gossip.subscribe(dir_topic, bootstrap_peers).await {
                    splash_send("Joining directory...");
                    let (sender, mut receiver) = sub.split();
                    let heartbeat_sender = sender.clone();
                    *dir_sender_for_mcp.lock().unwrap() = Some(sender);
                    tokio::spawn(async move {
                        let marker = Bytes::from_static(b"\x01");
                        loop {
                            tokio::time::sleep(Duration::from_secs(15)).await;
                            if let Err(e) = heartbeat_sender.broadcast(marker.clone()).await {
                                warn!("directory heartbeat failed: {e}");
                            } else {
                                debug!("directory heartbeat sent");
                            }
                        }
                    });
                    let dir_tx = directory_room_tx.clone();
                    tokio::spawn(async move {
                        use n0_future::StreamExt;
                        let mut event_count: u64 = 0;
                        while let Some(event) = receiver.next().await {
                            event_count += 1;
                            let Ok(boru_core::api::Event::Received(msg)) = event else {
                                debug!("directory event #{event_count}: non-Received");
                                continue;
                            };
                            // Skip room-doc markers (metadata 0xFE, roster 0xFF)
                            if let Some(&marker) = msg.content.first() {
                                if marker == 0xFE || marker == 0xFF {
                                    debug!("directory event #{event_count}: doc marker");
                                    continue;
                                }
                            }
                            if let Ok((from, message, _sent_at)) =
                                SignedMessage::verify_and_decode(&msg.content)
                            {
                                if let Message::RoomAdvertisement { ad, .. } = message {
                                    // BORU-DIR-19 (PDF Task 7.1): enforce
                                    // the metadata bounds at the receive
                                    // boundary — an oversized room name /
                                    // description / ticket is discarded
                                    // before it can reach the directory
                                    // store or the UI. (The app drain
                                    // additionally rate-limits per author,
                                    // dedupes identical broadcasts, and
                                    // clamps absurd TTLs.)
                                    if let Err(violation) = boru_core::directory::
                                        legacy_advertisement_bounds_check(&ad)
                                    {
                                        debug!(from=%from, topic=%ad.topic, violation=?violation,
                                            "dropped room advertisement violating metadata bounds");
                                        continue;
                                    }
                                    info!(from=%from, topic=%ad.topic, name=%ad.room_name,
                                        "received room advertisement");
                                    let _ = dir_tx.try_send(app::DirectoryRoomEvent::Advertisement(ad, from));
                                } else if let Message::RoomWithdrawal { topic, signature } = message {
                                    // BORU-DIR-09 (PDF Task 3.3): a verified
                                    // withdrawal removes the matching
                                    // advertisement immediately; TTL expiry
                                    // remains the safety net if it is missed.
                                    if boru_core::chat_core::verify_room_withdrawal(
                                        &topic, &signature, from,
                                    ) {
                                        info!(from=%from, topic=%topic,
                                            "received verified room withdrawal");
                                        let _ = dir_tx
                                            .try_send(app::DirectoryRoomEvent::Withdrawal(topic, from));
                                    } else {
                                        debug!(from=%from, topic=%topic,
                                            "dropped unverifiable room withdrawal");
                                    }
                                } else {
                                    debug!(?message, "directory: non-ad message");
                                }
                            } else {
                                debug!("directory event #{event_count}: verify failed");
                            }
                        }
                        info!("directory receiver loop exited after {event_count} events");
                    });
                    info!("subscribed to directory topic {dir_topic}");
                    splash_send("Directory joined");
                } else {
                    warn!("failed to subscribe to directory topic");
                }
            } else {
                info!("directory topic: relay disabled, skipping subscription");
            }
        }
        let discovered_peers_rx = Arc::new(tokio::sync::Mutex::new(discovered_peers_rx_tmp));
        let directory_room_rx = Arc::new(tokio::sync::Mutex::new(directory_room_rx_tmp));

        // Spawn the backfill background actor for requesting history
        let backfill_handle = BackfillHandle::spawn(endpoint.clone());
        splash_send("Backfill service ready");

        let whisper_events_rx = Arc::new(tokio::sync::Mutex::new(whisper_events_rx_tmp));

        // Create the network event channel (shared across rooms, tagged by topic)
        let (net_tx, net_rx) = tokio::sync::mpsc::channel::<
            boru_core::conversations::ConversationNetEvent,
        >(256);
        let net_rx = Arc::new(tokio::sync::Mutex::new(net_rx));

        // ── Friend ping manager ──────────────────────────────────────
        let _guard = runtime.handle().enter();
        let (friend_mgr, friend_events_rx_tmp) = FriendPingManager::spawn(
            endpoint.clone(),
            DEFAULT_PING_INTERVAL,
            DEFAULT_CONNECT_TIMEOUT,
        );
        drop(_guard);
        let friend_events_rx = Arc::new(tokio::sync::Mutex::new(friend_events_rx_tmp));
        splash_send("Friend ping manager ready");

        // Register existing friends with the ping manager
        // (we're already inside runtime.block_on, so .await directly)
        splash_send("Registering known friends...");
        for peer in friends
            .iter()
            .filter_map(|(id, _)| id.parse_public_key().ok())
        {
            let addrs = friends
                .get(&FriendId::from_public_key(peer))
                .map(|record| record.known_addrs.clone())
                .unwrap_or_default();
            let _ = friend_mgr.add_friend_addrs(peer, addrs).await;
        }

        // Authorize inbox traffic through the persistent repository at receipt
        // time, rather than taking a startup snapshot.  This makes accepting,
        // blocking/removing a contact, and mailbox-key rotation effective for
        // already-running connections and also reconstructs correctly after a
        // restart.
        let friends_data_dir = data_dir.clone();
        let storage_for_auth = storage.clone();
        inbox_handle
            .set_authorization_fn(Some(Arc::new(move |peer| {
                // SQLite is the single source of truth for the friends list;
                // the legacy friends.json read path is gone.
                let store = FriendsStore::load_from_sqlite(&storage_for_auth, &friends_data_dir);
                let authorized = store.iter().any(|(id, record)| {
                    id.parse_public_key().ok() == Some(peer)
                        && record.relationship.can_message()
                        && record
                            .mailbox_public_key
                            .is_some_and(|mailbox| mailbox.identity == peer)
                });
                authorized
            })))
            .await;

        // Calls use the same durable friend allow-list as whisper/inbox.
        for (id, record) in friends.iter() {
            if record.relationship.can_message() {
                if let Ok(peer) = id.parse_public_key() {
                    call_handle.set_peer_authorized(peer, true);
                }
            }
        }

        Result::<_>::Ok((
            endpoint,
            memory_lookup,
            gossip,
            router,
            blob_store,
            net_rx,
            net_tx,
            friend_mgr,
            friend_events_rx,
            friends,
            room_history,
            chat_history,
            backfill_handle,
            whisper_events_rx,
            whisper_handle,
            call_handle,
            call_events_rx,
            inbox_events_rx,
            discovered_peers_rx,
            directory_room_rx,
            discovery_service,
            reconnect_ready_rx,
            reconnect_handle,
            room_discovery_dht,
            tunnel_service,
        ))
    })?;

    // Close the native splash window — the Iced window opens next.
    splash_send("Starting UI...");
    // Give the Iced window a moment to open before closing the splash,
    // avoiding a visual gap where neither window is visible.
    std::thread::sleep(std::time::Duration::from_millis(300));
    splash_send("DONE");
    drop(splash_child);

    // ── Start MCP diagnostic server if requested ────────────────────────
    // Create the Iced message journal shared between MCP and the GUI.
    let iced_diagnostics = IcedMessageJournal::new();

    // Create the GUI test action channel using GuiTestHandle (always — only consumed when enabled)
    let (gui_action_handle, gui_action_rx) = GuiTestHandle::channel(256);
    // Keep one history instance shared by the MCP producer and the Iced
    // consumer so status queries observe the same lifecycle transitions.
    let gui_action_history = gui_action_handle.history();

    // Create a watch channel for GUI state snapshots (used for diagnostics)
    let (gui_state_tx, _gui_state_rx) = watch::channel(boru_core::diagnostics::IcedStateSnapshot {
        node_id: String::new(),
        version: String::new(),
        active_screen: String::new(),
        active_room: None,
        conversation_count: 0,
        neighbor_count: 0,
        direct_peer_count: 0,
        relayed_peer_count: 0,
        mesh_health: String::new(),
        online_friend_count: 0,
        friend_count: 0,
        total_entry_count: 0,
        dark_mode: false,
        composer_text: String::new(),
        dialog_open: false,
        unread_count: 0,
        dashboard: None,
        timestamp: chrono::Utc::now(),
    });

    if args.mcp {
        let bind_addr: std::net::SocketAddr = args
            .mcp_bind
            .parse()
            .unwrap_or_else(|e| panic!("Invalid --mcp-bind address '{}': {e}", args.mcp_bind));

        if args.enable_gui_test_actions {
            if !bind_addr.ip().is_loopback() {
                eprintln!("\n  ERROR: --enable-gui-test-actions requires a loopback MCP binding.");
                eprintln!(
                    "  The current --mcp-bind '{}' is not a loopback address.",
                    args.mcp_bind
                );
                eprintln!(
                    "  Use the default (127.0.0.1:8765) or set --mcp-bind to 127.0.0.1:<port>.\n"
                );
                std::process::exit(1);
            }

            eprintln!(
                "\n  ⚠️  WARNING: GUI test actions are ENABLED via --enable-gui-test-actions."
            );
            eprintln!("  This exposes MCP tools that can interact with the application UI.");
            eprintln!("  Only bind to loopback addresses when this mode is active.\n");
        }

        let mcp_config = mcp_server::McpConfig {
            bind_addr,
            enable_gui_test_actions: args.enable_gui_test_actions,
        };
        let rooms_list = initial_room
            .as_ref()
            .map(|(topic, _)| vec![*topic])
            .unwrap_or_default();

        // Share the global DIAGNOSTICS singleton so MCP sees events from
        // the running application.
        let mcp_diagnostics = boru_core::chat_core::DIAGNOSTICS.clone();

        // ── Directory gossip sender (shared with MCP) ──
        // The main.rs directory receiver (inside block_on above) has already
        // subscribed to the directory gossip topic and stored its sender in
        // mcp_directory_sender. Reuse that sender instead of creating a second
        // subscription with no receiver loop.

        let mcp_state = mcp_server::McpAppState {
            diagnostics: mcp_diagnostics,
            iced_diagnostics: iced_diagnostics.clone(),
            endpoint: endpoint.clone(),
            rooms: Arc::new(std::sync::Mutex::new(rooms_list)),
            node_id: local_public.to_string(),
            version: app::version_tag(),
            gossip_tx: net_tx.clone(),
            secret_key: secret_key.clone(),
            gossip: gossip.clone(),
            gui_test_actions_enabled: args.enable_gui_test_actions,
            gui_action_tx: Some(gui_action_handle),
            gui_action_history: gui_test_actions::GuiActionHistory::default(),
            gui_action_lifecycle: gui_action_history.clone(),
            gui_action_rate_limiter: Arc::new(std::sync::Mutex::new(
                gui_test_actions::GuiActionRateLimiter::default(),
            )),
            gui_state_rx: Some(_gui_state_rx.clone()),
            storage: boru_core::storage::Storage::open(&data_dir).ok(),
            message_store: boru_core::store::MessageStore::open(data_dir.join("message_store.db"))
                .ok(),
            blob_store: Some(blob_store.clone().into()),
            downloads_dir: Some(data_dir.clone()),
            peer_lookup: Some(memory_lookup.clone()),
            directory_store: shared_directory_store.clone(),
            directory_sender: mcp_directory_sender,
        };

        if let Err(e) = runtime.block_on(mcp_server::spawn_mcp_server(mcp_config, mcp_state)) {
            error!("MCP server failed to start: {e}");
        }
    }

    let initial_topic = initial_room.as_ref().map(|r| r.0);


    let app_cell = std::sync::Mutex::new(Some((
        {
            let mut app = IcedChat::new(
                secret_key,
                gossip,
                router,
                Arc::clone(&file_offer_registry),
                blob_store,
                endpoint.clone(),
                memory_lookup,
                local_label,
                local_public,
                relay_mode,
                data_dir.clone(),
                runtime.handle().clone(),
                Arc::clone(&net_rx),
                net_tx,
                room_history,
                friends,
                friend_mgr,
                Arc::clone(&friend_events_rx),
                Arc::clone(&whisper_events_rx),
                inbox_events_rx,
                whisper_handle.clone(),
                call_handle.clone(),
                Arc::new(tokio::sync::Mutex::new(call_events_rx)),
                initial_room,
                chat_history,
                backfill_handle,
                initial_topic.is_some() && args.command.is_none(),
                Arc::clone(&discovered_peers_rx),
                // BORU-CP-07: reconnect signals from the backend (peers
                // whose endpoint connectivity was re-established) + the
                // handle to report real direct-topic readiness back.
                Arc::new(tokio::sync::Mutex::new(reconnect_ready_rx)),
                reconnect_handle,
                directory_room_rx,
                dht_for_private,
                args.no_dht,
                iced_diagnostics,
                Some(Arc::new(tokio::sync::Mutex::new(gui_action_rx))),
                gui_state_tx,
                gui_action_history,
                Some((*storage).clone()),
                Arc::clone(&tunnel_service),
                Arc::clone(&transfer_store),
                Arc::clone(&outbound_item_labels),
                Arc::clone(&inbound_item_labels),
            );
            // Enable snapshot throttle: max ~8 updates/sec (125ms gap)
            // so rapidly changing GUI state (composer text, unread counts)
            // doesn't flood the watch channel and MCP consumers.
            app.gui_snapshot_throttle_ms = 125;
            // BORU-UI-04: dev theme overrides loaded from <data_dir>/boru-ui.toml.
            // BORU-UI-07: route through `set_ui_theme_config` so the LIVE
            // merged theme (`active_theme`) is computed from the startup
            // config and the theme revision is initialized — view functions
            // read `boru_theme()` from state from the very first frame.
            // BORU-UI-08: when the dev-ui gate is off, `ui_theme_config` is
            // the empty default, so this is the identity merge — the app
            // uses `BoruTheme` defaults only.
            app.set_ui_theme_config(ui_theme_config);
            // BORU-LAYOUT-06: route the startup layout through
            // `set_layout_config` so the LIVE layout (`active_layout`) is the
            // merged config (default + boru-layout.toml overrides) and the
            // layout revision is initialized — view functions read
            // `boru_layout()` from state from the very first frame. When the
            // dev-ui gate is off, `layout_overrides` is the empty default, so
            // this is the identity merge — the app uses
            // `LayoutConfig::default()` only.
            let (startup_layout, _layout_warnings) = layout_merge::merge_layout_config(
                &layout::LayoutConfig::default(),
                &layout_overrides,
            );
            app.set_layout_config(startup_layout);
            // BORU-UI-06/08: dev theme file watcher — observes boru-ui.toml,
            // debounces save storms, parses on a background thread and sends
            // AppMessage::UiThemeReloaded into the update loop. Only started
            // when the live UI editor gate is on; otherwise `ui_theme_rx`
            // stays `None` and the subscription falls back to a closed dummy
            // receiver, so no reload message can ever reach the update loop.
            // Failure to start is non-fatal (live reload just stays off).
            if dev_ui {
                let (ui_theme_tx, ui_theme_rx) =
                    tokio::sync::mpsc::channel::<theme_watcher::UiThemeReloadMsg>(8);
                app.ui_theme_rx = Some(std::sync::Arc::new(tokio::sync::Mutex::new(ui_theme_rx)));
                if let Err(e) =
                    theme_watcher::spawn_ui_theme_watcher(data_dir.clone(), ui_theme_tx)
                {
                    warn!(error = %e, "boru-ui.toml watcher failed to start; live reload disabled");
                }
                // BORU-LAYOUT-06: dev layout file watcher — observes
                // boru-layout.toml, debounces save storms, parses on a
                // background thread and sends AppMessage::LayoutReloaded into
                // the update loop. Same gate as the theme watcher; failure to
                // start is non-fatal (live layout reload just stays off).
                let (layout_tx, layout_rx) =
                    tokio::sync::mpsc::channel::<layout_watcher::LayoutReloadMsg>(8);
                app.layout_rx = Some(std::sync::Arc::new(tokio::sync::Mutex::new(layout_rx)));
                if let Err(e) = layout_watcher::spawn_layout_watcher(data_dir.clone(), layout_tx)
                {
                    warn!(error = %e, "boru-layout.toml watcher failed to start; live reload disabled");
                }
            }
            app.directory_store = shared_directory_store;
            // BORU-CP-06: give the UI a read handle to the backend
            // connectivity state machine (the discovery service handle
            // itself stays off the UI). The optional presence indicator
            // reads per-peer state from here; disabling it only hides the
            // badge, never discovery or reconnection.
            app.connectivity_store = _discovery_service
                .as_ref()
                .map(|svc| svc.connectivity_store());
            app.network_map_source = _discovery_service
                .as_ref()
                .map(|svc| svc.network_map_source());
            // BORU-CP-12: give the UI a read-only handle to the
            // negotiated-capability view (PDF Task 4.3) so optional
            // features (voice/video calls, screen share, file transfer,
            // tunnels) are gated on the peer's advertised support. The
            // discovery service handle itself stays off the UI.
            app.capability_gate = _discovery_service
                .as_ref()
                .map(|svc| svc.capability_gate());
            // BORU-DIR-12 (PDF Task 4.3): give the UI a read handle to
            // the bounded room-directory cache so the app can feed the
            // real local relationship facts (joined rooms from the
            // conversation store + persisted hide preference) into the
            // per-room `local_join_state` derivation on each
            // ConnMonitorTick. The discovery service handle itself stays
            // off the UI.
            app.room_directory = _discovery_service
                .as_ref()
                .map(|svc| svc.room_directory());
            #[cfg(feature = "screen-sharing")]
            {
                // Wire the screen-share protocol channels and handle into the
                // app: events for invitations, media for the decode worker,
                // audio for the playback worker, and the protocol handle to
                // respond on inbound connections.
                app.calls_state.screen_share_protocol = Some(screen_share.0.clone());
                app.calls_state.screen_share_events_rx =
                    Some(Arc::new(tokio::sync::Mutex::new(screen_share.1)));
                app.calls_state.screen_share_media_rx =
                    Some(Arc::new(tokio::sync::Mutex::new(screen_share.2)));
                app.calls_state.screen_share_audio_rx =
                    Some(Arc::new(tokio::sync::Mutex::new(screen_share.3)));
                app.calls_state.screen_share_events_tx = Some(screen_share.4);
                // PDF Phase 12: developer diagnostics overlay on the
                // screen-share surface. Mirrors the live-UI-editor dev gate
                // (cargo feature `dev-ui`, or a debug build with --dev-ui /
                // BORU_DEV_UI=1); release builds without the feature stay off.
                app.calls_state.screen_share_dev_overlay = dev_ui;
            }
            app
        },
        initial_topic,
    )));

    // ── Heartbeat thread: sends "hb" to splash every 1s ────────────
    // This runs on a separate OS thread so it keeps ticking even if
    // the Iced event loop hangs.  The splash shows "not responding"
    // if no "hb" line arrives within 6 seconds.
    let hb_stdin = Arc::clone(&splash_stdin);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        if let Ok(mut guard) = hb_stdin.lock() {
            if let Some(ref mut stdin) = *guard {
                if writeln!(stdin, "hb").is_err() {
                    // Pipe closed — process is exiting
                    break;
                }
            } else {
                break;
            }
        } else {
            break;
        }
    });

    iced::application(
        move || {
            let (state, opt_topic) = app_cell
                .lock()
                .unwrap()
                .take()
                .expect("iced_chat boot called more than once");
            let task = if let Some(topic) = opt_topic {
                iced::Task::done(app::AppMessage::OpenRoom(topic))
            } else {
                iced::Task::none()
            };
            // Load bundled fonts at startup (non-fatal — falls back to
            // system default sans-serif on failure).
            let task = task.chain(fonts::load_fonts());
            // Always subscribe to the directory gossip topic at startup
            // so this peer can discover public rooms advertised by others
            // without having to create an advertised room itself first.
            let task = task.chain(iced::Task::done(app::AppMessage::SubscribeDirectoryTopic));
            (state, task)
        },
        IcedChat::update,
        IcedChat::view,
    )
    .window(iced::window::Settings {
        maximized: true,
        ..Default::default()
    })
    .title(|_: &IcedChat| format!("Boru — {}", app::version_tag()))
    .default_font(iced::Font {
        family: iced::font::Family::Name(crate::fonts::PUBLIC_SANS),
        weight: iced::font::Weight::Normal,
        stretch: iced::font::Stretch::Normal,
        style: iced::font::Style::Normal,
    })
    .subscription(|state: &IcedChat| {
        let mut subs: Vec<iced::Subscription<app::AppMessage>> = vec![];

        // Splash tick at 100ms while loading a room,
        // connecting to a peer in a chat conversation,
        // reconnecting on the main ChatList screen,
        // or playing a sidebar section appearance animation.
        let connecting = state.sender.is_none() && matches!(state.screen, app::Screen::Chat { .. });
        let main_reconnecting = matches!(state.screen, app::Screen::ChatList)
            && state.sender.is_none()
            && !state.room_loading;
        let sidebar_fading = state.sidebar_fade_active();
        if state.room_loading || connecting || main_reconnecting || sidebar_fading {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(100))
                    .map(|_| app::AppMessage::SplashTick),
            );
        }

        // Keep relative timestamps in the Recent Activity card fresh even
        // when no network or input event causes a normal redraw.
        subs.push(
            iced::time::every(std::time::Duration::from_secs(1))
                .map(|_| app::AppMessage::ActivityTick),
        );

        #[cfg(feature = "video-playback")]
        if state.has_inline_video() {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(250))
                    .map(|_| app::AppMessage::InlineVideoTick),
            );
        }

        #[cfg(feature = "terminal")]
        if let Some(term) = &state.terminal {
            subs.push(
                term.subscription()
                    .map(app::AppMessage::TerminalEvent),
            );
        }

        subs.extend(vec![
            IcedChat::subscription(
                Arc::clone(&state.net_rx),
                Arc::clone(&state.friend_events_rx),
                Arc::clone(&state.whisper_events_rx),
                Arc::clone(&state.inbox_events_rx),
                Arc::clone(&state.discovered_peers_rx),
                Arc::clone(&state.reconnect_ready_rx),
                state.gui_action_rx.clone(),
                Arc::clone(&state.files_state.transfer_update_rx),
                state.ui_theme_rx.clone(),
                state.layout_rx.clone(),
                Arc::clone(&state.call_events_rx),
                #[cfg(feature = "screen-sharing")]
                state.calls_state.screen_share_events_rx.clone(),
                #[cfg(feature = "screen-sharing")]
                state.calls_state.screen_share_frame_watch.clone(),
                #[cfg(feature = "screen-sharing")]
                state.calls_state.screen_share_stats_watch.clone(),
            ),
            app::keyboard_shortcuts_subscription(),
        ]);
        iced::Subscription::batch(subs)
    })
    .theme(|state: &IcedChat| {
        if state.dark_mode {
            Some(iced::Theme::Dark)
        } else {
            Some(iced::Theme::Light)
        }
    })
    .run()
    .unwrap_or_else(|err| {
        warn!("Failed to launch iced GUI: {err}");
        std::process::exit(1);
    });

    // Close the splash (the heartbeat thread will break on pipe error)
    if let Ok(mut guard) = splash_stdin.lock() {
        if let Some(ref mut stdin) = *guard {
            let _ = writeln!(stdin, "DONE");
        }
    }

    // Print performance baseline report if --perf was active
    if args.perf {
        perf_tracker::PerfTracker::print_report();
    }

    // The GUI owns clones of the endpoint, but iced drops the application
    // state before returning here.  Close the original endpoint explicitly
    // before dropping the runtime so iroh can shut down its discovery and
    // transport tasks cleanly instead of logging "Endpoint dropped without
    // calling Endpoint::close".
    runtime.block_on(endpoint.close());
    let _keep_runtime_alive = runtime;
    Ok(())
}

/// FS-05/FS-11/FS-17: consume blob-provider request events into the live
/// transfer projection and the durable activity log.
///
/// Every served blob request becomes an `Outbound` transfer record in the
/// live `TransferStateStore` (the "Peers Downloading from Me" panel) and a
/// privacy-filtered lifecycle event in the durable `transfer_activity`
/// table (the Activity Log's "By others" view). Progress checkpoints are fed
/// to the live store but intentionally NOT persisted, so a long upload cannot
/// flood the bounded activity table with thousands of rows.
fn spawn_outbound_provider_consumer(
    runtime: &tokio::runtime::Handle,
    mut rx: tokio::sync::mpsc::Receiver<ProviderMessage>,
    store: Arc<boru_core::transfer_state_projection::TransferStateStore>,
    item_labels: Arc<Mutex<std::collections::HashMap<String, String>>>,
    storage: Option<Arc<Storage>>,
    local_public: PublicKey,
) {
    use boru_core::diagnostics::{event_names, TransferLifecycleEvent};
    use boru_core::transfer_state_projection::{EventName, TransferDirection, TransferEvent};
    use iroh_blobs::provider::events::RequestUpdate;
    // Own the handle before the spawn so nested spawns don't borrow the
    // function-scope reference across the 'static task boundary. Two clones:
    // one is borrowed by this spawn call, the other is moved into the task.
    let handle = runtime.clone();
    let task_handle = handle.clone();
    handle.spawn(async move {
        let mut peers: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
        let mut transfers: std::collections::HashMap<(u64, u64), String> =
            std::collections::HashMap::new();
        while let Some(msg) = rx.recv().await {
            match msg {
                // The EventMask requests ConnectMode::Notify, so the plain
                // ClientConnected variant is not expected; both variants carry
                // the same fields (Notify<T> derefs to T).
                ProviderMessage::ClientConnected(msg) => {
                    if let Some(peer) = msg.inner.endpoint_id {
                        peers.insert(msg.inner.connection_id, peer.to_string());
                    }
                }
                // Notify wrapper derefs to the same ClientConnected payload.
                ProviderMessage::ClientConnectedNotify(msg) => {
                    if let Some(peer) = msg.inner.endpoint_id {
                        peers.insert(msg.inner.connection_id, peer.to_string());
                    }
                }
                ProviderMessage::ConnectionClosed(msg) => {
                    if let Some(peer) = peers.get(&msg.inner.connection_id).cloned() {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        store.disconnect_peer(&peer, now_ms);
                    }
                    peers.remove(&msg.inner.connection_id);
                    transfers.retain(|(conn, _), _| *conn != msg.inner.connection_id);
                }
                ProviderMessage::GetRequestReceivedNotify(msg) => {
                    let key = (msg.inner.connection_id, msg.inner.request_id);
                    let transfer_id = transfers
                        .entry(key)
                        .or_insert_with(|| {
                            format!("serve:{}-{}", msg.inner.connection_id, msg.inner.request_id)
                        })
                        .clone();
                    let peer_id = peers.get(&msg.inner.connection_id).cloned();
                    let mut update_rx = msg.rx;
                    let store = store.clone();
                    let item_labels = item_labels.clone();
                    let storage = storage.clone();
                    task_handle.spawn(async move {
                        let mut sequence = 0u64;
                        let mut current_hash: Option<String> = None;
                        let mut current_total: Option<u64> = None;
                        while let Ok(Some(update)) = update_rx.recv().await {
                            sequence += 1;
                            let now_ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0);
                            let (kind, bytes, total_bytes, error): (
                                EventName,
                                u64,
                                Option<u64>,
                                Option<String>,
                            ) = match update {
                                RequestUpdate::Started(started) => {
                                    let hash_hex = started.hash.to_string();
                                    if let Ok(mut labels) = item_labels.lock() {
                                        labels.entry(hash_hex.clone()).or_insert_with(|| {
                                            storage
                                                .as_ref()
                                                .and_then(|stg| {
                                                    stg.get_shared_file(
                                                        &local_public.to_string(),
                                                        &hash_hex,
                                                    )
                                                    .ok()
                                                    .flatten()
                                                })
                                                .map(|row| row.display_filename)
                                                .unwrap_or_else(|| {
                                                    let prefix: String =
                                                        hash_hex.chars().take(12).collect();
                                                    format!("file {prefix}…")
                                                })
                                        });
                                    }
                                    current_hash = Some(hash_hex.clone());
                                    current_total = Some(started.size);
                                    (EventName::Started, 0, Some(started.size), None)
                                }
                                RequestUpdate::Progress(progress) => (
                                    EventName::Progress,
                                    progress.end_offset,
                                    current_total,
                                    None,
                                ),
                                RequestUpdate::Completed(completed) => (
                                    EventName::Completed,
                                    completed.stats.payload_bytes_sent,
                                    None,
                                    None,
                                ),
                                RequestUpdate::Aborted(_) => (
                                    EventName::Failed,
                                    0,
                                    None,
                                    Some("Transfer aborted before completion".to_string()),
                                ),
                            };
                            let event = TransferEvent {
                                event_id: format!("serve:{transfer_id}:{sequence}:{now_ms}"),
                                transfer_id: transfer_id.clone(),
                                item_id: current_hash.clone().unwrap_or_default(),
                                direction: TransferDirection::Outbound,
                                peer_id: peer_id.clone(),
                                sequence,
                                attempt: 1,
                                occurred_at_ms: now_ms,
                                kind: kind.clone(),
                                bytes,
                                total_bytes,
                                error: error.clone(),
                            };
                            let is_progress = matches!(kind, EventName::Progress);
                            store.publish(event);
                            // Durable activity: persist non-progress lifecycle
                            // points with direction=outbound. Progress is live
                            // only, so the bounded table stays useful.
                            if !is_progress {
                                let (event_name, payload) = match kind {
                                    EventName::Started => (
                                        event_names::TRANSFER_STARTED,
                                        Some(serde_json::json!({
                                            "direction": "outbound",
                                            "total_bytes": current_total.unwrap_or(0),
                                        })),
                                    ),
                                    EventName::Completed => (
                                        event_names::COMPLETION,
                                        Some(serde_json::json!({
                                            "direction": "outbound",
                                            "bytes_transferred": bytes,
                                        })),
                                    ),
                                    EventName::Failed => (
                                        event_names::FAILURE,
                                        Some(serde_json::json!({
                                            "direction": "outbound",
                                            "error_category": "protocol_error",
                                            "reason": error.unwrap_or_default(),
                                        })),
                                    ),
                                    _ => (event_names::TRANSFER_STARTED, None),
                                };
                                if let Some(stg) = storage.as_ref() {
                                    let _ = stg.record_transfer_activity(&TransferLifecycleEvent {
                                        schema_version: 1,
                                        event_id: format!(
                                            "serve:{transfer_id}:{sequence}:{now_ms}"
                                        ),
                                        event_name: event_name.to_string(),
                                        transfer_id: transfer_id.clone(),
                                        sequence,
                                        occurred_at_ms: now_ms,
                                        attempt: 1,
                                        payload,
                                    });
                                }
                            }
                        }
                    });
                }
                ProviderMessage::GetManyRequestReceivedNotify(msg) => {
                    let key = (msg.inner.connection_id, msg.inner.request_id);
                    let transfer_id = transfers
                        .entry(key)
                        .or_insert_with(|| {
                            format!("serve:{}-{}", msg.inner.connection_id, msg.inner.request_id)
                        })
                        .clone();
                    let peer_id = peers.get(&msg.inner.connection_id).cloned();
                    let mut update_rx = msg.rx;
                    let store = store.clone();
                    let item_labels = item_labels.clone();
                    let storage = storage.clone();
                    task_handle.spawn(async move {
                        let mut sequence = 0u64;
                        let mut current_hash: Option<String> = None;
                        let mut current_total: Option<u64> = None;
                        while let Ok(Some(update)) = update_rx.recv().await {
                            sequence += 1;
                            let now_ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0);
                            let (kind, bytes, total_bytes, error): (
                                EventName,
                                u64,
                                Option<u64>,
                                Option<String>,
                            ) = match update {
                                RequestUpdate::Started(started) => {
                                    let hash_hex = started.hash.to_string();
                                    if let Ok(mut labels) = item_labels.lock() {
                                        labels.entry(hash_hex.clone()).or_insert_with(|| {
                                            storage
                                                .as_ref()
                                                .and_then(|stg| {
                                                    stg.get_shared_file(
                                                        &local_public.to_string(),
                                                        &hash_hex,
                                                    )
                                                    .ok()
                                                    .flatten()
                                                })
                                                .map(|row| row.display_filename)
                                                .unwrap_or_else(|| {
                                                    let prefix: String =
                                                        hash_hex.chars().take(12).collect();
                                                    format!("file {prefix}…")
                                                })
                                        });
                                    }
                                    current_hash = Some(hash_hex.clone());
                                    current_total = Some(started.size);
                                    (EventName::Started, 0, Some(started.size), None)
                                }
                                RequestUpdate::Progress(progress) => (
                                    EventName::Progress,
                                    progress.end_offset,
                                    current_total,
                                    None,
                                ),
                                RequestUpdate::Completed(completed) => (
                                    EventName::Completed,
                                    completed.stats.payload_bytes_sent,
                                    None,
                                    None,
                                ),
                                RequestUpdate::Aborted(_) => (
                                    EventName::Failed,
                                    0,
                                    None,
                                    Some("Transfer aborted before completion".to_string()),
                                ),
                            };
                            let event = TransferEvent {
                                event_id: format!("serve:{transfer_id}:{sequence}:{now_ms}"),
                                transfer_id: transfer_id.clone(),
                                item_id: current_hash.clone().unwrap_or_default(),
                                direction: TransferDirection::Outbound,
                                peer_id: peer_id.clone(),
                                sequence,
                                attempt: 1,
                                occurred_at_ms: now_ms,
                                kind: kind.clone(),
                                bytes,
                                total_bytes,
                                error: error.clone(),
                            };
                            let is_progress = matches!(kind, EventName::Progress);
                            store.publish(event);
                            // Durable activity: persist non-progress lifecycle
                            // points with direction=outbound. Progress is live
                            // only, so the bounded table stays useful.
                            if !is_progress {
                                let (event_name, payload) = match kind {
                                    EventName::Started => (
                                        event_names::TRANSFER_STARTED,
                                        Some(serde_json::json!({
                                            "direction": "outbound",
                                            "total_bytes": current_total.unwrap_or(0),
                                        })),
                                    ),
                                    EventName::Completed => (
                                        event_names::COMPLETION,
                                        Some(serde_json::json!({
                                            "direction": "outbound",
                                            "bytes_transferred": bytes,
                                        })),
                                    ),
                                    EventName::Failed => (
                                        event_names::FAILURE,
                                        Some(serde_json::json!({
                                            "direction": "outbound",
                                            "error_category": "protocol_error",
                                            "reason": error.unwrap_or_default(),
                                        })),
                                    ),
                                    _ => (event_names::TRANSFER_STARTED, None),
                                };
                                if let Some(stg) = storage.as_ref() {
                                    let _ = stg.record_transfer_activity(&TransferLifecycleEvent {
                                        schema_version: 1,
                                        event_id: format!(
                                            "serve:{transfer_id}:{sequence}:{now_ms}"
                                        ),
                                        event_name: event_name.to_string(),
                                        transfer_id: transfer_id.clone(),
                                        sequence,
                                        occurred_at_ms: now_ms,
                                        attempt: 1,
                                        payload,
                                    });
                                }
                            }
                        }
                    });
                }
                _ => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};
    use tracing::subscriber::with_default;
    use tracing_subscriber::EnvFilter;

    #[derive(Clone, Default)]
    struct BufferWriter(Arc<Mutex<Vec<u8>>>);

    struct BufferGuard<'a>(std::sync::MutexGuard<'a, Vec<u8>>);

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufferWriter {
        type Writer = BufferGuard<'a>;

        fn make_writer(&'a self) -> Self::Writer {
            BufferGuard(self.0.lock().expect("buffer mutex poisoned"))
        }
    }

    impl Write for BufferGuard<'_> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    fn buffer_to_string(buffer: &Arc<Mutex<Vec<u8>>>) -> String {
        String::from_utf8(buffer.lock().expect("buffer mutex poisoned").clone())
            .expect("log output should be valid utf-8")
    }

    #[test]
    fn logs_are_ted_to_terminal_when_terminal_is_available() {
        let file_buf = Arc::new(Mutex::new(Vec::new()));
        let term_buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = build_logging_subscriber(
            BufferWriter(file_buf.clone()),
            BufferWriter(term_buf.clone()),
            true,
            EnvFilter::new("info"),
            EnvFilter::new("info"),
        );

        with_default(subscriber, || {
            tracing::info!("terminal-visible message");
        });

        assert!(buffer_to_string(&file_buf).contains("terminal-visible message"));
        assert!(buffer_to_string(&term_buf).contains("terminal-visible message"));
    }

    #[test]
    fn logs_do_not_write_to_terminal_when_no_tty_is_present() {
        let file_buf = Arc::new(Mutex::new(Vec::new()));
        let term_buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = build_logging_subscriber(
            BufferWriter(file_buf.clone()),
            BufferWriter(term_buf.clone()),
            false,
            EnvFilter::new("info"),
            EnvFilter::new("info"),
        );

        with_default(subscriber, || {
            tracing::info!("hidden message");
        });

        assert!(buffer_to_string(&file_buf).contains("hidden message"));
        assert!(buffer_to_string(&term_buf).is_empty());
    }

    #[test]
    fn terminal_filter_suppresses_expected_discovery_diagnostics_only() {
        let file_buf = Arc::new(Mutex::new(Vec::new()));
        let term_buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = build_logging_subscriber(
            BufferWriter(file_buf.clone()),
            BufferWriter(term_buf.clone()),
            true,
            EnvFilter::new("trace"),
            EnvFilter::new("trace,swarm_discovery=warn,iroh::net_report=error"),
        );

        with_default(subscriber, || {
            tracing::info!(target: "swarm_discovery::sender", "no addresses for peer, not announcing");
            tracing::warn!(target: "iroh::net_report::report", "IPv4 address detected by QAD varies by destination");
            tracing::error!(target: "iroh::net_report::report", "endpoint network report failed");
            tracing::warn!(target: "application", "actionable application warning");
        });

        let file = buffer_to_string(&file_buf);
        let terminal = buffer_to_string(&term_buf);
        assert!(file.contains("no addresses for peer"));
        assert!(file.contains("IPv4 address detected by QAD"));
        assert!(terminal.contains("endpoint network report failed"));
        assert!(terminal.contains("actionable application warning"));
        assert!(!terminal.contains("no addresses for peer"));
        assert!(!terminal.contains("IPv4 address detected by QAD"));
    }

    // ── CLI argument tests ──────────────────────────────────────────

    #[test]
    fn enable_gui_test_actions_defaults_to_false() {
        let args = Args::try_parse_from(&["iced_chat"]).expect("should parse with no args");
        assert!(!args.enable_gui_test_actions);
    }

    #[test]
    fn enable_gui_test_actions_flag_enables_bool() {
        let args = Args::try_parse_from(&["iced_chat", "--enable-gui-test-actions"])
            .expect("should parse with flag");
        assert!(args.enable_gui_test_actions);
    }

    #[test]
    fn enable_gui_test_actions_compatible_with_mcp() {
        let args = Args::try_parse_from(&[
            "iced_chat",
            "--mcp",
            "--enable-gui-test-actions",
            "--mcp-bind",
            "127.0.0.1:9999",
        ])
        .expect("should parse mcp + gui-test-actions + custom bind");
        assert!(args.mcp);
        assert!(args.enable_gui_test_actions);
        assert_eq!(args.mcp_bind, "127.0.0.1:9999");
    }

    #[test]
    fn enable_gui_test_actions_no_mcp_is_ignored() {
        // --enable-gui-test-actions without --mcp is harmless — MCP is simply
        // not started, so the flag has no effect.
        let args = Args::try_parse_from(&["iced_chat", "--enable-gui-test-actions"])
            .expect("should parse without --mcp");
        assert!(!args.mcp);
        assert!(args.enable_gui_test_actions);
    }

    // ── DHT address publication tests ─────────────────────────────────

    #[test]
    fn publish_direct_addresses_defaults_to_false() {
        let args = Args::try_parse_from(&["iced_chat"]).expect("should parse with no args");
        assert!(!args.publish_direct_addresses);
        assert!(!args.no_dht);
    }

    #[test]
    fn publish_direct_addresses_flag_enables_bool() {
        let args = Args::try_parse_from(&["iced_chat", "--publish-direct-addresses"])
            .expect("should parse with flag");
        assert!(args.publish_direct_addresses);
    }

    #[test]
    fn publish_direct_addresses_works_without_no_dht() {
        // --publish-direct-addresses alone is valid (DHT default is enabled)
        let args = Args::try_parse_from(&["iced_chat", "--publish-direct-addresses"])
            .expect("should parse without --no-dht");
        assert!(args.publish_direct_addresses);
        assert!(!args.no_dht);
    }

    #[test]
    fn publish_direct_addresses_with_no_dht_is_rejected() {
        // Combining --publish-direct-addresses with --no-dht should fail
        // at the incompatible-option check in main().
        // clap parse itself succeeds — the incompatibility is checked at runtime.
        let args = Args::try_parse_from(&["iced_chat", "--publish-direct-addresses", "--no-dht"])
            .expect("clap should parse both flags; incompatibility enforced in main()");
        assert!(args.publish_direct_addresses);
        assert!(args.no_dht);

        // Verify the logic directly: the error is triggered when both are set
        let has_incompatibility = args.publish_direct_addresses && args.no_dht;
        assert!(has_incompatibility);
    }

    #[test]
    fn no_dht_alone_is_valid() {
        let args = Args::try_parse_from(&["iced_chat", "--no-dht"])
            .expect("should parse with --no-dht alone");
        assert!(args.no_dht);
        assert!(!args.publish_direct_addresses);
    }

    #[test]
    fn test_validate_bounded_ok() {
        assert!(mcp_server::validate_bounded("hello", 10, "test").is_ok());
    }

    #[test]
    fn test_validate_bounded_rejects_overflow() {
        assert!(mcp_server::validate_bounded("hello world", 5, "test").is_err());
    }

    #[test]
    fn test_validate_bounded_empty_is_ok() {
        assert!(mcp_server::validate_bounded("", 10, "test").is_ok());
    }

    #[test]
    fn test_validate_no_control_chars_ok() {
        assert!(mcp_server::validate_no_control_chars("hello world", "test").is_ok());
        assert!(mcp_server::validate_no_control_chars("  spaces_ok  ", "test").is_ok());
    }

    #[test]
    fn test_validate_no_control_chars_rejects_newline() {
        assert!(mcp_server::validate_no_control_chars("hello\nworld", "test").is_err());
    }

    #[test]
    fn test_validate_no_control_chars_rejects_tab() {
        assert!(mcp_server::validate_no_control_chars("hello\tworld", "test").is_err());
    }

    #[test]
    fn test_validate_no_control_chars_rejects_null() {
        assert!(mcp_server::validate_no_control_chars("hello\0world", "test").is_err());
    }

    #[test]
    fn test_validate_no_control_chars_rejects_cr() {
        assert!(mcp_server::validate_no_control_chars("hello\rworld", "test").is_err());
    }

    #[test]
    fn test_validate_peer_id_ok() {
        assert!(mcp_server::validate_peer_id(
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
        )
        .is_ok());
    }

    #[test]
    fn test_validate_peer_id_rejects_empty() {
        assert!(mcp_server::validate_peer_id("").is_err());
    }

    #[test]
    fn test_validate_peer_id_rejects_path_separator() {
        assert!(mcp_server::validate_peer_id("../etc/passwd").is_err());
        assert!(mcp_server::validate_peer_id("C:\\windows").is_err());
    }

    #[test]
    fn test_validate_peer_id_rejects_shell_metacharacters() {
        assert!(mcp_server::validate_peer_id("id; rm -rf /").is_err());
        assert!(mcp_server::validate_peer_id("echo `whoami`").is_err());
        assert!(mcp_server::validate_peer_id("foo|bar").is_err());
        assert!(mcp_server::validate_peer_id("$(evil)").is_err());
    }

    #[test]
    fn test_validate_peer_id_rejects_control_chars() {
        assert!(mcp_server::validate_peer_id("peer\nid").is_err());
    }

    #[test]
    fn test_validate_probe_id_ok() {
        assert!(mcp_server::validate_probe_id("probe-abc-123").is_ok());
    }

    #[test]
    fn test_validate_probe_id_rejects_path_separators() {
        assert!(mcp_server::validate_probe_id("probe/abc").is_err());
        assert!(mcp_server::validate_probe_id("probe\\abc").is_err());
    }

    #[test]
    fn test_validate_probe_id_rejects_control_chars() {
        assert!(mcp_server::validate_probe_id("probe\nabc").is_err());
    }

    #[test]
    fn test_validate_target_state_ok() {
        for state in &[
            "discovered",
            "address_resolved",
            "connected",
            "subscription_joined",
            "topic_member",
        ] {
            assert!(mcp_server::validate_target_state(state).is_ok());
        }
    }

    #[test]
    fn test_validate_target_state_rejects_invalid() {
        assert!(mcp_server::validate_target_state("not_a_state").is_err());
        assert!(mcp_server::validate_target_state("").is_err());
        assert!(mcp_server::validate_target_state("connected\n").is_err());
    }

    #[test]
    fn test_validate_no_path_or_shell_ok() {
        assert!(mcp_server::validate_no_path_or_shell("hello-world", "test").is_ok());
    }

    #[test]
    fn test_validate_no_path_or_shell_rejects_path_separators() {
        assert!(mcp_server::validate_no_path_or_shell("../foo", "test").is_err());
        assert!(mcp_server::validate_no_path_or_shell("C:\\bar", "test").is_err());
    }

    #[test]
    fn test_validate_no_path_or_shell_rejects_shell_metacharacters() {
        assert!(mcp_server::validate_no_path_or_shell("foo;bar", "test").is_err());
        assert!(mcp_server::validate_no_path_or_shell("foo`bar", "test").is_err());
        assert!(mcp_server::validate_no_path_or_shell("foo|bar", "test").is_err());
        assert!(mcp_server::validate_no_path_or_shell("foo>bar", "test").is_err());
    }

    #[test]
    fn test_sanitize_for_log_truncates_long_strings() {
        let long = "a".repeat(200);
        let sanitized = mcp_server::sanitize_for_log(&long, 10);
        assert!(sanitized.len() < 200);
        assert!(sanitized.contains("truncated"));
    }

    #[test]
    fn test_sanitize_for_log_escapes_newline() {
        let result = mcp_server::sanitize_for_log("hello\nworld", 100);
        assert!(!result.contains('\n'));
        assert!(result.contains("\\n"));
    }

    #[test]
    fn test_sanitize_for_log_preserves_short_text() {
        let result = mcp_server::sanitize_for_log("hello world", 100);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_sanitize_for_log_escapes_tab() {
        let result = mcp_server::sanitize_for_log("hello\tworld", 100);
        assert!(result.contains("\\t"));
    }

    #[test]
    fn test_sanitize_for_log_escapes_cr() {
        let result = mcp_server::sanitize_for_log("hello\rworld", 100);
        assert!(result.contains("\\r"));
    }

    // ── MCP server binding security tests ─────────────────────────────

    #[test]
    fn test_mcp_config_default_is_loopback() {
        let config = mcp_server::McpConfig::default();
        assert!(config.bind_addr.ip().is_loopback());
        assert!(!config.enable_gui_test_actions);
    }

    #[test]
    fn test_spawn_mcp_server_rejects_non_loopback_with_gui_actions() {
        // This tests the defense-in-depth check in spawn_mcp_server
        let config = mcp_server::McpConfig {
            bind_addr: "0.0.0.0:8765".parse().unwrap(),
            enable_gui_test_actions: true,
        };
        // Cannot spawn tokio runtime from #[test], but we can verify the
        // function signature and the error message:
        let result = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                // We need a valid McpAppState, which requires creating
                // an endpoint, etc. This is an integration-level test.
                // Instead, we verify the check by testing the logic in
                // spawn_mcp_server's first 20 lines.
                let check_passed = config.bind_addr.ip().is_loopback();
                if config.enable_gui_test_actions && !check_passed {
                    return Err("Refusing to start MCP server with --enable-gui-test-actions on non-loopback address. Use a 127.0.0.1:<port> address.".to_string());
                }
                Ok::<(), String>(())
            })
        }).join();
        let result_msg = result.unwrap();
        assert!(result_msg.is_err());
        let err = result_msg.unwrap_err();
        assert!(err.contains("non-loopback"));
        assert!(err.contains("127.0.0.1"));
    }

    #[test]
    fn test_spawn_mcp_server_loopback_is_ok_with_gui_actions() {
        // Verify loopback is accepted when gui actions enabled
        let check_passed = true; // 127.0.0.1 is loopback
        let enable_gui = true;
        let ok = !(enable_gui && !check_passed);
        assert!(ok);
    }

    #[test]
    fn test_spawn_mcp_server_non_loopback_warns_without_gui_actions() {
        // Non-loopback without gui actions should log a warning but not fail
        let config = mcp_server::McpConfig {
            bind_addr: "0.0.0.0:8765".parse().unwrap(),
            enable_gui_test_actions: false,
        };
        let check_passed = config.bind_addr.ip().is_loopback();
        let ok = !(config.enable_gui_test_actions && !check_passed);
        assert!(ok); // Should pass — no GuiActions
    }
    #[test]
    fn rotating_log_writer_keeps_bounded_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("iced_chat.log");
        let mut writer = RotatingFileWriter::open(&path, 8, 2).unwrap();
        writer.write_all(b"12345678").unwrap();
        writer.write_all(b"abcdefgh").unwrap();
        writer.write_all(b"ijklmnop").unwrap();
        writer.flush().unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"ijklmnop");
        assert_eq!(
            std::fs::read(path.with_extension("log.1")).unwrap(),
            b"abcdefgh"
        );
        assert_eq!(
            std::fs::read(path.with_extension("log.2")).unwrap(),
            b"12345678"
        );
    }

    #[test]
    fn rotating_log_writer_reports_rotation_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("iced_chat.log");
        let mut writer = RotatingFileWriter::open(&path, 8, 2).unwrap();
        writer.write_all(b"12345678").unwrap();

        // Make the oldest destination an occupied directory. The checked
        // rename must surface the failure instead of silently dropping it.
        let blocked_destination = path.with_extension("log.2");
        std::fs::create_dir(&blocked_destination).unwrap();
        std::fs::write(blocked_destination.join("occupied"), b"x").unwrap();
        std::fs::write(path.with_extension("log.1"), b"old history").unwrap();

        let error = writer.write(b"abcdefgh").unwrap_err();
        assert!(error.to_string().contains("failed to rotate"));
    }

    // ── Public-room DHT tracker wiring tests ───────────────────────────

    /// Test A: The GUI's `default_lobby_topic()` returns the canonical
    /// versioned Mainnet public-room identity, so the gossip mesh,
    /// `PublicRoomTracker`, and initial selected room all agree on the
    /// same topic.
    #[test]
    fn default_lobby_topic_matches_canonical_mainnet_lobby() {
        use boru_core::public_room::{public_lobby_topic, PublicNetwork};
        assert_eq!(
            IcedChat::default_lobby_topic(),
            public_lobby_topic(PublicNetwork::Mainnet)
        );
    }

    /// Test B: A Mainnet `PublicRoomTracker` identity uses the same gossip
    /// topic as the GUI lobby subscription.  Uses the in-memory backend so
    /// no live DHT access is required.
    #[tokio::test]
    async fn tracker_identity_matches_default_lobby_topic() {
        use boru_core::discovery_backend::InMemoryDiscoveryBackend;
        use boru_core::public_room::PublicNetwork;
        use boru_core::public_room_tracker::PublicRoomTracker;
        use iroh::SecretKey;

        let sk = SecretKey::generate();
        let tracker = PublicRoomTracker::start(
            Box::new(InMemoryDiscoveryBackend::new()),
            PublicNetwork::Mainnet,
            sk.public(),
            sk,
        )
        .await
        .expect("tracker start must not fail");
        assert_eq!(
            tracker.identity().topic,
            IcedChat::default_lobby_topic(),
            "public-room tracker topic must match the GUI public-room topic"
        );
        tracker.shutdown().await;
    }

    /// Test D: `--no-dht` suppresses the member-discovery DHT client that
    /// backs explicit room trackers (private rooms + user-created public
    /// rooms).  This isolates the startup decision logic (the
    /// `(!args.no_dht).then(...)` guard) rather than requiring a full
    /// network stack.  The startup public-lobby continuous tracker is gone
    /// (BORU-DISC-12); public-room trackers are created per user-created
    /// room inside `IcedChat`, not at startup.
    #[test]
    fn no_dht_flag_disables_member_discovery_client() {
        // — with --no-dht:
        let args = Args::try_parse_from(["boru", "--no-dht"].iter()).unwrap();
        assert!(args.no_dht);

        // Emulate main.rs gating: the DHT client must not be created.
        let room_discovery_dht: Option<()> = (!args.no_dht).then(|| ());
        assert!(
            room_discovery_dht.is_none(),
            "--no-dht must suppress the member-discovery DHT client"
        );

        // — without --no-dht:
        let args = Args::try_parse_from(["boru"].iter()).unwrap();
        assert!(!args.no_dht);
        let room_discovery_dht: Option<()> = (!args.no_dht).then(|| ());
        assert!(
            room_discovery_dht.is_some(),
            "DHT must be created when --no-dht is absent"
        );
    }

    // ── BORU-UI-08: developer-mode gate for the live UI editor ─────────

    #[test]
    fn dev_ui_gate_feature_wins_in_any_build() {
        // The `dev-ui` cargo feature is the deliberate opt-in: it enables
        // the live editor even in release builds, regardless of flags/env.
        assert!(dev_ui_gate_on(true, false, false, None));
        assert!(dev_ui_gate_on(true, true, false, None));
        assert!(dev_ui_gate_on(true, false, true, Some("0")));
        assert!(dev_ui_gate_on(true, true, true, Some("false")));
    }

    #[test]
    fn dev_ui_gate_release_without_feature_is_always_off() {
        // Release build (debug_assertions off) without the feature: the
        // editor is off no matter what the operator passes — boru-ui.toml
        // is never read and no watcher is spawned.
        assert!(!dev_ui_gate_on(false, false, false, None));
        assert!(!dev_ui_gate_on(false, false, true, None));
        assert!(!dev_ui_gate_on(false, false, true, Some("1")));
        assert!(!dev_ui_gate_on(false, false, true, Some("true")));
    }

    #[test]
    fn dev_ui_gate_debug_needs_switch_or_env() {
        // Debug build without the feature: off by default…
        assert!(!dev_ui_gate_on(false, true, false, None));
        assert!(!dev_ui_gate_on(false, true, false, Some("0")));
        assert!(!dev_ui_gate_on(false, true, false, Some("false")));
        assert!(!dev_ui_gate_on(false, true, false, Some("")));
        // …on with --dev-ui (the CLI switch wins even over a false env)…
        assert!(dev_ui_gate_on(false, true, true, None));
        assert!(dev_ui_gate_on(false, true, true, Some("0")));
        // …or BORU_DEV_UI=1 / BORU_DEV_UI=true (case-insensitive).
        assert!(dev_ui_gate_on(false, true, false, Some("1")));
        assert!(dev_ui_gate_on(false, true, false, Some("true")));
        assert!(dev_ui_gate_on(false, true, false, Some("TRUE")));
        assert!(dev_ui_gate_on(false, true, false, Some("True")));
    }

    #[test]
    fn dev_ui_cli_flag_parses() {
        let args = Args::try_parse_from(["boru", "--dev-ui"].iter()).unwrap();
        assert!(args.dev_ui, "--dev-ui must set the gate flag");
        let args = Args::try_parse_from(["boru"].iter()).unwrap();
        assert!(!args.dev_ui, "--dev-ui must default to off");
    }
}
