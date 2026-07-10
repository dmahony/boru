//! Iced desktop frontend for iroh-gossip chat.
//!
//! Usage:
//!   cargo run --features gui --example iced_chat       # show chat list
//!   cargo run --features gui --example iced_chat open   # open new room
//!   cargo run --features gui --example iced_chat join <ticket>  # join room

use iroh::Watcher;

mod app;
mod log_viewer;

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, Endpoint, EndpointAddr, RelayMode,
    RelayUrl, SecretKey,
};
use iroh_blobs::{store::mem::MemStore, BlobsProtocol};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use iroh_mainline_address_lookup::DhtAddressLookup;
use iroh_gossip::backfill::{
    BackfillHandle, BackfillProtocolHandler, BACKFILL_ALPN, BACKFILL_TRIGGER_THRESHOLD,
};
use iroh_gossip::chat_core::friend_ping::{
    FriendPingManager, PingHandler, DEFAULT_CONNECT_TIMEOUT, DEFAULT_PING_INTERVAL,
    FRIEND_PING_ALPN,
};
use iroh_gossip::chat_history::ChatHistoryStore;
use iroh_gossip::friends::FriendsStore;
use iroh_gossip::net::{Gossip, GOSSIP_ALPN};
use iroh_gossip::proto::TopicId;
use iroh_gossip::room::RoomStore;
use iroh_gossip::room_history::RoomHistoryStore;
use iroh_gossip::whisper::{WhisperBuilder, WhisperEvent, WhisperHandle, WHISPER_ALPN};
#[cfg(feature = "tor-transport")]
use iroh_gossip::tor_transport::{bootstrap_tor, monitor_tor_health, TorStorageDirs, TorTransport};
use n0_error::{bail_any, Result, StdResultExt};
use tokio::sync::Mutex;

use app::IcedChat;

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
    /// Directory for persistent state (secret key, chat history, room history).
    /// Defaults to IROH_GOSSIP_CHAT_DATA_DIR env var, or ~/.local/share/iroh-gossip-chat/.
    #[clap(long)]
    data_dir: Option<PathBuf>,
    /// Use Tor hidden services instead of direct iroh connectivity.
    #[cfg(feature = "tor-transport")]
    #[clap(long)]
    tor: bool,
    #[clap(short, long)]
    name: Option<String>,
    #[clap(long, default_value = "0")]
    bind_port: u16,
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
pub use iroh_gossip::chat_core::{fmt_relay_mode, Message, NetEvent, SignedMessage, Ticket};

// ── Network event bridging ────────────────────────────────────────────
pub use iroh_gossip::chat_core::forward_gossip_events;

// ── Identity persistence ──────────────────────────────────────────────

fn get_data_dir(cli_override: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = cli_override {
        return dir;
    }
    if let Ok(val) = std::env::var("IROH_GOSSIP_CHAT_DATA_DIR") {
        return PathBuf::from(val);
    }
    if let Some(val) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(val).join("iroh-gossip-chat");
    }
    if let Some(val) = std::env::var_os("HOME") {
        return PathBuf::from(val)
            .join(".local")
            .join("share")
            .join("iroh-gossip-chat");
    }
    if let Some(val) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(val).join("iroh-gossip-chat");
    }
    std::env::current_dir()
        .unwrap_or_default()
        .join(".iroh-gossip-chat")
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
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(writer).with_ansi(false))
        .try_init();
    Ok(())
}

// ── Entry point ───────────────────────────────────────────────────────

fn main() -> Result<()> {
    let args = Args::parse();
    ensure_graphical_session();

    let data_dir = get_data_dir(args.data_dir.clone());

    if matches!(&args.command, Some(Command::Logs)) {
        return log_viewer::run(log_viewer::log_file_path(&data_dir));
    }

    init_logging(&data_dir)?;
    info!(data_dir = %data_dir.display(), "starting iced chat");

    let runtime = tokio::runtime::Runtime::new().std_context("failed to create tokio runtime")?;
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
                info!("showing chat list");
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

    let use_tor = {
        #[cfg(feature = "tor-transport")]
        {
            args.tor
        }
        #[cfg(not(feature = "tor-transport"))]
        {
            false
        }
    };
    let relay_mode = match (use_tor, args.no_relay, args.relay.clone()) {
        (_, true, Some(_)) => bail_any!("--no-relay and --relay are mutually exclusive"),
        (_, true, None) => RelayMode::Disabled,
        (true, false, None) => RelayMode::Disabled,
        (false, false, None) => RelayMode::Default,
        (_, false, Some(url)) => RelayMode::Custom(url.into()),
    };
    info!("> relay: {}", fmt_relay_mode(&relay_mode));

    // ── Tor reconnection monitor channel ──────────────────────────────
    #[allow(unused)]
    let (tor_reconnect_tx, tor_reconnect_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // ── Build the endpoint, gossip, and router (no topic subscription yet) ──

    let (
        endpoint,
        local_peer_addr,
        gossip,
        blob_store,
        net_rx,
        net_tx,
        friend_mgr,
        friend_events_rx,
        friends,
        room_history,
        tor_reconnect_rx_opt,
        notice,
        chat_history,
            backfill_handle,
            whisper_events_rx,
            whisper_handle,
    ) = runtime.block_on(async {
        let memory_lookup = MemoryLookup::new();
        use std::net::{Ipv4Addr, SocketAddrV4};

        let (endpoint, local_peer_addr) = {
            #[cfg(feature = "tor-transport")]
            if use_tor {
                let tor_dirs = TorStorageDirs::new()?;
                let (tor_client, tor_status_message) = bootstrap_tor(&tor_dirs).await?;
                let tor_transport =
                    TorTransport::new(secret_key.public(), Arc::clone(&tor_client), args.bind_port);
                let endpoint = Endpoint::builder(presets::N0DisableRelay)
                    .secret_key(secret_key.clone())
                    .address_lookup(memory_lookup.clone())
                    .relay_mode(relay_mode.clone())
                    .add_custom_transport(Arc::new(tor_transport.clone()))
                    .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                    .bind()
                    .await?;
                endpoint.online().await;
                let local_peer_addr = tor_transport.watch_local_peer_addr().initialized().await.endpoint_addr();

                // Spawn the Tor health-monitor background task
                let monitor_client = Arc::clone(&tor_client);
                let monitor_tx = tor_reconnect_tx.clone();
                tokio::spawn(async move {
                    monitor_tor_health(monitor_client, monitor_tx).await;
                });

                info!("> Tor bootstrap finished: {tor_status_message}");
                (endpoint, local_peer_addr)
            } else {
                let ep_builder = if matches!(relay_mode, RelayMode::Disabled) {
                    Endpoint::builder(presets::N0DisableRelay)
                } else {
                    Endpoint::builder(presets::N0)
                };
                let endpoint = ep_builder
                    .secret_key(secret_key.clone())
                    .address_lookup(memory_lookup.clone())
                    .relay_mode(relay_mode.clone())
                    .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                    .bind()
                    .await?;
                if !matches!(relay_mode, RelayMode::Disabled) {
                    endpoint.online().await;
                }
                let local_peer_addr = endpoint.addr();
                (endpoint, local_peer_addr)

            }
            #[cfg(not(feature = "tor-transport"))]
            {
                let ep_builder = if matches!(relay_mode, RelayMode::Disabled) {
                    Endpoint::builder(presets::N0DisableRelay)
                } else {
                    Endpoint::builder(presets::N0)
                };
                let endpoint = ep_builder
                    .secret_key(secret_key.clone())
                    .address_lookup(memory_lookup.clone())
                    .relay_mode(relay_mode.clone())
                    .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                    .bind()
                    .await?;
                if !matches!(relay_mode, RelayMode::Disabled) {
                    endpoint.online().await;
                }
                let local_peer_addr = endpoint.addr();
                (endpoint, local_peer_addr)

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

        let notice = if use_tor {
            "Tor-backed custom transport is operational.".to_string()
        } else {
            "Direct iroh transport is operational.".to_string()
        };

        let gossip = Gossip::builder().spawn(endpoint.clone());
        let blob_store = MemStore::new();
        let blobs_protocol = BlobsProtocol::new(&blob_store, None);

        // Load chat message history (needed before Router for backfill)
        let chat_history = ChatHistoryStore::load_or_default(&data_dir);
        if !chat_history.is_empty() {
            info!(
                "> loaded {} chat message(s) from history (durable local state in chat_history.json; use /leave to clear the active room, or delete the file to clear all rooms)",
                chat_history.len()
            );
        }
        let chat_history = Arc::new(std::sync::Mutex::new(chat_history));

        let backfill_handler = BackfillProtocolHandler::new(chat_history.clone());

        // ── Whisper protocol ──────────────────────────────────────────
        // Direct QUIC channels for private 1:1 messaging and file transfer.
        let whisper_builder = WhisperBuilder::new(endpoint.clone(), secret_key.clone());
        let whisper_handler = whisper_builder.protocol_handler();
        let (whisper_handle, whisper_events_rx_tmp) = whisper_builder.spawn().await;

        let _router = iroh::protocol::Router::builder(endpoint.clone())
            .accept(GOSSIP_ALPN, gossip.clone())
            .accept(iroh_blobs::ALPN, blobs_protocol.clone())
            .accept(FRIEND_PING_ALPN, PingHandler)
            .accept(BACKFILL_ALPN, backfill_handler)
            .accept(WHISPER_ALPN, whisper_handler)
            .spawn();

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

        // Create the network event channel (shared across rooms)
        let (net_tx, net_rx) = tokio::sync::mpsc::unbounded_channel();
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
            let _ = friend_mgr.add_friend(peer, None).await;
        }

        Result::<_>::Ok((
            endpoint,
            local_peer_addr,
            gossip,
            blob_store,
            net_rx,
            net_tx,
            friend_mgr,
            friend_events_rx,
            friends,
            room_history,
            use_tor.then(|| Arc::new(Mutex::new(tor_reconnect_rx))),
            notice,
            chat_history,
            backfill_handle,
            whisper_events_rx,
            whisper_handle,
        ))
    })?;

    let initial_topic = initial_room.as_ref().map(|r| r.0);

    let app_cell = std::sync::Mutex::new(Some((
        IcedChat::new(
            secret_key,
            gossip,
            blob_store,
            endpoint.clone(),
            local_label,
            local_public,
            local_peer_addr,
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
            whisper_handle.clone(),
            tor_reconnect_rx_opt,
            initial_room,
            notice,
            chat_history,
            backfill_handle,
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
    .title(|_: &IcedChat| "Iroh Gossip Chat".to_string())
    .subscription(|state: &IcedChat| {
        IcedChat::subscription(
            Arc::clone(&state.net_rx),
            Arc::clone(&state.friend_events_rx),
            Arc::clone(&state.whisper_events_rx),
        )
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

    let _keep_runtime_alive = runtime;
    Ok(())
}
