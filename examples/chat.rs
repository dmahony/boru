//! Terminal UI (TUI) chat frontend using iroh-gossip.
//!
//! Usage: `cargo chat open` or `cargo chat join <ticket>`.
//!
//! This example uses the shared [`iroh_gossip::chat_core`] module for the
//! protocol types, state machine, and network event handling.  Only the
//! TUI-specific rendering (ratatui) and input handling (crossterm) live here.

use std::{
    collections::{HashMap, HashSet},
    env, io,
    net::{Ipv4Addr, SocketAddrV4},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

#[cfg(feature = "tor-transport")]
use iroh_gossip::tor_transport::{
    bootstrap_tor, monitor_tor_health, TorStorageDirs, TorTransport,
};
#[cfg(feature = "tor-transport")]
use iroh::Watcher;
use clap::Parser;
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event as CEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, Endpoint, PublicKey, RelayMode,
    RelayUrl, SecretKey,
};
use iroh_blobs::{store::mem::MemStore, ticket::BlobTicket, BlobsProtocol};
use iroh_gossip::chat_core::friend_ping::{
    FriendEvent, FriendPingManager, FriendStatus, PingHandler, DEFAULT_CONNECT_TIMEOUT,
    DEFAULT_PING_INTERVAL, FRIEND_PING_ALPN,
};
use iroh_gossip::chat_core::{
    self, check_peer_connection_type, fmt_relay_mode, handle_net_event, message_hash,
    update_connection_counts, AppState, ChatEntry, ChatKind, ConnectionType, Message,
    SignedMessage, StatusContext, Ticket,
};
use iroh_gossip::friends::{FriendId, FriendRecord, FriendsStore};
use iroh_gossip::room::RoomStore;
use iroh_gossip::room_docs::{
    self, create_metadata_doc, create_roster_doc, list_members, read_metadata, RoomDocs,
    RoomMetadata,
};
use iroh_gossip::small_room::{
    room_size_fits_small_room, SmallRoomBuilder, SMALL_ROOM_ALPN, SMALL_ROOM_MAX_SIZE,
};
use iroh_gossip::{
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
};
use n0_error::{bail_any, Result, StdResultExt};
use n0_future::task;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame, Terminal,
};

/// Chat over iroh-gossip
///
/// This broadcasts signed messages over iroh-gossip and verifies signatures
/// on received messages.
///
/// By default a new endpoint id is created when starting the example. To reuse your identity,
/// set the `--secret-key` flag with the secret key printed on a previous invocation.
///
/// By default, the relay server run by n0 is used. To use a local relay server, run
///     cargo run --bin iroh-relay --features iroh-relay -- --dev
/// in another terminal and then set the `-d http://localhost:3340` flag on this example.
#[derive(Parser, Debug)]
struct Args {
    /// secret key to derive our endpoint id from.
    #[clap(long)]
    secret_key: Option<String>,
    /// Set a custom relay server. By default, the relay server hosted by n0 will be used.
    #[clap(short, long)]
    relay: Option<RelayUrl>,
    /// Disable relay completely.
    #[clap(long)]
    no_relay: bool,
    /// Use Tor hidden services instead of direct iroh connectivity.
    #[cfg(feature = "tor-transport")]
    #[clap(long)]
    tor: bool,
    /// Set your nickname.
    #[clap(short, long)]
    name: Option<String>,
    /// Set the bind port for our socket. By default, a random port will be used.
    #[clap(long, default_value = "0")]
    bind_port: u16,
    #[clap(subcommand)]
    command: Command,
}

#[derive(Parser, Debug)]
enum Command {
    /// Open a chat room for a topic and print a ticket for others to join.
    ///
    /// If no topic is provided, a new topic will be created.
    Open {
        /// Optionally set the topic id (64 bytes, as hex string).
        topic: Option<TopicId>,
    },
    /// Join a chat room from a ticket.
    Join {
        /// The ticket, as base32 string.
        ticket: String,
    },
}

fn get_data_dir() -> PathBuf {
    if let Ok(val) = env::var("IROH_GOSSIP_CHAT_DATA_DIR") {
        return PathBuf::from(val);
    }
    if let Some(val) = env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(val).join("iroh-gossip-chat");
    }
    if let Some(val) = env::var_os("HOME") {
        return PathBuf::from(val)
            .join(".local")
            .join("share")
            .join("iroh-gossip-chat");
    }
    if let Some(val) = env::var_os("LOCALAPPDATA") {
        return PathBuf::from(val).join("iroh-gossip-chat");
    }
    // Fallback
    std::env::current_dir()
        .unwrap_or_default()
        .join(".iroh-gossip-chat")
}

fn load_or_generate_secret_key() -> Result<(SecretKey, PathBuf)> {
    load_or_generate_secret_key_at(&get_data_dir())
}

fn load_or_generate_secret_key_at(data_dir: &Path) -> Result<(SecretKey, PathBuf)> {
    let key_path = data_dir.join("secret_key.txt");

    if key_path.exists() {
        let key_str =
            std::fs::read_to_string(&key_path).std_context("failed to read secret key file")?;
        let key_str = key_str.trim();
        let key =
            SecretKey::from_str(key_str).std_context("failed to parse secret key from file")?;
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    #[cfg(feature = "tor-transport")]
    let use_tor = args.tor;
    #[cfg(not(feature = "tor-transport"))]
    let use_tor = false;

    // parse the cli command
    let (topic, peers) = match &args.command {
        Command::Open { topic } => {
            let topic = match topic {
                Some(t) => *t,
                None => {
                    // Try to reuse a previously saved room topic.
                    let data_dir = get_data_dir();
                    match RoomStore::load_or_none(&data_dir) {
                        Some(store) => {
                            println!("> reusing saved room topic {}", store.topic);
                            store.topic
                        }
                        None => {
                            let t = TopicId::from_bytes(rand::random());
                            println!("> opening new chat room for topic {t}");
                            // Persist the new topic so reopening reuses it.
                            let room = RoomStore::new(&data_dir, t);
                            if let Err(err) = room.save() {
                                eprintln!("warning: failed to save room metadata: {err}");
                            }
                            t
                        }
                    }
                }
            };
            (topic, vec![])
        }
        Command::Join { ticket } => {
            let Ticket { topic, peers } = Ticket::from_str(ticket)?;
            println!("> joining chat room for topic {topic}");
            (topic, peers)
        }
    };

    // parse or generate our secret key
    let (secret_key, key_path) = match args.secret_key.as_ref() {
        None => load_or_generate_secret_key()?,
        Some(key) => {
            let key = key.parse()?;
            // When passed via CLI, we just pretend it was loaded from a synthetic path
            // so we don't save or overwrite the user's explicit CLI override.
            (key, PathBuf::from("<passed via cli flag>"))
        }
    };
    println!("> our public key: {}", secret_key.public());
    println!("> identity file: {}", key_path.display());

    // load or create the persistent friends list
    let data_dir = get_data_dir();
    let friends = FriendsStore::load_or_default(&data_dir);
    let friend_count = friends.len();
    if friend_count > 0 {
        println!("> loaded {friend_count} friend(s) from disk");
    }

    // configure our relay map
    // When Tor is used, default to disabled relays — Tor hidden services provide direct
    // connectivity without needing the iroh relay infrastructure.
    let relay_mode = match (use_tor, args.no_relay, args.relay.clone()) {
        (_, true, Some(_)) => bail_any!("You cannot set --no-relay and --relay at the same time"),
        (_, true, None) => RelayMode::Disabled,
        (true, false, None) => RelayMode::Disabled,
        (false, false, None) => RelayMode::Default,
        (_, false, Some(url)) => RelayMode::Custom(url.into()),
    };
    println!("> using relay servers: {}", fmt_relay_mode(&relay_mode));

    // create a memory lookup to pass in endpoint addresses to
    let memory_lookup = MemoryLookup::new();

    // ── Tor reconnection monitor channel ──────────────────────────────
    // Created unconditionally so the event loop always compiles.
    // The monitor task is only spawned in Tor mode; otherwise the
    // sender is never cloned and sits dormant.
    #[allow(unused)]
    let (tor_reconnect_tx, mut tor_reconnect_rx) =
        tokio::sync::mpsc::unbounded_channel::<String>();

    // build our iroh endpoint
    let (endpoint, transport_status_message, transport_notice_text, local_peer_addr) = {
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
            let local_peer_addr = tor_transport.watch_local_peer_addr().initialized().await;

            // Spawn the Tor health-monitor background task to detect
            // and reconnect with exponential backoff if Tor drops after
            // the initial bootstrap.
            let monitor_client = Arc::clone(&tor_client);
            let monitor_tx = tor_reconnect_tx.clone();
            tokio::spawn(async move {
                monitor_tor_health(monitor_client, monitor_tx).await;
            });

            (
                endpoint,
                format!("Tor bootstrap finished: {tor_status_message}"),
                "Tor-backed custom transport is operational. Gossip messages are relayed over Tor hidden services."
                    .to_string(),
                local_peer_addr.endpoint_addr(),
            )
        } else {
            let endpoint = if matches!(relay_mode, RelayMode::Disabled) {
                Endpoint::builder(presets::N0DisableRelay)
                    .secret_key(secret_key.clone())
                    .address_lookup(memory_lookup.clone())
                    .relay_mode(relay_mode.clone())
                    .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                    .bind()
                    .await?
            } else {
                Endpoint::builder(presets::N0)
                    .secret_key(secret_key.clone())
                    .address_lookup(memory_lookup.clone())
                    .relay_mode(relay_mode.clone())
                    .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                    .bind()
                    .await?
            };
            if !matches!(relay_mode, RelayMode::Disabled) {
                endpoint.online().await;
            }
            let local_peer_addr = endpoint.addr();
            (
                endpoint,
                "> Direct iroh transport is ready.".to_string(),
                "Direct iroh transport is operational. Gossip messages use standard iroh connectivity."
                    .to_string(),
                local_peer_addr,
            )
        }
        #[cfg(not(feature = "tor-transport"))]
        {
            let endpoint = if matches!(relay_mode, RelayMode::Disabled) {
                Endpoint::builder(presets::N0DisableRelay)
                    .secret_key(secret_key.clone())
                    .address_lookup(memory_lookup.clone())
                    .relay_mode(relay_mode.clone())
                    .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                    .bind()
                    .await?
            } else {
                Endpoint::builder(presets::N0)
                    .secret_key(secret_key.clone())
                    .address_lookup(memory_lookup.clone())
                    .relay_mode(relay_mode.clone())
                    .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                    .bind()
                    .await?
            };
            if !matches!(relay_mode, RelayMode::Disabled) {
                endpoint.online().await;
            }
            let local_peer_addr = endpoint.addr();
            (
                endpoint,
                "> Direct iroh transport is ready.".to_string(),
                "Direct iroh transport is operational. Gossip messages use standard iroh connectivity."
                    .to_string(),
                local_peer_addr,
            )
        }
    };
    println!("> our endpoint id: {}", endpoint.id());

    // create the gossip protocol
    let gossip = Gossip::builder().spawn(endpoint.clone());

    // create in-memory blob store and blobs protocol for file transfer
    let blob_store = MemStore::new();
    let blobs_protocol = BlobsProtocol::new(&blob_store, None);

    let ticket = Ticket {
        topic,
        peers: vec![local_peer_addr.clone()],
    };
    println!("> ticket to join us: {ticket}");

    // setup router with the gossip protocol, blob protocol, friend ping,
    // and small-room protocol (for ≤10-member rooms)
    let small_room_builder = SmallRoomBuilder::new(endpoint.clone(), endpoint.secret_key().clone());
    let small_room_handler = small_room_builder.protocol_handler();
    let router = iroh::protocol::Router::builder(endpoint.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .accept(iroh_blobs::ALPN, blobs_protocol.clone())
        .accept(FRIEND_PING_ALPN, PingHandler)
        .accept(SMALL_ROOM_ALPN, small_room_handler)
        .spawn();

    let peer_ids = peers.iter().map(|peer| peer.id).collect::<Vec<_>>();
    let peer_count = peer_ids.len();

    // ── Room protocol decision ──────────────────────────────────────────
    // Note: peer_count is the number of bootstrap peers from the ticket.
    // First-time room open has 0 (waiting for others to join); joining a
    // ticket has ≥1.  For ≤10 members the small-room (direct QUIC) module
    // replaces the broadcast-tree gossip protocol for lower latency.
    //
    // When no bootstrap peers exist (room open) we default to gossip so
    // the first peers to connect can discover each other via the gossip
    // mesh.  Small-room peers need bootstrap addresses to connect directly;
    // once the bench/harness receives an addr from another peer it can
    // call SmallRoomHandle::connect_to().
    //
    // For the latency-benchmark use case the small_room_bench example
    // demonstrates the full flow: spawn N peers, connect them directly,
    // broadcast messages, and record per-peer latency statistics.
    let uses_small_room = peer_count > 0 && room_size_fits_small_room(peer_count);
    if uses_small_room {
        println!("> using small-room protocol ({peer_count} members ≤ {SMALL_ROOM_MAX_SIZE})");
    } else if peer_count > 0 {
        println!("> using gossip protocol ({peer_count} members > {SMALL_ROOM_MAX_SIZE})");
    }

    // join the gossip topic by connecting to known peers, if any
    // (or for small rooms, use direct QUIC connections instead)
    for peer in &peers {
        memory_lookup.set_endpoint_info(peer.clone());
    }
    if peers.is_empty() {
        println!("> waiting for peers to join us...");
    } else {
        println!("> trying to connect to {} peers...", peers.len());
    };
    let (sender, receiver) = gossip.subscribe_and_join(topic, peer_ids).await?.split();
    println!("> connected!");

    let local_public = endpoint.secret_key().public();
    let local_label = args
        .name
        .clone()
        .unwrap_or_else(|| local_public.fmt_short().to_string());

    if let Some(name) = args.name.clone() {
        let message = Message::AboutMe { name };
        let encoded_message = SignedMessage::sign_and_encode(endpoint.secret_key(), &message)?;
        sender.broadcast(encoded_message).await?;
    }

    let mut app = AppState::new(
        StatusContext {
            transport_status: transport_status_message.clone(),
            topic,
            relay_mode: relay_mode.clone(),
            connected: true,
            peer_count,
            identity_label: local_label.clone(),
            transport_notice: transport_notice_text.clone(),
            direct_peers: 0,
            relayed_peers: 0,
            neighbors: HashSet::new(),
            peer_connection_types: HashMap::new(),
        },
        friends,
        local_public,
        Some(local_label.clone()),
    );
    app.push_system(format!("Ticket to join this room: {ticket}"));
    if peers.is_empty() {
        app.push_system("Waiting for peers to join us...");
    } else {
        app.push_system(format!(
            "Trying to connect to {} peers from the ticket...",
            peers.len()
        ));
    }
    app.push_system("Controls: Enter send • Ctrl-C or Esc quit • PgUp/PgDn scroll history");
    if let Some(name) = args.name.clone() {
        app.push_system(format!("You announced yourself as {name}."));
    }

    let (net_tx, mut net_rx) = tokio::sync::mpsc::unbounded_channel();

    // ── Room docs setup ─────────────────────────────────────────────────
    // Create metadata doc and roster doc for this room.
    let initial_metadata = RoomMetadata {
        name: Some("iroh-gossip-chat".to_string()),
        description: None,
        rules: None,
    };
    let metadata_doc = create_metadata_doc(topic, &sender, initial_metadata)
        .await
        .expect("create metadata doc");
    let roster_doc = create_roster_doc(
        topic,
        &sender,
        local_public.to_string(),
        local_label.clone(),
    )
    .await
    .expect("create roster doc");
    let room_docs = Arc::new(std::sync::RwLock::new(Some(RoomDocs {
        metadata: metadata_doc.clone(),
        roster: roster_doc.clone(),
        topic,
    })));

    // Spawn the room-aware event forwarder: metadata + roster docs consume
    // their respective messages; everything else goes to net_tx for chat.
    let room_forwarder_net_tx = net_tx.clone();
    task::spawn(async move {
        forward_room_events_for_chat(metadata_doc, roster_doc, receiver, room_forwarder_net_tx)
            .await;
    });

    let mut names = HashMap::new();
    names.insert(local_public, local_label.clone());

    // Show how many friends were loaded from disk at startup.
    if app.friends.is_empty() {
        app.push_system("No friends file yet; starting with an empty friends list.");
    } else {
        app.push_system(format!(
            "Loaded {} friends from {}.",
            app.friends.len(),
            app.friends.file_path().display()
        ));
    }

    let _terminal_guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    terminal.draw(|frame| render_app(frame, &mut app))?;

    // ── Friend ping manager ────────────────────────────────────────────
    let (friend_mgr, mut friend_events) = FriendPingManager::spawn(
        endpoint.clone(),
        DEFAULT_PING_INTERVAL,
        DEFAULT_CONNECT_TIMEOUT,
    );
    for peer in app
        .friends
        .iter()
        .filter_map(|(id, _)| id.parse_public_key().ok())
    {
        let _ = friend_mgr.add_friend(peer, None).await;
    }

    let (ui_tx, mut ui_rx) = tokio::sync::mpsc::unbounded_channel();
    spawn_input_thread(ui_tx);

    // Periodic connection status monitor — refreshes every 60 seconds.
    let mut conn_monitor = tokio::time::interval(Duration::from_secs(60));
    conn_monitor.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    while !app.should_quit {
        // Non-blocking check for Tor reconnection status updates
        // (infrequent messages only — every poll is cheap with try_recv)
        while let Ok(status_msg) = tor_reconnect_rx.try_recv() {
            app.push_system(status_msg);
        }

        tokio::select! {
            Some(event) = ui_rx.recv() => {
                let redraw = handle_ui_event(
                    event,
                    &mut app,
                    &sender,
                    endpoint.secret_key(),
                    &local_label,
                    &endpoint,
                    &blob_store,
                    &friend_mgr,
                    &room_docs,
                ).await?;
                if app.friends_dirty {
                    if let Err(err) = app.friends.save() {
                        app.push_system(format!("Failed to save friends: {err}"));
                    }
                    app.friends_dirty = false;
                }
                if redraw {
                    terminal.draw(|frame| render_app(frame, &mut app))?;
                }
            }
            Some(event) = net_rx.recv() => {
                handle_net_event(event, &mut app)?;
                // Auto-download pending image (ImageShare received)
                if let Some((name, hash, sender_pk)) = app.pending_image.take() {
                    let blob_hash: iroh_blobs::Hash = hash.into();
                    if let Err(err) = blob_store
                        .downloader(&endpoint)
                        .download(blob_hash, Some(sender_pk))
                        .await
                    {
                        app.push_system(format!("Failed to download image '{}': {err}", name));
                    } else {
                        app.push_system(format!("Downloaded image: {}", name));
                    }
                }
                update_connection_counts(&endpoint, &mut app.status).await;
                if app.friends_dirty {
                    if let Err(err) = app.friends.save() {
                        app.push_system(format!("Failed to save friends: {err}"));
                    }
                    app.friends_dirty = false;
                }
                terminal.draw(|frame| render_app(frame, &mut app))?;
            }
            Some(event) = friend_events.recv() => {
                handle_friend_event(event, &mut app);
                update_connection_counts(&endpoint, &mut app.status).await;
                if app.friends_dirty {
                    if let Err(err) = app.friends.save() {
                        app.push_system(format!("Failed to save friends: {err}"));
                    }
                    app.friends_dirty = false;
                }
                terminal.draw(|frame| render_app(frame, &mut app))?;
            }
            _ = conn_monitor.tick() => {
                // Periodic refresh of per-peer connection status.
                update_connection_counts(&endpoint, &mut app.status).await;
            }
            else => break,
        }
    }

    // Sync final friend ping status to persistent store before shutdown.
    if let Ok(tracked) = friend_mgr.list_friends().await {
        for (peer, status) in tracked {
            let id = FriendId::from_public_key(peer);
            let rec = app.friends.ensure_friend(id);
            rec.status.online = status.is_online();
        }
    }
    if app.friends_dirty {
        let _ = app.friends.save();
    }

    router.shutdown().await.anyerr()?;

    Ok(())
}

// ── Terminal guard ────────────────────────────────────────────────────────────

#[derive(Debug)]
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, Hide)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, Show, LeaveAlternateScreen);
    }
}

// ── UI event types ────────────────────────────────────────────────────────────

#[derive(Debug)]
enum UiEvent {
    Key(KeyEvent),
    Resize,
    Paste(String),
}



fn spawn_input_thread(ui_tx: tokio::sync::mpsc::UnboundedSender<UiEvent>) {
    std::thread::spawn(move || {
        while let Ok(event) = event::read() {
            let keep_running = match event {
                CEvent::Key(key) => {
                    // Only handle press events — skip Release and Repeat
                    if key.kind != event::KeyEventKind::Press {
                        true
                    } else {
                        ui_tx.send(UiEvent::Key(key)).is_ok()
                    }
                }
                CEvent::Resize(_width, _height) => ui_tx.send(UiEvent::Resize).is_ok(),
                CEvent::Paste(text) => ui_tx.send(UiEvent::Paste(text)).is_ok(),
                _ => true,
            };
            if !keep_running {
                break;
            }
        }
    });
}

// ── UI event handling ─────────────────────────────────────────────────────────

async fn handle_ui_event(
    event: UiEvent,
    app: &mut AppState,
    sender: &iroh_gossip::api::GossipSender,
    secret_key: &SecretKey,
    local_label: &str,
    endpoint: &Endpoint,
    blob_store: &MemStore,
    friend_mgr: &FriendPingManager,
    room_docs: &Arc<RwLock<Option<RoomDocs>>>,
) -> Result<bool> {
    match event {
        UiEvent::Key(key) => {
            handle_key_event(
                key,
                app,
                sender,
                secret_key,
                local_label,
                endpoint,
                blob_store,
                friend_mgr,
                room_docs,
            )
            .await?;
            Ok(true)
        }
        UiEvent::Resize => Ok(true),
        UiEvent::Paste(text) => {
            app.composer.insert_str(&text);
            Ok(true)
        }
    }
}

async fn handle_key_event(
    key: KeyEvent,
    app: &mut AppState,
    sender: &iroh_gossip::api::GossipSender,
    secret_key: &SecretKey,
    local_label: &str,
    endpoint: &Endpoint,
    blob_store: &MemStore,
    friend_mgr: &FriendPingManager,
    room_docs: &Arc<RwLock<Option<RoomDocs>>>,
) -> Result<()> {
    let visible_height = app.last_log_height;
    match key {
        KeyEvent {
            code: KeyCode::Esc, ..
        } => {
            if app.help_visible {
                app.help_visible = false;
                return Ok(());
            }
            // Best-effort goodbye broadcast before we disconnect.
            let goodbye = SignedMessage::sign_and_encode(secret_key, &Message::Goodbye);
            if let Ok(encoded) = goodbye {
                let _ = sender.broadcast(encoded).await;
            }
            app.should_quit = true;
        }
        KeyEvent {
            code: KeyCode::Char('c'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            // Best-effort goodbye broadcast before we disconnect.
            let goodbye = SignedMessage::sign_and_encode(secret_key, &Message::Goodbye);
            if let Ok(encoded) = goodbye {
                let _ = sender.broadcast(encoded).await;
            }
            app.should_quit = true;
        }
        KeyEvent {
            code: KeyCode::Enter,
            ..
        } => {
            let submitted = app.composer.take();
            let trimmed = submitted.trim().to_string();

            if trimmed.is_empty() {
                return Ok(());
            }

            if let Some(path) = trimmed.strip_prefix("/send ") {
                // ── File send via iroh-blobs ─────────────────────────────
                let path = path.trim().to_string();
                let path_buf = std::path::PathBuf::from(&path);
                let abs_path = match std::path::absolute(&path_buf) {
                    Ok(p) => p,
                    Err(e) => {
                        app.push_system(format!("Failed to resolve path: {e}"));
                        return Ok(());
                    }
                };
                if !abs_path.exists() {
                    app.push_system(format!("File not found: {}", path));
                    return Ok(());
                }
                let filename = match path_buf
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                {
                    Some(name) => name,
                    None => {
                        app.push_system("Invalid file path.");
                        return Ok(());
                    }
                };

                app.push_system(format!("Hashing file: {filename}..."));
                let tag = match blob_store.blobs().add_path(abs_path).await {
                    Ok(tag) => tag,
                    Err(e) => {
                        app.push_system(format!("Failed to hash file: {e}"));
                        return Ok(());
                    }
                };

                let node_id = endpoint.id();
                let ticket = BlobTicket::new(node_id.into(), tag.hash, tag.format);
                let ticket_str = ticket.to_string();

                let message = Message::FileShare {
                    name: filename.clone(),
                    ticket: ticket_str.clone(),
                };
                let encoded_message = SignedMessage::sign_and_encode(secret_key, &message)?;
                sender.broadcast(encoded_message).await?;
                app.push_local(local_label.to_string(), format!("/send {path}"));
                app.push_system(format!("Sharing: {filename} (ticket: {ticket_str})"));
                return Ok(());
            }

            if trimmed == "/download" {
                // ── File download ────────────────────────────────────────
                if let Some((filename, ticket_str)) = app.pending_file.clone() {
                    let ticket: BlobTicket = match ticket_str.parse() {
                        Ok(t) => t,
                        Err(e) => {
                            app.push_system(format!("Failed to parse ticket: {e}"));
                            return Ok(());
                        }
                    };
                    let peer_id = ticket.addr().id;
                    let downloader = blob_store.downloader(endpoint);
                    app.push_system(format!("Downloading: {filename}..."));
                    if let Err(e) = downloader.download(ticket.hash(), Some(peer_id)).await {
                        app.push_system(format!("Download failed: {e}"));
                        return Ok(());
                    }
                    app.push_system("Download complete. Exporting to disk...");
                    let dest = std::env::current_dir().unwrap_or_default().join(&filename);
                    if let Err(e) = blob_store.blobs().export(ticket.hash(), dest).await {
                        app.push_system(format!("Export failed: {e}"));
                        return Ok(());
                    }
                    app.push_system(format!("Saved: {filename}"));
                    app.pending_file = None;
                } else {
                    app.push_system("No pending file to download.");
                }
                return Ok(());
            }

            if trimmed == "/help" {
                app.help_visible = true;
                app.follow_latest = true;
                return Ok(());
            }

            if let Some(pubkey_str) = trimmed.strip_prefix("/friend add ") {
                let pubkey_str = pubkey_str.trim().to_string();
                let (pubkey, alias) =
                    if let Some((key_part, rest)) = pubkey_str.split_once(char::is_whitespace) {
                        (key_part.to_string(), Some(rest.trim().to_string()))
                    } else {
                        (pubkey_str, None)
                    };
                match pubkey.parse::<PublicKey>() {
                    Ok(peer) => {
                        let fid = FriendId::from_public_key(peer);
                        let was_new = app.friends.get(&fid).is_none();
                        if let Some(alias_text) = &alias {
                            app.friends.set_label(fid.clone(), alias_text.clone());
                        } else {
                            app.friends.ensure_friend(fid.clone());
                        }
                        app.friends_dirty = true;

                        match friend_mgr.add_friend(peer, None).await {
                            Ok(_) => {
                                if was_new {
                                    let label = if let Some(ref alias_text) = alias {
                                        format!("{alias_text} ({})", peer.fmt_short())
                                    } else {
                                        peer.fmt_short().to_string()
                                    };
                                    app.push_system(format!("Added friend: {label}"));
                                } else {
                                    app.push_system(format!(
                                        "Updated friend: {}",
                                        peer.fmt_short()
                                    ));
                                }
                            }
                            Err(e) => {
                                app.push_system(format!("Failed to add friend: {e}"));
                            }
                        }
                    }
                    Err(e) => {
                        app.push_system(format!("Invalid public key: {e}"));
                    }
                }
                return Ok(());
            }

            if let Some(rest) = trimmed.strip_prefix("/friend remove ") {
                let target = rest.trim().to_string();
                // Try to resolve by exact public key first, then by alias.
                let resolved = if let Ok(pk) = target.parse::<PublicKey>() {
                    Some((pk, FriendId::from_public_key(pk)))
                } else {
                    // Try to find by alias
                    app.friends
                        .iter()
                        .find(|(_, rec)| rec.label.as_deref() == Some(&target))
                        .map(|(fid, _)| (fid.parse_public_key().ok(), fid.clone()))
                        .and_then(|(pk_opt, fid)| pk_opt.map(|pk| (pk, fid)))
                };

                match resolved {
                    Some((peer, fid)) => {
                        let label = app
                            .friends
                            .get(&fid)
                            .and_then(|r| r.label.clone())
                            .unwrap_or_else(|| peer.fmt_short().to_string());
                        app.friends.remove(&fid);
                        app.friends_dirty = true;
                        let _ = friend_mgr.remove_friend(&peer).await;
                        app.push_system(format!("Removed friend: {label}"));
                    }
                    None => {
                        app.push_system(format!("Friend not found: {target}"));
                    }
                }
                return Ok(());
            }

            if let Some(rest) = trimmed.strip_prefix("/friend rename ") {
                let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
                if parts.len() < 2 {
                    app.push_system("Usage: /friend rename <public-key> <new-alias>");
                    return Ok(());
                }
                let target = parts[0].trim();
                let new_alias = parts[1].trim().to_string();
                let resolved = if let Ok(pk) = target.parse::<PublicKey>() {
                    Some(FriendId::from_public_key(pk))
                } else {
                    app.friends
                        .iter()
                        .find(|(_, rec)| rec.label.as_deref() == Some(target))
                        .map(|(fid, _)| fid.clone())
                };
                match resolved {
                    Some(fid) => {
                        app.friends.set_label(fid.clone(), &new_alias);
                        app.friends_dirty = true;
                        app.push_system(format!("Renamed friend to: {new_alias}"));
                    }
                    None => {
                        app.push_system(format!("Friend not found: {target}"));
                    }
                }
                return Ok(());
            }

            if trimmed == "/friend list" {
                match friend_mgr.list_friends().await {
                    Ok(list) => {
                        if list.is_empty() && app.friends.is_empty() {
                            app.push_system("No friends tracked yet.");
                        } else {
                            app.push_system(format!("Friends ({}):", app.friends.len()));
                            for (peer, status) in &list {
                                let fid = FriendId::from_public_key(*peer);
                                let label = app
                                    .friends
                                    .get(&fid)
                                    .and_then(|r| r.display_label(&fid).into())
                                    .or_else(|| Some(peer.fmt_short().to_string()))
                                    .unwrap();
                                let status_str = match status {
                                    FriendStatus::Unknown => "?",
                                    FriendStatus::Online => "ONLINE",
                                    FriendStatus::Offline => "offline",
                                };
                                let ping_status = app
                                    .friends
                                    .get(&fid)
                                    .map(|r| if r.status.online { "online" } else { "offline" })
                                    .unwrap_or("unknown");
                                app.push_system(format!(
                                    "  {label}: {status_str} (persisted: {ping_status})"
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        app.push_system(format!("Failed to list friends: {e}"));
                    }
                }
                return Ok(());
            }

            if trimmed == "/room info" {
                let docs = room_docs.read().unwrap();
                match docs.as_ref() {
                    None => {
                        app.push_system("No room docs available (room not initialised).");
                    }
                    Some(docs) => {
                        let md = read_metadata(&docs.metadata).await;
                        let members = list_members(&docs.roster).await;
                        app.push_system(format!(
                            "Room: {} | Description: {} | Rules: {}",
                            md.name.as_deref().unwrap_or("unnamed"),
                            md.description.as_deref().unwrap_or("none"),
                            md.rules.as_deref().unwrap_or("none"),
                        ));
                        app.push_system(format!("Members ({}):", members.len()));
                        for (pk, member) in &members {
                            app.push_system(format!(
                                "  {} ({}) — joined at {}",
                                member.display_name,
                                &pk[..16],
                                member.joined_at,
                            ));
                        }
                    }
                }
                return Ok(());
            }

            if trimmed == "/connections" {
                let peers: Vec<iroh::PublicKey> =
                    app.status.neighbors.iter().copied().collect();
                if peers.is_empty() {
                    app.push_system("No known peers to inspect.");
                } else {
                    app.push_system(format!("Connections ({}):", peers.len()));
                    for pk in &peers {
                        let ctype = check_peer_connection_type(endpoint, *pk).await;
                        let label = app.names.get(pk).cloned().unwrap_or_else(|| {
                            let s = pk.fmt_short();
                            s.to_string()
                        });
                        app.push_system(format!(
                            "  {label} — {} ({})",
                            match ctype {
                                ConnectionType::Direct => "direct",
                                ConnectionType::Relayed => "relayed",
                                ConnectionType::Unknown => "unknown",
                            },
                            pk.fmt_short(),
                        ));
                    }
                }
                return Ok(());
            }

            if let Some(rest) = trimmed.strip_prefix("/edit ") {
                // ── Edit a previously sent message ──────────────────────
                let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
                if parts.len() < 2 {
                    app.push_system("Usage: /edit <index> <new text> (index is 1-based message number)");
                    return Ok(());
                }
                let idx: usize = match parts[0].parse::<usize>() {
                    Ok(n) if n >= 1 => n - 1,
                    _ => {
                        app.push_system("Invalid index. Use a positive number (1-based).");
                        return Ok(());
                    }
                };
                let new_text = parts[1].to_string();
                let entry = match app.entries.get(idx) {
                    Some(e) => e,
                    None => {
                        app.push_system(format!("No message at index {}.", idx + 1));
                        return Ok(());
                    }
                };
                let original_text = entry.body.clone();
                let original_hash = message_hash(&Message::Message {
                    text: original_text,
                });
                let message = Message::Edit {
                    original_hash,
                    new_text: new_text.clone(),
                };
                let encoded = SignedMessage::sign_and_encode(secret_key, &message)?;
                sender.broadcast(encoded).await?;
                // Update the local entry.
                if let Some(entry) = app.entries.get_mut(idx) {
                    entry.body = new_text;
                    entry.edited = true;
                }
                app.push_system(format!("Edited message at index {}.", idx + 1));
                return Ok(());
            }

            if let Some(rest) = trimmed.strip_prefix("/delete ") {
                // ── Delete a previously sent message ────────────────────
                let idx: usize = match rest.trim().parse::<usize>() {
                    Ok(n) if n >= 1 => n - 1,
                    _ => {
                        app.push_system("Invalid index. Use a positive number (1-based).");
                        return Ok(());
                    }
                };
                let entry = match app.entries.get(idx) {
                    Some(e) => e,
                    None => {
                        app.push_system(format!("No message at index {}.", idx + 1));
                        return Ok(());
                    }
                };
                let original_text = entry.body.clone();
                let message_hash_val = message_hash(&Message::Message {
                    text: original_text,
                });
                let message = Message::Delete {
                    message_hash: message_hash_val,
                };
                let encoded = SignedMessage::sign_and_encode(secret_key, &message)?;
                sender.broadcast(encoded).await?;
                // Update the local entry.
                if let Some(entry) = app.entries.get_mut(idx) {
                    entry.body = "[deleted]".to_string();
                    entry.edited = true;
                }
                app.push_system(format!("Deleted message at index {}.", idx + 1));
                return Ok(());
            }

            if let Some(rest) = trimmed.strip_prefix("/react ") {
                // ── React to a message ──────────────────────────
                let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
                if parts.len() < 2 {
                    app.push_system(
                        "Usage: /react <index> <emoji> (index is 1-based message number)",
                    );
                    return Ok(());
                }
                let idx: usize = match parts[0].parse::<usize>() {
                    Ok(n) if n >= 1 => n - 1,
                    _ => {
                        app.push_system("Invalid index. Use a positive number (1-based).");
                        return Ok(());
                    }
                };
                let emoji = parts[1].to_string();
                let entry = match app.entries.get(idx) {
                    Some(e) => e,
                    None => {
                        app.push_system(format!("No message at index {}.", idx + 1));
                        return Ok(());
                    }
                };
                let original_text = entry.body.clone();
                let message_hash_val = message_hash(&Message::Message {
                    text: original_text,
                });
                let message = Message::Reaction {
                    message_hash: message_hash_val,
                    emoji: emoji.clone(),
                };
                let encoded = SignedMessage::sign_and_encode(secret_key, &message)?;
                sender.broadcast(encoded).await?;
                // Add reactions locally immediately.
                if let Some(entry) = app.entries.get_mut(idx) {
                    entry.reactions.push(emoji);
                }
                app.push_system(format!(
                    "Reacted to message at index {} with {}.",
                    idx + 1,
                    parts[1]
                ));
                return Ok(());
            }

            // Normal text message
            let message = Message::Message {
                text: trimmed.clone(),
            };
            let encoded_message = SignedMessage::sign_and_encode(secret_key, &message)?;
            sender.broadcast(encoded_message).await?;
            app.push_local(local_label.to_string(), trimmed);
        }
        KeyEvent {
            code: KeyCode::Backspace,
            ..
        } => app.composer.backspace(),
        KeyEvent {
            code: KeyCode::Delete,
            ..
        } => app.composer.delete(),
        KeyEvent {
            code: KeyCode::Left,
            ..
        } => app.composer.move_left(),
        KeyEvent {
            code: KeyCode::Right,
            ..
        } => app.composer.move_right(),
        KeyEvent {
            code: KeyCode::Home,
            ..
        } => app.composer.move_home(),
        KeyEvent {
            code: KeyCode::End, ..
        } => app.composer.move_end(),
        KeyEvent {
            code: KeyCode::PageUp,
            ..
        } => app.scroll_up(visible_height.max(1) / 2, visible_height),
        KeyEvent {
            code: KeyCode::PageDown,
            ..
        } => app.scroll_down(visible_height.max(1) / 2, visible_height),
        KeyEvent {
            code: KeyCode::Char(ch),
            modifiers,
            ..
        } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
            app.composer.insert_char(ch);
        }
        _ => {}
    }

    Ok(())
}

// ── Room-aware event forwarder ────────────────────────────────────────────

/// Combined gossip event forwarder that dispatches room doc messages
/// (metadata 0xFE, roster 0xFF) to the appropriate doc handles, and
/// forwards remaining events (chat messages, NeighborUp/Down) to NetEvent.
async fn forward_room_events_for_chat(
    metadata_doc: room_docs::RoomMetadataDoc,
    roster_doc: room_docs::RosterDoc,
    mut receiver: iroh_gossip::api::GossipReceiver,
    net_tx: tokio::sync::mpsc::UnboundedSender<chat_core::NetEvent>,
) {
    use iroh_gossip::api::Event as GossipEvent;
    use iroh_gossip::chat_core::{NetEvent, SignedMessage};
    use n0_future::StreamExt;

    while let Some(event_result) = receiver.next().await {
        let event = match event_result {
            Ok(e) => e,
            Err(_) => continue,
        };

        // Check marker byte before consuming the event.
        let is_metadata = matches!(
            &event,
            GossipEvent::Received(msg) if msg.content.first() == Some(&0xFE)
        );
        let is_roster = matches!(
            &event,
            GossipEvent::Received(msg) if msg.content.first() == Some(&0xFF)
        );

        if is_metadata {
            let _ = room_docs::process_gossip_event(&metadata_doc, Ok(event)).await;
            continue;
        }

        if is_roster {
            let _ = room_docs::process_roster_event(&roster_doc, Ok(event)).await;
            continue;
        }

        // Chat message or neighbor event — forward as NetEvent.
        match event {
            GossipEvent::Received(msg) => match SignedMessage::verify_and_decode(&msg.content) {
                Ok((from, message)) => {
                    if net_tx.send(NetEvent::Message { from, message }).is_err() {
                        return;
                    }
                }
                Err(err) => {
                    let _ = net_tx.send(NetEvent::Error(err.to_string()));
                    return;
                }
            },
            GossipEvent::NeighborUp(id) => {
                if net_tx.send(NetEvent::NeighborUp { peer: id }).is_err() {
                    return;
                }
            }
            GossipEvent::NeighborDown(id) => {
                if net_tx.send(NetEvent::NeighborDown { peer: id }).is_err() {
                    return;
                }
            }
            GossipEvent::Lagged => {
                // Not forwarded — protocol-level backpressure signal.
            }
        }
    }
    let _ = net_tx.send(NetEvent::Closed);
}

// ── Friend event handling ──────────────────────────────────────────────────────

/// Handle a [`FriendEvent`] from the friend ping manager background task.
fn handle_friend_event(event: FriendEvent, app: &mut AppState) {
    match event {
        FriendEvent::StatusChanged { peer, status } => {
            let fid = FriendId::from_public_key(peer);
            let label = app
                .friends
                .get(&fid)
                .and_then(|r| r.display_label(&fid).into())
                .unwrap_or_else(|| peer.fmt_short().to_string());

            match status {
                FriendStatus::Online => {
                    app.friends.mark_online(fid);
                    app.friends_dirty = true;
                    app.push_system(format!("Friend {label} is now ONLINE"));
                }
                FriendStatus::Offline => {
                    app.friends.mark_offline(fid);
                    app.friends_dirty = true;
                    app.push_system(format!("Friend {label} is now offline"));
                }
                FriendStatus::Unknown => {
                    // No transition to display for Unknown
                }
            }
        }
    }
}

// ── TUI rendering ─────────────────────────────────────────────────────────────

fn render_app(frame: &mut Frame<'_>, app: &mut AppState) {
    let status_height = status_panel_height(&app.status);
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(status_height),
            Constraint::Min(10),
            Constraint::Length(5),
        ])
        .split(frame.area());

    let body_area = layout[2];
    let body_layout = if body_area.width >= 100 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(40), Constraint::Length(34)])
            .split(body_area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(6), Constraint::Length(9)])
            .split(body_area)
    };

    let status_block = Block::default()
        .title(Span::styled(
            "Status",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let status_lines = status_lines(&app.status);
    let status_paragraph = Paragraph::new(Text::from(status_lines))
        .block(status_block)
        .wrap(Wrap { trim: true });
    frame.render_widget(status_paragraph, layout[0]);

    let log_block = Block::default()
        .title(Span::styled(
            "Chat log",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));
    let log_inner = log_block.inner(body_layout[0]);
    app.last_log_height = log_inner.height;
    let log_scroll = app.rendered_scroll_offset(log_inner.height);
    let log_text = app_chat_text(app);
    let log_paragraph = Paragraph::new(log_text)
        .block(log_block)
        .wrap(Wrap { trim: false })
        .scroll((log_scroll, 0));
    frame.render_widget(log_paragraph, body_layout[0]);

    let friends_block = Block::default()
        .title(Span::styled(
            format!("Friends ({})", app.friends.len()),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let friends_paragraph = Paragraph::new(Text::from(friends_panel_lines(app)))
        .block(friends_block)
        .wrap(Wrap { trim: true });
    frame.render_widget(friends_paragraph, body_layout[1]);

    let composer_block = Block::default()
        .title(Span::styled(
            "Composer",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));
    let composer_inner = composer_block.inner(layout[3]);
    frame.render_widget(composer_block, layout[3]);
    let prompt = "> ";
    let composer_line = Line::from(vec![
        Span::styled(
            prompt,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(app.composer.text().to_string()),
    ]);
    let composer_paragraph =
        Paragraph::new(Text::from(vec![composer_line])).wrap(Wrap { trim: false });
    frame.render_widget(composer_paragraph, composer_inner);
    let cursor_x = composer_inner
        .x
        .saturating_add(prompt.len() as u16)
        .saturating_add(app.composer.cursor_column());
    frame.set_cursor_position((cursor_x, composer_inner.y));

    if app.help_visible {
        let help_area = centered_rect(72, 58, frame.area());
        let help_block = Block::default()
            .title(Span::styled(
                "Help",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));
        let help_paragraph = Paragraph::new(Text::from(help_menu_lines()))
            .block(help_block)
            .wrap(Wrap { trim: false });
        frame.render_widget(help_paragraph, help_area);
    }
}

// ── TUI formatting helpers (ratatui-dependent) ────────────────────────────────

/// Render a [`ChatEntry`] as ratatui lines (message line + optional reactions).
fn entry_to_line(entry: &ChatEntry) -> Vec<Line<'static>> {
    let style = match entry.kind {
        ChatKind::System => Style::default().fg(Color::DarkGray),
        ChatKind::Local => Style::default().fg(Color::Green),
        ChatKind::Remote => Style::default().fg(Color::Blue),
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("[{}]", entry.label),
            style.add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::raw(if entry.edited {
            format!("{} ✎", entry.body)
        } else {
            entry.body.clone()
        }),
    ])];
    if !entry.reactions.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("  [", Style::default().fg(Color::Yellow)),
            Span::styled(
                entry.reactions.join(", "),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("]", Style::default().fg(Color::Yellow)),
        ]));
    }
    lines
}

/// Render the chat log as ratatui text.
fn app_chat_text(app: &AppState) -> Text<'static> {
    if app.entries.is_empty() {
        Text::from(Line::from(vec![Span::styled(
            "No messages yet. Say hello.",
            Style::default().fg(Color::DarkGray),
        )]))
    } else {
        Text::from(
            app.entries
                .iter()
                .flat_map(entry_to_line)
                .collect::<Vec<_>>(),
        )
    }
}

fn friends_panel_lines(app: &AppState) -> Vec<Line<'static>> {
    let title_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD);
    let hint_style = Style::default().fg(Color::DarkGray);
    let mut lines = vec![
        Line::from(vec![Span::styled("Tracked friends", title_style)]),
        Line::from(vec![Span::styled(
            "Manage entries with /friend add, /friend remove, /friend rename, and /friend list.",
            hint_style,
        )]),
    ];

    if app.friends.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "No friends yet.",
            Style::default().fg(Color::DarkGray),
        )]));
        lines.push(Line::from(vec![Span::styled(
            "Add one with /friend add <public-key> [alias].",
            hint_style,
        )]));
        return lines;
    }

    for (id, record) in app.friends.iter() {
        let name = record.display_label(id);
        let short_id: String = id.as_str().chars().take(12).collect();
        let (status_text, status_style) = friend_status_text(record);
        // Look up connection type (direct/relayed) if we know this peer.
        let conn_hint = id
            .parse_public_key()
            .ok()
            .and_then(|pk| app.status.peer_connection_types.get(&pk))
            .map(|ct| match ct {
                ConnectionType::Direct => " D",
                ConnectionType::Relayed => " ⤻",
                ConnectionType::Unknown => "",
            })
            .unwrap_or("");
        lines.push(Line::from(vec![
            Span::styled(name, label_style),
            Span::raw(" "),
            Span::styled(format!("[{status_text}]"), status_style),
            Span::styled(conn_hint, Style::default().fg(Color::Cyan)),
        ]));
        lines.push(Line::from(vec![Span::styled(
            format!("  {short_id}"),
            hint_style,
        )]));
    }

    lines
}

fn friend_status_text(record: &FriendRecord) -> (&'static str, Style) {
    if record.status.last_seen_at_unix_ms.is_none()
        && record.status.last_offline_at_unix_ms.is_none()
    {
        ("unknown", Style::default().fg(Color::Yellow))
    } else if record.status.online {
        ("online", Style::default().fg(Color::Green))
    } else {
        ("offline", Style::default().fg(Color::DarkGray))
    }
}

fn status_panel_height(context: &StatusContext) -> u16 {
    let height = status_lines(context).len() as u16 + 2;
    height.clamp(6, 10)
}

fn status_lines(context: &StatusContext) -> Vec<Line<'static>> {
    let label_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    vec![
        Line::from(vec![
            Span::styled("Transport", label_style),
            Span::raw(format!(": {}", context.transport_status)),
        ]),
        Line::from(vec![
            Span::styled("Topic", label_style),
            Span::raw(format!(": {}", context.topic)),
        ]),
        Line::from(vec![
            Span::styled("Identity", label_style),
            Span::raw(format!(": {}", context.identity_label)),
        ]),
        Line::from(vec![
            Span::styled("Relay", label_style),
            Span::raw(format!(": {}", fmt_relay_mode(&context.relay_mode))),
        ]),
        Line::from(vec![
            Span::styled("Peers", label_style),
            Span::raw(format!(
                ": {} known • {} direct, {} relay • connected: {}",
                context.peer_count,
                context.direct_peers,
                context.relayed_peers,
                context.connected
            )),
        ]),
        Line::from(vec![
            Span::styled("Notice", label_style),
            Span::raw(format!(": {}", context.transport_notice)),
        ]),
        Line::from(vec![
            Span::styled("Controls", label_style),
            Span::raw(
                ": Enter send • /help menu • /friend list • Ctrl-C or Esc quit • PgUp/PgDn scroll history",
            ),
        ]),
    ]
}

fn help_menu_lines() -> Vec<Line<'static>> {
    let title_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let hint_style = Style::default().fg(Color::DarkGray);

    vec![
        Line::from(vec![Span::styled("Quick help", title_style)]),
        Line::from(vec![Span::styled(
            "Send a message by typing it and pressing Enter.",
            Style::default(),
        )]),
        Line::from(vec![Span::styled("Available commands", title_style)]),
        Line::from(vec![
            Span::styled("/help", label_style),
            Span::raw("          open this menu"),
        ]),
        Line::from(vec![
            Span::styled("/send <path>", label_style),
            Span::raw("   share a file with peers"),
        ]),
        Line::from(vec![
            Span::styled("/download", label_style),
            Span::raw("      fetch the last shared file"),
        ]),
        Line::from(vec![
            Span::styled("/friend add <pubkey> [alias]", label_style),
            Span::raw("  track a peer's online status"),
        ]),
        Line::from(vec![
            Span::styled("/friend remove <pubkey|alias>", label_style),
            Span::raw("  stop tracking a friend"),
        ]),
        Line::from(vec![
            Span::styled("/friend rename <pubkey|alias> <name>", label_style),
            Span::raw("  change a friend's local alias"),
        ]),
        Line::from(vec![
            Span::styled("/friend list", label_style),
            Span::raw("     list tracked friends and their status"),
        ]),
        Line::from(vec![
            Span::styled("/room info", label_style),
            Span::raw("     show room metadata and members"),
        ]),
        Line::from(vec![
            Span::styled("/connections", label_style),
            Span::raw("   show per-peer connection status"),
        ]),
        Line::from(vec![
            Span::styled("/react <index> <emoji>", label_style),
            Span::raw("  react to a message"),
        ]),
        Line::from(vec![Span::styled("Tips", title_style)]),
        Line::from(vec![Span::styled(
            "Press Esc to close this help view. PgUp/PgDn scroll older messages.",
            hint_style,
        )]),
    ]
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::EndpointAddr;
    use iroh_gossip::chat_core::Composer;

    #[cfg(feature = "tor-transport")]
    #[test]
    fn formats_bootstrap_status_line_with_tor_prefix() {
        assert_eq!(
            format_tor_bootstrap_status_line("31%: bootstrapping"),
            "> Tor bootstrap status: 31%: bootstrapping"
        );
    }

    #[test]
    fn ticket_roundtrips_through_base32() {
        let ticket = Ticket {
            topic: TopicId::from_bytes([9u8; 32]),
            peers: vec![EndpointAddr::new(SecretKey::generate().public())],
        };
        let encoded = ticket.to_string();
        let decoded = Ticket::from_str(&encoded).expect("ticket should decode");
        assert_eq!(decoded, ticket);
    }

    #[test]
    fn composer_inserts_and_moves_cursor() {
        let mut composer = Composer::default();
        composer.insert_str("hi");
        composer.move_left();
        composer.insert_char('!');
        assert_eq!(composer.text(), "h!i");
        assert_eq!(composer.cursor(), 2);
    }

    #[test]
    fn composer_backspace_removes_character_before_cursor() {
        let mut composer = Composer::from("chat");
        composer.move_left();
        composer.move_left();
        composer.backspace();
        assert_eq!(composer.text(), "cat");
        assert_eq!(composer.cursor(), 1);
    }

    #[test]
    fn composer_take_clears_buffer() {
        let mut composer = Composer::from("hello");
        let submitted = composer.take();
        assert_eq!(submitted, "hello");
        assert!(composer.is_empty());
        assert_eq!(composer.cursor(), 0);
    }

    #[test]
    fn status_lines_include_transport_and_topic_context() {
        let status = StatusContext {
            transport_status: "Direct iroh transport is ready.".into(),
            topic: TopicId::from_bytes([7u8; 32]),
            relay_mode: RelayMode::Disabled,
            connected: true,
            peer_count: 3,
            identity_label: "alice".into(),
            transport_notice: "transport notice".into(),
            direct_peers: 0,
            relayed_peers: 0,
            neighbors: HashSet::new(),
            peer_connection_types: HashMap::new(),
        };
        let lines = status_lines(&status);
        let rendered: Vec<_> = lines.iter().map(|line| line.to_string()).collect();
        assert!(rendered
            .iter()
            .any(|line| line.contains("Direct iroh transport is ready.")));
        assert!(rendered.iter().any(|line| line.contains("alice")));
        assert!(rendered.iter().any(|line| line.contains("3 known")));
    }

    #[test]
    fn help_menu_lists_the_available_commands() {
        let rendered: Vec<String> = help_menu_lines()
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert!(rendered.iter().any(|line| line.contains("Quick help")));
        assert!(rendered.iter().any(|line| line.contains("/help")));
        assert!(rendered.iter().any(|line| line.contains("/send <path>")));
        assert!(rendered.iter().any(|line| line.contains("/download")));
    }

    #[test]
    fn friends_panel_shows_empty_state() {
        let status = StatusContext {
            transport_status: "Direct iroh transport is ready.".into(),
            topic: TopicId::from_bytes([7u8; 32]),
            relay_mode: RelayMode::Disabled,
            connected: true,
            peer_count: 0,
            identity_label: "alice".into(),
            transport_notice: "transport notice".into(),
            direct_peers: 0,
            relayed_peers: 0,
            neighbors: HashSet::new(),
            peer_connection_types: HashMap::new(),
        };
        let app = AppState::new(
            status,
            FriendsStore::empty_at(
                std::env::temp_dir()
                    .join(format!("iroh-chat-friends-empty-{}", rand::random::<u64>())),
            ),
            SecretKey::generate().public(),
            Some("alice".into()),
        );
        let rendered: Vec<String> = friends_panel_lines(&app)
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert!(rendered.iter().any(|line| line.contains("No friends yet.")));
        assert!(rendered.iter().any(|line| line.contains("/friend add")));
    }

    #[test]
    fn friends_panel_lists_live_status() {
        let status = StatusContext {
            transport_status: "Direct iroh transport is ready.".into(),
            topic: TopicId::from_bytes([8u8; 32]),
            relay_mode: RelayMode::Disabled,
            connected: true,
            peer_count: 1,
            identity_label: "alice".into(),
            transport_notice: "transport notice".into(),
            direct_peers: 0,
            relayed_peers: 0,
            neighbors: HashSet::new(),
            peer_connection_types: HashMap::new(),
        };
        let mut store = FriendsStore::empty_at(std::env::temp_dir().join(format!(
            "iroh-chat-friends-status-{}",
            rand::random::<u64>()
        )));
        let peer = SecretKey::generate().public();
        let friend_id = FriendId::from_public_key(peer);
        store.set_label(friend_id.clone(), "Bob");
        store.mark_online(friend_id.clone());
        let app = AppState::new(
            status,
            store,
            SecretKey::generate().public(),
            Some("alice".into()),
        );
        let rendered: Vec<String> = friends_panel_lines(&app)
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert!(rendered.iter().any(|line| line.contains("Bob")));
        assert!(rendered.iter().any(|line| line.contains("online")));
    }

    #[test]
    fn cli_parses_direct_mode_by_default() {
        let args = Args::try_parse_from(["chat", "open"]).expect("direct mode should parse");
        assert!(matches!(args.command, Command::Open { .. }));
    }

    #[cfg(feature = "tor-transport")]
    #[test]
    fn tor_transport_notice_mentions_tor_operational() {}

    #[cfg(feature = "tor-transport")]
    #[test]
    fn tor_client_config_builds_direct_tor_configuration() {
        let tor_dirs = TorStorageDirs::new().expect("test tor dirs should be creatable");
        let config = tor_client_config(&tor_dirs).expect("direct tor config should build");
        let _ = config;
    }

    // ── Identity persistence tests ────────────────────────────────────────

    #[test]
    fn secret_key_serialization_roundtrip() {
        // Generate a key, serialize to hex, deserialize, verify same key material.
        let key = SecretKey::generate();
        let hex = data_encoding::HEXLOWER.encode(&key.to_bytes());
        let recovered = SecretKey::from_str(&hex).expect("should parse hex-encoded secret key");
        assert_eq!(key.to_bytes(), recovered.to_bytes());
        assert_eq!(key.public(), recovered.public());
    }

    #[test]
    fn secret_key_public_key_is_deterministic() {
        // Same SecretKey bytes always produce the same PublicKey.
        let key = SecretKey::generate();
        let pk1 = key.public();
        let pk2 = key.public();
        assert_eq!(pk1, pk2);
    }

    #[test]
    fn get_data_dir_respects_env_var() {
        let test_dir = if cfg!(windows) {
            "C:\\tmp\\iroh-test"
        } else {
            "/tmp/iroh-test-dir"
        };
        let prior = std::env::var_os("IROH_GOSSIP_CHAT_DATA_DIR");
        std::env::set_var("IROH_GOSSIP_CHAT_DATA_DIR", test_dir);
        let dir = get_data_dir();
        assert_eq!(dir, PathBuf::from(test_dir));
        match prior {
            Some(v) => std::env::set_var("IROH_GOSSIP_CHAT_DATA_DIR", v),
            None => std::env::remove_var("IROH_GOSSIP_CHAT_DATA_DIR"),
        }
    }

    #[test]
    fn ticket_is_deterministic_for_same_key_and_topic() {
        let key = SecretKey::generate();
        let topic = TopicId::from_bytes([42u8; 32]);
        let peer_addr = EndpointAddr::new(key.public());

        let ticket_a = Ticket {
            topic,
            peers: vec![peer_addr.clone()],
        };
        let ticket_b = Ticket {
            topic,
            peers: vec![peer_addr],
        };

        // Same inputs produce identical ticket encoding.
        assert_eq!(ticket_a.to_string(), ticket_b.to_string());
        assert_eq!(ticket_a.to_bytes(), ticket_b.to_bytes());
    }

    #[test]
    fn secret_key_file_write_and_read_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("iroh-key-test-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");

        let key = SecretKey::generate();
        let hex = data_encoding::HEXLOWER.encode(&key.to_bytes());
        let key_path = tmp.join("secret_key.txt");
        std::fs::write(&key_path, format!("{hex}\n")).expect("write key hex");

        // Read back
        let read_back = std::fs::read_to_string(&key_path).expect("read key file");
        let recovered = SecretKey::from_str(read_back.trim()).expect("parse key");
        assert_eq!(key.public(), recovered.public());

        // Cleanup
        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_dir(&tmp);
    }

    #[test]
    fn load_or_generate_creates_and_reuses_key() {
        // Use a dedicated temp directory so tests don't clobber each other.
        let tmp = std::env::temp_dir().join(format!("iroh-key-test-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let key_path = tmp.join("secret_key.txt");

        // First call should generate a new key.
        let (key_a, path_a) = load_or_generate_secret_key_at(&tmp).expect("first load");
        assert!(key_path.exists(), "key file should exist after generation");
        assert_eq!(path_a, key_path);

        // Second call should load the same key.
        let (key_b, path_b) = load_or_generate_secret_key_at(&tmp).expect("second load");
        assert_eq!(path_b, key_path);
        assert_eq!(
            key_a.public(),
            key_b.public(),
            "second load returns same identity"
        );

        // Parsing the stored hex should also match.
        let stored = std::fs::read_to_string(&key_path)
            .expect("read stored key")
            .trim()
            .to_string();
        let from_stored = SecretKey::from_str(&stored).expect("parse stored key");
        assert_eq!(key_a.public(), from_stored.public());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&key_path).expect("key file metadata");
            let mode = meta.permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "key file should have restrictive 0o600 permissions"
            );
        }

        // Clean up.
        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_dir(&tmp);
    }

    #[test]
    fn load_or_generate_uses_existing_key_file() {
        // Pre-write a known key and verify load_or_generate reads it back.
        let tmp =
            std::env::temp_dir().join(format!("iroh-key-existing-test-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");

        let known_key = SecretKey::generate();
        let hex = data_encoding::HEXLOWER.encode(&known_key.to_bytes());
        let key_path = tmp.join("secret_key.txt");
        std::fs::write(&key_path, format!("{hex}\n")).expect("pre-write key");

        let (loaded, path) = load_or_generate_secret_key_at(&tmp).expect("load existing key");
        assert_eq!(path, key_path);
        assert_eq!(known_key.public(), loaded.public());

        std::env::remove_var("IROH_GOSSIP_CHAT_DATA_DIR");
        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_dir(&tmp);
    }
}
