//! Terminal UI (TUI) chat frontend using boru-chat.
//!
//! Usage: `cargo chat open` or `cargo chat join <ticket>`.
//!
//! This example uses the shared [`boru_chat::chat_core`] module for the
//! protocol types, state machine, and network event handling.  Only the
//! TUI-specific rendering (ratatui) and input handling (crossterm) live here.
//!
//! # Navigation
//!
//! | Key | Action |
//! |-----|--------|
//! | Tab | Switch between Chats / Friends / Friend Requests panels |
//! | ↑/↓ | Navigate conversation list |
//! | Enter | Open selected conversation / send message |
//! | Esc | Back to conversation list / close overlay / quit |
//! | Ctrl-C | Quit |
//! | PgUp/PgDn | Scroll chat history |
//! | F2 | Open friend requests view |
//! | F5 | Refresh |

use std::{
    collections::{HashMap, HashSet, VecDeque},
    env, io,
    net::{Ipv4Addr, SocketAddrV4},
    path::{Path, PathBuf},
    str::FromStr,
    sync::atomic::{AtomicUsize, Ordering},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use bytes::Bytes;
use clap::Parser;
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event as CEvent, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

use boru_chat::backfill::{
    BackfillHandle, BackfillProtocolHandler, BACKFILL_ALPN, BACKFILL_TRIGGER_THRESHOLD,
};
use boru_chat::chat_callbacks::TransferKind;
use boru_chat::chat_core::friend_ping::{
    FriendEvent, FriendPingManager, FriendStatus, PingHandler, DEFAULT_CONNECT_TIMEOUT,
    DEFAULT_PING_INTERVAL, FRIEND_PING_ALPN,
};
use boru_chat::chat_core::{
    collect_bootstrap_peers, download_blob_with_progress, download_candidates, fmt_relay_mode,
    handle_net_event, message_hash, refresh_bootstrap_peers, update_connection_counts, AppState,
    ChatEntry, ChatKind, ConnectionType, MeshHealth, Message, NetEvent, RoomInviteV2,
    SignedMessage, StatusContext, Ticket,
};
use boru_chat::chat_history::{ChatHistoryStore, DeliveryState, HistoryEntry};
use boru_chat::contact::direct_topic;
use boru_chat::conversations::ConversationStore;
use boru_chat::friend_request::{FriendRequest, FriendRequestStatus, FriendRequestStore};
use boru_chat::friends::{FriendId, FriendRecord, FriendsStore};
use boru_chat::inbox::{send_sync_request, InboxEvent, InboxHandle, InboxProtocol, INBOX_ALPN};
use boru_chat::mailbox::{MailboxAck, MailboxIdentity, MailboxStore};
use boru_chat::room::RoomStore;
use boru_chat::room_docs::{
    self, create_metadata_doc, create_roster_doc, list_members, read_metadata, RoomDocs,
    RoomMetadata,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, Endpoint, EndpointAddr, EndpointId,
    PublicKey, RelayMode, RelayUrl, SecretKey,
};
use iroh_blobs::{store::mem::MemStore, ticket::BlobTicket, BlobsProtocol};

use boru_chat::discovery_backend::MainlineDhtBackend;
use boru_chat::private_room_tracker::PrivateRoomTracker;
use boru_chat::public_room_continuous::{ContinuousTracker, ContinuousTrackerConfig};
use boru_chat::whisper::{WhisperBuilder, WhisperEvent, WhisperHandle, WHISPER_ALPN};
use boru_chat::{
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
};
use iroh_mainline_address_lookup::DhtAddressLookup;
use iroh_mdns_address_lookup::MdnsAddressLookup;
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
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

// ── Constants for the pinned public room ─────────────────────────────
/// Display name for the pinned public room.
const PUBLIC_ROOM_LABEL: &str = "★ Public Room";
/// Short label used in tab/status areas.
const PUBLIC_ROOM_SHORT: &str = "★Public";

// ── CLI ──────────────────────────────────────────────────────────────

/// Chat over boru-chat
#[derive(Parser, Debug)]
struct Args {
    /// secret key to derive our endpoint id from.
    #[clap(long)]
    secret_key: Option<String>,
    /// Set a custom relay server.
    #[clap(short, long)]
    relay: Option<RelayUrl>,
    /// Disable relay completely.
    #[clap(long)]
    no_relay: bool,
    /// Disable private-room DHT discovery (kept for compatibility; DHT is off by default).
    #[clap(long, conflicts_with = "dht")]
    no_dht: bool,
    /// Enable private-room DHT discovery.
    #[clap(long, conflicts_with = "no_dht")]
    dht: bool,
    /// Set your nickname.
    #[clap(short, long)]
    name: Option<String>,
    /// Set the bind port for our socket.
    #[clap(long, default_value = "0")]
    bind_port: u16,
    #[clap(subcommand)]
    command: Command,
}

#[derive(Parser, Debug)]
enum Command {
    Open {
        /// Optionally set the topic id.
        topic: Option<TopicId>,
    },
    Join {
        /// The ticket, as base32 string.
        ticket: String,
    },
}

// ── Data directory ──────────────────────────────────────────────────

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

// ── Persistent logging ───────────────────────────────────────────────

const LOG_FILE_NAME: &str = "chat.log";

fn log_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join("logs").join(LOG_FILE_NAME)
}

fn init_logging(data_dir: &Path) -> Result<()> {
    let log_path = log_file_path(data_dir);
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
    let file_writer = FileMakeWriter(Arc::new(Mutex::new(file)));
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"));
    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(file_writer).with_ansi(false));
    let _ = tracing::subscriber::set_global_default(subscriber);
    Ok(())
}

// ── TUI screen navigation ───────────────────────────────────────────

/// The active screen in the TUI.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TuiScreen {
    /// Conversation list (inbox) showing rooms and friends.
    ChatList,
    /// An individual conversation with a given topic.
    Chat { topic: TopicId },
    /// Friend request management.
    FriendRequests,
}

// ── TUI conversation state ──────────────────────────────────────────

/// Per-conversation runtime state for the TUI.
struct ConvState {
    /// Chat messages for this conversation.
    entries: Vec<ChatEntry>,
    /// Composer text.
    composer_text: String,
    /// Whether to auto-scroll to latest.
    follow_latest: bool,
    /// Scroll offset.
    scroll_offset: u16,
    /// Last rendered log height.
    last_log_height: u16,
    /// Pending file download: (filename, ticket_string).
    pending_file: Option<(String, String)>,
    /// Pending image downloads.
    pending_image: VecDeque<(String, iroh_blobs::Hash, PublicKey)>,
    /// Name cache.
    names: HashMap<PublicKey, String>,
    /// Unread count accumulated while conversation is not visible.
    unread: u64,
    /// Number of entries already saved to ChatHistoryStore.
    history_saved_count: usize,
    /// Maps content hash to stable event id for self-sent messages.
    self_sent_events: HashMap<[u8; 32], u64>,
    /// Display name for the conversation header.
    display_name: String,
}

impl ConvState {
    fn new(topic: TopicId, display_name: String) -> Self {
        Self {
            entries: Vec::new(),
            composer_text: String::new(),
            follow_latest: true,
            scroll_offset: 0,
            last_log_height: 10,
            pending_file: None,
            pending_image: VecDeque::new(),
            names: HashMap::new(),
            unread: 0,
            history_saved_count: 0,
            self_sent_events: HashMap::new(),
            display_name,
        }
    }

    fn push_entry(&mut self, entry: ChatEntry) {
        self.entries.push(entry);
        self.follow_latest = true;
    }

    fn max_scroll_offset(&self, visible_height: u16) -> u16 {
        let visible_height = visible_height as usize;
        self.entries.len().saturating_sub(visible_height) as u16
    }

    fn rendered_scroll_offset(&self, visible_height: u16) -> u16 {
        let max = self.max_scroll_offset(visible_height);
        if self.follow_latest {
            max
        } else {
            self.scroll_offset.min(max)
        }
    }

    fn scroll_up(&mut self, amount: u16, visible_height: u16) {
        let max = self.max_scroll_offset(visible_height);
        self.follow_latest = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(amount).min(max);
    }

    fn scroll_down(&mut self, amount: u16, visible_height: u16) {
        let max = self.max_scroll_offset(visible_height);
        self.scroll_offset = self.scroll_offset.saturating_add(amount).min(max);
        self.follow_latest = self.scroll_offset >= max;
    }
}

// ── TUI application state ──────────────────────────────────────────

/// Top-level TUI state, wrapping the shared model with navigation.
struct TuiState {
    // ── Navigation ──
    /// Current active screen.
    screen: TuiScreen,
    /// Index into the ordered conversation list for the ChatList screen.
    selected_conv_index: usize,
    /// Whether the help overlay is visible.
    help_visible: bool,
    /// Whether the user has requested to quit.
    should_quit: bool,

    // ── Conversation state ──
    /// Ordered list of conversation topic keys for the chat list.
    /// The public room is always first (index 0).
    conversation_order: Vec<TopicId>,
    /// Per-conversation runtime state.
    conversations: HashMap<TopicId, ConvState>,

    // ── Session state ──
    /// Connection status context.
    status: StatusContext,
    /// Whether the current room has a discovery secret (and can be found via DHT).
    room_discovery_secret_present: bool,
    /// Durable friends list store.
    friends: FriendsStore,
    /// Whether friends has unsaved changes.
    friends_dirty: bool,
    /// Durable conversation store.
    conversation_store: ConversationStore,
    /// Durable friend request store.
    friend_request_store: FriendRequestStore,

    // ── Core IDs ──
    /// Our own public key.
    local_public: PublicKey,
    /// Local display label.
    local_label: String,

    /// Display name map across all conversations.
    global_names: HashMap<PublicKey, String>,

    /// Per-conversation forward-handle slots for keeping subscriptions alive.
    forward_handles: HashMap<TopicId, n0_future::task::JoinHandle<()>>,
    /// Per-room continuous DHT trackers for private rooms with discovery enabled.
    /// The second element is the join-fanout CancellationToken.
    room_trackers: HashMap<TopicId, (ContinuousTracker, tokio_util::sync::CancellationToken)>,
}

impl TuiState {
    fn new(
        status: StatusContext,
        friends: FriendsStore,
        friend_request_store: FriendRequestStore,
        conversation_store: ConversationStore,
        local_public: PublicKey,
        local_label: String,
        public_room_topic: TopicId,
    ) -> Self {
        // Build conversation_order from the store, with public room first.
        let mut conversation_order = vec![public_room_topic];
        for entry in conversation_store.active_iter() {
            if entry.topic != public_room_topic {
                conversation_order.push(entry.topic);
            }
        }

        // Create per-conv state for each known topic.
        let mut conversations = HashMap::new();
        for &topic in &conversation_order {
            let name = if topic == public_room_topic {
                PUBLIC_ROOM_LABEL.to_string()
            } else {
                conversation_store
                    .find(&topic)
                    .map(|e| e.display_name().to_string())
                    .unwrap_or_else(|| "Chat".to_string())
            };
            conversations.insert(topic, ConvState::new(topic, name));
        }

        Self {
            screen: TuiScreen::Chat {
                topic: public_room_topic,
            },
            selected_conv_index: 0,
            help_visible: false,
            should_quit: false,
            conversation_order,
            conversations,
            status,
            room_discovery_secret_present: false,
            friends,
            friends_dirty: false,
            conversation_store,
            friend_request_store,
            local_public,
            local_label,
            global_names: HashMap::new(),
            forward_handles: HashMap::new(),
            room_trackers: HashMap::new(),
        }
    }

    /// Current conversation state (if on a Chat screen).
    fn current_conv_mut(&mut self) -> Option<&mut ConvState> {
        match &self.screen {
            TuiScreen::Chat { topic } => self.conversations.get_mut(topic),
            _ => None,
        }
    }

    /// Current conversation state (read-only).
    fn current_conv(&self) -> Option<&ConvState> {
        match &self.screen {
            TuiScreen::Chat { topic } => self.conversations.get(topic),
            _ => None,
        }
    }

    /// Push a system message to the active conversation, or to the first
    /// conversation if none is selected.
    fn push_system(&mut self, text: impl Into<String>) {
        let topic = match &self.screen {
            TuiScreen::Chat { topic } => *topic,
            _ if !self.conversation_order.is_empty() => self.conversation_order[0],
            _ => return,
        };
        if let Some(conv) = self.conversations.get_mut(&topic) {
            conv.push_entry(ChatEntry::system(text));
        }
    }

    /// Push a local (self-sent) message to the active conversation.
    fn push_local(&mut self, label: impl Into<String>, text: impl Into<String>) {
        let topic = match &self.screen {
            TuiScreen::Chat { topic } => *topic,
            _ if !self.conversation_order.is_empty() => self.conversation_order[0],
            _ => return,
        };
        if let Some(conv) = self.conversations.get_mut(&topic) {
            conv.push_entry(ChatEntry::local(label, text));
        }
    }

    /// Push a remote (received) message to a specific topic.
    fn push_remote_to(
        &mut self,
        topic: TopicId,
        label: impl Into<String>,
        text: impl Into<String>,
    ) {
        if let Some(conv) = self.conversations.get_mut(&topic) {
            conv.push_entry(ChatEntry::remote(label, text));
            // If this conversation is not currently visible, count it as unread.
            if !matches!(&self.screen, TuiScreen::Chat { topic: t } if *t == topic) {
                conv.unread += 1;
            }
        }
    }

    /// Get the display label for a peer.
    fn resolve_name(&self, peer: &PublicKey) -> String {
        let fid = FriendId::from_public_key(*peer);
        if let Some(record) = self.friends.get(&fid) {
            if let Some(label) = &record.label {
                return label.clone();
            }
            if let Some(name) = &record.last_announced_name {
                return name.clone();
            }
        }
        self.global_names
            .get(peer)
            .cloned()
            .unwrap_or_else(|| peer.fmt_short().to_string())
    }
}

// ── Shared session-level state (for backfill, forwarders, etc.) ──────

/// Context passed to event handlers that need access to shared resources.
struct SessionCtx {
    gossip: Arc<Gossip>,
    sender: Arc<tokio::sync::Mutex<Option<boru_chat::api::GossipSender>>>,
    secret_key: SecretKey,
    local_public: PublicKey,
    local_label: String,
    endpoint: Arc<Endpoint>,
    blob_store: MemStore,
    friend_mgr: Arc<FriendPingManager>,
    whisper_handle: WhisperHandle,
    chat_history: Arc<Mutex<ChatHistoryStore>>,
    backfill_handle: BackfillHandle,
    data_dir: PathBuf,
    room_docs: Arc<RwLock<Option<RoomDocs>>>,
    /// Net event channel for broadcasting to the event loop.
    net_tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
}

// ── Main ────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let data_dir = get_data_dir();
    init_logging(&data_dir)?;
    tracing::info!(path = %log_file_path(&data_dir).display(), "logging to file");
    let args = Args::parse();
    // DHT is opt-in. The default path must not construct a DHT client or
    // start any discovery tasks.
    let dht_enabled = args.dht && !args.no_dht;
    tracing::info!(dht_enabled, "private-room DHT discovery");
    // Shared atomic counter for DHT-discovered peers (updated by the
    // continuous-tracker fanout task).  Read each frame by the UI.
    let dht_peer_count: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));

    // Parse the CLI command.
    let (topic, mut peers, mut discovery_secret, room_created) = match &args.command {
        Command::Open { topic } => {
            let (topic, saved_peers, stored_secret, created) = match topic {
                Some(t) => (*t, Vec::new(), None, false),
                None => match RoomStore::load_or_none(&data_dir) {
                    Some(store) => {
                        let n_peers = store.peers.len();
                        let peer_info = if n_peers > 0 {
                            format!(" with {n_peers} saved bootstrap peer(s)")
                        } else {
                            String::new()
                        };
                        tracing::info!(topic = %store.topic, peer_info = %peer_info, "reusing saved room topic");
                        (
                            store.topic,
                            store.peers.clone(),
                            store.discovery_secret,
                            false,
                        )
                    }
                    None => {
                        let t = TopicId::from_bytes(rand::random());
                        tracing::info!(topic = %t, "opening new chat room");
                        let room = RoomStore::new(&data_dir, t);
                        if let Err(err) = room.save() {
                            tracing::warn!(error = %err, "failed to save room metadata");
                        }
                        (t, vec![], None, true)
                    }
                },
            };
            (topic, saved_peers, stored_secret, created)
        }
        Command::Join { ticket } => {
            // Try stable boru1: invitation first, then fall back to legacy ticket format.
            let (topic, peers, discovery_secret) =
                if let Ok(invite) = RoomInviteV2::parse(ticket) {
                    tracing::info!(topic = %invite.topic, "joining room via boru1 invitation");
                    (invite.topic, Vec::new(), Some(invite.discovery_secret))
                } else {
                    let Ticket {
                        topic,
                        peers,
                        discovery_secret,
                    } = Ticket::from_str(ticket)?;
                    tracing::info!(topic = %topic, "joining chat room via legacy ticket");
                    (topic, peers, discovery_secret)
                };
            (topic, peers, discovery_secret, false)
        }
    };
    let is_new_room = room_created;

    // Secret key.
    let (secret_key, key_path) = match args.secret_key.as_ref() {
        None => load_or_generate_secret_key()?,
        Some(key) => {
            let key = key.parse()?;
            (key, PathBuf::from("<passed via cli flag>"))
        }
    };
    tracing::info!(public_key = %secret_key.public(), identity_file = %key_path.display(), "loaded local identity");

    // Load stores.
    let friends = FriendsStore::load_or_default(&data_dir);
    let friend_count = friends.len();
    if friend_count > 0 {
        tracing::info!(count = friend_count, "loaded friends from disk");
    }
    let conversation_store = ConversationStore::load_or_default(&data_dir);
    let friend_request_store = FriendRequestStore::load_or_default(&data_dir);

    // Relay config.
    let relay_mode = match (args.no_relay, args.relay.clone()) {
        (true, Some(_)) => bail_any!("cannot set --no-relay and --relay at the same time"),
        (true, None) => RelayMode::Disabled,
        (false, None) => RelayMode::Default,
        (false, Some(url)) => RelayMode::Custom(url.into()),
    };
    tracing::info!(relay = %fmt_relay_mode(&relay_mode), "configured relay servers");

    let memory_lookup = MemoryLookup::new();

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
    tracing::info!(endpoint_id = %endpoint.id(), "endpoint ready");

    if let Ok(mdns) = MdnsAddressLookup::builder().build(endpoint.id()) {
        if let Ok(addr_lookup) = endpoint.address_lookup().as_ref() {
            addr_lookup.add(mdns);
        }
    }
    if dht_enabled {
        if let Ok(addr_lookup) = endpoint.address_lookup().as_ref() {
            if let Ok(dht) = DhtAddressLookup::builder()
                .secret_key(endpoint.secret_key().clone())
                .build()
            {
                addr_lookup.add(dht);
            }
        }
    }

    // ── Shared DHT client for private-room discovery ─────────────────
    // Used both for initial publish (new rooms) and for join-time
    // discovery (rooms with a discovery_secret from a ticket).
    let shared_dht = dht_enabled.then(|| {
        distributed_topic_tracker::Dht::new(&distributed_topic_tracker::DhtConfig::default())
    });

    // Will hold the continuous tracker (and its join-fanout resources) for this
    // room if DHT is enabled.  The rx is kept alive until `sender` is available,
    // then the join-fanout task is spawned.
    let mut room_tracker: Option<(
        ContinuousTracker,
        tokio::sync::mpsc::Receiver<Vec<iroh::EndpointId>>,
        tokio_util::sync::CancellationToken,
    )> = None;

    // ── Publish DHT discovery for newly created rooms ─────────────────
    if is_new_room && dht_enabled {
        let secret = boru_chat::private_room_tracker::create_and_publish_private_discovery(
            shared_dht.clone(),
            topic,
            &endpoint,
        )
        .await;
        if let Some(secret) = secret {
            discovery_secret = Some(secret);
            // Save the discovery secret to the room store.
            if let Some(mut room) = RoomStore::load_or_none(&data_dir) {
                if let Err(err) = room.set_discovery_secret(discovery_secret) {
                    tracing::warn!(error = %err, "failed to set discovery_secret in RoomStore");
                }
                if let Err(err) = room.save() {
                    tracing::warn!(error = %err, "failed to save RoomStore with discovery_secret");
                }
            }
            // Start continuous DHT publish/discover.
            let secret = secret;
            let dummy_ns = distributed_topic_tracker::TopicId::from_hash(&[0u8; 32]);
            let backend =
                MainlineDhtBackend::new(shared_dht.clone().expect("DHT enabled"), dummy_ns);
            let tracker = PrivateRoomTracker::new(
                Box::new(backend),
                topic,
                secret,
                endpoint.id(),
                endpoint.secret_key().clone(),
            );
            let (new_peers_tx, new_peers_rx) =
                tokio::sync::mpsc::channel::<Vec<iroh::EndpointId>>(64);
            let join_cancel = tokio_util::sync::CancellationToken::new();
            // DHT peer counting wrapper: forward batches to the join-fanout
            // task while counting them for the UI.
            let (fanout_tx, fanout_rx) = tokio::sync::mpsc::channel::<Vec<iroh::EndpointId>>(64);
            {
                let dht_count = dht_peer_count.clone();
                let mut rx = new_peers_rx;
                tokio::spawn(async move {
                    while let Some(peers) = rx.recv().await {
                        dht_count.fetch_add(peers.len(), Ordering::Relaxed);
                        tracing::info!(count = peers.len(), "DHT discovered new peers");
                        if fanout_tx.send(peers).await.is_err() {
                            break;
                        }
                    }
                });
            }
            room_tracker = Some((
                ContinuousTracker::start(
                    tracker.into_inner(),
                    ContinuousTrackerConfig::default(),
                    new_peers_tx,
                ),
                fanout_rx,
                join_cancel,
            ));
            tracing::info!("published DHT discovery for new private room");
        }
    }

    // ── DHT discovery for private-room tickets ──────────────────────
    // If the ticket includes a discovery secret, attempt to find additional
    // peers via the DHT before subscribing.  Non-fatal errors are silently
    // downgraded to a fallback (ticket peers only).
    let ticket_addrs: Vec<EndpointAddr> = peers.clone();
    if dht_enabled {
        if let Some(ref secret) = discovery_secret {
            let dummy_ns = distributed_topic_tracker::TopicId::from_hash(&[0u8; 32]);
            let backend =
                MainlineDhtBackend::new(shared_dht.clone().expect("DHT enabled"), dummy_ns);
            let tracker = PrivateRoomTracker::new(
                Box::new(backend),
                topic,
                secret.clone(),
                endpoint.id(),
                endpoint.secret_key().clone(),
            );
            match tracker.discover_once().await {
                Ok(discovered_ids) => {
                    let existing: HashSet<EndpointId> = peers.iter().map(|a| a.id).collect();
                    for id in discovered_ids {
                        if !existing.contains(&id) && id != endpoint.id() {
                            peers.push(EndpointAddr::new(id));
                        }
                    }
                    tracing::info!(
                        peer_count = peers.len(),
                        "DHT discovery merged additional peers"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "DHT discovery failed, falling back to ticket peers only"
                    );
                }
            }
            // Start continuous DHT publish/discover instead of shutting down.
            let (new_peers_tx, new_peers_rx) =
                tokio::sync::mpsc::channel::<Vec<iroh::EndpointId>>(64);
            let join_cancel = tokio_util::sync::CancellationToken::new();
            // DHT peer counting wrapper (see above).
            let (fanout_tx, fanout_rx) = tokio::sync::mpsc::channel::<Vec<iroh::EndpointId>>(64);
            {
                let dht_count = dht_peer_count.clone();
                let mut rx = new_peers_rx;
                tokio::spawn(async move {
                    while let Some(peers) = rx.recv().await {
                        dht_count.fetch_add(peers.len(), Ordering::Relaxed);
                        tracing::info!(count = peers.len(), "DHT discovered new peers");
                        if fanout_tx.send(peers).await.is_err() {
                            break;
                        }
                    }
                });
            }
            room_tracker = Some((
                ContinuousTracker::start(
                    tracker.into_inner(),
                    ContinuousTrackerConfig::default(),
                    new_peers_tx,
                ),
                fanout_rx,
                join_cancel,
            ));
        } else {
            tracing::debug!("legacy room/ticket has no discovery secret; skipping private DHT");
        }
    } else {
        tracing::debug!("private room DHT disabled by --no-dht; using ticket peers");
    }

    let gossip = Gossip::builder().spawn(endpoint.clone());
    let blob_store = MemStore::new();
    let blobs_protocol = BlobsProtocol::new(&blob_store, None);

    let ticket = Ticket {
        topic,
        peers: vec![endpoint.addr()],
        discovery_secret,
    };
    tracing::info!(ticket = %ticket, "created room ticket");

    // Also create a stable boru1: invitation for the room.
    let boru1_invite = ticket.discovery_secret.map(|secret| {
        let invite = RoomInviteV2::new(topic, secret);
        let invite_str = invite.encode();
        tracing::info!(invite = %invite_str, "created boru1 room invitation");
        invite_str
    });

    let whisper_builder = WhisperBuilder::new(endpoint.clone(), endpoint.secret_key().clone());
    let whisper_handler = whisper_builder.protocol_handler();
    let (whisper_handle, mut whisper_events) = whisper_builder.spawn();

    let (inbox_handle, mut inbox_events) = InboxHandle::new();
    let inbox_handler =
        InboxProtocol::new(inbox_handle.inner()).with_secret_key(endpoint.secret_key().clone());
    {
        let mailbox_dir = data_dir.clone();
        inbox_handle
            .set_pending_fn(Some(Arc::new(move |requester, _since_ms| {
                let mut store = MailboxStore::load(&mailbox_dir)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| MailboxStore::empty_at(&mailbox_dir));
                store.pending_for_recipient(requester)
            })))
            .await;
    }

    let chat_history = ChatHistoryStore::load_or_default(&data_dir);
    if !chat_history.is_empty() {
        tracing::info!(
            count = chat_history.len(),
            "retained active-session chat messages in memory"
        );
    }
    let chat_history = Arc::new(Mutex::new(chat_history));
    let backfill_handler = BackfillProtocolHandler::new(chat_history.clone());
    let backfill_handle = BackfillHandle::spawn(endpoint.clone());

    let router = iroh::protocol::Router::builder(endpoint.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .accept(iroh_blobs::ALPN, blobs_protocol.clone())
        .accept(FRIEND_PING_ALPN, PingHandler)
        .accept(WHISPER_ALPN, whisper_handler)
        .accept(BACKFILL_ALPN, backfill_handler)
        .accept(INBOX_ALPN, inbox_handler)
        .spawn();

    let inbox_topic = InboxHandle::inbox_topic(endpoint.secret_key().public());
    if let Err(e) = gossip.subscribe(inbox_topic, Vec::new()).await {
        tracing::warn!(error = %e, "failed to subscribe to inbox topic");
    }
    tracing::info!("subscribed to personal inbox topic");

    let (peer_ids, _addr_material) = {
        let room_peers = RoomStore::load_or_none(get_data_dir())
            .map(|s| s.peers)
            .unwrap_or_default();
        collect_bootstrap_peers([&peers[..], &room_peers[..]])
    };
    let peer_count = peer_ids.len();

    // Seed MemoryLookup from ticket addresses only (DHT returns IDs, not addrs).
    for addr in &ticket_addrs {
        memory_lookup.set_endpoint_info(addr.clone());
    }

    let sub = if peer_ids.is_empty() {
        tracing::info!("waiting for peers to join us");
        gossip.subscribe(topic, peer_ids.clone()).await
    } else {
        tracing::info!(count = peer_count, "trying to connect to peers");
        let timeout_result = tokio::time::timeout(Duration::from_secs(30), async {
            gossip.subscribe_and_join(topic, peer_ids.clone()).await
        })
        .await;
        match timeout_result {
            Ok(result) => result,
            Err(_) => {
                bail_any!(
                    "timed out after 30s waiting for bootstrap peer(s) — \
                     the ticket or saved addresses may be stale; the room is \
                     still subscribed, so any peer that connects later will work"
                )
            }
        }
    };
    let sub = match sub {
        Ok(topic) => topic,
        Err(e) => bail_any!("failed to join gossip topic: {e}"),
    };
    let (sender, receiver) = sub.split();
    tracing::info!("connected");

    // Spawn join-fanout task for any previously-created continuous tracker,
    // so discovered peers are automatically joined into the gossip mesh.
    let mut room_tracker_with_cancel: Option<(
        ContinuousTracker,
        tokio_util::sync::CancellationToken,
    )> = None;
    if let Some((tracker, new_peers_rx, join_cancel)) = room_tracker.take() {
        let _join_task = boru_chat::public_room_continuous::spawn_join_fanout(
            new_peers_rx,
            sender.clone(),
            join_cancel.clone(),
        );
        room_tracker_with_cancel = Some((tracker, join_cancel));
    }

    {
        if let Some(mut room) = RoomStore::load_or_none(get_data_dir()) {
            let mut neighbor_set: HashSet<_> = peer_ids.iter().copied().collect();
            neighbor_set.insert(endpoint.id());
            if refresh_bootstrap_peers(&mut room, &neighbor_set, &endpoint).await {
                if let Err(err) = room.save() {
                    tracing::warn!(error = %err, "failed to save refreshed bootstrap peers");
                } else {
                    tracing::info!(
                        count = room.peers.len(),
                        "refreshed bootstrap peers for future reconnections"
                    );
                }
            }
        }
    }

    let local_public = endpoint.secret_key().public();
    let local_label = args
        .name
        .clone()
        .unwrap_or_else(|| local_public.fmt_short().to_string());

    if let Some(name) = args.name.clone() {
        let message = Message::AboutMe {
            name,
            profile_image_ticket: None,
        };
        let encoded_message = SignedMessage::sign_and_encode(endpoint.secret_key(), &message)?;
        sender.broadcast(encoded_message).await?;
    }

    // ── Public room topic ───────────────────────────────────────────
    // Derive a stable public-room topic from the local identity.
    let public_room_topic = direct_topic(&local_public, &local_public);

    // Build the TUI state.
    let status_ctx = StatusContext {
        transport_status: "> Direct iroh transport is ready.".to_string(),
        topic,
        relay_mode: relay_mode.clone(),
        connected: true,
        peer_count,
        identity_label: local_label.clone(),
        transport_notice:
            "Direct iroh transport is operational. Gossip messages use standard iroh connectivity."
                .to_string(),
        direct_peers: 0,
        relayed_peers: 0,
        neighbors: HashSet::new(),
        peer_connection_types: HashMap::new(),
        last_activity: HashMap::new(),
        mesh_health: MeshHealth::Good,
        dht_enabled,
        dht_peer_count: dht_peer_count.load(Ordering::Relaxed),
    };

    let mut tui = TuiState::new(
        status_ctx,
        friends,
        friend_request_store,
        conversation_store,
        local_public,
        local_label.clone(),
        public_room_topic,
    );
    tui.room_discovery_secret_present = discovery_secret.is_some();

    // Store the continuous DHT tracker if one was created.
    if let Some((tracker, join_cancel)) = room_tracker_with_cancel {
        tui.room_trackers.insert(topic, (tracker, join_cancel));
    }

    // Set up the main room conversation.
    {
        let has_room_conv = tui.conversation_order.contains(&topic);
        if !has_room_conv {
            tui.conversation_order.push(topic);
            let display_name = format!("Room: {}", topic.fmt_short());
            let conv = ConvState::new(topic, display_name);
            tui.conversations.insert(topic, conv);
        }
        tui.screen = TuiScreen::ChatList;
        tui.selected_conv_index = 0;
    }

    tui.push_system(format!("Ticket to join this room: {ticket}"));
    if let Some(invite_str) = &boru1_invite {
        tui.push_system(format!("Invite to join this room (boru1): {invite_str}"));
    }
    if peers.is_empty() {
        tui.push_system("Waiting for peers to join us...");
    } else {
        tui.push_system(format!(
            "Trying to connect to {} peers from the ticket...",
            peers.len()
        ));
    }
    tui.push_system("Controls: Tab switch view • Enter send • F2 friend requests • PgUp/PgDn scroll • Esc/Ctrl-C quit");

    if let Some(name) = args.name.clone() {
        tui.push_system(format!("You announced yourself as {name}."));
    }

    // Load chat history into the main room conversation.
    if let Some(conv) = tui.conversations.get_mut(&topic) {
        let history = chat_history.lock().unwrap();
        for entry in history.entries() {
            if entry.topic == topic {
                let kind = if entry.sender == hex::encode(local_public.as_bytes()) {
                    ChatKind::Local
                } else if entry.kind == "system" || entry.sender.is_empty() {
                    ChatKind::System
                } else {
                    ChatKind::Remote
                };
                let label = match kind {
                    ChatKind::Local => local_label.clone(),
                    ChatKind::System => "System".to_string(),
                    ChatKind::Remote => {
                        let s = if entry.sender.len() > 8 {
                            format!(
                                "..{}",
                                &entry.sender[entry.sender.len().saturating_sub(8)..]
                            )
                        } else {
                            entry.sender.clone()
                        };
                        s
                    }
                };
                conv.entries.push(ChatEntry {
                    kind,
                    label,
                    body: entry.text_preview.clone(),
                    message_hash: None,
                    edited: false,
                    reactions: Vec::new(),
                    event_id: entry.event_id,
                    delivery_state: entry.delivery_state.clone(),
                    timestamp: Some(entry.timestamp),
                });
            }
        }
        conv.history_saved_count = conv.entries.len();
    }

    let (net_tx, mut net_rx) = tokio::sync::mpsc::unbounded_channel();

    // Room docs setup.
    let initial_metadata = RoomMetadata {
        name: Some("boru-chat".to_string()),
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
    let room_docs = Arc::new(RwLock::new(Some(RoomDocs {
        metadata: metadata_doc.clone(),
        roster: roster_doc.clone(),
        topic,
    })));

    let room_forwarder_net_tx = net_tx.clone();
    task::spawn(async move {
        room_docs::forward_room_events_for_chat(
            metadata_doc,
            roster_doc,
            receiver,
            room_forwarder_net_tx,
            None,
        )
        .await;
    });

    if tui.friends.is_empty() {
        tui.push_system("No friends file yet; starting with an empty friends list.");
    } else {
        tui.push_system(format!(
            "Loaded {} friends from {}.",
            tui.friends.len(),
            tui.friends.file_path().display()
        ));
    }

    let _terminal_guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    terminal.draw(|frame| render_app(frame, &mut tui))?;

    // Friend ping manager.
    let (friend_mgr, mut friend_events) = FriendPingManager::spawn(
        endpoint.clone(),
        DEFAULT_PING_INTERVAL,
        DEFAULT_CONNECT_TIMEOUT,
    );
    for peer in tui
        .friends
        .iter()
        .filter_map(|(id, _)| id.parse_public_key().ok())
    {
        let addrs = tui
            .friends
            .get(&FriendId::from_public_key(peer))
            .map(|record| record.known_addrs.clone())
            .unwrap_or_default();
        let _ = friend_mgr.add_friend_addrs(peer, addrs).await;
    }

    let (ui_tx, mut ui_rx) = tokio::sync::mpsc::unbounded_channel();
    spawn_input_thread(ui_tx);

    let mut conn_monitor = tokio::time::interval(Duration::from_secs(60));
    conn_monitor.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut last_mesh_health: Option<MeshHealth> = None;
    let mut mesh_watchdog = tokio::time::interval(Duration::from_secs(30));
    mesh_watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let presence_sender = sender.clone();
    let presence_secret_key = endpoint.secret_key().clone();
    tokio::spawn(async move {
        let mut presence_interval = tokio::time::interval(Duration::from_secs(5));
        presence_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            presence_interval.tick().await;
            let msg = Message::Presence;
            if let Ok(encoded) = SignedMessage::sign_and_encode(&presence_secret_key, &msg) {
                if presence_sender.broadcast(encoded).await.is_err() {
                    break;
                }
            }
        }
    });

    let heartbeat_sender = sender.clone();
    let heartbeat_secret_key = endpoint.secret_key().clone();
    tokio::spawn(async move {
        let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(2));
        heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            heartbeat_interval.tick().await;
            let msg = Message::Heartbeat;
            if let Ok(encoded) = SignedMessage::sign_and_encode(&heartbeat_secret_key, &msg) {
                if heartbeat_sender.broadcast(encoded).await.is_err() {
                    break;
                }
            }
        }
    });

    // Share context for event handlers.
    let session = Arc::new(SessionCtx {
        gossip: Arc::new(gossip.clone()),
        sender: Arc::new(tokio::sync::Mutex::new(Some(sender.clone()))),
        secret_key: secret_key.clone(),
        local_public,
        local_label: local_label.clone(),
        endpoint: Arc::new(endpoint.clone()),
        blob_store: blob_store.clone(),
        friend_mgr: Arc::new(friend_mgr.clone()),
        whisper_handle: whisper_handle.clone(),
        chat_history: chat_history.clone(),
        backfill_handle: backfill_handle.clone(),
        data_dir: data_dir.clone(),
        room_docs: room_docs.clone(),
        net_tx: net_tx.clone(),
    });

    // ── Main event loop ──────────────────────────────────────────────
    while !tui.should_quit {
        tokio::select! {
            Some(event) = ui_rx.recv() => {
                let redraw = handle_ui_event(
                    event,
                    &mut tui,
                    &sender,
                    endpoint.secret_key(),
                    &local_label,
                    &endpoint,
                    &blob_store,
                    &friend_mgr,
                    &room_docs,
                    &whisper_handle,
                    &chat_history,
                    topic,
                ).await?;
                if tui.friends_dirty {
                    if let Err(err) = tui.friends.save() {
                        tui.push_system(format!("Failed to save friends: {err}"));
                    }
                    tui.friends_dirty = false;
                }
                if redraw {
                    terminal.draw(|frame| render_app(frame, &mut tui))?;
                }
                // Persist new entries to history store.
                persist_new_entries(&mut tui, &chat_history, &local_public, topic);
            }
            Some(event) = net_rx.recv() => {
                handle_net_event_loop(&event, &mut tui, &chat_history, &local_public, &sender, &secret_key, &endpoint, &blob_store, &backfill_handle, &net_tx).await?;
                if tui.friends_dirty {
                    if let Err(err) = tui.friends.save() {
                        tui.push_system(format!("Failed to save friends: {err}"));
                    }
                    tui.friends_dirty = false;
                }
                terminal.draw(|frame| render_app(frame, &mut tui))?;
            }
            Some(event) = friend_events.recv() => {
                handle_friend_event(event, &mut tui);
                update_connection_counts(&endpoint, &mut tui.status).await;
                tui.status.recompute_mesh_health(&endpoint).await;
                if tui.friends_dirty {
                    if let Err(err) = tui.friends.save() {
                        tui.push_system(format!("Failed to save friends: {err}"));
                    }
                    tui.friends_dirty = false;
                }
                terminal.draw(|frame| render_app(frame, &mut tui))?;
            }
            Some(event) = whisper_events.recv() => {
                handle_whisper_event_loop(event, &mut tui, &session, &data_dir, &secret_key, &endpoint, &sender).await;
                terminal.draw(|frame| render_app(frame, &mut tui))?;
            }
            Some(event) = inbox_events.recv() => {
                handle_inbox_event_loop(&endpoint, event, &mut tui, &data_dir, &secret_key, &sender).await;
                terminal.draw(|frame| render_app(frame, &mut tui))?;
            }
            _ = conn_monitor.tick() => {
                update_connection_counts(&endpoint, &mut tui.status).await;
                tui.status.recompute_mesh_health(&endpoint).await;
                tui.status.dht_peer_count = dht_peer_count.load(Ordering::Relaxed);
            }
            _ = mesh_watchdog.tick() => {
                tui.status.recompute_mesh_health(&endpoint).await;
                if let Some(notification) = tui.status.check_mesh_quiescence(&mut last_mesh_health) {
                    tui.push_system(notification);
                    terminal.draw(|frame| render_app(frame, &mut tui))?;
                }
            }
            else => break,
        }
    }

    // Shutdown all per-room continuous DHT trackers.
    for (_topic, (tracker, join_cancel)) in tui.room_trackers.drain() {
        join_cancel.cancel();
        tracker.shutdown().await;
    }

    // Save state before exit.
    if let Ok(tracked) = friend_mgr.list_friends().await {
        for (peer, status) in tracked {
            let id = FriendId::from_public_key(peer);
            let rec = tui.friends.ensure_friend(id);
            rec.status.online = status.is_online();
        }
    }
    if tui.friends_dirty {
        let _ = tui.friends.save();
    }
    if let Err(err) = chat_history.lock().unwrap().save() {
        tracing::warn!(error = %err, "failed to save chat history");
    }
    if let Err(err) = tui.conversation_store.save() {
        tracing::warn!(error = %err, "failed to save conversation store");
    }
    if let Err(err) = tui.friend_request_store.save() {
        tracing::warn!(error = %err, "failed to save friend request store");
    }

    drop(backfill_handle);
    drop(whisper_handle);
    drop(friend_mgr);
    drop(whisper_events);
    drop(friend_events);

    router.shutdown().await.anyerr()?;
    endpoint.close().await;
    Ok(())
}

// ── Terminal guard ────────────────────────────────────────────────────

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

// ── UI event types ────────────────────────────────────────────────────

#[derive(Debug)]
enum UiEvent {
    Key(KeyEvent),
    Resize,
    Paste(String),
    /// Switch the active screen.
    SwitchScreen(TuiScreen),
    /// Select a conversation by index in the list.
    SelectConv(usize),
    /// Accept a friend request by id.
    AcceptFriendRequest(String),
    /// Decline a friend request by id.
    DeclineFriendRequest(String),
    /// Send a friend request to this peer.
    SendFriendRequest(String),
}

fn spawn_input_thread(ui_tx: tokio::sync::mpsc::UnboundedSender<UiEvent>) {
    std::thread::spawn(move || {
        while let Ok(event) = event::read() {
            let keep_running = match event {
                CEvent::Key(key) => {
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

// ── Persistence helpers ──────────────────────────────────────────────

/// Persist new entries from the given conversation to the chat history store.
fn persist_new_entries(
    tui: &mut TuiState,
    chat_history: &Arc<Mutex<ChatHistoryStore>>,
    local_public: &PublicKey,
    current_topic: TopicId,
) {
    if let Some(conv) = tui.current_conv_mut() {
        if conv.history_saved_count < conv.entries.len() {
            let local_hex = hex::encode(local_public.as_bytes());
            let mut store = chat_history.lock().unwrap();
            for entry in &conv.entries[conv.history_saved_count..] {
                if entry.event_id > 0 {
                    continue;
                }
                let kind = match entry.kind {
                    ChatKind::System => "system",
                    ChatKind::Local => "text",
                    ChatKind::Remote => "text",
                };
                let sender = match entry.kind {
                    ChatKind::Local => local_hex.clone(),
                    _ => String::new(),
                };
                store.push(HistoryEntry::new(
                    current_topic,
                    sender,
                    Vec::new(),
                    kind,
                    entry.body.clone(),
                ));
            }
            conv.history_saved_count = conv.entries.len();
            drop(store);
            if let Err(err) = chat_history.lock().unwrap().save() {
                tracing::warn!(error = %err, "failed to save chat history");
            }
        }
    }
}

// ── Net event handling (extracted for select! clarity) ────────────────

#[allow(clippy::too_many_arguments)]
async fn handle_net_event_loop(
    event: &NetEvent,
    tui: &mut TuiState,
    chat_history: &Arc<Mutex<ChatHistoryStore>>,
    local_public: &PublicKey,
    sender: &boru_chat::api::GossipSender,
    secret_key: &SecretKey,
    endpoint: &Endpoint,
    blob_store: &MemStore,
    backfill_handle: &BackfillHandle,
    net_tx: &tokio::sync::mpsc::UnboundedSender<NetEvent>,
) -> Result<()> {
    // ── Echo handling: our own messages returning via gossip ──
    if let NetEvent::Message {
        from, ref message, ..
    } = event
    {
        if *from == *local_public {
            let msg_hash = message_hash(message);
            if let Some(conv) = tui.current_conv_mut() {
                if let Some(entry) = conv
                    .entries
                    .iter_mut()
                    .find(|e| e.message_hash == Some(msg_hash))
                {
                    if entry.delivery_state == DeliveryState::Sent {
                        entry.delivery_state = DeliveryState::Delivered;
                        if entry.event_id > 0 {
                            let mut store = chat_history.lock().unwrap();
                            let _ = store
                                .update_delivery_state(entry.event_id, DeliveryState::Delivered);
                        }
                    }
                }
            }
        }
    }

    // ── Process event through standard handler ──
    // Use a temporary AppState wrapper for handle_net_event.
    let mut app = AppState::new(
        tui.status.clone(),
        tui.friends.clone(),
        tui.local_public,
        Some(tui.local_label.clone()),
    );
    // Sync global names.
    for (pk, name) in &tui.global_names {
        app.names.insert(*pk, name.clone());
    }
    // Sync entries from the current conversation if on a chat screen.
    if let Some(conv) = tui.current_conv() {
        app.entries = conv.entries.clone();
        app.self_sent_events = conv.self_sent_events.clone();
    }

    handle_net_event(event.clone(), &mut app)?;

    // Sync back.
    tui.status = app.status.clone();
    tui.friends = app.friends.clone();
    tui.friends_dirty = app.friends_dirty;
    tui.global_names = app.names.clone();
    if let Some(conv) = tui.current_conv_mut() {
        conv.entries = app.entries;
        conv.self_sent_events = app.self_sent_events;
    }

    // ── NeighborUp → reconnection replay ──
    if let NetEvent::NeighborUp { peer } = event {
        let peer_owned = *peer;
        let pending: Vec<(u64, Bytes)> = {
            let store = chat_history.lock().unwrap();
            let ids: Vec<u64> = store
                .entries()
                .iter()
                .filter(|e| {
                    matches!(
                        e.delivery_state,
                        DeliveryState::Queued | DeliveryState::Sent
                    )
                })
                .map(|e| e.event_id)
                .collect();
            let mut result = Vec::new();
            for eid in &ids {
                if let Some(entry) = store.get_by_event_id(*eid) {
                    if let Ok(pk) = iroh::PublicKey::from_str(&entry.sender) {
                        if pk == *local_public && !entry.signed_bytes.is_empty() {
                            result.push((*eid, Bytes::from(entry.signed_bytes.clone())));
                        }
                    }
                }
            }
            result
        };
        let mut replayed_count = 0u32;
        for (eid, raw) in &pending {
            if sender.broadcast(raw.clone()).await.is_ok() {
                let _ = chat_history
                    .lock()
                    .unwrap()
                    .update_delivery_state(*eid, DeliveryState::Sent);
                replayed_count += 1;
            }
        }
        if replayed_count > 0 {
            tracing::info!(
                count = replayed_count,
                "replayed pending messages on reconnection"
            );
        }
        if chat_history.lock().unwrap().len() < BACKFILL_TRIGGER_THRESHOLD {
            let handle = backfill_handle.clone();
            let endpoint = endpoint.clone();
            let net_tx = net_tx.clone();
            let local_history_count = chat_history.lock().unwrap().len();
            tokio::spawn(async move {
                let _ = handle
                    .try_backfill_from_peer(
                        &endpoint,
                        peer_owned,
                        local_history_count,
                        net_tx,
                        None,
                    )
                    .await;
            });
        }
    }

    // ── NeighborDown → mark pending as Failed ──
    if let NetEvent::NeighborDown { .. } = event {
        let failed_ids: Vec<u64> = {
            let mut store = chat_history.lock().unwrap();
            store
                .entries
                .iter_mut()
                .filter(|e| {
                    matches!(
                        e.delivery_state,
                        DeliveryState::Queued | DeliveryState::Sent
                    )
                })
                .map(|e| {
                    e.delivery_state = DeliveryState::Failed;
                    e.event_id
                })
                .collect()
        };
        if let Some(conv) = tui.current_conv_mut() {
            for ui_entry in conv.entries.iter_mut() {
                if failed_ids.contains(&ui_entry.event_id) {
                    ui_entry.delivery_state = DeliveryState::Failed;
                }
            }
        }
    }

    // ── ReadReceipt handling ──
    if let NetEvent::Message {
        message: Message::ReadReceipt {
            message_hash: receipt_hash,
        },
        from: receipt_from,
        ..
    } = event
    {
        if *receipt_from != *local_public {
            if let Some(conv) = tui.current_conv_mut() {
                if let Some(entry) = conv
                    .entries
                    .iter_mut()
                    .find(|e| e.message_hash == Some(*receipt_hash))
                {
                    if entry.delivery_state.can_transition_to(&DeliveryState::Seen) {
                        entry.delivery_state = DeliveryState::Seen;
                        if entry.event_id > 0 {
                            let mut store = chat_history.lock().unwrap();
                            let _ =
                                store.update_delivery_state(entry.event_id, DeliveryState::Seen);
                        }
                    }
                }
            }
        }
    }

    // ── Seen trigger: send ReadReceipt when viewing remote messages ──
    if let Some(conv) = tui.current_conv() {
        if conv.follow_latest {
            if let NetEvent::Message {
                message: Message::Message { .. },
                from: msg_from,
                ..
            } = event
            {
                if *msg_from != *local_public {
                    if let NetEvent::Message { ref message, .. } = event {
                        let msg_hash = message_hash(message);
                        let receipt = Message::ReadReceipt {
                            message_hash: msg_hash,
                        };
                        if let Ok(encoded) = SignedMessage::sign_and_encode(secret_key, &receipt) {
                            let _ = sender.broadcast(encoded).await;
                        }
                    }
                }
            }
        }
    }

    // ── Auto-download pending images ──
    if let Some(conv) = tui.current_conv_mut() {
        let pending_images: Vec<_> = conv.pending_image.drain(..).collect();
        for (name, hash, sender_pk) in pending_images {
            let candidates = download_candidates(sender_pk, &tui.status.neighbors);
            match download_blob_with_progress(
                blob_store,
                endpoint,
                hash,
                candidates,
                name.clone(),
                TransferKind::Image,
                |_| {},
                None,
            )
            .await
            {
                Ok(_) => {
                    tui.push_system(format!("Downloaded image: {name}"));
                }
                Err(err) => {
                    tui.push_system(format!("Failed to download image '{name}': {err}"));
                }
            }
        }
    }
    update_connection_counts(endpoint, &mut tui.status).await;
    tui.status.recompute_mesh_health(endpoint).await;

    // Persist new entries.
    let current_topic = match tui.screen {
        TuiScreen::Chat { topic } => topic,
        _ => return Ok(()),
    };
    if let Some(conv) = tui.current_conv_mut() {
        if conv.history_saved_count < conv.entries.len() {
            let local_hex = hex::encode(local_public.as_bytes());
            let mut store = chat_history.lock().unwrap();
            for entry in &conv.entries[conv.history_saved_count..] {
                if entry.event_id > 0 {
                    continue;
                }
                let kind = match entry.kind {
                    ChatKind::System => "system",
                    ChatKind::Local => "text",
                    ChatKind::Remote => "text",
                };
                let sender = match entry.kind {
                    ChatKind::Local => local_hex.clone(),
                    _ => String::new(),
                };
                store.push(HistoryEntry::new(
                    current_topic,
                    sender,
                    Vec::new(),
                    kind,
                    entry.body.clone(),
                ));
            }
            conv.history_saved_count = conv.entries.len();
            drop(store);
            if let Err(err) = chat_history.lock().unwrap().save() {
                tracing::warn!(error = %err, "failed to save chat history");
            }
        }
    }

    // ── Seen-on-visibility ──
    if let Some(conv) = tui.current_conv_mut() {
        if conv.follow_latest {
            let mut store = chat_history.lock().unwrap();
            for ui_entry in conv.entries.iter_mut() {
                if ui_entry.delivery_state == DeliveryState::Delivered && ui_entry.event_id > 0 {
                    ui_entry.delivery_state = DeliveryState::Seen;
                    let _ = store.update_delivery_state(ui_entry.event_id, DeliveryState::Seen);
                }
            }
        }
    }

    Ok(())
}

// ── Friend event handling ─────────────────────────────────────────────

fn handle_friend_event(event: FriendEvent, tui: &mut TuiState) {
    match event {
        FriendEvent::StatusChanged { peer, status } => {
            let fid = FriendId::from_public_key(peer);
            let label = tui
                .friends
                .get(&fid)
                .and_then(|r| r.display_label(&fid).into())
                .unwrap_or_else(|| peer.fmt_short().to_string());
            let has_been_seen = tui
                .friends
                .get(&fid)
                .map(|r| {
                    r.status.last_seen_at_unix_ms.is_some()
                        || r.status.last_offline_at_unix_ms.is_some()
                })
                .unwrap_or(false);
            match status {
                FriendStatus::Online => {
                    tui.friends.mark_online(fid);
                    tui.friends_dirty = true;
                    if has_been_seen {
                        tui.push_system(format!("Friend {label} is now ONLINE"));
                    }
                }
                FriendStatus::Offline => {
                    tui.friends.mark_offline(fid);
                    tui.friends_dirty = true;
                    if has_been_seen {
                        tui.push_system(format!("Friend {label} is now offline"));
                    }
                }
                FriendStatus::Unknown => {}
            }
        }
        FriendEvent::AddressUpdated { peer, addr } => {
            tui.friends
                .ensure_friend(FriendId::from_public_key(peer))
                .record_addrs([addr]);
            tui.friends_dirty = true;
        }
    }
}

// ── Whisper event handling ─────────────────────────────────────────────

async fn handle_whisper_event_loop(
    event: WhisperEvent,
    tui: &mut TuiState,
    session: &Arc<SessionCtx>,
    data_dir: &Path,
    secret_key: &SecretKey,
    endpoint: &Endpoint,
    sender: &boru_chat::api::GossipSender,
) {
    // Handle Connected events for mailbox sync.
    let (is_connected, connected_peer) = match &event {
        WhisperEvent::Connected { peer } => (true, Some(*peer)),
        _ => (false, None),
    };

    // Non-Connected events: use standard handler via AppState wrapper.
    if !is_connected {
        let mut app = AppState::new(
            tui.status.clone(),
            tui.friends.clone(),
            tui.local_public,
            Some(tui.local_label.clone()),
        );
        app.names = tui.global_names.clone();
        if let Some(conv) = tui.current_conv() {
            app.entries = conv.entries.clone();
        }

        // Handle the event using the shared handler pattern from chat.rs.
        match event {
            WhisperEvent::Message { from, content } => {
                let text = String::from_utf8_lossy(&content).to_string();
                let label = tui.resolve_name(&from);
                if text == "\x00PRIVATE_CHAT" {
                    tui.push_system(format!("{label} opened a private chat with you."));
                } else {
                    tui.push_remote_to(tui.status.topic, format!("Whisper from {label}"), text);
                }
            }
            WhisperEvent::FileTransfer { from, name, ticket } => {
                let label = tui.resolve_name(&from);
                tui.push_system(format!(
                    "[Whisper from {label}] File received: {name}. Use /download to fetch."
                ));
                if let Some(conv) = tui.current_conv_mut() {
                    conv.pending_file = Some((name, ticket));
                }
            }
            WhisperEvent::Disconnected { peer } => {
                let label = tui.resolve_name(&peer);
                tui.push_system(format!("[Whisper] Disconnected from {label}"));
            }
            WhisperEvent::Control { .. } => {}
            WhisperEvent::MailboxEnvelope { .. } => {}
            WhisperEvent::MailboxAck { .. } => {}
            _ => {}
        }
    }

    if let Some(peer) = connected_peer {
        let label = tui.resolve_name(&peer);
        tui.push_system(format!("[Whisper] Connected to {label}"));

        let has_mailbox = tui
            .friends
            .get(&FriendId::from_public_key(peer))
            .and_then(|r| r.mailbox_public_key)
            .is_some();
        if has_mailbox {
            match send_sync_request(endpoint, secret_key, peer, 0).await {
                Ok(envelopes) => {
                    let mut store =
                        MailboxStore::load(data_dir)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| {
                                MailboxStore::for_recipient(data_dir, secret_key.public())
                            });
                    let identity = MailboxIdentity::from_secret(secret_key);
                    for env in envelopes {
                        match store.accept_incoming(&identity, env, &[peer]) {
                            Ok((_msg_id, plaintext)) => {
                                if let Ok(text) = String::from_utf8(plaintext) {
                                    tui.push_system(format!("[Offline DM from {label}] {text}"));
                                }
                                let _ = store.save();
                                let ack = MailboxAck::sign(secret_key, &_msg_id);
                                let _ = boru_chat::inbox::send_ack(endpoint, secret_key, peer, ack)
                                    .await;
                            }
                            Err(e) => {
                                tui.push_system(format!(
                                    "[Mailbox] Failed to accept replayed envelope from {label}: {e}"
                                ));
                            }
                        }
                    }
                }
                Err(e) => {
                    tui.push_system(format!("[Mailbox] Failed to sync with {label}: {e}"));
                }
            }
        }
    }
}

// ── Inbox event handling ───────────────────────────────────────────────

async fn handle_inbox_event_loop(
    endpoint: &Endpoint,
    event: InboxEvent,
    tui: &mut TuiState,
    data_dir: &Path,
    secret_key: &SecretKey,
    sender: &boru_chat::api::GossipSender,
) {
    match event {
        InboxEvent::EnvelopeReceived { from, envelope } => {
            let label = tui.resolve_name(&from);
            let mut store = match MailboxStore::load(data_dir)
                .ok()
                .flatten()
                .unwrap_or_else(|| MailboxStore::for_recipient(data_dir, secret_key.public()))
            {
                s => s,
            };
            let identity = MailboxIdentity::from_secret(secret_key);
            match store.accept_incoming(&identity, envelope, &[from]) {
                Ok((_msg_id, plaintext)) => {
                    if let Ok(text) = String::from_utf8(plaintext) {
                        tui.push_system(format!("[Offline DM from {label}] {text}"));
                    }
                    let _ = store.save();
                    let ack = MailboxAck::sign(secret_key, &_msg_id);
                    let _ = boru_chat::inbox::send_ack(endpoint, secret_key, from, ack).await;
                }
                Err(e) => {
                    tui.push_system(format!(
                        "[Mailbox] Failed to accept envelope from {label}: {e}"
                    ));
                }
            }
        }
        InboxEvent::AckReceived {
            from: _from,
            ack: _ack,
        } => {
            let mut store = match MailboxStore::load(data_dir)
                .ok()
                .flatten()
                .unwrap_or_else(|| MailboxStore::empty_at(data_dir))
            {
                s => s,
            };
            if let Ok(true) = store.acknowledge_outgoing_and_save(&_ack) {
                tracing::debug!(
                    "mailbox: peer {} acknowledged envelope {}",
                    _from.fmt_short(),
                    _ack.message_id
                );
            }
        }
        InboxEvent::SyncRequested { from, since_ms } => {
            tracing::info!(
                "inbox: sync requested by {} since_ms={since_ms}",
                from.fmt_short()
            );
        }
    }
}

// ── UI event handling ─────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn handle_ui_event(
    event: UiEvent,
    tui: &mut TuiState,
    sender: &boru_chat::api::GossipSender,
    secret_key: &SecretKey,
    local_label: &str,
    endpoint: &Endpoint,
    blob_store: &MemStore,
    friend_mgr: &FriendPingManager,
    room_docs: &Arc<RwLock<Option<RoomDocs>>>,
    whisper_handle: &WhisperHandle,
    chat_history: &Arc<Mutex<ChatHistoryStore>>,
    topic: TopicId,
) -> Result<bool> {
    match event {
        UiEvent::Key(key) => {
            handle_key_event(
                key,
                tui,
                sender,
                secret_key,
                local_label,
                endpoint,
                blob_store,
                friend_mgr,
                room_docs,
                whisper_handle,
                chat_history,
                topic,
            )
            .await?;
            Ok(true)
        }
        UiEvent::Resize => Ok(true),
        UiEvent::Paste(text) => {
            if let Some(conv) = tui.current_conv_mut() {
                conv.composer_text.push_str(&text);
            }
            Ok(true)
        }
        UiEvent::SwitchScreen(screen) => {
            tui.screen = screen;
            Ok(true)
        }
        UiEvent::SelectConv(idx) => {
            if idx < tui.conversation_order.len() {
                let topic = tui.conversation_order[idx];
                tui.screen = TuiScreen::Chat { topic };
                tui.selected_conv_index = idx;
                // Clear unread when entering a conversation.
                if let Some(conv) = tui.conversations.get_mut(&topic) {
                    conv.unread = 0;
                }
            }
            Ok(true)
        }
        UiEvent::AcceptFriendRequest(request_id) => {
            let local_pk = tui.local_public.to_string();
            // Find the request and accept it.
            let request_clone = tui.friend_request_store.get(&request_id).cloned();
            if let Some(mut req) = request_clone {
                if req.status == FriendRequestStatus::Pending && req.recipient == local_pk {
                    // Update the FriendRequestStore.
                    let _ = tui
                        .friend_request_store
                        .accept_request(&request_id, &local_pk);
                    // Add as a friend.
                    if let Ok(pk) = req.requester.parse::<PublicKey>() {
                        let fid = FriendId::from_public_key(pk);
                        tui.friends.ensure_friend(fid);
                        tui.friends_dirty = true;
                        tui.push_system(format!("Accepted friend request from {}", req.requester));
                        // Start pinging the new friend.
                        let _ = friend_mgr.add_friend(pk, None).await;
                    }
                    // Save.
                    let _ = tui.friend_request_store.save();
                }
            }
            Ok(true)
        }
        UiEvent::DeclineFriendRequest(request_id) => {
            let local_pk = tui.local_public.to_string();
            let _ = tui
                .friend_request_store
                .decline_request(&request_id, &local_pk);
            tui.push_system("Declined friend request");
            let _ = tui.friend_request_store.save();
            Ok(true)
        }
        UiEvent::SendFriendRequest(peer_key) => {
            let local_pk = tui.local_public.to_string();
            match tui
                .friend_request_store
                .send_request(&local_pk, &peer_key, None)
            {
                Ok(req) => {
                    tui.push_system(format!("Friend request sent to {peer_key}"));
                    let _ = tui.friend_request_store.save();
                    Ok(true)
                }
                Err(e) => {
                    tui.push_system(format!("Failed to send friend request: {e}"));
                    Ok(true)
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_key_event(
    key: KeyEvent,
    tui: &mut TuiState,
    sender: &boru_chat::api::GossipSender,
    secret_key: &SecretKey,
    local_label: &str,
    endpoint: &Endpoint,
    blob_store: &MemStore,
    friend_mgr: &FriendPingManager,
    room_docs: &Arc<RwLock<Option<RoomDocs>>>,
    whisper_handle: &WhisperHandle,
    chat_history: &Arc<Mutex<ChatHistoryStore>>,
    _topic: TopicId,
) -> Result<()> {
    // ── Global keys (work in any screen) ──
    match key {
        KeyEvent {
            code: KeyCode::Esc, ..
        } => {
            if tui.help_visible {
                tui.help_visible = false;
                return Ok(());
            }
            // If on a Chat screen, go back to ChatList.
            if matches!(tui.screen, TuiScreen::Chat { .. }) {
                tui.screen = TuiScreen::ChatList;
                return Ok(());
            }
            // If on FriendRequests, go back to ChatList.
            if matches!(tui.screen, TuiScreen::FriendRequests) {
                tui.screen = TuiScreen::ChatList;
                return Ok(());
            }
            // Quit.
            let goodbye = SignedMessage::sign_and_encode(secret_key, &Message::Leave);
            if let Ok(encoded) = goodbye {
                let _ = sender.broadcast(encoded).await;
            }
            tui.should_quit = true;
        }
        KeyEvent {
            code: KeyCode::Char('c'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            let goodbye = SignedMessage::sign_and_encode(secret_key, &Message::Leave);
            if let Ok(encoded) = goodbye {
                let _ = sender.broadcast(encoded).await;
            }
            tui.should_quit = true;
        }
        KeyEvent {
            code: KeyCode::Tab, ..
        } => {
            // Cycle through screens: ChatList → Chat → FriendRequests → ChatList
            tui.screen = match &tui.screen {
                TuiScreen::ChatList => {
                    // Select the first conversation if available.
                    if !tui.conversation_order.is_empty() {
                        TuiScreen::Chat {
                            topic: tui.conversation_order[0],
                        }
                    } else {
                        TuiScreen::FriendRequests
                    }
                }
                TuiScreen::Chat { .. } => TuiScreen::FriendRequests,
                TuiScreen::FriendRequests => TuiScreen::ChatList,
            };
            return Ok(());
        }
        KeyEvent {
            code: KeyCode::BackTab,
            ..
        }
        | KeyEvent {
            code: KeyCode::Char('q'),
            ..
        } => {
            // We don't handle BackTab/Shift+Tab via this match arm
            // (ratatui doesn't always expose it cleanly). Fall through.
        }
        _ => {}
    }

    // ── Screen-specific keys ──
    match &tui.screen {
        TuiScreen::ChatList => {
            handle_chatlist_key(
                key,
                tui,
                sender,
                secret_key,
                endpoint,
                blob_store,
                friend_mgr,
                room_docs,
                whisper_handle,
                chat_history,
            )
            .await?
        }
        TuiScreen::Chat { .. } => {
            handle_chat_key(
                key,
                tui,
                sender,
                secret_key,
                local_label,
                endpoint,
                blob_store,
                friend_mgr,
                room_docs,
                whisper_handle,
                chat_history,
            )
            .await?
        }
        TuiScreen::FriendRequests => handle_friend_requests_key(key, tui).await?,
    }

    Ok(())
}

// ── Chat list screen key handling ─────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn handle_chatlist_key(
    key: KeyEvent,
    tui: &mut TuiState,
    sender: &boru_chat::api::GossipSender,
    secret_key: &SecretKey,
    endpoint: &Endpoint,
    blob_store: &MemStore,
    friend_mgr: &FriendPingManager,
    room_docs: &Arc<RwLock<Option<RoomDocs>>>,
    whisper_handle: &WhisperHandle,
    chat_history: &Arc<Mutex<ChatHistoryStore>>,
) -> Result<()> {
    let key_code = key.code;
    match key_code {
        KeyCode::Up => {
            if tui.selected_conv_index > 0 {
                tui.selected_conv_index -= 1;
            }
        }
        KeyCode::Down => {
            if tui.selected_conv_index < tui.conversation_order.len().saturating_sub(1) {
                tui.selected_conv_index += 1;
            }
        }
        KeyCode::Enter => {
            // Open the selected conversation.
            if tui.selected_conv_index < tui.conversation_order.len() {
                let topic = tui.conversation_order[tui.selected_conv_index];
                tui.screen = TuiScreen::Chat { topic };
                if let Some(conv) = tui.conversations.get_mut(&topic) {
                    conv.unread = 0;
                }
            }
        }
        KeyCode::F(2) => {
            tui.screen = TuiScreen::FriendRequests;
        }
        KeyCode::Char('h') | KeyCode::Char('?') => {
            tui.help_visible = true;
        }
        _ => {}
    }
    Ok(())
}

// ── Chat screen key handling ──────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn handle_chat_key(
    key: KeyEvent,
    tui: &mut TuiState,
    sender: &boru_chat::api::GossipSender,
    secret_key: &SecretKey,
    local_label: &str,
    endpoint: &Endpoint,
    blob_store: &MemStore,
    friend_mgr: &FriendPingManager,
    room_docs: &Arc<RwLock<Option<RoomDocs>>>,
    whisper_handle: &WhisperHandle,
    chat_history: &Arc<Mutex<ChatHistoryStore>>,
) -> Result<()> {
    let visible_height = tui.current_conv().map(|c| c.last_log_height).unwrap_or(10);

    match key {
        KeyEvent {
            code: KeyCode::Enter,
            ..
        } => {
            let topic = match &tui.screen {
                TuiScreen::Chat { topic } => *topic,
                _ => return Ok(()),
            };
            let conv = match tui.conversations.get_mut(&topic) {
                Some(c) => c,
                None => return Ok(()),
            };
            let submitted = std::mem::take(&mut conv.composer_text);
            let trimmed = submitted.trim().to_string();
            if trimmed.is_empty() {
                return Ok(());
            }

            // Handle slash commands.
            if let Some(path) = trimmed.strip_prefix("/send ") {
                // ── File send via iroh-blobs ──
                let path = path.trim().to_string();
                let path_buf = std::path::PathBuf::from(&path);
                let abs_path = match std::path::absolute(&path_buf) {
                    Ok(p) => p,
                    Err(e) => {
                        tui.push_system(format!("Failed to resolve path: {e}"));
                        return Ok(());
                    }
                };
                if !abs_path.exists() {
                    tui.push_system(format!("File not found: {path}"));
                    return Ok(());
                }
                let filename = match path_buf
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                {
                    Some(name) => name,
                    None => {
                        tui.push_system("Invalid file path.");
                        return Ok(());
                    }
                };
                tui.push_system(format!("Hashing file: {filename}..."));
                let tag = match blob_store.blobs().add_path(abs_path).await {
                    Ok(tag) => tag,
                    Err(e) => {
                        tui.push_system(format!("Failed to hash file: {e}"));
                        return Ok(());
                    }
                };
                let node_id = endpoint.id();
                let blob_ticket = BlobTicket::new(node_id.into(), tag.hash, tag.format);
                let ticket_str = blob_ticket.to_string();
                let message = Message::FileShare {
                    name: filename.clone(),
                    ticket: ticket_str.clone(),
                };
                let encoded_message = SignedMessage::sign_and_encode(secret_key, &message)?;
                sender.broadcast(encoded_message).await?;
                if let Some(c) = tui.conversations.get_mut(&topic) {
                    c.push_entry(ChatEntry::local(
                        local_label.to_string(),
                        format!("/send {path}"),
                    ));
                }
                tui.push_system(format!("Sharing: {filename} (ticket: {ticket_str})"));
                return Ok(());
            }

            if trimmed == "/download" {
                // ── File download ──
                let pending = tui
                    .conversations
                    .get(&topic)
                    .and_then(|c| c.pending_file.clone());
                if let Some((filename, ticket_str)) = pending {
                    let blob_ticket: BlobTicket = match ticket_str.parse() {
                        Ok(t) => t,
                        Err(e) => {
                            tui.push_system(format!("Failed to parse ticket: {e}"));
                            return Ok(());
                        }
                    };
                    let peer_id = blob_ticket.addr().id;
                    let candidates = download_candidates(peer_id, &tui.status.neighbors);
                    tui.push_system(format!("Downloading: {filename}..."));
                    // Safety: using the standard download without room-level safety checks.
                    if let Err(e) = download_blob_with_progress(
                        blob_store,
                        endpoint,
                        blob_ticket.hash(),
                        candidates,
                        filename.clone(),
                        TransferKind::File,
                        |_| {},
                        None,
                    )
                    .await
                    {
                        tui.push_system(format!("Download failed: {e}"));
                        return Ok(());
                    }
                    tui.push_system("Download complete. Exporting to disk...");
                    let dest = std::env::current_dir().unwrap_or_default().join(&filename);
                    if let Err(e) = blob_store.blobs().export(blob_ticket.hash(), dest).await {
                        tui.push_system(format!("Export failed: {e}"));
                        return Ok(());
                    }
                    tui.push_system(format!("Saved: {filename}"));
                    if let Some(conv) = tui.conversations.get_mut(&topic) {
                        conv.pending_file = None;
                    }
                } else {
                    tui.push_system("No pending file to download.");
                }
                return Ok(());
            }

            if trimmed == "/help" {
                tui.help_visible = true;
                return Ok(());
            }

            // Friend commands.
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
                        let was_new = tui.friends.get(&fid).is_none();
                        if let Some(alias_text) = &alias {
                            tui.friends.set_label(fid.clone(), alias_text.clone());
                        } else {
                            tui.friends.ensure_friend(fid.clone());
                        }
                        tui.friends_dirty = true;
                        let addr = tui
                            .friends
                            .get(&fid)
                            .and_then(|record| record.known_addrs.first().cloned());
                        match friend_mgr.add_friend(peer, addr).await {
                            Ok(_) => {
                                if was_new {
                                    let label = if let Some(ref alias_text) = alias {
                                        format!("{alias_text} ({})", peer.fmt_short())
                                    } else {
                                        peer.fmt_short().to_string()
                                    };
                                    tui.push_system(format!("Added friend: {label}"));
                                } else {
                                    tui.push_system(format!(
                                        "Updated friend: {}",
                                        peer.fmt_short()
                                    ));
                                }
                            }
                            Err(e) => {
                                tui.push_system(format!("Failed to add friend: {e}"));
                            }
                        }
                    }
                    Err(e) => {
                        tui.push_system(format!("Invalid public key: {e}"));
                    }
                }
                return Ok(());
            }

            if let Some(rest) = trimmed.strip_prefix("/friend remove ") {
                let target = rest.trim().to_string();
                let resolved = if let Ok(pk) = target.parse::<PublicKey>() {
                    Some((pk, FriendId::from_public_key(pk)))
                } else {
                    tui.friends
                        .iter()
                        .find(|(_, rec)| rec.label.as_deref() == Some(&target))
                        .map(|(fid, _)| (fid.parse_public_key().ok(), fid.clone()))
                        .and_then(|(pk_opt, fid)| pk_opt.map(|pk| (pk, fid)))
                };
                match resolved {
                    Some((peer, fid)) => {
                        let label = tui
                            .friends
                            .get(&fid)
                            .and_then(|r| r.label.clone())
                            .unwrap_or_else(|| peer.fmt_short().to_string());
                        tui.friends.remove(&fid);
                        tui.friends_dirty = true;
                        let _ = friend_mgr.remove_friend(&peer).await;
                        tui.push_system(format!("Removed friend: {label}"));
                    }
                    None => {
                        tui.push_system(format!("Friend not found: {target}"));
                    }
                }
                return Ok(());
            }

            if let Some(rest) = trimmed.strip_prefix("/friend rename ") {
                let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
                if parts.len() < 2 {
                    tui.push_system("Usage: /friend rename <public-key> <new-alias>");
                    return Ok(());
                }
                let target = parts[0].trim();
                let new_alias = parts[1].trim().to_string();
                let resolved = if let Ok(pk) = target.parse::<PublicKey>() {
                    Some(FriendId::from_public_key(pk))
                } else {
                    tui.friends
                        .iter()
                        .find(|(_, rec)| rec.label.as_deref() == Some(target))
                        .map(|(fid, _)| fid.clone())
                };
                match resolved {
                    Some(fid) => {
                        tui.friends.set_label(fid.clone(), &new_alias);
                        tui.friends_dirty = true;
                        tui.push_system(format!("Renamed friend to: {new_alias}"));
                    }
                    None => {
                        tui.push_system(format!("Friend not found: {target}"));
                    }
                }
                return Ok(());
            }

            if trimmed == "/friend list" {
                match friend_mgr.list_friends().await {
                    Ok(list) => {
                        if list.is_empty() && tui.friends.is_empty() {
                            tui.push_system("No friends tracked yet.");
                        } else {
                            tui.push_system(format!("Friends ({}):", tui.friends.len()));
                            for (peer, status) in &list {
                                let fid = FriendId::from_public_key(*peer);
                                let label = tui.resolve_name(peer);
                                let status_str = match status {
                                    FriendStatus::Unknown => "?",
                                    FriendStatus::Online => "ONLINE",
                                    FriendStatus::Offline => "offline",
                                };
                                let ping_status = tui
                                    .friends
                                    .get(&fid)
                                    .map(|r| if r.status.online { "online" } else { "offline" })
                                    .unwrap_or("unknown");
                                tui.push_system(format!(
                                    "  {label}: {status_str} (persisted: {ping_status})"
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        tui.push_system(format!("Failed to list friends: {e}"));
                    }
                }
                return Ok(());
            }

            if trimmed == "/room info" {
                let has_docs = room_docs.read().unwrap().as_ref().is_some();
                if !has_docs {
                    tui.push_system("No room docs available (room not initialised).");
                } else {
                    let metadata_doc = room_docs.read().unwrap().as_ref().unwrap().metadata.clone();
                    let roster = room_docs.read().unwrap().as_ref().unwrap().roster.clone();
                    let md = read_metadata(&metadata_doc).await;
                    let members = list_members(&roster).await;
                    tui.push_system(format!(
                        "Room: {} | Description: {} | Rules: {}",
                        md.name.as_deref().unwrap_or("unnamed"),
                        md.description.as_deref().unwrap_or("none"),
                        md.rules.as_deref().unwrap_or("none"),
                    ));
                    tui.push_system(format!(
                        "DHT discovery: {}",
                        if tui.room_discovery_secret_present {
                            "active (discovery secret present)"
                        } else {
                            "off (discovery secret absent)"
                        }
                    ));
                    tui.push_system(format!("Members ({}):", members.len()));
                    for (pk, member) in &members {
                        tui.push_system(format!(
                            "  {} ({}) — joined at {}",
                            member.display_name,
                            &pk[..16],
                            member.joined_at,
                        ));
                    }
                }
                return Ok(());
            }

            // Normal text message.
            let message = Message::Message {
                text: trimmed.clone(),
            };
            let encoded_message = SignedMessage::sign_and_encode(secret_key, &message)?;
            let msg_hash = message_hash(&message);
            let local_hex = hex::encode(secret_key.public().as_bytes());
            let event_id = {
                let mut store = chat_history.lock().unwrap();
                let entry = HistoryEntry::new(
                    tui.status.topic,
                    local_hex.clone(),
                    encoded_message.to_vec(),
                    "text",
                    trimmed.clone(),
                );
                let id = store.push_with_id(entry);
                let _ = store.update_delivery_state(id, DeliveryState::Sent);
                id
            };
            if let Some(conv) = tui.conversations.get_mut(&topic) {
                conv.self_sent_events.insert(msg_hash, event_id);
            }
            match sender.broadcast(encoded_message.clone()).await {
                Ok(()) => {
                    let mut entry = ChatEntry::local(local_label.to_string(), trimmed);
                    entry.message_hash = Some(msg_hash);
                    entry.event_id = event_id;
                    entry.delivery_state = DeliveryState::Sent;
                    if let Some(conv) = tui.conversations.get_mut(&topic) {
                        conv.push_entry(entry);
                    }
                }
                Err(e) => {
                    {
                        let mut store = chat_history.lock().unwrap();
                        let _ = store.update_delivery_state(event_id, DeliveryState::Failed);
                    }
                    tui.push_system(format!("Send failed: {e}"));
                }
            }
        }
        KeyEvent {
            code: KeyCode::Backspace,
            ..
        } => {
            if let Some(conv) = tui.current_conv_mut() {
                let mut chars: Vec<char> = conv.composer_text.chars().collect();
                if !chars.is_empty() {
                    chars.pop();
                    conv.composer_text = chars.into_iter().collect();
                }
            }
        }
        KeyEvent {
            code: KeyCode::Left,
            ..
        } => {
            if let Some(conv) = tui.current_conv_mut() {
                // Simple left navigation not implemented in TUI; ignore.
            }
        }
        KeyEvent {
            code: KeyCode::Right,
            ..
        } => {}
        KeyEvent {
            code: KeyCode::Home,
            ..
        } => {}
        KeyEvent {
            code: KeyCode::End, ..
        } => {}
        KeyEvent {
            code: KeyCode::PageUp,
            ..
        } => {
            if let Some(conv) = tui.current_conv_mut() {
                conv.scroll_up(visible_height.max(1) / 2, visible_height);
            }
        }
        KeyEvent {
            code: KeyCode::PageDown,
            ..
        } => {
            if let Some(conv) = tui.current_conv_mut() {
                conv.scroll_down(visible_height.max(1) / 2, visible_height);
            }
        }
        KeyEvent {
            code: KeyCode::Char(ch),
            modifiers,
            ..
        } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
            if let Some(conv) = tui.current_conv_mut() {
                conv.composer_text.push(ch);
            }
        }
        _ => {}
    }

    Ok(())
}

// ── Friend requests screen key handling ───────────────────────────────

async fn handle_friend_requests_key(_key: KeyEvent, tui: &mut TuiState) -> Result<()> {
    // Simple navigation: up/down selects, Enter accepts, Delete declines.
    // For now, show all actions inline and use the friend_request_store directly.
    match _key.code {
        KeyCode::Up | KeyCode::Down => {
            // Navigation placeholder: in simple TUI we don't have per-item selection yet.
        }
        KeyCode::Char('a') => {
            // Accept the first pending incoming request.
            let local_pk = tui.local_public.to_string();
            let incoming: Vec<_> = tui
                .friend_request_store
                .list_incoming_by_status(&local_pk, FriendRequestStatus::Pending);
            if let Some(first) = incoming.first() {
                let id = first.id.clone();
                let requester = first.requester.clone();
                let _ = tui.friend_request_store.accept_request(&id, &local_pk);
                if let Ok(pk) = requester.parse::<PublicKey>() {
                    let fid = FriendId::from_public_key(pk);
                    tui.friends.ensure_friend(fid);
                    tui.friends_dirty = true;
                }
                tui.push_system(format!("Accepted friend request from {requester}"));
                let _ = tui.friend_request_store.save();
            } else {
                tui.push_system("No pending friend requests to accept.");
            }
        }
        KeyCode::Char('d') => {
            // Decline the first pending incoming request.
            let local_pk = tui.local_public.to_string();
            let incoming: Vec<_> = tui
                .friend_request_store
                .list_incoming_by_status(&local_pk, FriendRequestStatus::Pending);
            if let Some(first) = incoming.first() {
                let id = first.id.clone();
                let requester = first.requester.clone();
                let _ = tui.friend_request_store.decline_request(&id, &local_pk);
                tui.push_system(format!("Declined friend request from {requester}"));
                let _ = tui.friend_request_store.save();
            } else {
                tui.push_system("No pending friend requests to decline.");
            }
        }
        _ => {}
    }
    Ok(())
}

// ── TUI rendering ─────────────────────────────────────────────────────

fn render_app(frame: &mut Frame<'_>, tui: &mut TuiState) {
    let status_height = status_panel_height(&tui.status);
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(status_height),
            Constraint::Min(10),
            Constraint::Length(1), // Status bar
        ])
        .split(frame.area());

    render_status_panel(frame, tui, layout[0]);

    match &tui.screen.clone() {
        TuiScreen::ChatList => render_chat_list(frame, tui, layout[1]),
        TuiScreen::Chat { topic } => render_chat_room(frame, tui, *topic, layout[1]),
        TuiScreen::FriendRequests => render_friend_requests(frame, tui, layout[1]),
    }

    render_status_bar(frame, tui, layout[2]);

    if tui.help_visible {
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

// ── Status panel ──────────────────────────────────────────────────────

fn render_status_panel(frame: &mut Frame<'_>, tui: &TuiState, area: Rect) {
    let status_block = Block::default()
        .title(Span::styled(
            "Status",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let status_lines = status_lines(&tui.status);
    let status_paragraph = Paragraph::new(Text::from(status_lines))
        .block(status_block)
        .wrap(Wrap { trim: true });
    frame.render_widget(status_paragraph, area);
}

// ── Chat list screen ──────────────────────────────────────────────────

fn render_chat_list(frame: &mut Frame<'_>, tui: &mut TuiState, area: Rect) {
    // Split into left (conversations) and right (friends + quick actions).
    let body_layout = if area.width >= 100 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Length(12)])
            .split(area)
    };

    // ── Left: Conversation list ──
    let conv_block = Block::default()
        .title(Span::styled(
            format!("Chats ({})", tui.conversation_order.len()),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));
    let conv_inner = conv_block.inner(body_layout[0]);
    let conv_lines = conversation_list_lines(tui);
    let conv_paragraph = Paragraph::new(Text::from(conv_lines))
        .block(conv_block)
        .wrap(Wrap { trim: true });
    frame.render_widget(conv_paragraph, body_layout[0]);

    // ── Right: Friends panel ──
    let friends_block = Block::default()
        .title(Span::styled(
            format!("Friends ({})", tui.friends.len()),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let friends_paragraph = Paragraph::new(Text::from(friends_panel_lines(tui)))
        .block(friends_block)
        .wrap(Wrap { trim: true });
    frame.render_widget(friends_paragraph, body_layout[1]);
}

/// Build the conversation list lines for the ChatList screen.
fn conversation_list_lines(tui: &TuiState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    let selected_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::REVERSED);
    let normal_style = Style::default();
    let unread_style = Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD);
    let public_room_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    let mut index = 0usize;
    // Always show the public room first.
    for (idx, topic) in tui.conversation_order.iter().enumerate() {
        let is_selected = idx == tui.selected_conv_index;
        let conv = match tui.conversations.get(topic) {
            Some(c) => c,
            None => continue,
        };

        let display_name = &conv.display_name;
        let is_public_room = display_name == PUBLIC_ROOM_LABEL;

        // Unread badge.
        let badge = if conv.unread > 0 {
            format!(" [{}]", conv.unread)
        } else {
            String::new()
        };

        let line_style = if is_selected {
            selected_style
        } else if is_public_room {
            public_room_style
        } else {
            normal_style
        };

        let badge_style = if conv.unread > 0 {
            unread_style
        } else {
            Style::default()
        };

        // Room-level DHT status indicator.
        let has_dht = tui.room_trackers.contains_key(topic);
        let dht_badge = if has_dht { " 📡" } else { "" };

        let prefix = if is_public_room { "★ " } else { "  " };
        let short_topic = topic.fmt_short();
        lines.push(Line::from(vec![
            Span::styled(prefix, line_style),
            Span::styled(display_name.clone(), line_style),
            Span::styled(dht_badge, Style::default().fg(Color::Green)),
            Span::styled(badge, badge_style),
            Span::styled(
                format!("  {short_topic}"),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        index += 1;
    }

    // Empty state.
    if tui.conversation_order.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "No conversations yet.",
            Style::default().fg(Color::DarkGray),
        )]));
        lines.push(Line::from(vec![Span::styled(
            "Use /friend add <pubkey> to start a conversation.",
            Style::default().fg(Color::DarkGray),
        )]));
    }

    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(vec![Span::styled(
        "↑↓ Select • Enter open • Tab switch view • F2 friend requests • h/? help",
        Style::default().fg(Color::DarkGray),
    )]));

    lines
}

// ── Chat room screen ──────────────────────────────────────────────────

fn render_chat_room(frame: &mut Frame<'_>, tui: &mut TuiState, topic: TopicId, area: Rect) {
    let conv = match tui.conversations.get(&topic) {
        Some(c) => c,
        None => {
            let empty_block = Block::default().title("Conversation").borders(Borders::ALL);
            frame.render_widget(empty_block, area);
            return;
        }
    };

    let composer_height = 3u16;
    let chat_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(composer_height)])
        .split(area);

    // ── Chat log ──
    let display_name = &conv.display_name;
    let log_block = Block::default()
        .title(Span::styled(
            display_name.clone(),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));
    let log_inner = log_block.inner(chat_layout[0]);

    // We can't modify conv through a shared reference, so build a temp mutable view.
    // Use interior mutability isn't available, so we'll store computed scroll on next render.
    let log_text = app_chat_text(conv);
    let log_scroll = conv.rendered_scroll_offset(log_inner.height);
    let log_paragraph = Paragraph::new(log_text)
        .block(log_block)
        .wrap(Wrap { trim: false })
        .scroll((log_scroll, 0));

    // Store the log height for scroll calculations on next key event.
    // We use a Cell-like approach: since we have &mut TuiState, we can update it.
    // But we only have &TuiState here. We'll store height via a different mechanism.
    frame.render_widget(log_paragraph, chat_layout[0]);

    // ── Composer ──
    let composer_block = Block::default()
        .title(Span::styled(
            "Composer",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));
    let composer_inner = composer_block.inner(chat_layout[1]);
    frame.render_widget(composer_block, chat_layout[1]);
    let prompt = "> ";
    let composer_line = Line::from(vec![
        Span::styled(
            prompt,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(conv.composer_text.clone()),
    ]);
    let composer_paragraph =
        Paragraph::new(Text::from(vec![composer_line])).wrap(Wrap { trim: false });
    frame.render_widget(composer_paragraph, composer_inner);
    let cursor_x = composer_inner
        .x
        .saturating_add(prompt.len() as u16)
        .saturating_add(conv.composer_text.len() as u16);
    frame.set_cursor_position((cursor_x, composer_inner.y));
}

// ── Friend requests screen ────────────────────────────────────────────

fn render_friend_requests(frame: &mut Frame<'_>, tui: &mut TuiState, area: Rect) {
    let local_pk = tui.local_public.to_string();

    // Incoming pending requests.
    let incoming: Vec<&FriendRequest> = tui
        .friend_request_store
        .list_incoming_by_status(&local_pk, FriendRequestStatus::Pending);

    // Outgoing pending requests.
    let outgoing: Vec<&FriendRequest> = tui
        .friend_request_store
        .list_outgoing_by_status(&local_pk, FriendRequestStatus::Pending);

    let mut lines: Vec<Line> = Vec::new();

    let title_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let pending_style = Style::default().fg(Color::Green);
    let hint_style = Style::default().fg(Color::DarkGray);

    // Incoming section.
    lines.push(Line::from(vec![Span::styled(
        format!("Incoming requests ({})", incoming.len()),
        title_style,
    )]));
    if incoming.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "  No pending incoming requests.",
            hint_style,
        )]));
    } else {
        for req in &incoming {
            lines.push(Line::from(vec![
                Span::styled("  From: ", Style::default().fg(Color::Cyan)),
                Span::styled(&req.requester, pending_style),
                Span::styled(" (pending)", hint_style),
            ]));
            lines.push(Line::from(vec![Span::styled(
                format!("    [a]ccept  [d]ecline  id: {}", req.id),
                hint_style,
            )]));
        }
    }

    lines.push(Line::from(Span::raw("")));

    // Outgoing section.
    lines.push(Line::from(vec![Span::styled(
        format!("Outgoing requests ({})", outgoing.len()),
        title_style,
    )]));
    if outgoing.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "  No pending outgoing requests.",
            hint_style,
        )]));
    } else {
        for req in &outgoing {
            lines.push(Line::from(vec![
                Span::styled("  To: ", Style::default().fg(Color::Cyan)),
                Span::styled(&req.recipient, pending_style),
                Span::styled(" (pending)", hint_style),
            ]));
        }
    }

    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(vec![Span::styled(
        "Esc back • Tab switch view • a accept first • d decline first",
        hint_style,
    )]));

    let request_block = Block::default()
        .title(Span::styled(
            "Friend Requests",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    let request_paragraph = Paragraph::new(Text::from(lines))
        .block(request_block)
        .wrap(Wrap { trim: true });
    frame.render_widget(request_paragraph, area);
}

// ── Status bar (bottom) ───────────────────────────────────────────────

fn render_status_bar(frame: &mut Frame<'_>, tui: &TuiState, area: Rect) {
    let mode_label = match &tui.screen {
        TuiScreen::ChatList => "CHAT LIST",
        TuiScreen::Chat { .. } => "CHAT",
        TuiScreen::FriendRequests => "FRIEND REQUESTS",
    };
    let screen_label = Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    let dht_label = if tui.status.dht_enabled {
        format!(" DHT: active ({})", tui.status.dht_peer_count)
    } else {
        " DHT: off".to_string()
    };

    let content = format!(
        " [{mode_label}] | {n} peer(s) | topic: {t} |{dht_label} | Esc back, Tab switch",
        n = tui.status.peer_count,
        t = tui.status.topic.fmt_short(),
        dht_label = dht_label,
    );

    let bar = Paragraph::new(Text::from(Line::from(vec![
        Span::styled(format!(" [{mode_label}] "), screen_label),
        Span::raw(format!(
            " {n} peer(s) | topic: {t} |{dht_label} | Esc back • Tab switch • PgUp/PgDn scroll",
            n = tui.status.peer_count,
            t = tui.status.topic.fmt_short(),
            dht_label = dht_label,
        )),
    ])))
    .style(Style::default().bg(Color::Blue).fg(Color::White));
    frame.render_widget(bar, area);
}

// ── TUI formatting helpers ────────────────────────────────────────────

fn entry_to_line(entry: &ChatEntry) -> Vec<Line<'static>> {
    let style = match entry.kind {
        ChatKind::System => Style::default().fg(Color::DarkGray),
        ChatKind::Local => Style::default().fg(Color::Green),
        ChatKind::Remote => Style::default().fg(Color::Blue),
    };
    let time_tag = entry
        .timestamp
        .map(|ms| format_epoch_ms_utc(ms))
        .unwrap_or_default();
    let label = if matches!(entry.kind, ChatKind::Local) && entry.event_id > 0 {
        format!(
            "[{} {}]{}",
            entry.label,
            entry.delivery_state.display_icon(),
            time_tag
        )
    } else {
        format!("[{}]{}", entry.label, time_tag)
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(label, style.add_modifier(Modifier::BOLD)),
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

fn app_chat_text(conv: &ConvState) -> Text<'static> {
    if conv.entries.is_empty() {
        Text::from(Line::from(vec![Span::styled(
            "No messages yet. Say hello.",
            Style::default().fg(Color::DarkGray),
        )]))
    } else {
        Text::from(
            conv.entries
                .iter()
                .flat_map(entry_to_line)
                .collect::<Vec<_>>(),
        )
    }
}

fn friends_panel_lines(tui: &TuiState) -> Vec<Line<'static>> {
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
            "Manage with /friend add/remove/rename/list in chat.",
            hint_style,
        )]),
    ];

    if tui.friends.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "No friends yet.",
            Style::default().fg(Color::DarkGray),
        )]));
        lines.push(Line::from(vec![Span::styled(
            "Add with /friend add <public-key> [alias].",
            hint_style,
        )]));
        return lines;
    }

    for (id, record) in tui.friends.iter() {
        let name = record.display_label(id);
        let short_id: String = id.as_str().chars().take(12).collect();
        let (status_text, status_style) = friend_status_text(record);
        let conn_hint = id
            .parse_public_key()
            .ok()
            .and_then(|pk| tui.status.peer_connection_types.get(&pk))
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
    height.clamp(6, 11)
}

fn status_lines(context: &StatusContext) -> Vec<Line<'static>> {
    let label_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let (health_label, health_value, health_color) = match &context.mesh_health {
        MeshHealth::Good => ("Mesh health", "Good".to_string(), Color::Green),
        MeshHealth::Degraded(reason) => {
            ("Mesh health", format!("Degraded: {reason}"), Color::Yellow)
        }
        MeshHealth::Offline(reason) => ("Mesh health", format!("Offline: {reason}"), Color::Red),
    };
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
            Span::styled(health_label, label_style),
            Span::styled(
                format!(": {health_value}"),
                Style::default().fg(health_color),
            ),
        ]),
        Line::from(vec![
            Span::styled("DHT", label_style),
            Span::raw(if context.dht_enabled {
                format!(
                    ": active ({} peer{})",
                    context.dht_peer_count,
                    if context.dht_peer_count == 1 { "" } else { "s" }
                )
            } else {
                ": off".to_string()
            }),
        ]),
        Line::from(vec![
            Span::styled("Notice", label_style),
            Span::raw(format!(": {}", context.transport_notice)),
        ]),
        Line::from(vec![
            Span::styled("Controls", label_style),
            Span::raw(
                ": Enter send • /help menu • Tab switch view • F2 friend req • PgUp/PgDn scroll history",
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
        Line::from(vec![Span::styled("Navigation", title_style)]),
        Line::from(vec![
            Span::styled("Tab", label_style),
            Span::raw("              switch between Chats / Friend Requests"),
        ]),
        Line::from(vec![
            Span::styled("↑/↓", label_style),
            Span::raw("            navigate conversation list"),
        ]),
        Line::from(vec![
            Span::styled("Enter", label_style),
            Span::raw("          open selected conversation or send message"),
        ]),
        Line::from(vec![
            Span::styled("Esc", label_style),
            Span::raw("            back to conversation list / close overlay / quit"),
        ]),
        Line::from(vec![
            Span::styled("F2", label_style),
            Span::raw("             open friend requests view"),
        ]),
        Line::from(vec![
            Span::styled("PgUp/PgDn", label_style),
            Span::raw("      scroll chat history"),
        ]),
        Line::from(vec![Span::styled("Commands", title_style)]),
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
            Span::raw("  track a friend"),
        ]),
        Line::from(vec![
            Span::styled("/friend remove <pubkey|alias>", label_style),
            Span::raw("  stop tracking"),
        ]),
        Line::from(vec![
            Span::styled("/friend list", label_style),
            Span::raw("     list friends and status"),
        ]),
        Line::from(vec![
            Span::styled("/room info", label_style),
            Span::raw("     show room metadata"),
        ]),
        Line::from(vec![Span::styled("Tips", title_style)]),
        Line::from(vec![Span::styled(
            "Press Esc to close this help view. F2 opens friend requests.",
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

fn format_epoch_ms_utc(ms: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let ts_secs = ms / 1000;
    let now_secs = now_ms / 1000;
    let days_since_epoch = |secs: u64| secs / 86400;
    let today = days_since_epoch(now_secs);
    let ts_day = days_since_epoch(ts_secs);
    if ts_day == today {
        let hour = (ts_secs % 86400) / 3600;
        let min = (ts_secs % 3600) / 60;
        format!(" {:02}:{:02}Z", hour, min)
    } else {
        format_iso8601_date_utc(ms)
    }
}

fn format_iso8601_date_utc(ms: u64) -> String {
    let secs = ms / 1000;
    let days = secs / 86400;
    let remaining = days;
    let mut year = 1970u64;
    let mut d = remaining;
    loop {
        let days_in_year = if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
            366
        } else {
            365
        };
        if d < days_in_year {
            break;
        }
        d -= days_in_year;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let mdays: [u64; 12] = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 1u64;
    let mut day = d + 1;
    for &md in &mdays {
        if day <= md {
            break;
        }
        day -= md;
        month += 1;
    }
    format!(" {year:04}-{month:02}-{day:02}")
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use boru_chat::chat_core::Composer;
    use iroh::EndpointAddr;

    #[test]
    fn ticket_roundtrips_through_base32() {
        let ticket = Ticket {
            topic: TopicId::from_bytes([9u8; 32]),
            peers: vec![EndpointAddr::new(SecretKey::generate().public())],
            discovery_secret: None,
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
            last_activity: HashMap::new(),
            mesh_health: MeshHealth::Good,
            dht_enabled: false,
            dht_peer_count: 0,
        };
        let lines = status_lines(&status);
        let rendered: Vec<_> = lines.iter().map(|line| line.to_string()).collect();
        assert!(rendered
            .iter()
            .any(|line| line.contains("Direct iroh transport is ready.")));
        assert!(rendered.iter().any(|line| line.contains("alice")));
        assert!(rendered.iter().any(|line| line.contains("3 known")));
        assert!(rendered.iter().any(|line| line.contains("DHT: off")));
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
            last_activity: HashMap::new(),
            mesh_health: MeshHealth::Good,
            dht_enabled: false,
            dht_peer_count: 0,
        };
        let friends = FriendsStore::empty_at(
            std::env::temp_dir().join(format!("iroh-chat-friends-empty-{}", rand::random::<u64>())),
        );
        let conversation_store = ConversationStore::empty_at(
            std::env::temp_dir().join(format!("iroh-chat-conv-empty-{}", rand::random::<u64>())),
        );
        let friend_request_store = FriendRequestStore::empty_at(
            std::env::temp_dir().join(format!("iroh-chat-fr-empty-{}", rand::random::<u64>())),
        );
        let tui = TuiState::new(
            status,
            friends,
            friend_request_store,
            conversation_store,
            SecretKey::generate().public(),
            "alice".into(),
            TopicId::from_bytes([0u8; 32]),
        );
        let rendered: Vec<String> = friends_panel_lines(&tui)
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert!(rendered.iter().any(|line| line.contains("No friends yet.")));
    }

    #[test]
    fn render_app_does_not_panic_on_normal_terminal_size() {
        let status = StatusContext {
            transport_status: "Direct iroh transport is ready.".into(),
            topic: TopicId::from_bytes([7u8; 32]),
            relay_mode: RelayMode::Disabled,
            connected: true,
            peer_count: 1,
            identity_label: "alice".into(),
            transport_notice: "transport notice".into(),
            direct_peers: 0,
            relayed_peers: 0,
            neighbors: HashSet::new(),
            peer_connection_types: HashMap::new(),
            last_activity: HashMap::new(),
            mesh_health: MeshHealth::Good,
            dht_enabled: false,
            dht_peer_count: 0,
        };
        let friends = FriendsStore::empty_at(
            std::env::temp_dir().join(format!("iroh-chat-render-test-{}", rand::random::<u64>())),
        );
        let conversation_store = ConversationStore::empty_at(
            std::env::temp_dir().join(format!("iroh-chat-render-conv-{}", rand::random::<u64>())),
        );
        let friend_request_store = FriendRequestStore::empty_at(
            std::env::temp_dir().join(format!("iroh-chat-render-fr-{}", rand::random::<u64>())),
        );
        let mut tui = TuiState::new(
            status,
            friends,
            friend_request_store,
            conversation_store,
            SecretKey::generate().public(),
            "alice".into(),
            TopicId::from_bytes([0u8; 32]),
        );
        let backend = ratatui::backend::TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_app(frame, &mut tui))
            .expect("render should not panic");
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
            last_activity: HashMap::new(),
            mesh_health: MeshHealth::Good,
            dht_enabled: false,
            dht_peer_count: 0,
        };
        let mut store = FriendsStore::empty_at(std::env::temp_dir().join(format!(
            "iroh-chat-friends-status-{}",
            rand::random::<u64>()
        )));
        let peer = SecretKey::generate().public();
        let friend_id = FriendId::from_public_key(peer);
        store.set_label(friend_id.clone(), "Bob");
        store.mark_online(friend_id.clone());
        let conversation_store = ConversationStore::empty_at(
            std::env::temp_dir().join(format!("iroh-chat-conv-status-{}", rand::random::<u64>())),
        );
        let friend_request_store = FriendRequestStore::empty_at(
            std::env::temp_dir().join(format!("iroh-chat-fr-status-{}", rand::random::<u64>())),
        );
        let tui = TuiState::new(
            status,
            store,
            friend_request_store,
            conversation_store,
            SecretKey::generate().public(),
            "alice".into(),
            TopicId::from_bytes([0u8; 32]),
        );
        let rendered: Vec<String> = friends_panel_lines(&tui)
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert!(rendered.iter().any(|line| line.contains("Bob")));
        assert!(rendered.iter().any(|line| line.contains("online")));
    }

    #[test]
    fn conversation_list_includes_public_room() {
        let status = StatusContext {
            transport_status: "ok".into(),
            topic: TopicId::from_bytes([9u8; 32]),
            relay_mode: RelayMode::Default,
            connected: true,
            peer_count: 0,
            identity_label: "alice".into(),
            transport_notice: "ok".into(),
            direct_peers: 0,
            relayed_peers: 0,
            neighbors: HashSet::new(),
            peer_connection_types: HashMap::new(),
            last_activity: HashMap::new(),
            mesh_health: MeshHealth::Good,
            dht_enabled: false,
            dht_peer_count: 0,
        };
        let public_topic = TopicId::from_bytes([1u8; 32]);
        let tui = TuiState::new(
            status,
            FriendsStore::empty_at(std::env::temp_dir()),
            FriendRequestStore::empty_at(std::env::temp_dir()),
            ConversationStore::empty_at(std::env::temp_dir()),
            SecretKey::generate().public(),
            "alice".into(),
            public_topic,
        );
        let rendered: Vec<String> = conversation_list_lines(&tui)
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert!(rendered.len() > 1);
        assert!(rendered.iter().any(|line| line.contains(PUBLIC_ROOM_LABEL)));
    }

    // ── Identity persistence tests ──

    #[test]
    fn secret_key_serialization_roundtrip() {
        let key = SecretKey::generate();
        let hex = data_encoding::HEXLOWER.encode(&key.to_bytes());
        let recovered = SecretKey::from_str(&hex).expect("should parse hex-encoded secret key");
        assert_eq!(key.to_bytes(), recovered.to_bytes());
        assert_eq!(key.public(), recovered.public());
    }

    #[test]
    fn secret_key_public_key_is_deterministic() {
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
        let prior = std::env::var_os("BORU_CHAT_DATA_DIR");
        std::env::set_var("BORU_CHAT_DATA_DIR", test_dir);
        let dir = get_data_dir();
        assert_eq!(dir, PathBuf::from(test_dir));
        match prior {
            Some(v) => std::env::set_var("BORU_CHAT_DATA_DIR", v),
            None => std::env::remove_var("BORU_CHAT_DATA_DIR"),
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
            discovery_secret: None,
        };
        let ticket_b = Ticket {
            topic,
            peers: vec![peer_addr],
            discovery_secret: None,
        };
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
        let read_back = std::fs::read_to_string(&key_path).expect("read key file");
        let recovered = SecretKey::from_str(read_back.trim()).expect("parse key");
        assert_eq!(key.public(), recovered.public());
        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_dir(&tmp);
    }

    #[test]
    fn load_or_generate_creates_and_reuses_key() {
        let tmp = std::env::temp_dir().join(format!("iroh-key-test-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let key_path = tmp.join("secret_key.txt");
        let (key_a, path_a) = load_or_generate_secret_key_at(&tmp).expect("first load");
        assert!(key_path.exists(), "key file should exist after generation");
        assert_eq!(path_a, key_path);
        let (key_b, path_b) = load_or_generate_secret_key_at(&tmp).expect("second load");
        assert_eq!(path_b, key_path);
        assert_eq!(key_a.public(), key_b.public());
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
        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_dir(&tmp);
    }

    #[test]
    fn load_or_generate_uses_existing_key_file() {
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
        std::env::remove_var("BORU_CHAT_DATA_DIR");
        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_dir(&tmp);
    }
}
