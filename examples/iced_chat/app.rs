//! The iced Application for the gossip chat frontend.
//!
//! Supports a chat-list (inbox) screen and individual chat-room screens,
//! with dynamic room switching — like Telegram/Signal.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use iroh::{EndpointAddr, PublicKey, RelayMode, SecretKey};
use iroh_blobs::{store::mem::MemStore, ticket::BlobTicket};
use iroh_gossip::api::GossipSender;
use iroh_gossip::backfill::BackfillHandle;
use iroh_gossip::chat_callbacks::ChatCallbacks;
use iroh_gossip::chat_core::handle_net_event as chat_net_event;
use iroh_gossip::chat_core::{
    download_candidates,
    friend_ping::{FriendEvent, FriendPingManager, FriendStatus},
    MeshHealth, MessageHash,
};
use iroh_gossip::whisper::{WhisperEvent, WhisperHandle};
use iroh_gossip::chat_history::{ChatHistoryStore, HistoryEntry};
use iroh_gossip::friends::{FriendId, FriendsStore};
use iroh_gossip::net::Gossip;
use iroh_gossip::proto::TopicId;
use iroh_gossip::room_docs::{self, RoomMetadata};
use iroh_gossip::room_history::{RoomHistoryEntry, RoomHistoryStore};
use n0_future::task;
use n0_future::Stream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;

use crate::{fmt_relay_mode, Message, NetEvent, SignedMessage, Ticket};
use iced::Color;

/// Scrollable ID for the chat log — used to auto-scroll to bottom.
const CHAT_LOG: &str = "chat_log";

// ── Typography scale (minor-second ratio ~1.125) ─────────────────────
const TYPO_XL: f32 = 24.0;   // Primary heading (chat list title)
const TYPO_LG: f32 = 18.0;   // Secondary heading (room name, help title)
const TYPO_MD: f32 = 15.0;   // Body / section headers / button labels
const TYPO_SM: f32 = 13.0;   // Secondary body, previews, entry labels
const TYPO_XS: f32 = 11.0;   // Metadata, identity info, secondary labels
const TYPO_XXS: f32 = 10.0;  // Fine print, ticket, instruction text

// ── Spacing units (4px base) ─────────────────────────────────────────
const SPACE_2: f32 = 2.0;
const SPACE_4: f32 = 4.0;
const SPACE_6: f32 = 6.0;
const SPACE_8: f32 = 8.0;
const SPACE_10: f32 = 10.0;
const SPACE_12: f32 = 12.0;
const SPACE_16: f32 = 16.0;
const SPACE_24: f32 = 24.0;

// ── Theme-aware chat colors ──────────────────────────────────────────
/// Return the muted secondary color for labels, previews, and counts.
fn text_muted(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.6, 0.6, 0.6)   // ~#999, ~4.5:1 on dark bg ✓ AA
    } else {
        Color::from_rgb(0.4, 0.4, 0.4)   // ~#666, ~5.2:1 on white ✓ AA
    }
}

/// Color for system message text (label and body).
fn text_system(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.5, 0.5, 0.5)
    } else {
        Color::from_rgb(0.35, 0.35, 0.35)  // #595959, ~6.5:1 ✓ AA
    }
}

/// Color for local (self) message label.
fn text_local_label(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.2, 0.8, 0.2)
    } else {
        Color::from_rgb(0.0, 0.45, 0.0)     // #0073, ~5.8:1 ✓ AA
    }
}

/// Color for local message body text.
fn text_local_body(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.3, 0.9, 0.3)
    } else {
        Color::from_rgb(0.0, 0.35, 0.0)     // #0059, ~6.5:1 ✓ AA
    }
}

/// Color for remote message label (nickname).
fn text_remote_label(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.4, 0.65, 1.0)     // light blue on dark
    } else {
        Color::from_rgb(0.0, 0.33, 0.66)    // #0054A8, ~5.5:1 ✓ AA
    }
}

/// Color for remote message body text.
fn text_remote_body(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.8, 0.8, 0.8)
    } else {
        Color::from_rgb(0.13, 0.13, 0.13)   // #222, ~11.5:1 ✓ AA
    }
}

/// Background tint for message bubbles. System messages get no bubble.
fn bubble_bg(theme: &iced::Theme, kind: ChatKind) -> Option<iced::Background> {
    if matches!(kind, ChatKind::System) {
        return None;
    }
    let (r, g, b, a) = match (theme, kind) {
        (iced::Theme::Dark, ChatKind::Local) => (0.15, 0.3, 0.15, 0.4),
        (iced::Theme::Dark, ChatKind::Remote) => (0.2, 0.2, 0.25, 0.4),
        (_, ChatKind::Local) => (0.0, 0.5, 0.0, 0.06),
        (_, ChatKind::Remote) => (0.1, 0.2, 0.5, 0.05),
        _ => return None,
    };
    Some(iced::Background::Color(Color::from_rgba(r, g, b, a)))
}

// ── Systematic palette (dark/light) ─────────────────────────────────────
/// Main window background — dark: #1a1a2e, light: #f0f0f5.
fn bg_primary(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.10, 0.10, 0.18)
    } else {
        Color::from_rgb(0.94, 0.94, 0.96)
    }
}

/// Surface/card background (slightly lighter than primary).
fn bg_surface(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.16, 0.16, 0.24)  // #2a2a3e
    } else {
        Color::from_rgb(1.0, 1.0, 1.0)     // #ffffff
    }
}

/// Input field background.
fn bg_input(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.13, 0.13, 0.22)  // #222238
    } else {
        Color::from_rgb(0.94, 0.94, 0.96)  // #f0f0f4
    }
}

/// Hover-state background for rows and interactive surfaces.
fn bg_hover(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.20, 0.20, 0.30)  // #33334d
    } else {
        Color::from_rgb(0.90, 0.90, 0.95)  // #e6e6f2
    }
}

/// Subtle border for surfaces and cards.
fn border_muted(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.22, 0.22, 0.32)  // #383852
    } else {
        Color::from_rgb(0.85, 0.85, 0.88)  // #d9d9e0
    }
}

/// Primary accent (blue).
fn accent_primary(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.29, 0.62, 1.0)   // #4a9eff
    } else {
        Color::from_rgb(0.18, 0.44, 0.80)  // #2e70cc
    }
}

/// Success / online indicator (green).
fn accent_green(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.24, 0.86, 0.52)  // #3ddc84
    } else {
        Color::from_rgb(0.10, 0.55, 0.20)  // #1a8c33
    }
}

/// Error / destructive colour.
fn color_error(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.90, 0.25, 0.25)  // #e64040
    } else {
        Color::from_rgb(0.75, 0.15, 0.15)  // #bf2626
    }
}

// ── Container style helpers ──────────────────────────────────────────────
/// Container style for the primary window background.
fn container_primary(theme: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(bg_primary(theme))),
        ..Default::default()
    }
}

/// Container style for a surface/card background.
fn container_surface(theme: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(bg_surface(theme))),
        ..Default::default()
    }
}

/// Container style for hover-state background.
fn container_hover(theme: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(bg_hover(theme))),
        ..Default::default()
    }
}

// ── Chat entry types ──────────────────────────────────────────────────

/// Current time as Unix epoch milliseconds.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[derive(Clone, Copy, Debug)]
enum ChatKind {
    System,
    Local,
    Remote,
}

#[derive(Clone, Debug)]
struct ChatEntry {
    kind: ChatKind,
    label: String,
    body: String,
    /// Protocol message content hash, for edit/delete/reaction matching.
    message_hash: Option<MessageHash>,
    /// Whether this entry has been edited after initial delivery.
    edited: bool,
    /// Emoji reactions attached to this entry.
    reactions: Vec<String>,
    /// Decoded image bytes for inline rendering, if this is an image message.
    image_bytes: Option<Vec<u8>>,
    /// Cached image handle to avoid re-decoding every frame.
    #[allow(clippy::rc_clone_in_vec_init)]
    image_handle: Option<iced::widget::image::Handle>,
    /// Unix epoch milliseconds when this message was sent (protocol sent_at
    /// for remote messages, local creation time for system/local messages).
    timestamp: Option<i64>,
}

impl ChatEntry {
    fn system(text: impl Into<String>) -> Self {
        let body = text.into();
        Self {
            kind: ChatKind::System,
            label: "System".into(),
            body: body.clone(),
            message_hash: None,
            edited: false,
            reactions: Vec::new(),
            image_bytes: None,
            image_handle: None,
            timestamp: Some(now_ms()),
        }
    }
    fn local(label: impl Into<String>, text: impl Into<String>) -> Self {
        let body = text.into();
        Self {
            kind: ChatKind::Local,
            label: label.into(),
            body: body.clone(),
            message_hash: None,
            edited: false,
            reactions: Vec::new(),
            image_bytes: None,
            image_handle: None,
            timestamp: Some(now_ms()),
        }
    }
    fn remote(
        label: impl Into<String>,
        text: impl Into<String>,
        hash: Option<MessageHash>,
        sent_at_secs: Option<u64>,
    ) -> Self {
        let body = text.into();
        Self {
            kind: ChatKind::Remote,
            label: label.into(),
            body: body.clone(),
            message_hash: hash,
            edited: false,
            reactions: Vec::new(),
            image_bytes: None,
            image_handle: None,
            timestamp: sent_at_secs.map(|s| s as i64 * 1000),
        }
    }
    fn image(
        label: impl Into<String>,
        body: impl Into<String>,
        image_bytes: Vec<u8>,
        hash: Option<MessageHash>,
        sent_at_secs: Option<u64>,
    ) -> Self {
        let body_str = body.into();
        let handle = iced::widget::image::Handle::from_bytes(image_bytes.clone());
        Self {
            kind: ChatKind::Remote,
            label: label.into(),
            body: body_str.clone(),
            message_hash: hash,
            edited: false,
            reactions: Vec::new(),
            image_bytes: Some(image_bytes),
            image_handle: Some(handle),
            timestamp: sent_at_secs.map(|s| s as i64 * 1000),
        }
    }
}

// ── Screen navigation ─────────────────────────────────────────────────

/// The active screen in the application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    /// The chat-list / inbox showing recent rooms.
    ChatList,
    /// An individual chat room with a given topic.
    Chat { topic: TopicId },
}

// ── Application state ─────────────────────────────────────────────────

pub struct IcedChat {
    // ── Navigation ──
    screen: Screen,
    /// Pending topic we're connecting to (used during the async handoff
    /// from clicking a room to actually subscribing).
    pending_topic: Option<TopicId>,

    // ── ChatList state ──
    room_history: RoomHistoryStore,
    room_history_dirty: bool,
    /// Text input for the "Join via ticket" field in the chat list.
    join_ticket_input: String,
    /// Optional error message shown in the chat list.
    chat_list_error: String,

    // ── Chat state (active room) ──
    entries: Vec<ChatEntry>,
    composer_text: String,
    help_visible: bool,
    pending_file: Option<(String, String)>,
    /// Pending image download: (filename, blob_hash, sender_pk).
    pending_image: Option<(String, MessageHash, PublicKey)>,
    names: HashMap<PublicKey, String>,
    topic: TopicId,
    ticket_str: String,

    // ── Shared network state ──
    secret_key: SecretKey,
    gossip: Gossip,
    sender: Option<GossipSender>,
    blob_store: MemStore,
    endpoint: iroh::Endpoint,
    local_label: String,
    local_public: PublicKey,
    local_peer_addr: EndpointAddr,
    relay_mode: RelayMode,
    runtime_handle: tokio::runtime::Handle,
    pub net_rx: Arc<Mutex<UnboundedReceiver<NetEvent>>>,
    net_tx: UnboundedSender<NetEvent>,
    backfill_handle: iroh_gossip::backfill::BackfillHandle,
    /// JoinHandle to abort the current forward_gossip_events task when
    /// switching rooms.
    forward_handle: Option<task::JoinHandle<()>>,
    /// Pending forwarder handle waiting to be transferred into `forward_handle`.
    forward_handle_slot: Arc<StdMutex<Option<task::JoinHandle<()>>>>,
    friends: FriendsStore,
    friends_dirty: bool,
    friend_mgr: FriendPingManager,
    pub friend_events_rx: Arc<Mutex<UnboundedReceiver<FriendEvent>>>,
    /// Set of peer PublicKeys currently connected as gossip neighbors.
    neighbors: HashSet<PublicKey>,
    /// Number of peers reachable via a direct (hole-punched) connection.
    direct_peers: usize,
    /// Number of peers connected through a relay server.
    relayed_peers: usize,
    /// Counter for periodic connection refresh (decremented per ConnMonitorTick).
    conn_refresh_counter: u32,
    /// Current mesh health summary from the quiescence watchdog.
    mesh_health: MeshHealth,
    /// Previous mesh health state, used to detect transitions.
    last_mesh_health: Option<MeshHealth>,
    /// Counter for periodic presence broadcast (decremented per ConnMonitorTick,
    /// broadcasts Message::Presence when it hits 0, resets to 5).
    presence_counter: u32,
    /// Counter for periodic invisible keepalive heartbeat (decremented per
    /// ConnMonitorTick, broadcasts Message::Heartbeat when it hits 0, resets to 2).
    heartbeat_counter: u32,
    /// Optional receiver for Tor reconnection status updates.
    tor_reconnect_rx: Option<Arc<Mutex<UnboundedReceiver<String>>>>,
    /// Whether to auto-scroll to the latest message.
    follow_latest: bool,
    /// Whether dark mode is enabled.
    pub dark_mode: bool,
    /// Transport notice displayed in the header (e.g. "Direct iroh transport is operational").
    pub notice: String,
    /// Persistent chat message history (loaded on startup, saved on each message).
    chat_history: Arc<std::sync::Mutex<ChatHistoryStore>>,
    /// Whether chat history has unsaved changes.
    chat_history_dirty: bool,
    /// Number of entries that have already been saved to chat_history
    /// for the current room. Used to avoid re-saving the same entries
    /// on every room-navigation event.
    history_saved_count: usize,
    /// Cache of friend PublicKey -> is_online for quick lookup in the UI.
    friend_online_cache: HashSet<PublicKey>,
    /// Bootstrap peer addresses from the initial join ticket (if any).
    /// Used only for the first room subscription; cleared after use.
    initial_bootstrap_peers: Vec<EndpointAddr>,
    /// Handle for sending whisper/private messages.
    whisper_handle: WhisperHandle,
    /// Receiver for incoming whisper events.
    pub whisper_events_rx: Arc<Mutex<UnboundedReceiver<WhisperEvent>>>,
}

#[derive(Debug, Clone)]
pub enum AppMessage {
    // ── Navigation ──
    /// Open the chat list screen (go back from a chat).
    GoToChatList,
    /// Open a specific room.
    OpenRoom(TopicId),
    /// A new room was created and we're now connected to it.
    RoomOpened {
        topic: TopicId,
        ticket: String,
        sender: GossipSender,
    },
    /// Finished creating a new room (random topic).
    CreateNewRoom,
    /// Join a room from a ticket string.
    JoinFromTicket,
    /// The room switch / join failed.
    RoomJoinFailed(String),

    // ── ChatList ──
    JoinTicketInputChanged(String),
    NewChatCreated,
    RoomSelected(TopicId),

    // ── Chat ──
    InputChanged(String),
    SendPressed,
    AttachPressed,
    ToggleHelp,
    NetEvent(NetEvent),
    FriendEvent(FriendEvent),
    /// An event from the whisper (DM) protocol.
    WhisperEvent(WhisperEvent),
    MessageSent(String),
    FileSent(String),
    DownloadDone(String),
    ErrorMsg(String),
    ExecuteFileSend(String),
    ExecuteDownload,
    ExecuteImageSend(String),
    ImageDownloaded {
        sender: PublicKey,
        name: String,
        image_bytes: Vec<u8>,
    },
    FriendAdded {
        fid: String,
        label: String,
        was_new: bool,
    },
    FriendRemoved {
        label: String,
    },
    FriendListResult(Vec<(String, String)>),
    /// Delete a room from history (home screen delete or /leave).
    DeleteRoom(TopicId),
    /// Periodic tick for connection type refresh.
    ConnMonitorTick,
    /// Periodic tick for mesh quiescence watchdog.
    MeshWatchdogTick,
    /// Status update from the Tor reconnection monitor.
    TorReconnect(String),
    /// Toggle dark mode on/off.
    ToggleDark(bool),
    /// Internal no-op for async task completions that should not change UI state.
    Noop,
    /// Copy text to the system clipboard.
    CopyToClipboard(String),
    /// Open a direct chat with an online friend.
    OpenFriendChat(PublicKey),
}

impl IcedChat {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        secret_key: SecretKey,
        gossip: Gossip,
        blob_store: MemStore,
        endpoint: iroh::Endpoint,
        local_label: String,
        local_public: PublicKey,
        local_peer_addr: EndpointAddr,
        relay_mode: RelayMode,
        _data_dir: std::path::PathBuf,
        runtime_handle: tokio::runtime::Handle,
        net_rx: Arc<Mutex<UnboundedReceiver<NetEvent>>>,
        net_tx: UnboundedSender<NetEvent>,
        room_history: RoomHistoryStore,
        friends: FriendsStore,
        friend_mgr: FriendPingManager,
        friend_events_rx: Arc<Mutex<UnboundedReceiver<FriendEvent>>>,
        whisper_events_rx: Arc<Mutex<UnboundedReceiver<WhisperEvent>>>,
        whisper_handle: WhisperHandle,
        tor_reconnect_rx: Option<Arc<Mutex<UnboundedReceiver<String>>>>,
        initial_room: Option<(TopicId, Vec<EndpointAddr>)>,
        notice: String,
        chat_history: Arc<std::sync::Mutex<ChatHistoryStore>>,
        backfill_handle: BackfillHandle,
    ) -> Self {
        let (initial_topic, initial_bootstrap) = initial_room
            .unwrap_or_else(|| (TopicId::from_bytes([0u8; 32]), vec![]));
        let friend_online_cache = HashSet::new();
        Self {
            screen: Screen::ChatList,
            pending_topic: None,
            room_history,
            room_history_dirty: false,
            join_ticket_input: String::new(),
            chat_list_error: String::new(),
            entries: Vec::new(),
            composer_text: String::new(),
            help_visible: false,
            pending_file: None,
            pending_image: None,
            names: HashMap::new(),
            topic: initial_topic,
            ticket_str: String::new(),
            secret_key,
            gossip,
            sender: None,
            blob_store,
            endpoint,
            local_label,
            local_public,
            local_peer_addr,
            relay_mode,
            runtime_handle,
            net_rx,
            net_tx,
            backfill_handle,
            forward_handle: None,
            forward_handle_slot: Arc::new(StdMutex::new(None)),
            friends,
            friends_dirty: false,
            friend_mgr,
            friend_events_rx,
            neighbors: HashSet::new(),
            direct_peers: 0,
            relayed_peers: 0,
            conn_refresh_counter: 0,
            mesh_health: MeshHealth::Good,
            last_mesh_health: None,
            presence_counter: 5,
            heartbeat_counter: 2,
            tor_reconnect_rx,
            follow_latest: true,
            dark_mode: false,
            notice,
            chat_history,
            chat_history_dirty: false,
            history_saved_count: 0,
            friend_online_cache,
            initial_bootstrap_peers: initial_bootstrap,
            whisper_handle,
            whisper_events_rx,
        }
    }

    fn room_ticket(&self, topic: TopicId) -> Ticket {
        Ticket {
            topic,
            peers: vec![self.local_peer_addr.clone()],
        }
    }

    fn push_system(&mut self, text: impl Into<String>) {
        self.entries.push(ChatEntry::system(text));
    }
    fn push_local(&mut self, text: impl Into<String>) {
        self.entries.push(ChatEntry::local(&self.local_label, text));
    }
}

// ── Room switching helpers ───────────────────────────────────────────

impl IcedChat {
    fn leave_current_room(&mut self) {
        // Abort the forwarding task
        if let Some(handle) = self.forward_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.forward_handle_slot.lock().unwrap().take() {
            handle.abort();
        }
        self.sender = None;
        self.entries.clear();
        self.names.clear();
        self.pending_file = None;
        self.pending_image = None;
        self.history_saved_count = 0;
    }

    /// Save any new entries in the current room to durable history.
    /// Only saves entries that have not yet been persisted (tracked by
    /// `history_saved_count`), avoiding duplication on room navigation.
    fn save_room_to_history(&mut self) {
        let topic = self.topic;
        let current_count = self.entries.len();
        if self.history_saved_count >= current_count {
            return;
        }

        // Persist chat messages to durable history — only the new ones.
        for entry in &self.entries[self.history_saved_count..] {
            let kind = match entry.kind {
                ChatKind::System => "system",
                ChatKind::Local => "text",
                ChatKind::Remote => "text",
            };
            let body_text = entry.body.clone();
            let sender = match entry.kind {
                ChatKind::Local => hex::encode(self.local_public.as_bytes()),
                _ => String::new(),
            };
            let history_entry = HistoryEntry::new(
                topic,
                sender,
                Vec::new(), // signed bytes not available here
                kind,
                body_text,
            );
            self.chat_history.lock().unwrap().push(history_entry);
        }
        self.history_saved_count = current_count;
        self.chat_history_dirty = true;
    }
}

// ── Deterministic private topic ────────────────────────────────────

/// Format a unix-ms timestamp into a human-readable relative time string.
fn format_last_seen(last_seen_ms: Option<u64>) -> String {
    let Some(ms) = last_seen_ms else {
        return String::new();
    };
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let elapsed_secs = if now_ms > ms {
        (now_ms - ms) / 1000
    } else {
        0
    };

    if elapsed_secs < 60 {
        if elapsed_secs <= 5 {
            "just now".to_string()
        } else {
            format!("{}s ago", elapsed_secs)
        }
    } else if elapsed_secs < 3600 {
        let mins = elapsed_secs / 60;
        format!("{}m ago", mins)
    } else if elapsed_secs < 86400 {
        let hours = elapsed_secs / 3600;
        format!("{}h ago", hours)
    } else {
        let days = elapsed_secs / 86400;
        format!("{}d ago", days)
    }
}

/// Format a Unix-millis timestamp into a message time label.
///
/// - Today:    "12:34"
/// - This week (same year): "Mon 12:34"
/// - Older:    "Jan 5"
fn format_message_time(timestamp_ms: i64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    // Both in millis; seconds = ms / 1000
    let ts_secs = timestamp_ms / 1000;
    let now_secs = now_ms / 1000;

    // Use chrono-like math from unix epoch
    let days_since_epoch = |secs: i64| secs / 86400;
    let today = days_since_epoch(now_secs);
    let ts_day = days_since_epoch(ts_secs);

    // Day-of-week from unix epoch. 1970-01-05 was Monday (Mon=1..Sun=7).
    let dow = |secs: i64| {
        let day = days_since_epoch(secs).rem_euclid(7);
        match day {
            0 => "Thu",
            1 => "Fri",
            2 => "Sat",
            3 => "Sun",
            4 => "Mon",
            5 => "Tue",
            6 => "Wed",
            _ => unreachable!(),
        }
    };

    // Hour/minute in 24h format
    let hour = (ts_secs % 86400) / 3600;
    let min = (ts_secs % 3600) / 60;

    if ts_day == today {
        // Today: show time only
        format!("{:02}:{:02}", hour, min)
    } else if ts_day >= today - 6 {
        // This week (within last 7 days): "Mon 12:34"
        format!("{} {:02}:{:02}", dow(ts_secs), hour, min)
    } else {
        // Older: "Jan 5"
        let days_off = ts_day;
        // Approximate: every 4 years has ~1461 days
        let y4 = days_off / 1461;
        let rem = days_off % 1461;
        let year = 1970 + y4 * 4 + (if rem >= 366 { 1 + (rem - 366) / 365 } else { 0 });
        let is_leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let yday = (ts_secs - {
            let mut y = 1970i64;
            let mut s = 0i64;
            while y < year {
                let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
                s += if leap { 366 } else { 365 };
                y += 1;
            }
            s * 86400
        }) / 86400;
        let month_days = if is_leap {
            [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        } else {
            [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        };
        let mut accum = 0i64;
        let mut month = 1usize;
        for (i, &md) in month_days.iter().enumerate() {
            if accum + md as i64 > yday {
                month = i + 1;
                break;
            }
            accum += md as i64;
        }
        let day_of_month = yday - accum + 1;
        let month_name = match month {
            1 => "Jan", 2 => "Feb", 3 => "Mar", 4 => "Apr",
            5 => "May", 6 => "Jun", 7 => "Jul", 8 => "Aug",
            9 => "Sep", 10 => "Oct", 11 => "Nov", 12 => "Dec",
            _ => "?",
        };
        format!("{month_name} {day_of_month}")
    }
}

/// Create a deterministic topic id from two peer public keys.
///
/// Both peers derive the same topic by sorting their public keys
/// before hashing, so either side can initiate a private chat.
fn private_topic(a: &PublicKey, b: &PublicKey) -> TopicId {
    let (pk1, pk2) = if a <= b { (a, b) } else { (b, a) };
    let mut hasher = blake3::Hasher::new();
    hasher.update(pk1.as_bytes());
    hasher.update(pk2.as_bytes());
    let hash = hasher.finalize();
    TopicId::from_bytes(*hash.as_bytes())
}

// ── Update ────────────────────────────────────────────────────────────

impl IcedChat {
    pub fn update(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
            // ── Navigation ────────────────────────────────────────────
            AppMessage::GoToChatList => {
                // Save current room to history.
                self.save_room_to_history();
                // Update room list preview.
                let name = self
                    .names
                    .get(&self.local_public)
                    .cloned()
                    .unwrap_or_default();
                let preview = self
                    .entries
                    .last()
                    .map(|e| {
                        let t = e.body.clone();
                        if t.len() > 60 {
                            format!("{}…", &t[..60])
                        } else {
                            t
                        }
                    })
                    .unwrap_or_default();
                self.room_history.upsert(self.topic, &name, true);
                if !preview.is_empty() {
                    self.room_history.update_preview(&self.topic, &preview);
                }
                self.room_history_dirty = true;
                self.persist_room_history();
                self.try_save_chat_history();

                // Going back to the chat list only changes the UI screen.
                // Keep the room subscription alive so returning is instant
                // and the local peer stays online in the room.
                self.screen = Screen::ChatList;
                iced::Task::none()
            }

            AppMessage::CreateNewRoom => {
                let topic = TopicId::from_bytes(rand::random());
                let gossip = self.gossip.clone();
                let net_tx = self.net_tx.clone();
                let sk = self.secret_key.clone();
                let label = self.local_label.clone();
                let ticket_str = self.room_ticket(topic).to_string();
                let forward_handle_slot = self.forward_handle_slot.clone();

                iced::Task::perform(
                    async move {
                        // Subscribe to the new topic
                        let sub = gossip
                            .subscribe(topic, vec![])
                            .await
                            .map_err(|e| e.to_string())?;
                        let (sender, receiver) = sub.split();

                        let metadata_doc = room_docs::create_metadata_doc(
                            topic,
                            &sender,
                            RoomMetadata {
                                name: Some("iroh-gossip-chat".to_string()),
                                description: None,
                                rules: None,
                            },
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                        let roster_doc = room_docs::create_roster_doc(
                            topic,
                            &sender,
                            sk.public().to_string(),
                            label.clone(),
                        )
                        .await
                        .map_err(|e| e.to_string())?;

                        let forward_handle = task::spawn(room_docs::forward_room_events_for_chat(
                            metadata_doc,
                            roster_doc,
                            receiver,
                            net_tx,
                        ));
                        *forward_handle_slot.lock().unwrap() = Some(forward_handle);

                        // Broadcast our presence (AboutMe + periodic Presence/Heartbeat
                        // handled by ConnMonitorTick).
                        let msg = SignedMessage::sign_and_encode(
                            &sk,
                            &crate::Message::AboutMe { name: label },
                        )
                        .map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(msg).await;

                        Ok::<(GossipSender, TopicId, String), String>((sender, topic, ticket_str))
                    },
                    |result| match result {
                        Ok((sender, topic, ticket_str)) => AppMessage::RoomOpened {
                            topic,
                            ticket: ticket_str,
                            sender,
                        },
                        Err(e) => AppMessage::RoomJoinFailed(e),
                    },
                )
            }

            AppMessage::OpenRoom(topic) => {
                // Save the current room first
                self.save_room_to_history();
                // Update room list preview for previous room
                let name = self
                    .names
                    .get(&self.local_public)
                    .cloned()
                    .unwrap_or_default();
                let preview = self
                    .entries
                    .last()
                    .map(|e| {
                        let t = e.body.clone();
                        if t.len() > 60 {
                            format!("{}…", &t[..60])
                        } else {
                            t
                        }
                    })
                    .unwrap_or_default();
                self.room_history.upsert(self.topic, &name, true);
                if !preview.is_empty() {
                    self.room_history.update_preview(&self.topic, &preview);
                }
                self.room_history_dirty = true;
                self.try_save_chat_history();
                self.leave_current_room();

                let gossip = self.gossip.clone();
                let net_tx = self.net_tx.clone();
                let sk = self.secret_key.clone();
                let label = self.local_label.clone();
                let ticket_str = self.room_ticket(topic).to_string();
                let forward_handle_slot = self.forward_handle_slot.clone();
                let endpoint = self.endpoint.clone();
                // Extract bootstrap peer addresses from ticket/RoomStore (if any)
                // and clear them so room switching doesn't reuse them.
                let initial_addrs: Vec<EndpointAddr> = self.initial_bootstrap_peers
                    .drain(..)
                    .collect();
                let bootstrap_peers: Vec<_> = initial_addrs.iter().map(|addr| addr.id).collect();

                iced::Task::perform(
                    async move {
                        // Seed the endpoint address lookup with bootstrap peer
                        // addresses so the endpoint can resolve them by their
                        // transport info (relay URL, direct addresses) from the
                        // ticket or RoomStore — not just by public key.
                        if !initial_addrs.is_empty() {
                            let memory_lookup = iroh::address_lookup::memory::MemoryLookup::new();
                            if let Ok(addr_lookup) = endpoint.address_lookup() {
                                addr_lookup.add(memory_lookup.clone());
                            }
                            for addr in &initial_addrs {
                                memory_lookup.set_endpoint_info(addr.clone());
                            }
                        }
                        let sub = gossip
                            .subscribe(topic, bootstrap_peers)
                            .await
                            .map_err(|e| e.to_string())?;
                        let (sender, receiver) = sub.split();

                        let metadata_doc = room_docs::create_metadata_doc(
                            topic,
                            &sender,
                            RoomMetadata {
                                name: Some("iroh-gossip-chat".to_string()),
                                description: None,
                                rules: None,
                            },
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                        let roster_doc = room_docs::create_roster_doc(
                            topic,
                            &sender,
                            sk.public().to_string(),
                            label.clone(),
                        )
                        .await
                        .map_err(|e| e.to_string())?;

                        let forward_handle = task::spawn(room_docs::forward_room_events_for_chat(
                            metadata_doc,
                            roster_doc,
                            receiver,
                            net_tx,
                        ));
                        *forward_handle_slot.lock().unwrap() = Some(forward_handle);

                        // Broadcast our presence (AboutMe + periodic Presence/Heartbeat
                        // handled by ConnMonitorTick).
                        let msg = SignedMessage::sign_and_encode(
                            &sk,
                            &crate::Message::AboutMe { name: label },
                        )
                        .map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(msg).await;

                        Ok::<(GossipSender, TopicId, String), String>((sender, topic, ticket_str))
                    },
                    |result| match result {
                        Ok((sender, topic, ticket_str)) => AppMessage::RoomOpened {
                            topic,
                            ticket: ticket_str,
                            sender,
                        },
                        Err(e) => AppMessage::RoomJoinFailed(e),
                    },
                )
            }

            AppMessage::RoomOpened {
                topic,
                ticket,
                sender,
            } => {
                self.pending_topic = None;
                self.sender = Some(sender);
                self.forward_handle = self.forward_handle_slot.lock().unwrap().take();

                self.screen = Screen::Chat { topic };
                self.topic = topic;
                self.ticket_str = ticket.clone();
                self.entries.clear();
                self.names.clear();
                self.composer_text.clear();
                self.push_system(format!(
                    "Connected as {}.  Topic: {topic}",
                    self.local_label
                ));
                self.push_system("Type a message and press Enter to send.  /help for commands.");
                self.push_system(format!("Ticket to join this room: {ticket}"));

                // Replay chat history for this topic
                let history_entries: Vec<_> = self
                    .chat_history
                    .lock()
                    .unwrap()
                    .for_topic(&topic)
                    .into_iter()
                    .cloned()
                    .collect();
                if !history_entries.is_empty() {
                    for entry in &history_entries {
                        match entry.kind.as_str() {
                            "system" => {
                                self.push_system(&entry.text_preview);
                            }
                            _ => {
                                // Parse sender from hex, use short display
                                let label = if entry.sender.is_empty() {
                                    "peer".to_string()
                                } else if let Ok(bytes) = hex::decode(&entry.sender) {
                                    if bytes.len() == 32 {
                                        let arr: [u8; 32] = bytes.try_into().unwrap();
                                        PublicKey::from_bytes(&arr)
                                            .map(|pk| pk.fmt_short().to_string())
                                            .unwrap_or_else(|_| "local".to_string())
                                    } else {
                                        "local".to_string()
                                    }
                                } else {
                                    "local".to_string()
                                };
                                if label == self.local_public.fmt_short().to_string() {
                                    self.push_local(&entry.text_preview);
                                } else {
                                    self.entries.push(ChatEntry::remote(
                                        &label,
                                        &entry.text_preview,
                                        None,
                                        None, // history entries don't carry sent_at
                                    ));
                                }
                            }
                        }
                    }
                }

                // Update room history
                self.room_history.upsert(topic, &self.local_label, true);
                self.room_history_dirty = true;
                self.persist_room_history();
                self.try_save_chat_history();

                iced::Task::none()
            }

            AppMessage::RoomJoinFailed(e) => {
                self.pending_topic = None;
                self.chat_list_error = format!("Failed to join room: {e}");
                self.screen = Screen::ChatList;
                iced::Task::none()
            }

            AppMessage::JoinFromTicket => {
                let ticket_input = self.join_ticket_input.clone();
                let gossip = self.gossip.clone();
                let net_tx = self.net_tx.clone();
                let sk = self.secret_key.clone();
                let label = self.local_label.clone();
                let endpoint = self.endpoint.clone();
                let local_peer_addr = self.local_peer_addr.clone();
                let forward_handle_slot = self.forward_handle_slot.clone();

                iced::Task::perform(
                    async move {
                        let ticket: Ticket = ticket_input
                            .parse()
                            .map_err(|e: n0_error::AnyError| e.to_string())?;
                        let memory_lookup = iroh::address_lookup::memory::MemoryLookup::new();
                        if let Ok(addr_lookup) = endpoint.address_lookup() {
                            addr_lookup.add(memory_lookup.clone());
                        }
                        for peer in &ticket.peers {
                            memory_lookup.set_endpoint_info(peer.clone());
                        }
                        let topic = ticket.topic;
                        let peers: Vec<_> = ticket.peers.iter().map(|p| p.id).collect();

                        let sub = gossip
                            .subscribe(topic, peers)
                            .await
                            .map_err(|e| e.to_string())?;
                        let (sender, receiver) = sub.split();
                        let new_ticket = Ticket {
                            topic,
                            peers: vec![local_peer_addr],
                        };
                        let ticket_str = new_ticket.to_string();

                        let metadata_doc = room_docs::create_metadata_doc(
                            topic,
                            &sender,
                            RoomMetadata::empty(),
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                        let roster_doc = room_docs::create_roster_doc(
                            topic,
                            &sender,
                            sk.public().to_string(),
                            label.clone(),
                        )
                        .await
                        .map_err(|e| e.to_string())?;

                        let forward_handle = task::spawn(room_docs::forward_room_events_for_chat(
                            metadata_doc,
                            roster_doc,
                            receiver,
                            net_tx,
                        ));
                        *forward_handle_slot.lock().unwrap() = Some(forward_handle);

                        let msg = SignedMessage::sign_and_encode(
                            &sk,
                            &crate::Message::AboutMe { name: label },
                        )
                        .map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(msg).await;

                        Ok::<(GossipSender, TopicId, String), String>((sender, topic, ticket_str))
                    },
                    |result| match result {
                        Ok((sender, topic, ticket_str)) => AppMessage::RoomOpened {
                            topic,
                            ticket: ticket_str,
                            sender,
                        },
                        Err(e) => AppMessage::RoomJoinFailed(e),
                    },
                )
            }

            AppMessage::NewChatCreated => {
                // Navigate to the newly created room — handled via OpenRoom
                iced::Task::done(AppMessage::CreateNewRoom)
            }

            AppMessage::OpenFriendChat(peer) => {
                let topic = private_topic(&self.local_public, &peer);
                // Send a whisper notification to alert the peer that a
                // private chat room has been opened for them.
                let whisper_handle = self.whisper_handle.clone();
                iced::Task::batch(vec![
                    iced::Task::perform(
                        async move {
                            let _ = whisper_handle
                                .send_dm(peer, "\x00PRIVATE_CHAT".to_string())
                                .await;
                        },
                        |_| AppMessage::Noop,
                    ),
                    iced::Task::done(AppMessage::OpenRoom(topic)),
                ])
            }

            AppMessage::RoomSelected(topic) => {
                if let Screen::ChatList = self.screen {
                    iced::Task::done(AppMessage::OpenRoom(topic))
                } else {
                    iced::Task::none()
                }
            }

            // ── ChatList ─────────────────────────────────────────────
            AppMessage::JoinTicketInputChanged(text) => {
                self.join_ticket_input = text;
                iced::Task::none()
            }

            // ── Chat ─────────────────────────────────────────────────
            AppMessage::InputChanged(text) => {
                self.composer_text = text;

                iced::Task::none()
            }

            AppMessage::SendPressed => {
                let trimmed = self.composer_text.trim().to_string();
                if trimmed.is_empty() {
                    return iced::Task::none();
                }
                self.composer_text.clear();

                if let Some(path) = trimmed.strip_prefix("/send ") {
                    let path = path.trim().to_string();
                    return iced::Task::perform(
                        async move {
                            let path_buf = std::path::PathBuf::from(&path);
                            let abs_path = std::path::absolute(&path_buf)
                                .map_err(|_| format!("Invalid path: {path}"))?;
                            if !abs_path.exists() {
                                return Err(format!("File not found: {path}"));
                            }
                            let filename = path_buf
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default();
                            if filename.is_empty() {
                                return Err("Invalid file path.".to_string());
                            }
                            Ok(format!("{filename}|{}|{path}", abs_path.display()))
                        },
                        |r: Result<String, String>| match r {
                            Ok(v) => AppMessage::ExecuteFileSend(v),
                            Err(e) => AppMessage::ErrorMsg(e),
                        },
                    );
                }

                if let Some(path) = trimmed.strip_prefix("/image ") {
                    let path = path.trim().to_string();
                    return iced::Task::perform(
                        async move {
                            let path_buf = std::path::PathBuf::from(&path);
                            let abs_path = std::path::absolute(&path_buf)
                                .map_err(|_| format!("Invalid path: {path}"))?;
                            if !abs_path.exists() {
                                return Err(format!("File not found: {path}"));
                            }
                            let filename = path_buf
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default();
                            if filename.is_empty() {
                                return Err("Invalid file path.".to_string());
                            }
                            Ok(format!("{filename}|{}|{path}", abs_path.display()))
                        },
                        |r: Result<String, String>| match r {
                            Ok(v) => AppMessage::ExecuteImageSend(v),
                            Err(e) => AppMessage::ErrorMsg(e),
                        },
                    );
                }

                if trimmed == "/download" {
                    return iced::Task::done(AppMessage::ExecuteDownload);
                }
                if trimmed == "/help" {
                    self.help_visible = !self.help_visible;
                    return iced::Task::none();
                }

                // ── Leave room / delete from history ──
                if trimmed == "/leave" {
                    let topic = self.topic;
                    // Broadcast Leave (best-effort)
                    if let Some(ref sender) = self.sender {
                        if let Ok(encoded) =
                            SignedMessage::sign_and_encode(&self.secret_key, &crate::Message::Leave)
                        {
                            let sender = sender.clone();
                            task::spawn(async move {
                                sender.broadcast(encoded).await.ok();
                            });
                        }
                    }
                    // Remove room and chat history (not just go back — delete it)
                    self.room_history.remove(&topic);
                    self.room_history_dirty = true;
                    self.chat_history.lock().unwrap().remove_topic(&topic);
                    self.chat_history_dirty = true;
                    self.persist_room_history();
                    self.try_save_chat_history();
                    // Leave the room and go back to chat list
                    self.leave_current_room();
                    self.screen = Screen::ChatList;
                    return iced::Task::none();
                }

                // ── Friend commands ──────────────────
                if let Some(pubkey_str) = trimmed.strip_prefix("/friend add ") {
                    let pubkey_str = pubkey_str.trim().to_string();
                    let (key_part, alias) = if let Some((key_part, rest)) =
                        pubkey_str.split_once(char::is_whitespace)
                    {
                        (key_part.to_string(), Some(rest.trim().to_string()))
                    } else {
                        (pubkey_str, None)
                    };
                    let mgr = self.friend_mgr.clone();
                    return iced::Task::perform(
                        async move {
                            match key_part.parse::<PublicKey>() {
                                Ok(peer) => {
                                    let fid = FriendId::from_public_key(peer);
                                    let label = alias
                                        .clone()
                                        .unwrap_or_else(|| peer.fmt_short().to_string());
                                    let was_new = mgr.add_friend(peer, None).await.unwrap_or(false);
                                    AppMessage::FriendAdded {
                                        fid: fid.as_str().to_string(),
                                        label,
                                        was_new,
                                    }
                                }
                                Err(e) => AppMessage::ErrorMsg(format!("Invalid public key: {e}")),
                            }
                        },
                        |msg| msg,
                    );
                }

                if let Some(target) = trimmed.strip_prefix("/friend remove ") {
                    let target = target.trim().to_string();
                    let mgr = self.friend_mgr.clone();
                    return iced::Task::perform(
                        async move {
                            match target.parse::<PublicKey>() {
                                Ok(peer) => {
                                    let removed = mgr.remove_friend(&peer).await.unwrap_or(false);
                                    let label = if removed {
                                        peer.fmt_short().to_string()
                                    } else {
                                        target.clone()
                                    };
                                    AppMessage::FriendRemoved { label }
                                }
                                Err(_) => {
                                    AppMessage::ErrorMsg(format!("Friend not found: {target}"))
                                }
                            }
                        },
                        |msg| msg,
                    );
                }

                if trimmed == "/friend list" {
                    let mgr = self.friend_mgr.clone();
                    return iced::Task::perform(
                        async move {
                            match mgr.list_friends().await {
                                Ok(list) => {
                                    let items: Vec<(String, String)> = list
                                        .into_iter()
                                        .map(|(pk, status)| {
                                            let status_str = match status {
                                                FriendStatus::Unknown => "?".to_string(),
                                                FriendStatus::Online => "ONLINE".to_string(),
                                                FriendStatus::Offline => "offline".to_string(),
                                            };
                                            (pk.fmt_short().to_string(), status_str)
                                        })
                                        .collect();
                                    AppMessage::FriendListResult(items)
                                }
                                Err(e) => {
                                    AppMessage::ErrorMsg(format!("Failed to list friends: {e}"))
                                }
                            }
                        },
                        |msg| msg,
                    );
                }

                if trimmed == "/connections" {
                    use iroh_gossip::chat_core::check_peer_connection_type;
                    let neighbors: Vec<iroh::PublicKey> = self.neighbors.iter().copied().collect();
                    if neighbors.is_empty() {
                        self.push_system("No known peers to inspect.");
                    } else {
                        self.push_system(format!("Connections ({}):", neighbors.len()));
                        let rt = self.runtime_handle.clone();
                        let ep = self.endpoint.clone();
                        let names = self.names.clone();
                        // Query each peer and push results inline via block_on.
                        for pk in &neighbors {
                            let ctype =
                                rt.block_on(async { check_peer_connection_type(&ep, *pk).await });
                            let label = names
                                .get(pk)
                                .cloned()
                                .unwrap_or_else(|| pk.fmt_short().to_string());
                            self.push_system(format!(
                                "  {label} — {} ({})",
                                match ctype {
                                    iroh_gossip::chat_core::ConnectionType::Direct => "direct",
                                    iroh_gossip::chat_core::ConnectionType::Relayed => "relayed",
                                    iroh_gossip::chat_core::ConnectionType::Unknown => "unknown",
                                },
                                pk.fmt_short(),
                            ));
                        }
                    }
                    return iced::Task::none();
                }

                // ── Reactions ──
                if let Some(rest) = trimmed.strip_prefix("/react ") {
                    let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                    if parts.len() < 2 {
                        self.push_system("Usage: /react <msg_index> <emoji>".to_string());
                        return iced::Task::none();
                    }
                    let idx: usize = match parts[0].parse() {
                        Ok(i) => i,
                        Err(_) => {
                            self.push_system("Usage: /react <msg_index> <emoji>".to_string());
                            return iced::Task::none();
                        }
                    };
                    let emoji = parts[1].to_string();
                    if idx == 0 || idx > self.entries.len() {
                        self.push_system(format!("No message at index {idx}"));
                        return iced::Task::none();
                    }
                    let Some(hash) = self.entries[idx - 1].message_hash else {
                        self.push_system("Cannot react to a system message".to_string());
                        return iced::Task::none();
                    };
                    // Apply locally first
                    self.add_reaction(&hash, emoji.clone());
                    // Broadcast
                    match SignedMessage::sign_and_encode(
                        &self.secret_key,
                        &crate::Message::Reaction {
                            message_hash: hash,
                            emoji,
                        },
                    ) {
                        Ok(encoded) => {
                            if let Some(ref sender) = self.sender {
                                let sender = sender.clone();
                                return iced::Task::perform(
                                    async move {
                                        sender.broadcast(encoded).await.ok();
                                    },
                                    |_| AppMessage::Noop,
                                );
                            }
                        }
                        Err(e) => {
                            return iced::Task::done(AppMessage::ErrorMsg(e.to_string()));
                        }
                    }
                    return iced::Task::done(AppMessage::ErrorMsg(
                        "Not connected to any room.".into(),
                    ));
                }

                // ── Edit ──
                if let Some(rest) = trimmed.strip_prefix("/edit ") {
                    let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                    if parts.len() < 2 {
                        self.push_system("Usage: /edit <msg_index> <new_text>".to_string());
                        return iced::Task::none();
                    }
                    let idx: usize = match parts[0].parse() {
                        Ok(i) => i,
                        Err(_) => {
                            self.push_system("Usage: /edit <msg_index> <new_text>".to_string());
                            return iced::Task::none();
                        }
                    };
                    let new_text = parts[1].to_string();
                    if idx == 0 || idx > self.entries.len() {
                        self.push_system(format!("No message at index {idx}"));
                        return iced::Task::none();
                    }
                    let Some(hash) = self.entries[idx - 1].message_hash else {
                        self.push_system("Cannot edit a system message".to_string());
                        return iced::Task::none();
                    };
                    // Apply locally first
                    self.edit_message(&hash, new_text.clone());
                    // Broadcast
                    match SignedMessage::sign_and_encode(
                        &self.secret_key,
                        &crate::Message::Edit {
                            original_hash: hash,
                            new_text,
                        },
                    ) {
                        Ok(encoded) => {
                            if let Some(ref sender) = self.sender {
                                let sender = sender.clone();
                                return iced::Task::perform(
                                    async move {
                                        sender.broadcast(encoded).await.ok();
                                    },
                                    |_| AppMessage::Noop,
                                );
                            }
                        }
                        Err(e) => {
                            return iced::Task::done(AppMessage::ErrorMsg(e.to_string()));
                        }
                    }
                    return iced::Task::done(AppMessage::ErrorMsg(
                        "Not connected to any room.".into(),
                    ));
                }

                // ── Delete ──
                if let Some(idx_str) = trimmed.strip_prefix("/delete ") {
                    let idx_str = idx_str.trim().to_string();
                    let idx: usize = match idx_str.parse() {
                        Ok(i) => i,
                        Err(_) => {
                            self.push_system("Usage: /delete <msg_index>".to_string());
                            return iced::Task::none();
                        }
                    };
                    if idx == 0 || idx > self.entries.len() {
                        self.push_system(format!("No message at index {idx}"));
                        return iced::Task::none();
                    }
                    let Some(hash) = self.entries[idx - 1].message_hash else {
                        self.push_system("Cannot delete a system message".to_string());
                        return iced::Task::none();
                    };
                    // Apply locally first
                    self.delete_message(&hash);
                    // Broadcast
                    match SignedMessage::sign_and_encode(
                        &self.secret_key,
                        &crate::Message::Delete { message_hash: hash },
                    ) {
                        Ok(encoded) => {
                            if let Some(ref sender) = self.sender {
                                let sender = sender.clone();
                                return iced::Task::perform(
                                    async move {
                                        sender.broadcast(encoded).await.ok();
                                    },
                                    |_| AppMessage::Noop,
                                );
                            }
                        }
                        Err(e) => {
                            return iced::Task::done(AppMessage::ErrorMsg(e.to_string()));
                        }
                    }
                    return iced::Task::done(AppMessage::ErrorMsg(
                        "Not connected to any room.".into(),
                    ));
                }

                // Normal text message — check for whisper commands first
                if let Some(rest) = trimmed.strip_prefix("/whisper ") {
                    // ── Whisper DM ──────────────────────────────────────────
                    let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
                    if parts.len() < 2 {
                        self.push_system("Usage: /whisper <peer-key|friend-alias> <message>");
                        return iced::Task::none();
                    }
                    let target = parts[0].trim().to_string();
                    let message = parts[1].trim().to_string();
                    // Resolve peer key from alias or direct public key
                    let peer_key = self.resolve_peer_key(&target);
                    let peer_key = match peer_key {
                        Some(pk) => pk,
                        None => {
                            self.push_system(format!(
                                "Unknown peer: {target}. Use a public key or friend alias."
                            ));
                            return iced::Task::none();
                        }
                    };
                    let whisper_handle = self.whisper_handle.clone();
                    let text = message.clone();
                    let label = self
                        .names
                        .get(&peer_key)
                        .cloned()
                        .unwrap_or_else(|| peer_key.fmt_short().to_string());
                    self.push_system(format!("[Whisper to {label}] {message}"));
                    return iced::Task::perform(
                        async move {
                            whisper_handle
                                .send_dm(peer_key, text)
                                .await
                                .map_err(|e| e.to_string())
                        },
                        |r: Result<(), String>| match r {
                            Ok(()) => AppMessage::Noop,
                            Err(e) => AppMessage::ErrorMsg(format!("Whisper failed: {e}")),
                        },
                    );
                }

                if let Some(rest) = trimmed.strip_prefix("/whisper-file ") {
                    // ── Whisper file transfer ──────────────────────────────
                    let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
                    if parts.len() < 2 {
                        self.push_system("Usage: /whisper-file <peer-key|friend-alias> <path>");
                        return iced::Task::none();
                    }
                    let target = parts[0].trim().to_string();
                    let path = parts[1].trim().to_string();
                    let peer_key = self.resolve_peer_key(&target);
                    let peer_key = match peer_key {
                        Some(pk) => pk,
                        None => {
                            self.push_system(format!(
                                "Unknown peer: {target}. Use a public key or friend alias."
                            ));
                            return iced::Task::none();
                        }
                    };
                    let path_buf = std::path::PathBuf::from(&path);
                    let abs_path = match std::path::absolute(&path_buf) {
                        Ok(p) => p,
                        Err(e) => {
                            self.push_system(format!("Failed to resolve path: {e}"));
                            return iced::Task::none();
                        }
                    };
                    if !abs_path.exists() {
                        self.push_system(format!("File not found: {path}"));
                        return iced::Task::none();
                    }
                    let filename = path_buf
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if filename.is_empty() {
                        self.push_system("Invalid file path.");
                        return iced::Task::none();
                    }
                    let blob_store = self.blob_store.clone();
                    let whisper_handle = self.whisper_handle.clone();
                    let fname = filename.clone();
                    self.push_system(format!("[Whisper] Hashing file: {filename}..."));
                    return iced::Task::perform(
                        async move {
                            let tag = blob_store
                                .blobs()
                                .add_path(abs_path)
                                .await
                                .map_err(|e| format!("Failed to hash file: {e}"))?;
                            let ticket = tag.hash.to_string();
                            whisper_handle
                                .send_file(peer_key, fname.clone(), ticket)
                                .await
                                .map_err(|e| format!("Whisper file failed: {e}"))?;
                            Ok::<String, String>(fname)
                        },
                        |r: Result<String, String>| match r {
                            Ok(name) => AppMessage::FileSent(name),
                            Err(e) => AppMessage::ErrorMsg(e),
                        },
                    );
                }

                // Normal text message
                let text = trimmed.clone();
                match SignedMessage::sign_and_encode(
                    &self.secret_key,
                    &crate::Message::Message { text: trimmed },
                ) {
                    Ok(encoded) => {
                        if let Some(ref sender) = self.sender {
                            let sender = sender.clone();
                            iced::Task::perform(
                                async move {
                                    sender.broadcast(encoded).await.ok();
                                    text
                                },
                                AppMessage::MessageSent,
                            )
                        } else {
                            iced::Task::done(AppMessage::ErrorMsg(
                                "Not connected to any room.".into(),
                            ))
                        }
                    }
                    Err(e) => iced::Task::done(AppMessage::ErrorMsg(e.to_string())),
                }
            }

            AppMessage::AttachPressed => {
                iced::Task::perform(
                    rfd::AsyncFileDialog::new()
                        .set_title("Select a file to share")
                        .pick_file(),
                    |file| {
                        if let Some(file) = file {
                            let name = file.file_name().to_string();
                            let path = file.path().to_string_lossy().to_string();
                            let encoded = format!("{name}|{path}|{path}");
                            // Auto-detect image files by extension for inline display
                            let is_image = name.to_lowercase().ends_with(".png")
                                || name.to_lowercase().ends_with(".jpg")
                                || name.to_lowercase().ends_with(".jpeg")
                                || name.to_lowercase().ends_with(".gif")
                                || name.to_lowercase().ends_with(".webp")
                                || name.to_lowercase().ends_with(".bmp");
                            if is_image {
                                AppMessage::ExecuteImageSend(encoded)
                            } else {
                                AppMessage::ExecuteFileSend(encoded)
                            }
                        } else {
                            AppMessage::Noop
                        }
                    },
                )
            }

            AppMessage::ToggleHelp => {
                self.help_visible = !self.help_visible;
                iced::Task::none()
            }

            AppMessage::NetEvent(event) => {
                self.update_room_preview(&event);
                let _ = chat_net_event(event, self);
                self.try_save_friends();
                self.try_save_chat_history();
                // Check if an ImageShare was just received and auto-download
                if let Some((name, hash, sender_pk)) = self.pending_image.take() {
                    let blob_store = self.blob_store.clone();
                    let endpoint = self.endpoint.clone();
                    let neighbors = self.neighbors.clone();
                    return iced::Task::perform(
                        async move {
                            let blob_hash: iroh_blobs::Hash = hash.into();
                            let candidates = download_candidates(sender_pk, &neighbors);
                            blob_store
                                .downloader(&endpoint)
                                .download(blob_hash, candidates)
                                .await
                                .map_err(|e| format!("Download: {e}"))?;
                            let mut reader = blob_store.blobs().reader(blob_hash);
                            let mut buf = Vec::new();
                            use tokio::io::AsyncReadExt;
                            reader
                                .read_to_end(&mut buf)
                                .await
                                .map_err(|e| format!("Read: {e}"))?;
                            Ok((name, buf))
                        },
                        move |r: Result<(String, Vec<u8>), String>| match r {
                            Ok((name, data)) => AppMessage::ImageDownloaded {
                                sender: sender_pk,
                                name,
                                image_bytes: data,
                            },
                            Err(e) => AppMessage::ErrorMsg(e),
                        },
                    );
                }
                iced::Task::none()
            }

            AppMessage::FriendEvent(event) => {
                self.handle_friend_event(event);
                self.try_save_friends();
                iced::Task::none()
            }

            AppMessage::WhisperEvent(event) => {
                match event {
                    iroh_gossip::whisper::WhisperEvent::Message { from, content } => {
                        let text = String::from_utf8_lossy(&content).to_string();
                        let label = self
                            .names
                            .get(&from)
                            .cloned()
                            .unwrap_or_else(|| from.fmt_short().to_string());

                        // Check if this is a private chat invitation marker.
                        let is_invite = text == "\x00PRIVATE_CHAT";
                        if is_invite {
                            self.push_system(format!(
                                "{label} opened a private chat with you."
                            ));
                        }

                        // If the sender is a tracked friend, auto-open the
                        // private gossip room so the receiving user sees the
                        // conversation immediately.
                        let fid = FriendId::from_public_key(from);
                        if self.friends.get(&fid).is_some() {
                            let private_topic = private_topic(&self.local_public, &from);
                            let already_on_topic = matches!(self.screen, Screen::Chat { topic } if topic == private_topic);
                            if !already_on_topic && !is_invite {
                                // Save current room before switching
                                self.save_room_to_history();
                                // Navigate to the private chat room
                                return iced::Task::done(AppMessage::OpenRoom(private_topic));
                            }
                        }

                        if !is_invite {
                            self.entries.push(ChatEntry::remote(
                                format!("Whisper from {label}"),
                                text,
                                None,
                                None, // whisper events carry no sent_at
                            ));
                        }
                    }
                    iroh_gossip::whisper::WhisperEvent::FileTransfer { from, name, ticket } => {
                        let label = self
                            .names
                            .get(&from)
                            .cloned()
                            .unwrap_or_else(|| from.fmt_short().to_string());
                        self.push_system(format!(
                            "[Whisper from {label}] File received: {name}. Use /download to fetch."
                        ));
                        self.pending_file = Some((name, ticket));
                    }
                    iroh_gossip::whisper::WhisperEvent::Connected { peer } => {
                        let label = self
                            .names
                            .get(&peer)
                            .cloned()
                            .unwrap_or_else(|| peer.fmt_short().to_string());
                        self.push_system(format!("[Whisper] Connected to {label}"));
                    }
                    iroh_gossip::whisper::WhisperEvent::Disconnected { peer } => {
                        let label = self
                            .names
                            .get(&peer)
                            .cloned()
                            .unwrap_or_else(|| peer.fmt_short().to_string());
                        self.push_system(format!("[Whisper] Disconnected from {label}"));
                    }
                }
                iced::Task::none()
            }

            AppMessage::MessageSent(text) => {
                self.push_local(text);
                iced::Task::none()
            }

            AppMessage::ExecuteFileSend(encoded) => {
                let parts: Vec<&str> = encoded.splitn(3, '|').collect();
                if parts.len() < 3 {
                    return iced::Task::none();
                }
                let filename = parts[0].to_string();
                let abs_path = parts[1].to_string();

                let blob_store = self.blob_store.clone();
                let sender = self.sender.clone();
                let secret_key = self.secret_key.clone();
                let fname = filename.clone();

                iced::Task::perform(
                    async move {
                        let tag = blob_store
                            .blobs()
                            .add_path(std::path::PathBuf::from(&abs_path))
                            .await
                            .map_err(|e| format!("Failed to hash file: {e}"))?;
                        let ticket_str = format!("blob:{:?}", tag.hash);
                        let msg = crate::Message::FileShare {
                            name: filename.clone(),
                            ticket: ticket_str,
                        };
                        let encoded = SignedMessage::sign_and_encode(&secret_key, &msg)
                            .map_err(|e| format!("Failed to sign: {e}"))?;
                        if let Some(ref sender) = sender {
                            sender.broadcast(encoded).await.ok();
                        }
                        Ok(fname)
                    },
                    |r: Result<String, String>| match r {
                        Ok(name) => AppMessage::FileSent(name),
                        Err(e) => AppMessage::ErrorMsg(e),
                    },
                )
            }

            AppMessage::ExecuteImageSend(encoded) => {
                let parts: Vec<&str> = encoded.splitn(3, '|').collect();
                if parts.len() < 3 {
                    return iced::Task::none();
                }
                let filename = parts[0].to_string();
                let abs_path = parts[1].to_string();

                let blob_store = self.blob_store.clone();
                let sender = self.sender.clone();
                let secret_key = self.secret_key.clone();
                let fname = filename.clone();
                let local_pk = self.local_public;

                iced::Task::perform(
                    async move {
                        let path_buf = std::path::PathBuf::from(&abs_path);
                        let image_bytes = tokio::fs::read(&path_buf)
                            .await
                            .map_err(|e| format!("Failed to read image: {e}"))?;
                        let tag = blob_store
                            .blobs()
                            .add_path(path_buf)
                            .await
                            .map_err(|e| format!("Failed to hash image: {e}"))?;
                        let hash: MessageHash = *tag.hash.as_bytes();
                        let msg = crate::Message::ImageShare {
                            name: filename.clone(),
                            hash,
                        };
                        let encoded = SignedMessage::sign_and_encode(&secret_key, &msg)
                            .map_err(|e| format!("Failed to sign: {e}"))?;
                        if let Some(ref sender) = sender {
                            sender.broadcast(encoded).await.ok();
                        }
                        Ok((local_pk, fname, image_bytes))
                    },
                    |r: Result<(PublicKey, String, Vec<u8>), String>| match r {
                        Ok((sender_pk, name, bytes)) => AppMessage::ImageDownloaded {
                            sender: sender_pk,
                            name,
                            image_bytes: bytes,
                        },
                        Err(e) => AppMessage::ErrorMsg(e),
                    },
                )
            }

            AppMessage::ExecuteDownload => {
                let pending = self.pending_file.clone();
                match pending {
                    Some((filename, ticket_str)) => {
                        let blob_store = self.blob_store.clone();
                        let endpoint = self.endpoint.clone();
                        let neighbors = self.neighbors.clone();
                        iced::Task::perform(
                            async move {
                                let ticket: BlobTicket = ticket_str
                                    .parse::<BlobTicket>()
                                    .map_err(|e| format!("Parse ticket: {e}"))?;
                                let peer_id = ticket.addr().id;
                                let candidates = download_candidates(peer_id, &neighbors);
                                blob_store
                                    .downloader(&endpoint)
                                    .download(ticket.hash(), candidates)
                                    .await
                                    .map_err(|e| format!("Download: {e}"))?;
                                let dest =
                                    std::env::current_dir().unwrap_or_default().join(&filename);
                                blob_store
                                    .blobs()
                                    .export(ticket.hash(), dest)
                                    .await
                                    .map_err(|e| format!("Export: {e}"))?;
                                Ok(filename)
                            },
                            |r: Result<String, String>| match r {
                                Ok(name) => AppMessage::DownloadDone(name),
                                Err(e) => AppMessage::ErrorMsg(e),
                            },
                        )
                    }
                    None => iced::Task::done(AppMessage::ErrorMsg(
                        "No pending file to download.".into(),
                    )),
                }
            }

            AppMessage::FileSent(name) => {
                self.push_system(format!("Sharing: {name}"));
                iced::Task::none()
            }
            AppMessage::DownloadDone(name) => {
                self.push_system(format!("Saved: {name}"));
                self.pending_file = None;
                iced::Task::none()
            }
            AppMessage::ImageDownloaded {
                sender,
                name,
                image_bytes,
            } => {
                let sender_name = self
                    .names
                    .get(&sender)
                    .cloned()
                    .unwrap_or_else(|| sender.fmt_short().to_string());
                self.entries.push(ChatEntry::image(
                    &sender_name,
                    format!("[Image: {name}]"),
                    image_bytes,
                    None,
                    None,
                ));
                iced::Task::none()
            }
            AppMessage::ErrorMsg(msg) => {
                self.push_system(msg);
                iced::Task::none()
            }

            AppMessage::FriendAdded {
                fid,
                label,
                was_new,
            } => {
                let friend_id = FriendId::new(fid);
                self.friends.ensure_friend(friend_id.clone());
                if self
                    .friends
                    .get(&friend_id)
                    .and_then(|r| r.label.clone())
                    .is_some()
                {
                    // Already has a label
                } else if label != friend_id.as_str().chars().take(12).collect::<String>() {
                    self.friends.set_label(friend_id, &label);
                }
                self.friends_dirty = true;
                if was_new {
                    self.push_system(format!("Added friend: {label}"));
                } else {
                    self.push_system(format!("Updated friend: {label}"));
                }
                self.try_save_friends();
                iced::Task::none()
            }

            AppMessage::FriendRemoved { label } => {
                self.push_system(format!("Removed friend: {label}"));
                iced::Task::none()
            }

            AppMessage::DeleteRoom(topic) => {
                // Remove room and chat history, then persist.
                self.room_history.remove(&topic);
                self.room_history_dirty = true;
                self.chat_history.lock().unwrap().remove_topic(&topic);
                self.chat_history_dirty = true;
                self.persist_room_history();
                self.try_save_chat_history();
                iced::Task::none()
            }

            AppMessage::FriendListResult(items) => {
                if items.is_empty() {
                    self.push_system("No friends tracked yet.");
                } else {
                    self.push_system(format!("Friends ({}):", items.len()));
                    for (peer, status) in &items {
                        self.push_system(format!("  {peer}: {status}"));
                    }
                }
                iced::Task::none()
            }

            AppMessage::ConnMonitorTick => {
                // Periodic connection type refresh (~60s).
                if self.conn_refresh_counter == 0 {
                    self.recompute_connection_counts();
                    self.conn_refresh_counter = 60;
                } else {
                    self.conn_refresh_counter -= 1;
                }

                // Poll Tor reconnection status updates
                if let Some(ref rx) = self.tor_reconnect_rx {
                    let msgs: Vec<String> = match rx.try_lock() {
                        Ok(mut guard) => {
                            let mut msgs = Vec::new();
                            loop {
                                match guard.try_recv() {
                                    Ok(msg) => msgs.push(msg),
                                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                                        break
                                    }
                                }
                            }
                            msgs
                        }
                        Err(_) => Vec::new(),
                    };
                    for msg in msgs {
                        self.push_system(msg);
                    }
                }

                // Periodic presence heartbeat — broadcasts Message::Presence every ~5s.
                let mut tasks: Vec<iced::Task<AppMessage>> = Vec::new();
                if self.presence_counter == 0 {
                    self.presence_counter = 5;
                    if let Some(ref sender) = self.sender {
                        let sk = self.secret_key.clone();
                        let s = sender.clone();
                        tasks.push(iced::Task::perform(
                            async move {
                                if let Ok(encoded) =
                                    SignedMessage::sign_and_encode(&sk, &crate::Message::Presence)
                                {
                                    s.broadcast(encoded).await.ok();
                                }
                            },
                            |_| AppMessage::Noop,
                        ));
                    }
                } else {
                    self.presence_counter -= 1;
                }

                // Periodic invisible keepalive heartbeat — broadcasts Message::Heartbeat
                // every ~2s to keep connections warm and update mesh health timestamps
                // without producing any chat log entry or UI notification.
                if self.heartbeat_counter == 0 {
                    self.heartbeat_counter = 2;
                    if let Some(ref sender) = self.sender {
                        let sk = self.secret_key.clone();
                        let s = sender.clone();
                        tasks.push(iced::Task::perform(
                            async move {
                                if let Ok(encoded) =
                                    SignedMessage::sign_and_encode(&sk, &crate::Message::Heartbeat)
                                {
                                    s.broadcast(encoded).await.ok();
                                }
                            },
                            |_| AppMessage::Noop,
                        ));
                    }
                } else {
                    self.heartbeat_counter -= 1;
                }

                if tasks.is_empty() {
                    iced::Task::none()
                } else {
                    iced::Task::batch(tasks)
                }
            }

            AppMessage::MeshWatchdogTick => {
                // Periodic mesh quiescence check — monitors for prolonged inactivity.
                let new_health = if self.sender.is_none() {
                    MeshHealth::Offline("Not connected to any room".to_string())
                } else if self.neighbors.is_empty() {
                    MeshHealth::Degraded("No peers in the mesh".to_string())
                } else {
                    MeshHealth::Good
                };

                // Detect transitions and push system notifications.
                let notification = match (&self.last_mesh_health, &new_health) {
                    (Some(MeshHealth::Good), MeshHealth::Degraded(reason)) => {
                        Some(format!("⚠ Mesh health degraded: {reason}"))
                    }
                    (Some(MeshHealth::Good), MeshHealth::Offline(reason)) => {
                        Some(format!("⚠ Mesh offline: {reason}"))
                    }
                    (Some(MeshHealth::Degraded(_)), MeshHealth::Good) => {
                        Some("✓ Mesh health recovered: all peers are active.".to_string())
                    }
                    (Some(MeshHealth::Offline(_)), MeshHealth::Good) => {
                        Some("✓ Mesh health recovered: endpoint is back online.".to_string())
                    }
                    (None, _) => None,
                    _ => None,
                };

                self.mesh_health = new_health;
                self.last_mesh_health = Some(self.mesh_health.clone());

                if let Some(msg) = notification {
                    self.push_system(msg);
                }

                iced::Task::none()
            }

            AppMessage::TorReconnect(msg) => {
                self.push_system(msg);
                iced::Task::none()
            }

            AppMessage::ToggleDark(enabled) => {
                self.dark_mode = enabled;
                iced::Task::none()
            }

            AppMessage::Noop => iced::Task::none(),

            AppMessage::CopyToClipboard(text) => {
                return iced::clipboard::write(text);
            }
        }
    }

    fn persist_room_history(&mut self) {
        if self.room_history_dirty {
            let _ = self.room_history.save();
            self.room_history_dirty = false;
        }
    }

    fn update_room_preview(&mut self, event: &NetEvent) {
        if let NetEvent::Message {
            from: _, message, ..
        } = event
        {
            if let Message::Message { text } = message {
                let preview = if text.len() > 60 {
                    format!("{}…", &text[..60])
                } else {
                    text.clone()
                };
                self.room_history.update_preview(&self.topic, &preview);
                self.room_history_dirty = true;
            }
        }
    }

    fn try_save_friends(&mut self) {
        if self.friends_dirty {
            let _ = self.friends.save();
            self.friends_dirty = false;
        }
    }

    fn try_save_chat_history(&mut self) {
        if self.chat_history_dirty {
            let _ = self.chat_history.lock().unwrap().save();
            self.chat_history_dirty = false;
        }
    }
}

// ── Net event handling ────────────────────────────────────────────────

impl IcedChat {
    /// Resolve a peer identifier (public key string or friend alias) to a [`PublicKey`].
    fn resolve_peer_key(&self, target: &str) -> Option<PublicKey> {
        if let Ok(pk) = target.parse::<PublicKey>() {
            return Some(pk);
        }
        // Try to resolve by friend alias.
        self.friends
            .iter()
            .find(|(_, rec)| rec.label.as_deref() == Some(target))
            .and_then(|(fid, _)| fid.parse_public_key().ok())
    }
    /// Query the iroh endpoint for each neighbor to recompute direct/relay counts.
    fn recompute_connection_counts(&mut self) {
        let mut direct = 0usize;
        let mut relayed = 0usize;
        let rt = self.runtime_handle.clone();
        for peer in &self.neighbors {
            let has_direct = rt
                .block_on(async { self.endpoint.remote_info(*peer).await })
                .map(|info| info.addrs().any(|a| !a.addr().is_relay()))
                .unwrap_or(false);
            if has_direct {
                direct += 1;
            } else {
                relayed += 1;
            }
        }
        self.direct_peers = direct;
        self.relayed_peers = relayed;
    }

    fn handle_friend_event(&mut self, event: FriendEvent) {
        match event {
            FriendEvent::StatusChanged { peer, status } => {
                let fid = FriendId::from_public_key(peer);
                let label = self
                    .friends
                    .get(&fid)
                    .map(|r| r.display_label(&fid))
                    .unwrap_or_else(|| peer.fmt_short().to_string());

                // Only show system messages for runtime transitions, not the
                // initial scan. A friend with no last_seen_at or last_offline_at
                // is being heard from for the first time.
                let has_been_seen = self
                    .friends
                    .get(&fid)
                    .map(|r| {
                        r.status.last_seen_at_unix_ms.is_some()
                            || r.status.last_offline_at_unix_ms.is_some()
                    })
                    .unwrap_or(false);

                match status {
                    FriendStatus::Online => {
                        self.friends.mark_online(fid);
                        self.friends_dirty = true;
                        self.friend_online_cache.insert(peer);
                        if has_been_seen {
                            self.push_system(format!("Friend {label} is now ONLINE"));
                        }
                    }
                    FriendStatus::Offline => {
                        self.friends.mark_offline(fid);
                        self.friends_dirty = true;
                        self.friend_online_cache.remove(&peer);
                        if has_been_seen {
                            self.push_system(format!("Friend {label} is now offline"));
                        }
                    }
                    FriendStatus::Unknown => {}
                }
            }
        }
    }
}

// ── ChatCallbacks impl for IcedChat ────────────────────────────────────

impl ChatCallbacks for IcedChat {
    fn local_public(&self) -> PublicKey {
        self.local_public
    }

    fn resolve_name(&self, peer: &PublicKey) -> String {
        // Priority: friend label > friend's last announced name > session name > short key.
        let fid = FriendId::from_public_key(*peer);
        if let Some(record) = self.friends.get(&fid) {
            if let Some(label) = &record.label {
                return label.clone();
            }
            if let Some(name) = &record.last_announced_name {
                return name.clone();
            }
        }
        self.names
            .get(peer)
            .cloned()
            .unwrap_or_else(|| peer.fmt_short().to_string())
    }

    fn set_name(&mut self, peer: PublicKey, name: String) {
        self.names.insert(peer, name);
    }

    fn is_friend(&self, peer: &PublicKey) -> bool {
        let fid = FriendId::from_public_key(*peer);
        self.friends.get(&fid).is_some()
    }

    fn friend_mark_online(&mut self, fid: FriendId) {
        self.friends.mark_online(fid);
    }

    fn friend_mark_offline(&mut self, fid: FriendId) {
        self.friends.mark_offline(fid);
    }

    fn friend_set_name(&mut self, fid: FriendId, name: String) {
        self.friends.set_last_announced_name(fid, name);
    }

    fn mark_friends_dirty(&mut self) {
        self.friends_dirty = true;
    }

    fn push_system(&mut self, text: String) {
        self.entries.push(ChatEntry::system(text));
    }

    fn push_remote(&mut self, label: String, text: String, hash: Option<MessageHash>, sent_at: Option<u64>) {
        self.entries.push(ChatEntry::remote(label, text, hash, sent_at));
    }

    fn set_pending_file(&mut self, name: String, ticket: String) {
        self.pending_file = Some((name, ticket));
    }

    fn set_pending_image(&mut self, name: String, hash: MessageHash, from: PublicKey) {
        self.pending_image = Some((name, hash, from));
    }

    fn has_message(&self, hash: &MessageHash) -> bool {
        self.entries
            .iter()
            .any(|e| e.message_hash.as_ref() == Some(hash))
    }

    fn edit_message(&mut self, hash: &MessageHash, new_text: String) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.message_hash.as_ref() == Some(hash))
        {
            entry.body = new_text.clone();
            entry.edited = true;
        }
    }

    fn delete_message(&mut self, hash: &MessageHash) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.message_hash.as_ref() == Some(hash))
        {
            entry.body = "[message deleted]".to_string();
            entry.edited = false;
            entry.reactions.clear();
        }
    }

    fn add_reaction(&mut self, hash: &MessageHash, emoji: String) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.message_hash.as_ref() == Some(hash))
        {
            entry.reactions.push(emoji);
        }
    }

    fn on_neighbor_up(&mut self, peer: PublicKey) {
        self.neighbors.insert(peer);
        self.recompute_connection_counts();
    }

    fn on_neighbor_down(&mut self, peer: PublicKey) {
        self.neighbors.remove(&peer);
        self.recompute_connection_counts();
    }

    fn record_activity(&mut self, _peer: PublicKey) {}

    fn request_quit(&mut self) {
        // IcedChat handles window close through the iced framework.
    }
}

// ── View ──────────────────────────────────────────────────────────────

impl IcedChat {
    /// Muted secondary text color, adapted to current theme.
    fn color_muted(&self) -> Color {
        if self.dark_mode {
            Color::from_rgb(0.6, 0.6, 0.6)
        } else {
            Color::from_rgb(0.4, 0.4, 0.4)
        }
    }

    /// Return the iced Theme enum matching the current dark_mode toggle.
    fn theme(&self) -> iced::Theme {
        if self.dark_mode {
            iced::Theme::Dark
        } else {
            iced::Theme::Light
        }
    }

    pub fn view(&self) -> iced::Element<'_, AppMessage> {
        let inner: iced::Element<'_, AppMessage> = match self.screen {
            Screen::ChatList => self.view_chat_list().into(),
            Screen::Chat { .. } => self.view_chat_screen().into(),
        };
        // Every view is wrapped in the primary background so the entire
        // window responds to the theme toggle — not just text colors.
        iced::widget::container(inner)
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .style(move |t| container_primary(t))
            .into()
    }

    // ── Chat list (inbox) view ───────────────────────────────────────

    fn view_chat_list(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, container, row, scrollable, text, text_input, Column, Space};
        use iced::{Alignment, Color, Length};
        let theme = self.theme();

        let mut content = Column::new().spacing(SPACE_12).padding(SPACE_16);

        // ── Identity card ──
        content = content.push(
            container(
                Column::new()
                    .push(row![text("Iroh Gossip Chat").size(TYPO_XL),].spacing(SPACE_8))
                    .push(
                        text(format!(
                            "Identity: {}  |  Relay: {}",
                            self.local_label,
                            fmt_relay_mode(&self.relay_mode)
                        ))
                        .size(TYPO_XS)
                        .color(self.color_muted()),
                    )
                    .spacing(SPACE_4),
            )
            .width(Length::Fill)
            .padding(SPACE_12)
            .style(move |t| container_surface(t)),
        );

        // Small visual pause before action buttons
        content = content.push(Space::new().height(Length::Fixed(SPACE_4)));

        // ── New Chat / Join buttons (surface) ──
        content = content.push(
            container(
                row![
                    button(
                        row![text(" ➕ ").size(TYPO_MD), text("New Chat").size(TYPO_MD),]
                            .align_y(Alignment::Center)
                            .spacing(SPACE_4),
                    )
                    .on_press(AppMessage::NewChatCreated)
                    .padding(SPACE_8),
                    button(
                        row![text(" 🔗 ").size(TYPO_MD), text("Join via Ticket").size(TYPO_MD),]
                            .align_y(Alignment::Center)
                            .spacing(SPACE_4),
                    )
                    .on_press(AppMessage::JoinFromTicket)
                    .padding(SPACE_8),
                ]
                .spacing(SPACE_8),
            )
            .width(Length::Fill)
            .padding(SPACE_12)
            .style(move |t| container_surface(t)),
        );

        // ── Join ticket input ──
        content = content.push(
            container(
                row![
                    text_input("Paste ticket to join a room…", &self.join_ticket_input)
                        .on_input(AppMessage::JoinTicketInputChanged)
                        .on_submit(AppMessage::JoinFromTicket)
                        .width(Length::Fill),
                ]
                .spacing(SPACE_4),
            )
            .width(Length::Fill)
            .padding(SPACE_12)
            .style(move |t| container_surface(t)),
        );

        // Error message
        if !self.chat_list_error.is_empty() {
            content = content.push(
                text(&self.chat_list_error)
                    .color(color_error(&theme))
                    .size(TYPO_SM),
            );
        }

        // ── Recent chats list ──
        {
            let mut section = Column::new().spacing(SPACE_8);
            section = section.push(
                row![
                    text("Recent Chats").size(TYPO_MD).width(Length::Fill),
                    text("(click room to open, click ✕ to remove)")
                        .size(TYPO_XXS)
                        .color(self.color_muted()),
                ]
                .spacing(SPACE_4),
            );
            if self.room_history.is_empty() {
                section = section.push(
                    text("No recent chats. Create a new chat or join an existing one.")
                        .color(self.color_muted())
                        .size(TYPO_SM),
                );
            } else {
                let mut list = Column::new().spacing(SPACE_2).width(Length::Fill);
                for room in &self.room_history.rooms {
                    list = list.push(self.view_room_row(room));
                }
                section = section.push(scrollable(list).height(Length::Fill));
            }
            content = content.push(
                container(section)
                    .width(Length::Fill)
                    .padding(SPACE_12)
                    .style(move |t| container_surface(t)),
            );
        }

        // ── All Friends ──
        {
            let mut section = Column::new().spacing(SPACE_8);
            section = section.push(
                row![
                    text("Friends").size(TYPO_MD).width(Length::Fill),
                    text(format!("{} total", self.friends.len()))
                        .size(TYPO_XXS)
                        .color(self.color_muted()),
                ]
                .spacing(SPACE_4),
            );
            if self.friends.is_empty() {
                section = section.push(
                    text("No friends yet. Add friends via /friend add <pk> in a chat.")
                        .color(self.color_muted())
                        .size(TYPO_SM),
                );
            } else {
                let mut friends_list = Column::new().spacing(SPACE_2).width(Length::Fill);
                let mut sorted: Vec<(&FriendId, &iroh_gossip::friends::FriendRecord)> =
                    self.friends.iter().collect();
                sorted.sort_by(|a, b| {
                    let label_a = a.1.display_label(a.0);
                    let label_b = b.1.display_label(b.0);
                    label_a.cmp(&label_b)
                });
                for (fid, record) in sorted {
                    if let Ok(pk) = fid.parse_public_key() {
                        friends_list = friends_list.push(self.view_friend_row(pk, fid, record));
                    }
                }
                section = section.push(scrollable(friends_list).height(Length::Shrink));
            }
            content = content.push(
                container(section)
                    .width(Length::Fill)
                    .padding(SPACE_12)
                    .style(move |t| container_surface(t)),
            );
        }

        // ── Discovered Users ──
        let mut discovered_users: Vec<(PublicKey, String)> = self
            .neighbors
            .iter()
            .copied()
            .filter(|peer| *peer != self.local_public)
            .filter(|peer| {
                let fid = FriendId::from_public_key(*peer);
                self.friends.get(&fid).is_none()
            })
            .map(|peer| (peer, self.resolve_name(&peer)))
            .collect();
        discovered_users.sort_by(|a, b| a.1.cmp(&b.1));
        // ── Discovered Users ──
        {
            let mut section = Column::new().spacing(SPACE_8);
            section = section.push(
                row![
                    text("Discovered Users").size(TYPO_MD).width(Length::Fill),
                    text(format!("{} user(s) discovered", discovered_users.len()))
                        .size(TYPO_XXS)
                        .color(self.color_muted()),
                ]
                .spacing(SPACE_4),
            );
            if discovered_users.is_empty() {
                section = section.push(
                    text("No other users discovered yet.")
                        .color(self.color_muted())
                        .size(TYPO_SM),
                );
            } else {
                let mut discovered_list = Column::new().spacing(SPACE_2).width(Length::Fill);
                for (pk, label) in discovered_users {
                    discovered_list = discovered_list.push(self.view_discovered_user_row(pk, &label));
                }
                section = section.push(scrollable(discovered_list).height(Length::Shrink));
            }
            content = content.push(
                container(section)
                    .width(Length::Fill)
                    .padding(SPACE_12)
                    .style(move |t| container_surface(t)),
            );
        }

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// A single row for a known friend: status indicator + label + last-seen + Chat button.
    fn view_friend_row(
        &self,
        pk: PublicKey,
        fid: &FriendId,
        record: &iroh_gossip::friends::FriendRecord,
    ) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, container, row, text};
        use iced::{Color, Length};
        let theme = self.theme();

        let label = record.display_label(fid);
        let online = self.friend_online_cache.contains(&pk);

        let status_emoji = if online { "🟢" } else { "🔴" };
        let status_color = if online {
            Color::from_rgb(0.2, 0.7, 0.2)
        } else {
            self.color_muted()
        };

        let last_seen_str = if online {
            String::new()
        } else {
            format_last_seen(record.status.last_seen_at_unix_ms)
        };

        container(
            row![
                text(status_emoji).size(TYPO_XS),
                text(label).size(TYPO_MD).width(Length::Fill),
                text(last_seen_str)
                    .size(TYPO_XS)
                    .color(status_color),
                button("💬 Chat")
                    .on_press(AppMessage::OpenFriendChat(pk))
                    .padding(SPACE_4),
            ]
            .spacing(SPACE_8)
            .align_y(iced::Alignment::Center)
            .padding(SPACE_8),
        )
        .width(Length::Fill)
        .style(move |t| container_surface(t))
        .into()
    }

    /// A single row for a discovered (non-friend) user.
    fn view_discovered_user_row(
        &self,
        pk: PublicKey,
        label: impl Into<String>,
    ) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, container, row, text};
        use iced::Length;
        let theme = self.theme();

        let label = label.into();

        container(
            row![
                text("🟢").size(TYPO_XS),
                text(label).size(TYPO_MD).width(Length::Fill),
                button("💬 Chat")
                    .on_press(AppMessage::OpenFriendChat(pk))
                    .padding(SPACE_4),
            ]
            .spacing(SPACE_8)
            .align_y(iced::Alignment::Center)
            .padding(SPACE_8),
        )
        .width(Length::Fill)
        .style(move |t| container_surface(t))
        .into()
    }

    fn view_room_row(&self, room: &RoomHistoryEntry) -> iced::Element<'_, AppMessage> {
        use iced::widget::text::Wrapping;
        use iced::widget::{button, column, container, row, text};
        use iced::Length;

        let topic = room.topic;
        let display_name = room.display_name();

        let preview = if room.last_preview.is_empty() {
            if room.is_owner {
                "Created this room".to_string()
            } else {
                "Joined this room".to_string()
            }
        } else {
            room.last_preview.clone()
        };

        let btn = button(
            row![
                column![
                row![text(display_name)
                    .size(TYPO_MD)
                    .width(Length::Fill)
                    .wrapping(Wrapping::Word),],
                row![text(preview)
                    .size(TYPO_XS)
                    .color(self.color_muted())
                    .width(Length::Fill)
                    .wrapping(Wrapping::Word),],
                ]
                .spacing(SPACE_2)
                .padding(SPACE_8)
                .width(Length::Fill),
                button("✕")
                .on_press(AppMessage::DeleteRoom(topic))
                .padding(SPACE_4),
                ]
                .spacing(SPACE_4)
            .align_y(iced::Alignment::Center),
        )
        .on_press(AppMessage::RoomSelected(topic))
        .width(Length::Fill)
        .padding(0);

        container(btn).width(Length::Fill).into()
    }

    // ── Chat screen view ─────────────────────────────────────────────

    fn view_chat_screen(&self) -> iced::Element<'_, AppMessage> {
        use iced::{widget, Length};
        let theme = self.theme();

        let content = widget::column![
            self.view_chat_header(),
            self.view_chat_log(),
            self.view_composer(),
        ]
        .spacing(SPACE_8);

        let inner = widget::container(content)
            .padding(SPACE_16)
            .width(Length::Fill)
            .height(Length::Fill);

        if self.help_visible {
            widget::container(self.view_help())
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(move |t| container_primary(t))
                .into()
        } else {
            inner.style(move |t| container_primary(t)).into()
        }
    }

    fn view_chat_header(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::text::Wrapping;
        use iced::widget::{button, column, container, row, text};
        use iced::{Color, Length};
        let theme = self.theme();

        let topic_hex = self.topic.to_string();
        let short_topic = if topic_hex.len() > 8 {
            format!("{}…", &topic_hex[..8])
        } else {
            topic_hex.clone()
        };

        let room_name = self
            .room_history
            .find(&self.topic)
            .map(|r| r.display_name())
            .unwrap_or_else(|| format!("Room {}", short_topic));

        let mut header = column![
            row![
                button(" ◀ ").on_press(AppMessage::GoToChatList),
                text(room_name)
                    .size(TYPO_LG)
                    .width(Length::Fill)
                    .wrapping(Wrapping::Word),
                button(text(if self.dark_mode { "☀" } else { "🌙" }).size(TYPO_MD))
                    .on_press(AppMessage::ToggleDark(!self.dark_mode))
                    .padding(SPACE_4),
            ]
            .spacing(SPACE_4),
            text(format!(
                "{} direct · {} relay · {}",
                self.direct_peers,
                self.relayed_peers,
                fmt_relay_mode(&self.relay_mode),
            ))
            .size(TYPO_XS)
            .color(self.color_muted())
            .width(Length::Fill)
            .wrapping(Wrapping::Glyph),
        ]
        .spacing(SPACE_4);

        if !self.ticket_str.is_empty() {
            let ticket = self.ticket_str.clone();
            header = header.push(
                button(text(&self.ticket_str).size(TYPO_XXS).wrapping(Wrapping::Word))
                    .on_press(AppMessage::CopyToClipboard(ticket))
                    .padding(0)
                    .style(button::text),
            );
        }

        container(header)
            .width(Length::Fill)
            .padding(SPACE_12)
            .style(move |t| container_surface(t))
            .into()
    }
    fn view_chat_log(&self) -> iced::widget::Scrollable<'_, AppMessage> {
        use iced::widget::space;
        use iced::widget::text::Wrapping;
        use iced::widget::{container, scrollable, text, Column, Row};
        use iced::Length;

        let mut col = Column::new().spacing(SPACE_4).width(Length::Fill);
        let theme = self.theme();
        let mut prev_day: Option<i64> = None;

        for entry in self.entries.iter() {
            // ── Date separator ──
            let entry_day = entry.timestamp.map(|ts| ts / 86400000);
            if let Some(day) = entry_day {
                if prev_day.map_or(true, |prev| prev != day) {
                    let date_label = format_message_time(day * 86400000);
                    let sep_text = format!(" — {date_label} — ");
                    let sep = Row::new()
                        .push(space::horizontal())
                        .push(
                            text(sep_text)
                                .size(TYPO_XS)
                                .color(text_system(&theme)),
                        )
                        .push(space::horizontal())
                        .width(Length::Fill)
                        .padding([SPACE_8, 0.0]);
                    col = col.push(sep);
                    prev_day = Some(day);
                }
            }

            // ── System messages: centered, no bubble ──
            if matches!(entry.kind, ChatKind::System) {
                let system_row = Row::new()
                    .push(space::horizontal())
                    .push(
                        text(&entry.body)
                            .size(TYPO_SM)
                            .wrapping(Wrapping::Word)
                            .color(text_system(&theme)),
                    )
                    .push(space::horizontal())
                    .width(Length::Fill);
                col = col.push(system_row);
                continue;
            }

            // ── Local / Remote messages ──
            let label_color = match entry.kind {
                ChatKind::Local => text_local_label(&theme),
                ChatKind::Remote => text_remote_label(&theme),
                _ => unreachable!(),
            };
            let body_color = match entry.kind {
                ChatKind::Local => text_local_body(&theme),
                ChatKind::Remote => text_remote_body(&theme),
                _ => unreachable!(),
            };

            // Nickname label sits above the bubble (Signal/WhatsApp style)
            let label_el = text(format!("[{}]", entry.label))
                .size(TYPO_XS)
                .color(label_color);

            // Body text inside the bubble
            let body_el = text(&entry.body)
                .size(TYPO_SM)
                .wrapping(Wrapping::Word)
                .width(Length::Fill)
                .color(body_color);

            // The speech bubble container
            let bubble = container(body_el)
                .padding([SPACE_4, SPACE_8])
                .style(move |t: &iced::Theme| {
                    let mut s = iced::widget::container::Style::default();
                    if let Some(bg) = bubble_bg(t, entry.kind) {
                        s.background = Some(bg);
                    }
                    s.border.radius = (8.0_f32).into();
                    s
                });

            // Timestamp label (small, muted, right-aligned)
            let ts_text = entry
                .timestamp
                .map(format_message_time)
                .unwrap_or_default();
            let ts_el = text(ts_text)
                .size(TYPO_XXS)
                .color(text_muted(&theme));

            // Column: label above, bubble in middle, timestamp below
            let bubble_col = Column::new()
                .push(label_el)
                .push(bubble)
                .push(ts_el)
                .spacing(SPACE_2)
                .max_width(480.0);

            // Align: received → left, sent → right
            let msg_row = match entry.kind {
                ChatKind::Remote => Row::new().push(bubble_col).push(space::horizontal()),
                ChatKind::Local => Row::new().push(space::horizontal()).push(bubble_col),
                _ => unreachable!(),
            }
            .width(Length::Fill);

            col = col.push(msg_row);

            // ── Image ──
            if let Some(ref handle) = entry.image_handle {
                let img = iced::widget::image(handle.clone())
                    .content_fit(iced::ContentFit::ScaleDown)
                    .width(Length::Fill)
                    .height(Length::Fixed(300.0));
                col = col.push(img);
            }

            // ── Reactions ──
            if !entry.reactions.is_empty() {
                let reactions_text = entry.reactions.join("  ");
                let reactions_line = Row::new()
                    .push(
                        text(reactions_text)
                            .color(text_muted(&theme))
                            .size(TYPO_SM)
                            .wrapping(Wrapping::Word)
                            .width(Length::Fill),
                    )
                    .spacing(0)
                    .padding([0.0, SPACE_8])
                    .width(Length::Fill);
                col = col.push(reactions_line);
            }
        }

        if self.entries.is_empty() {
            col = col.push(
                container(text("No messages yet.").color(self.color_muted()))
                    .padding([0.0, SPACE_8])
                    .width(Length::Fill),
            );
        }

        scrollable(col)
            .id(CHAT_LOG)
            .anchor_bottom()
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
    }

    fn view_composer(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, container, row, text, text_input};
        use iced::{Alignment, Color, Length};
        let theme = self.theme();
        let has_text = !self.composer_text.is_empty();

        // ── Tertiary: help button ── smallest, subdued, sits at the edge
        let help_btn = button(text("❓").size(TYPO_XS))
            .on_press(AppMessage::ToggleHelp)
            .style(move |t: &iced::Theme, _status| {
                let mut s = iced::widget::button::Style::default();
                s.background = None;
                s.text_color = if matches!(t, iced::Theme::Dark) {
                    Color::from_rgb(0.40, 0.40, 0.40)
                } else {
                    Color::from_rgb(0.55, 0.55, 0.55)
                };
                s.border.radius = 0.0.into();
                s
            })
            .padding([SPACE_2, SPACE_4]);

        // ── Secondary: attach button ── subdued but visible
        let attach_btn = button(text("📎").size(TYPO_SM))
            .on_press(AppMessage::AttachPressed)
            .style(move |t: &iced::Theme, _status| {
                let mut s = iced::widget::button::Style::default();
                s.background = None;
                s.text_color = if matches!(t, iced::Theme::Dark) {
                    Color::from_rgb(0.50, 0.50, 0.50)
                } else {
                    Color::from_rgb(0.45, 0.45, 0.45)
                };
                s.border.radius = 0.0.into();
                s
            })
            .padding([SPACE_4, SPACE_6]);

        // ── Primary: send button ── filled accent colour when text exists,
        // ghost when empty (progressive disclosure)
        let send_btn = button(text("➤").size(TYPO_MD))
            .on_press(AppMessage::SendPressed)
            .style(move |t: &iced::Theme, _status| {
                let mut s = iced::widget::button::Style::default();
                if has_text {
                    // Filled primary — accent green
                    let accent = if matches!(t, iced::Theme::Dark) {
                        Color::from_rgb(0.20, 0.72, 0.20)
                    } else {
                        Color::from_rgb(0.05, 0.50, 0.05)   // #0d800d
                    };
                    s.background = Some(iced::Background::Color(accent));
                    s.text_color = Color::WHITE;
                    s.border.radius = 6.0.into();
                } else {
                    // Ghost when empty — same as attach styling
                    s.background = None;
                    s.text_color = if matches!(t, iced::Theme::Dark) {
                        Color::from_rgb(0.40, 0.40, 0.40)
                    } else {
                        Color::from_rgb(0.50, 0.50, 0.50)
                    };
                    s.border.radius = 0.0.into();
                }
                s
            })
            .padding([SPACE_4, SPACE_8]);

        // ── Action button group ── tighter spacing, secondary + primary + tertiary
        let actions = row![
            attach_btn,
            send_btn,
            help_btn,
        ]
        .spacing(SPACE_2)
        .align_y(Alignment::Center);

        // ── Main composer row ──
        let composer = row![
            text_input("Type a message…", &self.composer_text)
                .on_input(AppMessage::InputChanged)
                .on_submit(AppMessage::SendPressed)
                .width(Length::Fill),
            actions,
        ]
        .spacing(SPACE_6)
        .align_y(Alignment::Center)
        .padding([SPACE_4, SPACE_8]);

        // ── Wrapping container ── subtle border + distinct background
        container(composer)
            .width(Length::Fill)
            .style(move |t: &iced::Theme| {
                let mut s = iced::widget::container::Style::default();
                s.border = iced::Border {
                    width: 1.0,
                    color: if matches!(t, iced::Theme::Dark) {
                        Color::from_rgb(0.23, 0.23, 0.25)
                    } else {
                        Color::from_rgb(0.80, 0.80, 0.82)
                    },
                    radius: 8.0.into(),
                };
                s.background = Some(iced::Background::Color(
                    if matches!(t, iced::Theme::Dark) {
                        Color::from_rgb(0.10, 0.10, 0.12)
                    } else {
                        Color::from_rgb(0.97, 0.97, 0.98)
                    },
                ));
                s
            })
            .into()
    }

    fn view_help(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, container, text, Column, Space};
        use iced::{Alignment, Length};
        let theme = self.theme();

        let col = Column::new()
            .push(text("Help").size(TYPO_LG))
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(text("/send <path>    Share a file with peers").size(TYPO_SM))
            .push(text("/image <path>   Share an image inline").size(TYPO_SM))
            .push(text("/download       Fetch the last shared file").size(TYPO_SM))
            .push(text(
                "/leave          Leave this room and delete from history",
            ).size(TYPO_SM))
            .push(text("/help           Toggle this menu").size(TYPO_SM))
            .push(text(
                "/friend add <pk> [alias]  Track a friend's online status",
            ).size(TYPO_SM))
            .push(text("/friend remove <pk|alias> Stop tracking a friend").size(TYPO_SM))
            .push(text(
                "/friend list    List tracked friends and their status",
            ).size(TYPO_SM))
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(text("/react <idx> <emoji>  Add a reaction to a message").size(TYPO_SM))
            .push(text("/edit <idx> <text>   Edit a message").size(TYPO_SM))
            .push(text("/delete <idx>        Delete a message").size(TYPO_SM))
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(text("Type a message and press Enter to send.").size(TYPO_SM))
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(text(
                "Tip: click ✕ on a room in the chat list to remove it.",
            ).size(TYPO_SM))
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(button("❌").on_press(AppMessage::ToggleHelp))
            .spacing(SPACE_6)
            .padding(SPACE_24)
            .align_x(Alignment::Center);

        container(col)
            .width(Length::Shrink)
            .height(Length::Shrink)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(move |t| container_surface(t))
            .into()
    }
}

// ── Subscription ──────────────────────────────────────────────────────

struct RxHandle(Arc<Mutex<UnboundedReceiver<NetEvent>>>);

impl std::hash::Hash for RxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

struct FriendRxHandle(Arc<Mutex<UnboundedReceiver<FriendEvent>>>);

impl std::hash::Hash for FriendRxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

struct WhisperRxHandle(Arc<Mutex<UnboundedReceiver<WhisperEvent>>>);

impl std::hash::Hash for WhisperRxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

fn subscription_stream(
    rx: &RxHandle,
    friend_rx: &FriendRxHandle,
    whisper_rx: &WhisperRxHandle,
) -> Pin<Box<dyn Stream<Item = AppMessage> + Send>> {
    let rx = Arc::clone(&rx.0);
    let friend_rx = Arc::clone(&friend_rx.0);
    let whisper_rx = Arc::clone(&whisper_rx.0);
    Box::pin(n0_future::stream::unfold(
        (rx, friend_rx, whisper_rx),
        |(rx, friend_rx, whisper_rx)| async move {
            let mut rx_guard = rx.lock().await;
            let mut friend_guard = friend_rx.lock().await;
            let mut whisper_guard = whisper_rx.lock().await;
            tokio::select! {
                event = rx_guard.recv() => {
                    drop(whisper_guard);
                    drop(friend_guard);
                    drop(rx_guard);
                    event.map(|e| (AppMessage::NetEvent(e), (rx, friend_rx, whisper_rx)))
                }
                event = friend_guard.recv() => {
                    drop(whisper_guard);
                    drop(rx_guard);
                    drop(friend_guard);
                    event.map(|e| (AppMessage::FriendEvent(e), (rx, friend_rx, whisper_rx)))
                }
                event = whisper_guard.recv() => {
                    drop(friend_guard);
                    drop(rx_guard);
                    drop(whisper_guard);
                    event.map(|e| (AppMessage::WhisperEvent(e), (rx, friend_rx, whisper_rx)))
                }
            }
        },
    ))
}

impl IcedChat {
    pub fn subscription(
        rx: Arc<Mutex<UnboundedReceiver<NetEvent>>>,
        friend_rx: Arc<Mutex<UnboundedReceiver<FriendEvent>>>,
        whisper_rx: Arc<Mutex<UnboundedReceiver<WhisperEvent>>>,
    ) -> iced::Subscription<AppMessage> {
        iced::Subscription::batch(vec![
            iced::time::every(std::time::Duration::from_secs(1))
                .map(|_| AppMessage::ConnMonitorTick),
            iced::time::every(std::time::Duration::from_secs(30))
                .map(|_| AppMessage::MeshWatchdogTick),
            iced::Subscription::run_with(
                (RxHandle(rx), FriendRxHandle(friend_rx), WhisperRxHandle(whisper_rx)),
                |(rx, friend_rx, whisper_rx)| subscription_stream(&rx, &friend_rx, &whisper_rx),
            ),
        ])
    }
}
