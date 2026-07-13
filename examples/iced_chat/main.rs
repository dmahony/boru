//! Iced desktop frontend for boru-chat.
//!
//! Usage:
//!   cargo run --features gui --example iced_chat       # show chat list
//!   cargo run --features gui --example iced_chat open   # open new room
//!   cargo run --features gui --example iced_chat join <ticket>  # join room

mod app;
mod log_viewer;
mod perf_tracker;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use boru_chat::backfill::{BackfillHandle, BackfillProtocolHandler, BACKFILL_ALPN};
use boru_chat::chat_core::friend_ping::{
    FriendPingManager, PingHandler, DEFAULT_CONNECT_TIMEOUT, DEFAULT_PING_INTERVAL,
    FRIEND_PING_ALPN,
};
use boru_chat::chat_history::ChatHistoryStore;
use boru_chat::discovery_backend::MainlineDhtBackend;
use boru_chat::friends::{FriendId, FriendsStore};
use boru_chat::inbox::{InboxHandle, InboxProtocol, INBOX_ALPN};
use boru_chat::mailbox::MailboxStore;
use boru_chat::net::{Gossip, GOSSIP_ALPN};
use boru_chat::proto::TopicId;
use boru_chat::public_room::PublicNetwork;
use boru_chat::public_room_continuous::{ContinuousTracker, ContinuousTrackerConfig};
use boru_chat::public_room_tracker::PublicRoomTracker;
use boru_chat::room::RoomStore;
use boru_chat::room_history::RoomHistoryStore;
use clap::Parser;
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, Endpoint, EndpointAddr, RelayMode,
    RelayUrl, SecretKey,
};
use iroh_blobs::{store::mem::MemStore, BlobsProtocol};

use boru_chat::whisper::{WhisperBuilder, WHISPER_ALPN};
use iroh_mainline_address_lookup::DhtAddressLookup;
#[cfg(feature = "gui")]
use iroh_mdns_address_lookup::MdnsAddressLookup;
use n0_error::{bail_any, Result, StdResultExt};
use tokio::sync::Mutex;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use app::IcedChat;

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
#[command(name = "iced_chat")]
struct Args {
    #[clap(long)]
    secret_key: Option<String>,
    #[clap(short, long)]
    relay: Option<RelayUrl>,
    #[clap(long)]
    no_relay: bool,
    /// Directory for persistent identity and friend state. Chat and room
    /// history are kept in memory only.
    /// Defaults to BORU_CHAT_DATA_DIR env var, or ~/.local/share/boru-chat/.
    #[clap(long)]
    data_dir: Option<PathBuf>,

    #[clap(short, long)]
    name: Option<String>,
    #[clap(long, default_value = "0")]
    bind_port: u16,
    /// Enable performance instrumentation and print baseline report at exit.
    #[clap(long)]
    perf: bool,
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
}

// ── Message protocol ──────────────────────────────────────────────────
pub use boru_chat::chat_core::{fmt_relay_mode, Message, NetEvent, SignedMessage, Ticket};

// ── Network event bridging ────────────────────────────────────────────
pub use boru_chat::chat_core::forward_gossip_events;

// ── Identity persistence ──────────────────────────────────────────────

fn get_data_dir(cli_override: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = cli_override {
        return dir;
    }
    if let Ok(val) = std::env::var("BORU_CHAT_DATA_DIR") {
        return PathBuf::from(val);
    }
    if let Some(val) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(val).join("boru-chat");
    }
    if let Some(val) = std::env::var_os("HOME") {
        return PathBuf::from(val)
            .join(".local")
            .join("share")
            .join("boru-chat");
    }
    if let Some(val) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(val).join("boru-chat");
    }
    std::env::current_dir()
        .unwrap_or_default()
        .join(".boru-chat")
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

fn init_logging(data_dir: &Path) -> Result<()> {
    let log_path = log_viewer::log_file_path(data_dir);
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).std_context("failed to create log directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    use std::fs::OpenOptions;
    use std::sync::{Arc, Mutex};

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .std_context("failed to open log file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&log_path, std::fs::Permissions::from_mode(0o600));
    }

    struct FileMakeWriter(Arc<Mutex<std::fs::File>>);
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for FileMakeWriter {
        type Writer = FileWriterGuard<'a>;
        fn make_writer(&'a self) -> Self::Writer {
            FileWriterGuard(self.0.lock().expect("log file mutex poisoned"))
        }
    }
    struct FileWriterGuard<'a>(std::sync::MutexGuard<'a, std::fs::File>);
    impl std::io::Write for FileWriterGuard<'_> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            std::io::Write::write(&mut *self.0, buf)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            std::io::Write::flush(&mut *self.0)
        }
    }

    let writer = FileMakeWriter(Arc::new(Mutex::new(file)));
    // Keep the persistent log useful by default.  The iroh endpoint emits
    // very high-volume discovery and DNS diagnostics at DEBUG; leaving that
    // level enabled made a single GUI session grow iced_chat.log to tens of
    // megabytes.  Operators can still opt into the full trace with RUST_LOG.
    let file_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // These are expected during normal endpoint startup and address
    // discovery. Keep them in the persistent log, but avoid making the GUI
    // terminal noisy. More severe events from either target remain visible.
    let terminal_filter = EnvFilter::new("info,swarm_discovery=warn,iroh::net_report=error");
    let subscriber = build_logging_subscriber(
        writer,
        std::io::stderr,
        std::io::stderr().is_terminal(),
        file_filter,
        terminal_filter,
    );
    let _ = tracing::subscriber::set_global_default(subscriber);
    Ok(())
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
    ensure_graphical_session();

    // Enable perf tracking if requested
    if args.perf {
        perf_tracker::PerfTracker::set_enabled(true);
    } else {
        perf_tracker::PerfTracker::set_enabled(false);
    }

    let _startup_timer = perf_tracker::PerfTracker::timer("app_startup", "full startup");

    let data_dir = get_data_dir(args.data_dir.clone());

    if matches!(&args.command, Some(Command::Logs)) {
        return log_viewer::run(log_viewer::log_file_path(&data_dir));
    }

    init_logging(&data_dir)?;
    info!(data_dir = %data_dir.display(), "starting iced chat");

    let runtime = tokio::runtime::Runtime::new().std_context("failed to create tokio runtime")?;
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
                            let room = RoomStore::new(&data_dir, t);
                            if let Err(err) = room.save() {
                                warn!(error = %err, "failed to save room metadata");
                            }
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
            Some(Command::Logs) => None,
            None => {
                let topic = app::IcedChat::default_lobby_topic();
                info!(topic = %topic, "opening default discovery lobby");
                Some((topic, vec![]))
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
        (false, None) => RelayMode::Default,
        (false, Some(url)) => RelayMode::Custom(url.into()),
    };
    info!("> relay: {}", fmt_relay_mode(&relay_mode));

    // ── Build the endpoint, gossip, and router (no topic subscription yet) ──

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
        notice,
        chat_history,
        backfill_handle,
        whisper_events_rx,
        whisper_handle,
        inbox_events_rx,
        continuous_tracker,
        discovered_peers_rx,
    ) = runtime.block_on(async {
        let memory_lookup = MemoryLookup::new();
        use std::net::{Ipv4Addr, SocketAddrV4};

        let endpoint = {
            {
                let ep_builder = if matches!(relay_mode, RelayMode::Disabled) {
                    Endpoint::builder(presets::N0DisableRelay)
                } else {
                    Endpoint::builder(presets::N0)
                };
                let endpoint = ep_builder
                    .secret_key(secret_key.clone())
                    .address_lookup(MdnsAddressLookup::builder())
                    .relay_mode(relay_mode.clone())
                    .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                    .bind()
                    .await?;
                #[allow(unused)]
                endpoint.address_lookup()?.add(memory_lookup.clone());
                if !matches!(relay_mode, RelayMode::Disabled) {
                    endpoint.online().await;
                }
                endpoint
            }
        };
        info!("> endpoint: {}", endpoint.id());

        // Add mDNS local address lookup for LAN peer discovery
        if let Ok(mdns) = MdnsAddressLookup::builder().build(endpoint.id()) {
            if let Ok(addr_lookup) = endpoint.address_lookup().as_ref() {
                addr_lookup.add(mdns);
            }
        }

        // Add DHT address lookup for global peer discovery via Mainline DHT.
        //
        // Enables peer discovery by EndpointID alone, without depending on
        // n0's DNS server.  Tradeoffs versus DNS/Pkarr:
        //
        //   + No central dependency — fully decentralized
        //   + Works in censorship-resistant or air-gapped setups
        //   - Slower lookups (500ms–5s vs ~100ms for DNS)
        //   - May be blocked by corporate/ISP firewalls (wide UDP port range)
        //   - Publishing a record takes time (~seconds)
        //
        // DHT supplements DNS/Pkarr: if DNS fails, DHT may still resolve.
        // Both are used alongside the default DNS/Pkarr from `presets::N0`.
        if let Ok(addr_lookup) = endpoint.address_lookup().as_ref() {
            if let Ok(dht) = DhtAddressLookup::builder()
                .secret_key(endpoint.secret_key().clone())
                .build()
            {
                addr_lookup.add(dht);
            }
        }

        let notice = "Direct iroh transport is operational.".to_string();

        let gossip = Gossip::builder().spawn(endpoint.clone());
        let blob_store = MemStore::new();
        let blobs_protocol = BlobsProtocol::new(&blob_store, None);

        // Load durable chat history. Outbox entries are loaded by IcedChat
        // alongside this store so queued messages can be replayed on reconnect.
        let chat_history = ChatHistoryStore::load_or_default(&data_dir);
        if !chat_history.is_empty() {
            info!(
                "> loaded {} chat message(s) from history",
                chat_history.len()
            );
        }
        let chat_history = Arc::new(std::sync::Mutex::new(chat_history));

        let backfill_handler = BackfillProtocolHandler::new(chat_history.clone());

        // ── Whisper protocol ──────────────────────────────────────────
        // Direct QUIC channels for private 1:1 messaging and file transfer.
        let whisper_builder = WhisperBuilder::new(endpoint.clone(), secret_key.clone());
        let whisper_handler = whisper_builder.protocol_handler();
        let (whisper_handle, whisper_events_rx_tmp) = whisper_builder.spawn();

        // ── Inbox protocol ────────────────────────────────────────────
        // Direct offline-message delivery via /iroh-chat-inbox/1.
        let (inbox_handle, inbox_events_rx_tmp) = InboxHandle::new();
        let inbox_handler =
            InboxProtocol::new(inbox_handle.inner()).with_secret_key(secret_key.clone());

        // Register the pending-envelopes provider so SyncRequest returns
        // envelopes stored locally for the requesting peer.
        {
            let mailbox_dir = data_dir.clone();
            let inbox_handle = inbox_handle.clone();
            // Use tokio::spawn since set_pending_fn is async
            let _ = tokio::spawn(async move {
                inbox_handle
                    .set_pending_fn(Some(Arc::new(move |requester, _since_ms| {
                        let mut store = MailboxStore::load(&mailbox_dir)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| MailboxStore::empty_at(&mailbox_dir));
                        store.pending_for_recipient(requester)
                    })))
                    .await;
            });
        }
        let inbox_events_rx = Arc::new(Mutex::new(inbox_events_rx_tmp));

        let router = iroh::protocol::Router::builder(endpoint.clone())
            .accept(GOSSIP_ALPN, gossip.clone())
            .accept(iroh_blobs::ALPN, blobs_protocol.clone())
            .accept(FRIEND_PING_ALPN, PingHandler)
            .accept(BACKFILL_ALPN, backfill_handler)
            .accept(WHISPER_ALPN, whisper_handler)
            .accept(INBOX_ALPN, inbox_handler)
            .spawn();

        // Subscribe to the personal inbox gossip topic so peers can always
        // deliver offline messages, independent of the visible chat room.
        let inbox_topic = InboxHandle::inbox_topic(secret_key.public());
        if let Err(e) = gossip.subscribe(inbox_topic, Vec::new()).await {
            warn!(error = %e, "failed to subscribe to inbox topic");
        }
        info!("subscribed to personal inbox topic");

        // Spawn the backfill background actor for requesting history
        let backfill_handle = BackfillHandle::spawn(endpoint.clone());

        let whisper_events_rx = Arc::new(Mutex::new(whisper_events_rx_tmp));

        // Load or create the persistent friends list
        let friends = FriendsStore::load_or_default(&data_dir);
        if friends.len() > 0 {
            info!("> loaded {} friend(s) from disk", friends.len());
        }

        // Load room history
        let room_history = RoomHistoryStore::load_or_default(&data_dir);
        if !room_history.is_empty() {
            info!("> loaded {} room(s) from history", room_history.len());
        }

        // Create the network event channel (shared across rooms, tagged by topic)
        let (net_tx, net_rx) = tokio::sync::mpsc::unbounded_channel::<
            boru_chat::conversations::ConversationNetEvent,
        >();
        let net_rx = Arc::new(Mutex::new(net_rx));

        // ── Friend ping manager ──────────────────────────────────────
        let _guard = runtime.handle().enter();
        let (friend_mgr, friend_events_rx_tmp) = FriendPingManager::spawn(
            endpoint.clone(),
            DEFAULT_PING_INTERVAL,
            DEFAULT_CONNECT_TIMEOUT,
        );
        drop(_guard);
        let friend_events_rx = Arc::new(Mutex::new(friend_events_rx_tmp));

        // Register existing friends with the ping manager
        // (we're already inside runtime.block_on, so .await directly)
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

        // ── Continuous DHT discovery & publication ────────────────────
        // Spawn background tasks that periodically publish local presence
        // and discover new peers on the DHT for the public lobby topic.
        let dht =
            distributed_topic_tracker::Dht::new(&distributed_topic_tracker::DhtConfig::default());
        let dummy_namespace = distributed_topic_tracker::TopicId::from_hash(&[0u8; 32]);
        let dht_backend = MainlineDhtBackend::new(dht, dummy_namespace);
        let public_room_tracker = PublicRoomTracker::start(
            Box::new(dht_backend),
            PublicNetwork::Mainnet,
            endpoint.id(),
            endpoint.secret_key().clone(),
        )
        .await?;
        let (new_peers_tx, new_peers_rx) = tokio::sync::mpsc::channel::<Vec<iroh::EndpointId>>(64);
        let continuous_tracker = ContinuousTracker::start(
            public_room_tracker,
            ContinuousTrackerConfig::default(),
            new_peers_tx,
        );
        let discovered_peers_rx = Arc::new(Mutex::new(new_peers_rx));

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
            notice,
            chat_history,
            backfill_handle,
            whisper_events_rx,
            whisper_handle,
            inbox_events_rx,
            continuous_tracker,
            discovered_peers_rx,
        ))
    })?;

    let initial_topic = initial_room.as_ref().map(|r| r.0);

    let app_cell = std::sync::Mutex::new(Some((
        IcedChat::new(
            secret_key,
            gossip,
            router,
            blob_store,
            endpoint.clone(),
            memory_lookup,
            local_label,
            local_public,
            relay_mode,
            data_dir,
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
            initial_room,
            notice,
            chat_history,
            backfill_handle,
            initial_topic.is_some() && args.command.is_none(),
            Some(continuous_tracker),
            Arc::clone(&discovered_peers_rx),
        ),
        initial_topic,
    )));

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
            (state, task)
        },
        IcedChat::update,
        IcedChat::view,
    )
    .title(|_: &IcedChat| format!("Boru Chat {}", app::version_tag()))
    .subscription(|state: &IcedChat| {
        let subs: Vec<iced::Subscription<app::AppMessage>> = vec![
            IcedChat::subscription(
                Arc::clone(&state.net_rx),
                Arc::clone(&state.friend_events_rx),
                Arc::clone(&state.whisper_events_rx),
                Arc::clone(&state.inbox_events_rx),
                Arc::clone(&state.discovered_peers_rx),
            ),
            app::keyboard_shortcuts_subscription(),
        ];
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
}
