//! # GUI chat frontend for iroh-gossip (iced 0.14)
//!
//! Modern inbox-style UI: shows a list of recent chats on startup,
//! then opens a room when you click one — like Telegram / Signal.
//!
//! ## Build / run
//!
//! ```text
//! cargo chat-gui                    # show recent-chat list
//! cargo chat-gui open               # open a new room
//! cargo chat-gui join <ticket>      # join a room
//! ```
//!
//! Long form:
//! ```text
//! cargo run --features gui --example chat-gui --
//! ```

use std::{
    collections::{HashMap, HashSet},
    env,
    net::{Ipv4Addr, SocketAddrV4},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
    time::Duration,
};

use clap::Parser;
use iced::widget::text::Wrapping;
use iced::{
    border, clipboard,
    widget::{button, column, container, row, scrollable, text, text_input, toggler},
    Alignment, Color, Element, Length, Subscription, Task, Theme,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, Endpoint, EndpointAddr, PublicKey,
    RelayMode, RelayUrl, SecretKey,
};
use iroh_blobs::{store::mem::MemStore, ticket::BlobTicket, BlobsProtocol};
use iroh_gossip::{
    api::{Event as GossipEvent, GossipReceiver, GossipSender, GossipTopic},
    chat_core::friend_ping::{
        FriendEvent, FriendPingManager, FriendStatus, PingHandler, DEFAULT_CONNECT_TIMEOUT,
        DEFAULT_PING_INTERVAL, FRIEND_PING_ALPN,
    },
    chat_core::{fmt_relay_mode, Message, NetEvent, SignedMessage, Ticket},
    friends::{FriendId, FriendsStore},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    room::RoomStore,
};
#[cfg(feature = "tor-transport")]
use iroh_gossip::tor_transport::{
    bootstrap_tor, monitor_tor_health, TorStorageDirs, TorTransport,
};
use n0_error::{bail_any, Result, StdResultExt};
use n0_future::{task, StreamExt};

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

// ── CLI ───────────────────────────────────────────────────────────────

/// Chat over iroh-gossip (GUI)
#[derive(Parser, Debug)]
struct Args {
    #[clap(long)]
    secret_key: Option<String>,
    #[clap(short, long)]
    relay: Option<RelayUrl>,
    #[clap(long)]
    no_relay: bool,
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
}

// ── Identity persistence ──────────────────────────────────────────────

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

// ── Channel messages & app state ──────────────────────────────────────

#[derive(Debug, Clone)]
enum ChatLineKind {
    System,
    Local,
    Remote,
}

#[derive(Debug, Clone)]
struct ChatLine {
    kind: ChatLineKind,
    text: String,
}

/// Which screen is currently visible.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Screen {
    ChatList,
    Chat { topic: TopicId },
}

#[derive(Debug, Clone)]
enum AppMessage {
    // ── Navigation ──
    GoToChatList,
    OpenRoom(TopicId),
    RoomOpened {
        topic: TopicId,
        ticket: String,
    },
    RoomJoinFailed(String),
    // ── Chat list ──
    JoinTicketInputChanged(String),
    CreateNewRoom,
    JoinFromTicket,
    // ── Chat ──
    InputChanged(String),
    SendPressed,
    ToggleDark(bool),
    Tick,
    AcceptDownload,
    CopyToClipboard(String),
    NetEvent(NetEvent),
    FriendEvent(FriendEvent),
    /// Delete a room from history (home screen delete or /leave).
    DeleteRoom(TopicId),
    /// Status update from the Tor reconnection monitor.
    TorReconnect(String),
}

// ── App state ─────────────────────────────────────────────────────────

struct AppState {
    // ── Navigation ──
    screen: Screen,

    // ── Runtime / network ──
    runtime_handle: tokio::runtime::Handle,
    local_label: String,
    local_public: PublicKey,
    secret_key: SecretKey,
    gossip: Gossip,
    sender: Option<GossipSender>,
    blob_store: MemStore,
    endpoint: Endpoint,
    router: iroh::protocol::Router,
    net_rx: Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<NetEvent>>>,
    net_tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
    /// Handle to abort the current gossip forwarding task.
    forward_handle: Option<task::JoinHandle<()>>,

    // ── Chat screen state ──
    messages: Vec<ChatLine>,
    input_value: String,
    ticket: String,
    transport_status: String,
    notice: String,
    topic: String,
    relay_info: String,
    connected: bool,
    peer_count: usize,
    dark_mode: bool,
    pending_file: Option<(String, String)>,
    /// Set of peer PublicKeys currently connected as gossip neighbors.
    neighbors: HashSet<PublicKey>,
    /// Number of peers reachable via a direct (hole-punched) connection.
    direct_peers: usize,
    /// Number of peers connected through a relay server.
    relayed_peers: usize,
    /// Counter for periodic connection refresh (decremented each Tick, ~60s at 50ms).
    conn_refresh_counter: u32,

    // ── Chat list state ──
    room_history: iroh_gossip::room_history::RoomHistoryStore,
    join_ticket_input: String,

    // ── Friends ──
    friends: FriendsStore,
    friends_dirty: bool,
    friend_mgr: FriendPingManager,
    friend_events_rx: Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<FriendEvent>>>,
    /// Optional receiver for Tor reconnection status updates.
    tor_reconnect_rx: Option<Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<String>>>>,
}

impl AppState {
    fn push_system(&mut self, text: String) {
        self.messages.push(ChatLine {
            kind: ChatLineKind::System,
            text,
        });
    }

    fn push_local_msg(&mut self, text: String) {
        self.messages.push(ChatLine {
            kind: ChatLineKind::Local,
            text: format!("[{}] {}", self.local_label, text),
        });
    }

    fn push_remote_msg(&mut self, label: String, text: String) {
        self.messages.push(ChatLine {
            kind: ChatLineKind::Remote,
            text: format!("[{}] {}", label, text),
        });
    }
}

// ── Main ──────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let args = Args::parse();
    ensure_graphical_session();

    let runtime = tokio::runtime::Runtime::new().std_context("failed to create tokio runtime")?;
    let runtime_handle = runtime.handle().clone();

    // Determine initial topic from CLI
    let initial_topic: Option<TopicId> = match &args.command {
        Some(Command::Open { topic }) => {
            let data_dir = get_data_dir();
            let t = match topic {
                Some(t) => *t,
                None => match RoomStore::load_or_none(&data_dir) {
                    Some(store) => {
                        println!("> reusing saved room topic {}", store.topic);
                        store.topic
                    }
                    None => {
                        let t = TopicId::from_bytes(rand::random());
                        println!("> opening new chat room for topic {t}");
                        let room = RoomStore::new(&data_dir, t);
                        if let Err(err) = room.save() {
                            eprintln!("warning: failed to save room metadata: {err}");
                        }
                        t
                    }
                },
            };
            Some(t)
        }
        Some(Command::Join { ticket }) => match Ticket::from_str(ticket) {
            Ok(t) => {
                println!("> joining chat room for topic {}", t.topic);
                Some(t.topic)
            }
            Err(e) => {
                eprintln!("error: failed to parse ticket: {e}");
                None
            }
        },
        None => {
            println!("> no subcommand — showing chat list");
            None
        }
    };

    let (secret_key, key_path) = match args.secret_key.as_ref() {
        None => load_or_generate_secret_key()?,
        Some(key) => (key.parse()?, PathBuf::from("<passed via cli flag>")),
    };
    let local_public = secret_key.public();
    let local_label = args
        .name
        .clone()
        .unwrap_or_else(|| local_public.fmt_short().to_string());
    println!("> our public key: {local_public}");
    println!("> identity file: {}", key_path.display());

    let use_tor = {
        #[cfg(feature = "tor-transport")]
        { args.tor }
        #[cfg(not(feature = "tor-transport"))]
        { false }
    };
    let relay_mode = match (use_tor, args.no_relay, args.relay.clone()) {
        (_, true, Some(_)) => bail_any!("You cannot set --no-relay and --relay at the same time"),
        (_, true, None) => RelayMode::Disabled,
        (true, false, None) => RelayMode::Disabled,
        (false, false, None) => RelayMode::Default,
        (_, false, Some(url)) => RelayMode::Custom(url.into()),
    };
    println!("> relay servers: {}", fmt_relay_mode(&relay_mode));

    // ── Tor reconnection monitor channel ──────────────────────────────
    // Created unconditionally so the event loop always compiles.
    // The monitor task is only spawned in Tor mode; otherwise the
    // sender is never cloned and sits dormant.
    #[allow(unused)]
    let (tor_reconnect_tx, tor_reconnect_rx) =
        tokio::sync::mpsc::unbounded_channel::<String>();

    // ── Build endpoint, gossip, router (no topic subscription yet) ──

    let (
        endpoint,
        gossip,
        blob_store,
        router,
        net_rx,
        net_tx,
        friend_mgr,
        friend_events_rx,
        friends,
        room_history,
        transport_status,
        notice,
        tor_reconnect_rx_opt,
    ) = runtime.block_on(async {
        let memory_lookup = MemoryLookup::new();

        let (endpoint, ep_transport_status, ep_notice) = {
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

                let ts = format!("Tor bootstrap finished: {tor_status_message}");
                let nt = "Tor-backed custom transport is operational. Gossip messages are relayed over Tor hidden services."
                    .to_string();
                // local_peer_addr is consumed by endpoint_addr(), so note it's EndpointAddr
                #[allow(clippy::let_unit_value)]
                { let _ = local_peer_addr; }
                (endpoint, ts, nt)
            } else {
                let builder = if matches!(relay_mode, RelayMode::Disabled) {
                    Endpoint::builder(presets::N0DisableRelay)
                } else {
                    Endpoint::builder(presets::N0)
                };
                let endpoint = builder
                    .secret_key(secret_key.clone())
                    .address_lookup(memory_lookup.clone())
                    .relay_mode(relay_mode.clone())
                    .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                    .bind()
                    .await?;
                if !matches!(relay_mode, RelayMode::Disabled) {
                    endpoint.online().await;
                }
                (endpoint, "Direct iroh transport is ready.".into(), "Direct iroh transport is operational.".into())
            }
            #[cfg(not(feature = "tor-transport"))]
            {
                let builder = if matches!(relay_mode, RelayMode::Disabled) {
                    Endpoint::builder(presets::N0DisableRelay)
                } else {
                    Endpoint::builder(presets::N0)
                };
                let endpoint = builder
                    .secret_key(secret_key.clone())
                    .address_lookup(memory_lookup.clone())
                    .relay_mode(relay_mode.clone())
                    .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                    .bind()
                    .await?;
                if !matches!(relay_mode, RelayMode::Disabled) {
                    endpoint.online().await;
                }
                (endpoint, "Direct iroh transport is ready.".into(), "Direct iroh transport is operational.".into())
            }
        };
        println!("> our endpoint id: {}", endpoint.id());

        let gossip = Gossip::builder().spawn(endpoint.clone());
        let blob_store = MemStore::new();
        let blobs_protocol = BlobsProtocol::new(&blob_store, None);

        let router = iroh::protocol::Router::builder(endpoint.clone())
            .accept(GOSSIP_ALPN, gossip.clone())
            .accept(iroh_blobs::ALPN, blobs_protocol.clone())
            .accept(FRIEND_PING_ALPN, PingHandler)
            .spawn();

        // Load or create the persistent friends list
        let data_dir = get_data_dir();
        let friends = FriendsStore::load_or_default(&data_dir);
        if friends.len() > 0 {
            println!("> loaded {} friend(s) from disk", friends.len());
        }

        // Load room history
        let room_history = iroh_gossip::room_history::RoomHistoryStore::load_or_default(&data_dir);
        if !room_history.is_empty() {
            println!("> loaded {} room(s) from history", room_history.len());
        }

        // Network event channel (shared across rooms)
        let (net_tx, net_rx_tmp) = tokio::sync::mpsc::unbounded_channel::<NetEvent>();
        let net_rx = Arc::new(Mutex::new(net_rx_tmp));

        // Friend ping manager
        let _guard = runtime.handle().enter();
        let (friend_mgr, friend_events_rx_tmp) = FriendPingManager::spawn(
            endpoint.clone(),
            DEFAULT_PING_INTERVAL,
            DEFAULT_CONNECT_TIMEOUT,
        );
        drop(_guard);
        let friend_events_rx = Arc::new(Mutex::new(friend_events_rx_tmp));

        // Register existing friends (we're already inside runtime.block_on, so .await directly)
        for peer in friends
            .iter()
            .filter_map(|(id, _)| id.parse_public_key().ok())
        {
            let _ = friend_mgr.add_friend(peer, None).await;
        }

        Result::<_>::Ok((
            endpoint,
            gossip,
            blob_store,
            router,
            net_rx,
            net_tx,
            friend_mgr,
            friend_events_rx,
            friends,
            room_history,
            ep_transport_status,
            ep_notice,
            use_tor.then(|| Arc::new(Mutex::new(tor_reconnect_rx))),
        ))
    })?;

    let app = AppState {
        screen: Screen::ChatList,
        runtime_handle: runtime_handle.clone(),
        local_label,
        local_public,
        secret_key,
        gossip,
        sender: None,
        blob_store,
        endpoint,
        router,
        net_rx,
        net_tx,
        forward_handle: None,
        messages: vec![],
        input_value: String::new(),
        ticket: String::new(),
        transport_status,
        notice,
        topic: String::new(),
        relay_info: fmt_relay_mode(&relay_mode),
        connected: false,
        peer_count: 0,
        neighbors: HashSet::new(),
        direct_peers: 0,
        relayed_peers: 0,
        conn_refresh_counter: 0,
        dark_mode: false,
        pending_file: None,
        room_history,
        join_ticket_input: String::new(),
        friends,
        friends_dirty: false,
        friend_mgr,
        friend_events_rx,
        tor_reconnect_rx: tor_reconnect_rx_opt,
    };

    let app_cell = std::sync::Mutex::new(Some((app, initial_topic)));

    iced::application(
        move || {
            let (state, init_topic) = app_cell
                .lock()
                .unwrap()
                .take()
                .expect("chat-gui boot called more than once");
            let task = if let Some(topic) = init_topic {
                Task::done(AppMessage::OpenRoom(topic))
            } else {
                Task::none()
            };
            (state, task)
        },
        update,
        view,
    )
    .subscription(subscription)
    .theme(|state: &AppState| {
        if state.dark_mode {
            Theme::Dark
        } else {
            Theme::Light
        }
    })
    .title(|state: &AppState| match state.screen {
        Screen::ChatList => "Iroh Gossip Chat — Inbox".to_string(),
        Screen::Chat { .. } => format!("iroh-gossip Chat — {}", state.local_label),
    })
    .run()
    .unwrap_or_else(|err| {
        eprintln!("Failed to launch iced GUI: {err}");
        std::process::exit(1);
    });

    let _keep_runtime_alive = runtime;
    Ok(())
}

// ── Helpers: room switching ───────────────────────────────────────────

/// Subscribe to a gossip topic and set up event forwarding.
/// Returns the ticket string.  Runs inside the tokio runtime.
async fn subscribe_to_topic(
    gossip: &Gossip,
    endpoint: &Endpoint,
    topic: TopicId,
    secret_key: &SecretKey,
    label: &str,
    net_tx: &tokio::sync::mpsc::UnboundedSender<NetEvent>,
    forward_handle: &mut Option<task::JoinHandle<()>>,
    sender_out: &mut Option<GossipSender>,
) -> Result<String, String> {
    // Abort any existing forwarding task
    if let Some(handle) = forward_handle.take() {
        handle.abort();
    }

    let sub: GossipTopic = gossip
        .subscribe(topic, vec![])
        .await
        .map_err(|e| e.to_string())?;
    let (sender, receiver) = sub.split();
    *sender_out = Some(sender.clone());

    let ticket = Ticket {
        topic,
        peers: vec![EndpointAddr::new(endpoint.id())],
    };
    let ticket_str = ticket.to_string();

    // Spawn forwarding task
    let tx = net_tx.clone();
    let handle = task::spawn(async move {
        forward_gossip_events(receiver, tx).await;
    });
    *forward_handle = Some(handle);

    // Broadcast our presence
    let msg = SignedMessage::sign_and_encode(
        secret_key,
        &Message::AboutMe {
            name: label.to_string(),
        },
    )
    .map_err(|e| e.to_string())?;
    let _ = sender.broadcast(msg).await;

    Ok(ticket_str)
}

// ── Update ────────────────────────────────────────────────────────────

fn update(state: &mut AppState, message: AppMessage) -> Task<AppMessage> {
    match message {
        // ── Navigation ──
        AppMessage::GoToChatList => {
            // Save current room to history
            let preview = state
                .messages
                .last()
                .map(|e| {
                    if e.text.len() > 60 {
                        format!("{}…", &e.text[..60])
                    } else {
                        e.text.clone()
                    }
                })
                .unwrap_or_default();
            if !state.topic.is_empty() {
                if let Ok(topic) = TopicId::from_str(&state.topic) {
                    state.room_history.upsert(topic, &state.local_label, true);
                    if !preview.is_empty() {
                        state.room_history.update_preview(&topic, &preview);
                    }
                    let _ = state.room_history.save();
                }
            }

            // Abort forwarding
            if let Some(handle) = state.forward_handle.take() {
                handle.abort();
            }
            state.sender = None;
            state.messages.clear();
            state.pending_file = None;
            state.connected = false;
            state.screen = Screen::ChatList;
            return Task::none();
        }

        AppMessage::OpenRoom(topic) => {
            let gossip = state.gossip.clone();
            let endpoint = state.endpoint.clone(); // Endpoint is Clone
            let sk = state.secret_key.clone();
            let label = state.local_label.clone();
            let net_tx = state.net_tx.clone();

            Task::perform(
                async move {
                    let mut fwd = None;
                    let mut sender = None;
                    let ticket_str = subscribe_to_topic(
                        &gossip,
                        &endpoint,
                        topic,
                        &sk,
                        &label,
                        &net_tx,
                        &mut fwd,
                        &mut sender,
                    )
                    .await?;
                    // We can't return the JoinHandle from here, so it lives until
                    // GoToChatList or another OpenRoom replaces it.
                    // Store sender for the mapping callback.
                    Ok::<(TopicId, String, Option<GossipSender>), String>((
                        topic, ticket_str, sender,
                    ))
                },
                move |result| match result {
                    Ok((topic, ticket_str, _sender)) => {
                        // The sender is stored in the closure but we need it in state.
                        // RoomOpened handler will set it up.
                        AppMessage::RoomOpened {
                            topic,
                            ticket: ticket_str,
                        }
                    }
                    Err(e) => AppMessage::RoomJoinFailed(e),
                },
            )
        }

        AppMessage::RoomOpened { topic, ticket } => {
            state.screen = Screen::Chat { topic };
            state.topic = topic.to_string();
            state.ticket = ticket.clone();
            state.messages.clear();
            state.connected = true;
            state.push_system(format!("Ticket to join this room: {ticket}"));
            state.push_system("Type a message and press Enter.  /send <path> shares a file  |  /help lists commands".into());
            // Update room history
            state.room_history.upsert(topic, &state.local_label, true);
            let _ = state.room_history.save();
            return Task::none();
        }

        AppMessage::RoomJoinFailed(e) => {
            state.push_system(format!("Failed to join room: {e}"));
            return Task::none();
        }

        // ── Chat list ──
        AppMessage::JoinTicketInputChanged(text) => {
            state.join_ticket_input = text;
            return Task::none();
        }

        AppMessage::CreateNewRoom => {
            let topic = TopicId::from_bytes(rand::random());
            return Task::done(AppMessage::OpenRoom(topic));
        }

        AppMessage::JoinFromTicket => {
            let input = state.join_ticket_input.clone();
            return match Ticket::from_str(&input) {
                Ok(ticket) => Task::done(AppMessage::OpenRoom(ticket.topic)),
                Err(e) => {
                    state.push_system(format!("Invalid ticket: {e}"));
                    Task::none()
                }
            };
        }

        // ── Chat events ──
        AppMessage::NetEvent(event) => {
            handle_net_event(state, event);
            return Task::none();
        }

        AppMessage::FriendEvent(event) => {
            let fid = FriendId::from_public_key(match &event {
                FriendEvent::StatusChanged { peer, .. } => *peer,
            });
            let label = state
                .friends
                .get(&fid)
                .map(|r| r.display_label(&fid))
                .unwrap_or_else(|| fid.as_str().to_string());
            match event {
                FriendEvent::StatusChanged { peer, status } => {
                    let fid = FriendId::from_public_key(peer);
                    match status {
                        FriendStatus::Online => {
                            state.friends.mark_online(fid);
                            state.friends_dirty = true;
                            state.push_system(format!("Friend {label} is now ONLINE"));
                        }
                        FriendStatus::Offline => {
                            state.friends.mark_offline(fid);
                            state.friends_dirty = true;
                            state.push_system(format!("Friend {label} is now offline"));
                        }
                        FriendStatus::Unknown => {}
                    }
                }
            }
            if state.friends_dirty {
                let _ = state.friends.save();
                state.friends_dirty = false;
            }
            return Task::none();
        }

        AppMessage::Tick => {
            // Collect pending net events, then process them outside the lock
            let mut pending_events: Vec<NetEvent> = Vec::new();

            if let Screen::Chat { .. } = state.screen {
                let mut disconnected = false;
                {
                    let mut guard = match state.net_rx.try_lock() {
                        Ok(g) => g,
                        Err(_) => return Task::none(),
                    };
                    loop {
                        match guard.try_recv() {
                            Ok(event) => pending_events.push(event),
                            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                                disconnected = true;
                                break;
                            }
                        }
                    }
                } // guard dropped here
                if disconnected {
                    state.push_system("Network channel closed.".into());
                    state.connected = false;
                }
            }

            // Process net events after releasing the lock
            for event in pending_events {
                handle_net_event(state, event);
            }

            // Poll friend events regardless of screen
            let friend_rx = Arc::clone(&state.friend_events_rx);
            if let Ok(mut friend_guard) = friend_rx.try_lock() {
                loop {
                    match friend_guard.try_recv() {
                        Ok(FriendEvent::StatusChanged { peer, status }) => {
                            let fid = FriendId::from_public_key(peer);
                            let label = state
                                .friends
                                .get(&fid)
                                .map(|r| r.display_label(&fid))
                                .unwrap_or_else(|| peer.fmt_short().to_string());
                            match status {
                                FriendStatus::Online => {
                                    state.friends.mark_online(fid);
                                    state.friends_dirty = true;
                                    state.push_system(format!("Friend {label} is now ONLINE"));
                                }
                                FriendStatus::Offline => {
                                    state.friends.mark_offline(fid);
                                    state.friends_dirty = true;
                                    state.push_system(format!("Friend {label} is now offline"));
                                }
                                FriendStatus::Unknown => {}
                            }
                        }
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                    }
                }
            }

            if state.friends_dirty {
                let _ = state.friends.save();
                state.friends_dirty = false;
            }

            // Periodic connection type refresh (~60s at 50ms tick)
            if state.conn_refresh_counter == 0 {
                recompute_connection_counts(state);
                state.conn_refresh_counter = 1200;
            } else {
                state.conn_refresh_counter -= 1;
            }

            // Poll Tor reconnection status updates
            if let Some(ref rx) = state.tor_reconnect_rx {
                let msgs: Vec<String> = match rx.try_lock() {
                    Ok(mut guard) => {
                        let mut msgs = Vec::new();
                        loop {
                            match guard.try_recv() {
                                Ok(msg) => msgs.push(msg),
                                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                            }
                        }
                        msgs
                    }
                    Err(_) => Vec::new(),
                };
                for msg in msgs {
                    state.push_system(msg);
                }
            }

            return Task::none();
        }

        AppMessage::InputChanged(value) => {
            state.input_value = value;
            return Task::none();
        }
        AppMessage::SendPressed => {
            handle_send(state);
            return Task::none();
        }
        AppMessage::AcceptDownload => {
            handle_download(state);
            return Task::none();
        }
        AppMessage::ToggleDark(dark) => {
            state.dark_mode = dark;
            return Task::none();
        }
        AppMessage::CopyToClipboard(text) => return clipboard::write(text),
        AppMessage::DeleteRoom(topic) => {
            // Remove from history and persist
            state.room_history.remove(&topic);
            let _ = state.room_history.save();
            return Task::none();
        }

        AppMessage::TorReconnect(msg) => {
            state.push_system(msg);
            return Task::none();
        }
    }
}

// ── View ──────────────────────────────────────────────────────────────

fn view(state: &AppState) -> Element<'_, AppMessage, Theme, iced::Renderer> {
    match state.screen {
        Screen::ChatList => view_chat_list(state),
        Screen::Chat { .. } => view_chat_screen(state),
    }
}

/// Render the chat list (inbox) screen.
fn view_chat_list(state: &AppState) -> Element<'_, AppMessage, Theme, iced::Renderer> {
    let header = column![
        text("Iroh Gossip Chat").size(22),
        text(format!(
            "Identity: {}  |  Relay: {}",
            state.local_label, state.relay_info
        ))
        .size(11)
        .color(Color::from_rgb(0.5, 0.5, 0.5)),
    ]
    .spacing(2);

    let join_input = text_input("Paste ticket to join a room…", &state.join_ticket_input)
        .on_input(AppMessage::JoinTicketInputChanged)
        .on_submit(AppMessage::JoinFromTicket)
        .width(Length::Fill);

    let action_row = row![
        button(
            row![text(" + ").size(16), text("New Chat").size(14)]
                .align_y(Alignment::Center)
                .spacing(4),
        )
        .on_press(AppMessage::CreateNewRoom)
        .padding(8),
        button(
            row![text(" ⇄ ").size(16), text("Join via Ticket").size(14)]
                .align_y(Alignment::Center)
                .spacing(4),
        )
        .on_press(AppMessage::JoinFromTicket)
        .padding(8),
    ]
    .spacing(8);

    let mut list = column![].spacing(2).width(Length::Fill);

    if state.room_history.is_empty() {
        list = list.push(
            text("No recent chats. Create a new chat or join an existing one.")
                .color(Color::from_rgb(0.5, 0.5, 0.5))
                .size(13),
        );
    } else {
        for room in &state.room_history.rooms {
            let topic = room.topic;
            let display_name = room.display_name();
            let preview = if room.last_preview.is_empty() {
                if room.is_owner {
                    "Created this room"
                } else {
                    "Joined this room"
                }
            } else {
                &room.last_preview
            };

            let delete_btn = button("×")
                .on_press(AppMessage::DeleteRoom(topic))
                .padding(4);

            let row_btn = button(
                row![
                    column![
                        row![text(display_name).size(14).width(Length::Fill)],
                        row![text(preview)
                            .size(11)
                            .color(Color::from_rgb(0.5, 0.5, 0.5))
                            .width(Length::Fill)],
                    ]
                    .spacing(2)
                    .padding(8)
                    .width(Length::Fill),
                    delete_btn,
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .on_press(AppMessage::OpenRoom(topic))
            .width(Length::Fill)
            .padding(0);

            list = list.push(container(row_btn).width(Length::Fill));
        }
    }

    let body = column![
        header,
        action_row,
        join_input,
        scrollable(list).height(Length::Fill)
    ]
    .spacing(8)
    .padding(12);

    container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// Render the chat screen (messages for the current room).
fn view_chat_screen(state: &AppState) -> Element<'_, AppMessage, Theme, iced::Renderer> {
    let sys_color = if state.dark_mode {
        Color::from_rgb(0.6, 0.6, 0.6)
    } else {
        Color::from_rgb(0.4, 0.4, 0.4)
    };
    let local_color = Color::from_rgb(0.15, 0.65, 0.15);
    let remote_color = Color::from_rgb(0.15, 0.35, 0.85);

    let status_text = format!(
        "Identity: {}\nTopic: {}\nTransport: {}\nPeers: {} known  ·  {} direct, {} relay  ·  connected: {}\nRelay: {}",
        state.local_label,
        state.topic,
        state.transport_status,
        state.peer_count,
        state.direct_peers,
        state.relayed_peers,
        if state.connected { "yes" } else { "no" },
        state.relay_info,
    );

    let back_btn = button(" ← ").on_press(AppMessage::GoToChatList);

    let status_panel = container(
        column![row![
            back_btn,
            text(status_text).size(13).width(Length::Fill)
        ],]
        .spacing(2),
    )
    .padding(8)
    .style(move |_| {
        let bg = if state.dark_mode {
            Color::from_rgb(0.18, 0.18, 0.20)
        } else {
            Color::from_rgb(0.93, 0.93, 0.96)
        };
        container::Style::default().background(bg).border(
            border::Border::default()
                .width(1)
                .color(Color::from_rgb(0.6, 0.6, 0.6)),
        )
    });

    let ticket_prefix: &str = "Ticket to join this room: ";

    let log_col =
        state
            .messages
            .iter()
            .fold(column![].spacing(1).width(Length::Fill), |col, line| {
                let color = match line.kind {
                    ChatLineKind::System => sys_color,
                    ChatLineKind::Local => local_color,
                    ChatLineKind::Remote => remote_color,
                };
                let elem: Element<'_, AppMessage, Theme, iced::Renderer> =
                    if let Some(ticket_val) = line.text.strip_prefix(ticket_prefix) {
                        button(
                            text(&line.text)
                                .color(color)
                                .size(14)
                                .wrapping(Wrapping::Word),
                        )
                        .on_press(AppMessage::CopyToClipboard(ticket_val.to_string()))
                        .style(button::text)
                        .into()
                    } else {
                        text(&line.text)
                            .color(color)
                            .size(14)
                            .wrapping(Wrapping::Word)
                            .into()
                    };
                col.push(elem)
            });
    let log_panel = container(scrollable(log_col).height(Length::Fill).width(Length::Fill))
        .padding(8)
        .style(|_| {
            container::Style::default()
                .background(Color::from_rgb(0.98, 0.98, 1.0))
                .border(
                    border::Border::default()
                        .width(1)
                        .color(Color::from_rgb(0.6, 0.6, 0.6)),
                )
        });

    let input = text_input("Type a message...", &state.input_value)
        .on_input(AppMessage::InputChanged)
        .on_submit(AppMessage::SendPressed)
        .width(Length::Fill)
        .size(16);
    let send_btn = button("Send").on_press(AppMessage::SendPressed);
    let input_row = row![input, send_btn].spacing(8).align_y(Alignment::Center);

    let mut composer_children: Vec<Element<'_, AppMessage, Theme, iced::Renderer>> = Vec::new();
    if state.pending_file.is_some() {
        composer_children.push(
            button("Download pending file")
                .on_press(AppMessage::AcceptDownload)
                .into(),
        );
    }
    composer_children.push(
        row![
            input_row,
            toggler(state.dark_mode)
                .label("Dark")
                .on_toggle(AppMessage::ToggleDark)
        ]
        .spacing(16)
        .align_y(Alignment::Center)
        .into(),
    );

    let composer = container(
        composer_children
            .into_iter()
            .fold(column![].spacing(4), |col, child| col.push(child)),
    )
    .padding(8)
    .style(move |_| {
        let bg = if state.dark_mode {
            Color::from_rgb(0.16, 0.16, 0.18)
        } else {
            Color::from_rgb(0.95, 0.95, 0.97)
        };
        container::Style::default().background(bg).border(
            border::Border::default()
                .width(1)
                .color(Color::from_rgb(0.6, 0.6, 0.6)),
        )
    });

    let content = column![status_panel, log_panel, composer]
        .spacing(4)
        .height(Length::Fill);

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| {
            let bg = if state.dark_mode {
                Color::from_rgb(0.12, 0.12, 0.14)
            } else {
                Color::from_rgb(0.90, 0.90, 0.93)
            };
            container::Style::default().background(bg)
        })
        .into()
}

// ── Subscription ──────────────────────────────────────────────────────

fn subscription(_state: &AppState) -> Subscription<AppMessage> {
    iced::time::every(Duration::from_millis(50)).map(|_| AppMessage::Tick)
}

// ── Network event handling ────────────────────────────────────────────

/// Query the iroh endpoint for each neighbor and recompute direct/relay counts.
fn recompute_connection_counts(state: &mut AppState) {
    let mut direct = 0usize;
    let mut relayed = 0usize;
    let rt = state.runtime_handle.clone();
    for peer in &state.neighbors {
        let has_direct = rt
            .block_on(async { state.endpoint.remote_info(*peer).await })
            .map(|info| info.addrs().any(|a| !a.addr().is_relay()))
            .unwrap_or(false);
        if has_direct {
            direct += 1;
        } else {
            relayed += 1;
        }
    }
    state.direct_peers = direct;
    state.relayed_peers = relayed;
}

fn handle_net_event(state: &mut AppState, event: NetEvent) {
    match event {
        NetEvent::Message { from, message, .. } => match message {
            Message::AboutMe { name } => {
                state.push_system(format!("{} is now known as {name}", from.fmt_short()));
            }
            Message::Message { text } => {
                if from == state.local_public {
                    return;
                }
                state.push_remote_msg(from.fmt_short().to_string(), text);
            }
            Message::FileShare { name, ticket } => {
                if from == state.local_public {
                    return;
                }
                state.pending_file = Some((name.clone(), ticket));
                state.push_system(format!(
                    "{} shared — click Download or type /download to fetch",
                    name
                ));
            }
            Message::Goodbye
            | Message::Typing
            | Message::ReadReceipt { .. }
            | Message::Edit { .. }
            | Message::Delete { .. }
            | Message::Reaction { .. } => {
                // Handled via NeighborDown or by the shared chat_core handler.
            }
        },
        NetEvent::NeighborUp { peer } => {
            state.push_system(format!("{} joined the chat", peer.fmt_short()));
            // Track friend state
            let fid = FriendId::from_public_key(peer);
            if state.friends.get(&fid).is_some() {
                state.friends.mark_online(fid);
                state.friends_dirty = true;
            }
            // Track for direct-vs-relay counting
            state.neighbors.insert(peer);
            // Recompute direct/relay counts via the endpoint
            recompute_connection_counts(state);
        }
        NetEvent::NeighborDown { peer } => {
            state.push_system(format!("{} left the chat", peer.fmt_short()));
            // Track friend state
            let fid = FriendId::from_public_key(peer);
            if state.friends.get(&fid).is_some() {
                state.friends.mark_offline(fid);
                state.friends_dirty = true;
            }
            // Remove from neighbor tracking
            state.neighbors.remove(&peer);
            // Recompute direct/relay counts
            recompute_connection_counts(state);
        }
        NetEvent::Error(err) => state.push_system(format!("Error: {err}")),
        NetEvent::Closed => {
            state.push_system("The gossip receiver closed.".into());
            state.connected = false;
        }
    }
    // Update preview in room history
    if !state.topic.is_empty() {
        if let Ok(topic) = TopicId::from_str(&state.topic) {
            if let Some(last) = state.messages.last() {
                let preview = if last.text.len() > 60 {
                    format!("{}…", &last.text[..60])
                } else {
                    last.text.clone()
                };
                if !preview.is_empty() && preview != "Ticket to join this room: " {
                    state.room_history.update_preview(&topic, &preview);
                }
            }
        }
    }
}

fn handle_send(state: &mut AppState) {
    let trimmed = state.input_value.trim().to_string();
    if trimmed.is_empty() {
        return;
    }
    state.input_value.clear();

    if let Some(path) = trimmed.strip_prefix("/send ") {
        let path = path.trim().to_string();
        let abs_path = match std::path::absolute(&PathBuf::from(&path)) {
            Ok(p) => p,
            Err(e) => {
                state.push_system(format!("Failed to resolve path: {e}"));
                return;
            }
        };
        if !abs_path.exists() {
            state.push_system(format!("File not found: {path}"));
            return;
        }
        let filename = match abs_path.file_name().map(|s| s.to_string_lossy()) {
            Some(n) => n.to_string(),
            None => {
                state.push_system("Invalid file path.".into());
                return;
            }
        };
        state.push_system(format!("Hashing file: {filename}..."));
        let rt = state.runtime_handle.clone();
        let tag = rt.block_on(async { state.blob_store.blobs().add_path(abs_path).await.unwrap() });
        let node_id = state.endpoint.id();
        let blob_ticket = BlobTicket::new(node_id.into(), tag.hash, tag.format);
        let ticket_str = blob_ticket.to_string();
        if let Ok(encoded) = SignedMessage::sign_and_encode(
            &state.secret_key,
            &Message::FileShare {
                name: filename.clone(),
                ticket: ticket_str.clone(),
            },
        ) {
            if let Some(ref sender) = state.sender {
                let _ = rt.block_on(async { sender.broadcast(encoded).await });
            }
        }
        state.push_local_msg(format!("/send {path}"));
        state.push_system(format!("Sharing: {filename} (ticket: {ticket_str})"));
        return;
    }

    if trimmed == "/download" {
        handle_download(state);
        return;
    }
    if trimmed == "/help" {
        state.push_system(
            "Commands:  /send <path> — share a file  |  /download — fetch pending file  |  /leave — leave and delete from history  |  /help — this help  |  /friend add <pk> [alias] — track friend  |  /friend remove <pk|alias> — remove friend  |  /friend list — list friends".into(),
        );
        return;
    }

    // ── Leave room / delete from history ──
    if trimmed == "/leave" {
        let topic_str = state.topic.clone();
        if let Ok(topic) = TopicId::from_str(&topic_str) {
            // Broadcast Goodbye (best-effort)
            if let Some(ref sender) = state.sender {
                if let Ok(encoded) =
                    SignedMessage::sign_and_encode(&state.secret_key, &Message::Goodbye)
                {
                    let sender = sender.clone();
                    task::spawn(async move {
                        sender.broadcast(encoded).await.ok();
                    });
                }
            }
            // Remove room from history
            state.room_history.remove(&topic);
            let _ = state.room_history.save();
        }
        // Leave the room and go back to chat list
        if let Some(handle) = state.forward_handle.take() {
            handle.abort();
        }
        state.sender = None;
        state.messages.clear();
        state.screen = Screen::ChatList;
        return;
    }

    // ── Friend commands ────────────────────────────────────────
    if let Some(pubkey_str) = trimmed.strip_prefix("/friend add ") {
        let pubkey_str = pubkey_str.trim().to_string();
        let (pubkey_str, alias) =
            if let Some((key_part, rest)) = pubkey_str.split_once(char::is_whitespace) {
                (key_part.to_string(), Some(rest.trim().to_string()))
            } else {
                (pubkey_str, None)
            };
        let rt = state.runtime_handle.clone();
        match pubkey_str.parse::<PublicKey>() {
            Ok(peer) => {
                let fid = FriendId::from_public_key(peer);
                let was_new = state.friends.get(&fid).is_none();
                if let Some(alias_text) = &alias {
                    state.friends.set_label(fid.clone(), alias_text.clone());
                } else {
                    state.friends.ensure_friend(fid.clone());
                }
                state.friends_dirty = true;
                match rt.block_on(async { state.friend_mgr.add_friend(peer, None).await }) {
                    Ok(_) => {
                        let label = if let Some(ref alias_text) = alias {
                            format!("{alias_text} ({})", peer.fmt_short())
                        } else {
                            peer.fmt_short().to_string()
                        };
                        if was_new {
                            state.push_system(format!("Added friend: {label}"));
                        } else {
                            state.push_system(format!("Updated friend: {label}"));
                        }
                    }
                    Err(e) => state.push_system(format!("Failed to add friend: {e}")),
                }
            }
            Err(e) => state.push_system(format!("Invalid public key: {e}")),
        }
        return;
    }

    if let Some(target) = trimmed.strip_prefix("/friend remove ") {
        let target = target.trim().to_string();
        let rt = state.runtime_handle.clone();
        let resolved = if let Ok(pk) = target.parse::<PublicKey>() {
            Some((pk, FriendId::from_public_key(pk)))
        } else {
            state
                .friends
                .iter()
                .find(|(_, rec)| rec.label.as_deref() == Some(&target))
                .map(|(fid, _)| (fid.parse_public_key().ok(), fid.clone()))
                .and_then(|(pk_opt, fid)| pk_opt.map(|pk| (pk, fid)))
        };
        match resolved {
            Some((peer, fid)) => {
                let label = state
                    .friends
                    .get(&fid)
                    .and_then(|r| r.label.clone())
                    .unwrap_or_else(|| peer.fmt_short().to_string());
                state.friends.remove(&fid);
                state.friends_dirty = true;
                let _ = rt.block_on(async { state.friend_mgr.remove_friend(&peer).await });
                state.push_system(format!("Removed friend: {label}"));
            }
            None => state.push_system(format!("Friend not found: {target}")),
        }
        return;
    }

    if trimmed == "/friend list" {
        let rt = state.runtime_handle.clone();
        match rt.block_on(async { state.friend_mgr.list_friends().await }) {
            Ok(list) => {
                if list.is_empty() && state.friends.is_empty() {
                    state.push_system("No friends tracked yet.".into());
                } else {
                    state.push_system(format!("Friends ({}):", state.friends.len()));
                    for (peer, status) in &list {
                        let fid = FriendId::from_public_key(*peer);
                        let label = state
                            .friends
                            .get(&fid)
                            .map(|r| r.display_label(&fid))
                            .unwrap_or_else(|| peer.fmt_short().to_string());
                        let status_str = match status {
                            FriendStatus::Unknown => "?",
                            FriendStatus::Online => "ONLINE",
                            FriendStatus::Offline => "offline",
                        };
                        state.push_system(format!("  {label}: {status_str}"));
                    }
                }
            }
            Err(e) => state.push_system(format!("Failed to list friends: {e}")),
        }
        return;
    }

    if trimmed == "/connections" {
        use iroh_gossip::chat_core::check_peer_connection_type;
        let neighbors: Vec<iroh::PublicKey> = state.neighbors.iter().copied().collect();
        if neighbors.is_empty() {
            state.push_system("No known peers to inspect.".into());
        } else {
            state.push_system(format!("Connections ({}):", neighbors.len()));
            let rt = state.runtime_handle.clone();
            let ep = state.endpoint.clone();
            // Ensure we yield to the Tokio runtime in each iteration.
            for pk in &neighbors {
                let ctype = rt.block_on(async { check_peer_connection_type(&ep, *pk).await });
                state.push_system(format!(
                    "  {} — {} ({})",
                    pk.fmt_short(),
                    match ctype {
                        iroh_gossip::chat_core::ConnectionType::Direct => "direct",
                        iroh_gossip::chat_core::ConnectionType::Relayed => "relayed",
                        iroh_gossip::chat_core::ConnectionType::Unknown => "unknown",
                    },
                    pk.fmt_short(),
                ));
            }
        }
        return;
    }

    let msg = Message::Message {
        text: trimmed.clone(),
    };
    let rt = state.runtime_handle.clone();
    if let Ok(encoded) = SignedMessage::sign_and_encode(&state.secret_key, &msg) {
        if let Some(ref sender) = state.sender {
            let _ = rt.block_on(async { sender.broadcast(encoded).await });
        }
    }
    state.push_local_msg(trimmed);
}

fn handle_download(state: &mut AppState) {
    let (filename, ticket_str) = match state.pending_file.clone() {
        Some(p) => p,
        None => {
            state.push_system("No pending file to download.".into());
            return;
        }
    };
    let ticket: BlobTicket = match ticket_str.parse() {
        Ok(t) => t,
        Err(e) => {
            state.push_system(format!("Failed to parse blob ticket: {e}"));
            return;
        }
    };
    let peer_id = ticket.addr().id;
    let rt = state.runtime_handle.clone();
    let downloader = state.blob_store.downloader(&state.endpoint);
    state.push_system(format!("Downloading: {filename}..."));

    let ok = rt.block_on(async {
        match downloader.download(ticket.hash(), Some(peer_id)).await {
            Ok(_) => {
                let dest = std::env::current_dir().unwrap_or_default().join(&filename);
                match state.blob_store.blobs().export(ticket.hash(), dest).await {
                    Ok(_) => true,
                    Err(e) => {
                        state.push_system(format!("Export failed: {e}"));
                        false
                    }
                }
            }
            Err(e) => {
                state.push_system(format!("Download failed: {e}"));
                false
            }
        }
    });

    if ok {
        state.push_system(format!("Saved: {filename}"));
        state.pending_file = None;
    }
}

// ── Background network task ───────────────────────────────────────────

async fn forward_gossip_events(
    mut receiver: GossipReceiver,
    net_tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
) {
    while let Ok(Some(event)) = receiver.try_next().await {
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
            GossipEvent::Lagged => {}
        }
    }
    let _ = net_tx.send(NetEvent::Closed);
}
