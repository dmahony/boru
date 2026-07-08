//! # GUI chat frontend for iroh-gossip (iced 0.14)
//!
//! Alternative frontend using the iced GUI toolkit.  Networking runs in
//! background tokio tasks; events flow through channels to iced.
//!
//! ## Build / run
//!
//! ```text
//! cargo chat-gui open              # open a room
//! cargo chat-gui join <ticket>     # join a room
//! ```
//!
//! Long form:
//! ```text
//! cargo run --features gui --example chat-gui -- open
//! ```

use std::{
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
    border,
    widget::{button, column, container, row, scrollable, text, text_input, toggler},
    Alignment, Color, Element, Length, Subscription, Task, Theme,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, Endpoint, PublicKey, RelayMode,
    RelayUrl, SecretKey,
};
use iroh_blobs::{store::mem::MemStore, ticket::BlobTicket, BlobsProtocol};
use iroh_gossip::{
    api::{Event as GossipEvent, GossipReceiver, GossipSender},
    chat_core::friend_ping::{
        FriendEvent, FriendPingManager, FriendStatus, PingHandler, DEFAULT_CONNECT_TIMEOUT,
        DEFAULT_PING_INTERVAL, FRIEND_PING_ALPN,
    },
    chat_core::{fmt_relay_mode, Message, NetEvent, SignedMessage, Ticket},
    friends::{FriendId, FriendsStore},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
};
use n0_error::{bail_any, Result, StdResultExt};
use n0_future::{task, StreamExt};
use tokio::sync::mpsc;

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
    #[clap(short, long)]
    name: Option<String>,
    #[clap(long, default_value = "0")]
    bind_port: u16,
    #[clap(subcommand)]
    command: Command,
}

#[derive(Parser, Debug)]
enum Command {
    Open { topic: Option<TopicId> },
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

// ── Protocol types ────────────────────────────────────────────────────
// (imported from iroh_gossip::chat_core)

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

// (imported from iroh_gossip::chat_core)

#[derive(Debug, Clone)]
enum AppMessage {
    InputChanged(String),
    SendPressed,
    ToggleDark(bool),
    Tick,
    AcceptDownload,
}

struct AppState {
    runtime_handle: tokio::runtime::Handle,
    local_label: String,
    local_public: PublicKey,
    secret_key: SecretKey,
    sender: GossipSender,
    blob_store: MemStore,
    endpoint: Endpoint,
    router: iroh::protocol::Router,
    net_rx: Arc<Mutex<mpsc::UnboundedReceiver<NetEvent>>>,
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
    friends: FriendsStore,
    friends_dirty: bool,
    friend_mgr: FriendPingManager,
    friend_events_rx: Arc<Mutex<mpsc::UnboundedReceiver<FriendEvent>>>,
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

    let (
        topic,
        peers,
        secret_key,
        local_public,
        local_label,
        relay_mode,
        endpoint,
        blob_store,
        sender,
        net_rx,
        ticket,
        router,
    ) = runtime.block_on(async {
        let (topic, peers) = match &args.command {
            Command::Open { topic } => {
                let topic = topic.unwrap_or_else(|| TopicId::from_bytes(rand::random()));
                println!("> opening chat room for topic {topic}");
                (topic, vec![])
            }
            Command::Join { ticket } => {
                let Ticket { topic, peers } = Ticket::from_str(ticket)?;
                println!("> joining chat room for topic {topic}");
                (topic, peers)
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

        let relay_mode = match (args.no_relay, args.relay.clone()) {
            (true, Some(_)) => bail_any!("You cannot set --no-relay and --relay at the same time"),
            (true, None) => RelayMode::Disabled,
            (false, None) => RelayMode::Default,
            (false, Some(url)) => RelayMode::Custom(url.into()),
        };
        println!("> relay servers: {}", fmt_relay_mode(&relay_mode));

        let memory_lookup = MemoryLookup::new();

        let endpoint = {
            let builder = if matches!(relay_mode, RelayMode::Disabled) {
                Endpoint::builder(presets::N0DisableRelay)
            } else {
                Endpoint::builder(presets::N0)
            };
            builder
                .secret_key(secret_key.clone())
                .address_lookup(memory_lookup.clone())
                .relay_mode(relay_mode.clone())
                .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))
                .unwrap()
                .bind()
                .await?
        };
        if !matches!(relay_mode, RelayMode::Disabled) {
            endpoint.online().await;
        }
        let local_peer_addr = endpoint.addr();
        println!("> our endpoint id: {}", endpoint.id());

        let gossip = Gossip::builder().spawn(endpoint.clone());
        let blob_store = MemStore::new();
        let blobs_protocol = BlobsProtocol::new(&blob_store, None);
        let ticket = Ticket {
            topic,
            peers: vec![local_peer_addr],
        };
        println!("> ticket to join us: {ticket}");

        let router = iroh::protocol::Router::builder(endpoint.clone())
            .accept(GOSSIP_ALPN, gossip.clone())
            .accept(iroh_blobs::ALPN, blobs_protocol.clone())
            .accept(FRIEND_PING_ALPN, PingHandler)
            .spawn();

        let peer_ids = peers.iter().map(|p| p.id).collect::<Vec<_>>();
        for peer in &peers {
            memory_lookup.set_endpoint_info(peer.clone());
        }
        let (sender, receiver) = gossip.subscribe(topic, peer_ids).await?.split();

        if let Some(ref name) = args.name {
            let msg = SignedMessage::sign_and_encode(
                &secret_key,
                &Message::AboutMe { name: name.clone() },
            )?;
            sender.broadcast(msg).await?;
        }

        let (net_tx, net_rx_tmp) = mpsc::unbounded_channel::<NetEvent>();
        let net_rx = Arc::new(Mutex::new(net_rx_tmp));
        task::spawn(forward_gossip_events(receiver, net_tx));

        Result::<_>::Ok((
            topic,
            peers,
            secret_key,
            local_public,
            local_label,
            relay_mode,
            endpoint,
            blob_store,
            sender,
            net_rx,
            ticket.to_string(),
            router,
        ))
    })?;

    // Load or create the persistent friends list
    let data_dir = get_data_dir();
    let friends = FriendsStore::load_or_default(&data_dir);
    if friends.len() > 0 {
        println!("> loaded {} friend(s) from disk", friends.len());
    }

    // ── Friend ping manager ────────────────────────────────────
    // Enter the Tokio runtime context so tokio::task::spawn inside
    // FriendPingManager::spawn has a reactor to attach to.  The spawn
    // is non-async (it fires a task then returns), so we just need a
    // momentary EnterGuard.
    let _guard = runtime.handle().enter();
    let (friend_mgr, friend_events_rx_tmp) = FriendPingManager::spawn(
        endpoint.clone(),
        DEFAULT_PING_INTERVAL,
        DEFAULT_CONNECT_TIMEOUT,
    );
    drop(_guard);
    let friend_events_rx = Arc::new(Mutex::new(friend_events_rx_tmp));

    // Register existing friends with the ping manager
    // Use the outer runtime_handle (not Handle::current) because we dropped
    // the EnterGuard above and this thread is no longer inside a tokio context.
    for peer in friends
        .iter()
        .filter_map(|(id, _)| id.parse_public_key().ok())
    {
        let _ = runtime_handle
            .block_on(async { friend_mgr.add_friend(peer, None).await });
    }

    let app = AppState {
        runtime_handle: runtime_handle.clone(),
        local_label,
        local_public,
        secret_key,
        sender,
        blob_store,
        endpoint,
        router,
        net_rx,
        messages: vec![
            ChatLine { kind: ChatLineKind::System, text: format!("Ticket to join this room: {}", ticket) },
            ChatLine { kind: ChatLineKind::System, text: if peers.is_empty() { "Waiting for peers to join us...".into() } else { format!("Trying to connect to {} peers...", peers.len()) } },
            ChatLine { kind: ChatLineKind::System, text: "Type a message and press Enter.  /send <path> shares a file  |  /help lists commands".into() },
        ],
        input_value: String::new(),
        ticket: ticket.clone(),
        transport_status: "Direct iroh transport is ready.".into(),
        notice: "Direct iroh transport is operational.".into(),
        topic: topic.to_string(),
        relay_info: fmt_relay_mode(&relay_mode),
        connected: true,
        peer_count: peers.len(),
        dark_mode: false,
        pending_file: None,
        friends,
        friends_dirty: false,
        friend_mgr,
        friend_events_rx,
    };

    let app_cell = std::sync::Mutex::new(Some(app));

    iced::application(
        move || {
            let state = app_cell
                .lock()
                .unwrap()
                .take()
                .expect("iced_chat boot called more than once");
            (state, iced::Task::none())
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
    .title(|state: &AppState| format!("iroh-gossip Chat — {}", state.local_label))
    .run()
    .unwrap_or_else(|err| {
        eprintln!("Failed to launch iced GUI: {err}");
        std::process::exit(1);
    });

    let _keep_runtime_alive = runtime;
    Ok(())
}

// ── Update ────────────────────────────────────────────────────────────

fn update(state: &mut AppState, message: AppMessage) -> Task<AppMessage> {
    match message {
        AppMessage::Tick => {
            // Poll network events
            let connected = {
                let mut guard = match state.net_rx.try_lock() {
                    Ok(g) => g,
                    Err(_) => return Task::none(),
                };
                loop {
                    match guard.try_recv() {
                        Ok(event) => {
                            drop(guard);
                            handle_net_event(state, event);
                            guard = match state.net_rx.try_lock() {
                                Ok(g) => g,
                                Err(_) => return Task::none(),
                            };
                        }
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => {
                            drop(guard);
                            state.push_system("Network channel closed.".into());
                            state.connected = false;
                            break;
                        }
                    }
                }
                state.connected
            };
            if !connected {
                return Task::none();
            }

            // Poll friend events (simple non-reentrant drain)
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
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => break,
                    }
                }
            }

            if state.friends_dirty {
                let _ = state.friends.save();
                state.friends_dirty = false;
            }
        }
        AppMessage::InputChanged(value) => state.input_value = value,
        AppMessage::SendPressed => handle_send(state),
        AppMessage::AcceptDownload => handle_download(state),
        AppMessage::ToggleDark(dark) => state.dark_mode = dark,
    }
    Task::none()
}

// ── View ──────────────────────────────────────────────────────────────

fn view(state: &AppState) -> Element<'_, AppMessage, Theme, iced::Renderer> {
    let sys_color = if state.dark_mode {
        Color::from_rgb(0.6, 0.6, 0.6)
    } else {
        Color::from_rgb(0.4, 0.4, 0.4)
    };
    let local_color = Color::from_rgb(0.15, 0.65, 0.15);
    let remote_color = Color::from_rgb(0.15, 0.35, 0.85);

    let status_text = format!(
        "Identity: {}\nTopic: {}\nTransport: {}\nPeers: {} known  •  connected: {}\nRelay: {}",
        state.local_label,
        state.topic,
        state.transport_status,
        state.peer_count,
        if state.connected { "yes" } else { "no" },
        state.relay_info,
    );
    let status_panel = container(column![text(status_text).size(13)].spacing(2))
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
                col.push(
                    text(&line.text)
                        .color(color)
                        .size(14)
                        .wrapping(Wrapping::Word),
                )
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

fn handle_net_event(state: &mut AppState, event: NetEvent) {
    match event {
        NetEvent::Message { from, message } => match message {
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
            Message::Goodbye => {
                // Handled via NeighborDown (cleaner, covers both clean and unclean exits)
            }
        },
        NetEvent::NeighborUp { peer } => {
            state.push_system(format!("{} joined the chat", peer.fmt_short()));
        }
        NetEvent::NeighborDown { peer } => {
            state.push_system(format!("{} left the chat", peer.fmt_short()));
        }
        NetEvent::Error(err) => state.push_system(format!("Error: {err}")),
        NetEvent::Closed => {
            state.push_system("The gossip receiver closed.".into());
            state.connected = false;
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
            let _ = rt.block_on(async { state.sender.broadcast(encoded).await });
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
            "Commands:  /send <path> — share a file  |  /download — fetch pending file  |  /help — this help  |  /friend add <pk> [alias] — track friend  |  /friend remove <pk|alias> — remove friend  |  /friend list — list friends".into(),
        );
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
            state.friends.iter()
                .find(|(_, rec)| rec.label.as_deref() == Some(&target))
                .map(|(fid, _)| (fid.parse_public_key().ok(), fid.clone()))
                .and_then(|(pk_opt, fid)| pk_opt.map(|pk| (pk, fid)))
        };
        match resolved {
            Some((peer, fid)) => {
                let label = state.friends.get(&fid)
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
                        let label = state.friends.get(&fid)
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

    let msg = Message::Message {
        text: trimmed.clone(),
    };
    let rt = state.runtime_handle.clone();
    if let Ok(encoded) = SignedMessage::sign_and_encode(&state.secret_key, &msg) {
        let _ = rt.block_on(async { state.sender.broadcast(encoded).await });
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
    net_tx: mpsc::UnboundedSender<NetEvent>,
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
            GossipEvent::NeighborDown(id) => {
                if net_tx.send(NetEvent::NeighborDown { peer: id }).is_err() {
                    return;
                }
            }
            GossipEvent::NeighborUp(_) | GossipEvent::Lagged => {}
        }
    }
    let _ = net_tx.send(NetEvent::Closed);
}

// ── Helpers ───────────────────────────────────────────────────────────
// fmt_relay_mode imported from iroh_gossip::chat_core
