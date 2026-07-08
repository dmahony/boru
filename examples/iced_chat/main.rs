//! Iced desktop frontend for iroh-gossip chat.
//!
//! Usage:
//!   cargo run --features gui --example iced_chat       # show chat list
//!   cargo run --features gui --example iced_chat open   # open new room
//!   cargo run --features gui --example iced_chat join <ticket>  # join room

mod app;

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use clap::Parser;
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, Endpoint, RelayMode,
    RelayUrl, SecretKey,
};
use iroh_blobs::{store::mem::MemStore, BlobsProtocol};
use iroh_gossip::chat_core::friend_ping::{
    FriendPingManager, PingHandler, DEFAULT_CONNECT_TIMEOUT, DEFAULT_PING_INTERVAL,
    FRIEND_PING_ALPN,
};
use iroh_gossip::friends::FriendsStore;
use iroh_gossip::net::{Gossip, GOSSIP_ALPN};
use iroh_gossip::proto::TopicId;
use iroh_gossip::room::RoomStore;
use iroh_gossip::room_history::RoomHistoryStore;
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

// ── Message protocol ──────────────────────────────────────────────────
pub use iroh_gossip::chat_core::{fmt_relay_mode, Message, NetEvent, SignedMessage, Ticket};

// ── Network event bridging ────────────────────────────────────────────
pub use iroh_gossip::chat_core::forward_gossip_events;

// ── Identity persistence ──────────────────────────────────────────────

fn get_data_dir() -> PathBuf {
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

// ── Entry point ───────────────────────────────────────────────────────

fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let args = Args::parse();
    ensure_graphical_session();

    let runtime = tokio::runtime::Runtime::new().std_context("failed to create tokio runtime")?;
    // Determine if there's an initial room to connect to
    let initial_topic: Option<TopicId> = runtime.block_on(async {
        match &args.command {
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
            Some(Command::Join { ticket }) => {
                let ticket: Ticket = match Ticket::from_str(ticket) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("error: failed to parse ticket: {e}");
                        return None;
                    }
                };
                println!("> joining chat room for topic {}", ticket.topic);
                Some(ticket.topic)
            }
            None => {
                println!("> no subcommand — showing chat list");
                None
            }
        }
    });

    let (secret_key, key_path) = match args.secret_key.as_ref() {
        None => load_or_generate_secret_key()?,
        Some(key) => (key.parse()?, PathBuf::from("<passed via cli flag>")),
    };
    let local_public = secret_key.public();
    println!("> our public key: {local_public}");
    println!("> identity file: {}", key_path.display());

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
    println!("> relay: {}", fmt_relay_mode(&relay_mode));

    // ── Build the endpoint, gossip, and router (no topic subscription yet) ──

    let (
        endpoint,
        gossip,
        blob_store,
        net_rx,
        net_tx,
        friend_mgr,
        friend_events_rx,
        friends,
        room_history,
    ) = runtime.block_on(async {
        let memory_lookup = MemoryLookup::new();
        use std::net::{Ipv4Addr, SocketAddrV4};
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
        println!("> endpoint: {}", endpoint.id());

        let gossip = Gossip::builder().spawn(endpoint.clone());
        let blob_store = MemStore::new();
        let blobs_protocol = BlobsProtocol::new(&blob_store, None);

        let _router = iroh::protocol::Router::builder(endpoint.clone())
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
        let room_history = RoomHistoryStore::load_or_default(&data_dir);
        if !room_history.is_empty() {
            println!("> loaded {} room(s) from history", room_history.len());
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
            gossip,
            blob_store,
            net_rx,
            net_tx,
            friend_mgr,
            friend_events_rx,
            friends,
            room_history,
        ))
    })?;

    let app_cell = std::sync::Mutex::new(Some((IcedChat::new(
        secret_key,
        gossip,
        blob_store,
        endpoint.clone(),
        local_label,
        local_public,
        relay_mode,
        Arc::clone(&net_rx),
        net_tx,
        room_history,
        friends,
        friend_mgr,
        Arc::clone(&friend_events_rx),
        initial_topic,
    ), initial_topic)));

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
    .subscription(|state: &IcedChat| IcedChat::subscription(Arc::clone(&state.net_rx), Arc::clone(&state.friend_events_rx)))
    .theme(|_: &IcedChat| Some(iced::Theme::Dark))
    .run()
    .unwrap_or_else(|err| {
        eprintln!("Failed to launch iced GUI: {err}");
        std::process::exit(1);
    });

    let _keep_runtime_alive = runtime;
    Ok(())
}
