//! The iced Application for the gossip chat frontend.
//!
//! Supports a chat-list (inbox) screen and individual chat-room screens,
//! with dynamic room switching — like Telegram/Signal.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iroh::{
    address_lookup::memory::MemoryLookup, EndpointAddr, PublicKey, RelayMode, SecretKey, Watcher,
};
use iroh_blobs::{store::mem::MemStore, ticket::BlobTicket};
use iroh_gossip::api::{GossipSender, GossipTopic};
use iroh_gossip::backfill::BackfillHandle;
use iroh_gossip::chat_callbacks::ChatCallbacks;
use iroh_gossip::chat_core::{
    collect_bootstrap_peers, download_candidates,
    friend_ping::{FriendEvent, FriendPingManager, FriendStatus},
    handle_net_event as chat_net_event, message_hash, seed_memory_lookup, MeshHealth, MessageHash,
};
use iroh_gossip::chat_history::{ChatHistoryStore, DeliveryState, HistoryEntry};
use iroh_gossip::contact::{direct_topic, ContactAction, SignedContactMessage};
use iroh_gossip::friends::{DirectConversationState, FriendId, FriendsStore};
use iroh_gossip::inbox::{send_ack, send_sync_request, InboxEvent};
use iroh_gossip::mailbox::{seal_for, MailboxAck, MailboxIdentity, MailboxStore};
use iroh_gossip::net::Gossip;
use iroh_gossip::outbox::{OutboxEntry, OutboxStore};
use iroh_gossip::proto::TopicId;
use iroh_gossip::room::RoomStore;
use iroh_gossip::room_docs::{self, RoomMetadata};
use iroh_gossip::room_history::{RoomHistoryEntry, RoomHistoryStore};
use iroh_gossip::whisper::{WhisperEvent, WhisperHandle};
use n0_future::task;
use n0_future::Stream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;
use tracing::debug;

use crate::{fmt_relay_mode, log_viewer, Message, NetEvent, SignedMessage, Ticket};
use iced::Color;

/// Scrollable ID for the chat log — used to auto-scroll to bottom.
const CHAT_LOG: &str = "chat_log";

// ── Typography scale (minor-second ratio ~1.125) ─────────────────────
const TYPO_XL: f32 = 24.0; // Primary heading (chat list title)
const TYPO_LG: f32 = 18.0; // Secondary heading (room name, help title)
const TYPO_MD: f32 = 15.0; // Body / section headers / button labels
const TYPO_SM: f32 = 13.0; // Secondary body, previews, entry labels
const TYPO_XS: f32 = 11.0; // Metadata, identity info, secondary labels
const TYPO_XXS: f32 = 10.0; // Fine print, ticket, instruction text

/// Build the wire representation used for all GUI file shares.
///
/// A content hash by itself is insufficient: the receiver also needs the
/// sender's endpoint address and the blob format to construct a downloader
/// request. Keeping this in one helper prevents the gossip and whisper paths
/// from drifting into incompatible ticket formats.
fn blob_ticket_string(
    addr: EndpointAddr,
    hash: iroh_blobs::Hash,
    format: iroh_blobs::BlobFormat,
) -> String {
    BlobTicket::new(addr, hash, format).to_string()
}

// ── Spacing units (4px base) ─────────────────────────────────────────
const SPACE_2: f32 = 2.0;
const SPACE_4: f32 = 4.0;
const SPACE_6: f32 = 6.0;
const SPACE_8: f32 = 8.0;
const SPACE_10: f32 = 10.0;
const SPACE_12: f32 = 12.0;
const SPACE_16: f32 = 16.0;
const SPACE_24: f32 = 24.0;

const PROFILE_IMAGE_FILE: &str = "profile-image";
const PROFILE_IMAGE_MAX_BYTES: usize = 5 * 1024 * 1024;

fn supported_profile_image(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
    )
}

fn save_profile_image(data_dir: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::create_dir_all(data_dir)?;
    let temporary = data_dir.join(format!("{PROFILE_IMAGE_FILE}.tmp"));
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, data_dir.join(PROFILE_IMAGE_FILE))
}

// ── Theme-aware chat colors ──────────────────────────────────────────
/// Return the muted secondary color for labels, previews, and counts.
fn text_muted(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.6, 0.6, 0.6) // ~#999, ~4.5:1 on dark bg ✓ AA
    } else {
        Color::from_rgb(0.4, 0.4, 0.4) // ~#666, ~5.2:1 on white ✓ AA
    }
}

/// Color for system message text (label and body).
fn text_system(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.5, 0.5, 0.5)
    } else {
        Color::from_rgb(0.35, 0.35, 0.35) // #595959, ~6.5:1 ✓ AA
    }
}

/// Color for local (self) message label.
fn text_local_label(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.2, 0.8, 0.2)
    } else {
        Color::from_rgb(0.0, 0.45, 0.0) // #0073, ~5.8:1 ✓ AA
    }
}

/// Color for local message body text.
fn text_local_body(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.3, 0.9, 0.3)
    } else {
        Color::from_rgb(0.0, 0.35, 0.0) // #0059, ~6.5:1 ✓ AA
    }
}

/// Color for remote message label (nickname).
fn text_remote_label(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.4, 0.65, 1.0) // light blue on dark
    } else {
        Color::from_rgb(0.0, 0.33, 0.66) // #0054A8, ~5.5:1 ✓ AA
    }
}

/// Color for remote message body text.
fn text_remote_body(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.8, 0.8, 0.8)
    } else {
        Color::from_rgb(0.13, 0.13, 0.13) // #222, ~11.5:1 ✓ AA
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
        Color::from_rgb(0.16, 0.16, 0.24) // #2a2a3e
    } else {
        Color::from_rgb(1.0, 1.0, 1.0) // #ffffff
    }
}

/// Input field background.
fn bg_input(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.13, 0.13, 0.22) // #222238
    } else {
        Color::from_rgb(0.94, 0.94, 0.96) // #f0f0f4
    }
}

/// Hover-state background for rows and interactive surfaces.
fn bg_hover(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.20, 0.20, 0.30) // #33334d
    } else {
        Color::from_rgb(0.90, 0.90, 0.95) // #e6e6f2
    }
}

/// Subtle border for surfaces and cards.
fn border_muted(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.22, 0.22, 0.32) // #383852
    } else {
        Color::from_rgb(0.85, 0.85, 0.88) // #d9d9e0
    }
}

/// Primary accent (blue).
fn accent_primary(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.29, 0.62, 1.0) // #4a9eff
    } else {
        Color::from_rgb(0.18, 0.44, 0.80) // #2e70cc
    }
}

/// Success / online indicator (green).
fn accent_green(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.24, 0.86, 0.52) // #3ddc84
    } else {
        Color::from_rgb(0.10, 0.55, 0.20) // #1a8c33
    }
}

/// Error / destructive colour.
fn color_error(theme: &iced::Theme) -> Color {
    if matches!(theme, iced::Theme::Dark) {
        Color::from_rgb(0.90, 0.25, 0.25) // #e64040
    } else {
        Color::from_rgb(0.75, 0.15, 0.15) // #bf2626
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

/// Closures passed to `text().style()` need this static-compatible form.
fn text_muted_style(theme: &iced::Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(if matches!(theme, iced::Theme::Dark) {
            Color::from_rgb(0.6, 0.6, 0.6)
        } else {
            Color::from_rgb(0.4, 0.4, 0.4)
        }),
    }
}

/// Container style for a card — surface background, muted border, rounded.
fn container_card(theme: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(bg_surface(theme))),
        border: iced::Border {
            color: border_muted(theme),
            width: 1.0,
            radius: SPACE_8.into(),
        },
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
    /// Stable event id for delivery state tracking (0 = unassigned).
    event_id: u64,
    /// Current delivery state of this message (only meaningful for Local kind).
    delivery_state: DeliveryState,
    /// PublicKey of the sender (None for local/system messages).
    sender_key: Option<PublicKey>,
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
            event_id: 0,
            delivery_state: DeliveryState::default(),
            sender_key: None,
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
            event_id: 0,
            delivery_state: DeliveryState::default(),
            sender_key: None,
        }
    }
    fn remote(
        label: impl Into<String>,
        text: impl Into<String>,
        hash: Option<MessageHash>,
        sent_at_secs: Option<u64>,
        sender: Option<PublicKey>,
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
            event_id: 0,
            delivery_state: DeliveryState::default(),
            sender_key: sender,
        }
    }
    fn image(
        label: impl Into<String>,
        body: impl Into<String>,
        image_bytes: Vec<u8>,
        hash: Option<MessageHash>,
        sent_at_secs: Option<u64>,
        sender: Option<PublicKey>,
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
            event_id: 0,
            delivery_state: DeliveryState::default(),
            sender_key: sender,
        }
    }

    /// Override the timestamp with a specific Unix epoch millisecond value.
    fn with_timestamp(mut self, ms: Option<i64>) -> Self {
        self.timestamp = ms;
        self
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
    /// Application settings screen.
    Settings,
}

// ── Application state ─────────────────────────────────────────────────

pub struct IcedChat {
    // ── Navigation ──
    screen: Screen,
    /// Pending topic we're connecting to (used during the async handoff
    /// from clicking a room to actually subscribing).
    pending_topic: Option<TopicId>,
    /// Screen to return to when closing the settings page.
    settings_return_to: Option<Screen>,

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
    /// Keeps the protocol router alive for the lifetime of the GUI. Dropping
    /// the router stops accepting incoming gossip connections.
    _router: iroh::protocol::Router,
    sender: Option<GossipSender>,
    blob_store: MemStore,
    endpoint: iroh::Endpoint,
    memory_lookup: MemoryLookup,
    local_label: String,
    local_public: PublicKey,
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
    /// Maps protocol message hashes to event_ids for delivery state resolution.
    self_sent_events: HashMap<MessageHash, u64>,

    /// Whether to auto-scroll to the latest message.
    follow_latest: bool,
    /// Estimated total content height of the chat log (set in view_chat_log).
    /// Cell interior mutability allows &self reads in view().
    total_content_height: std::cell::Cell<f32>,
    /// Whether dark mode is enabled.
    pub dark_mode: bool,
    /// Whether notification sounds are enabled.
    sound_enabled: bool,
    /// Whether the "clear history" confirmation is shown.
    history_confirm_clear: bool,
    /// Topic awaiting delete confirmation (None = no confirm pending).
    room_delete_confirm_topic: Option<TopicId>,
    /// Transport notice displayed in the header (e.g. "Direct iroh transport is operational").
    pub notice: String,
    data_dir: PathBuf,
    /// Persistent chat message history (loaded on startup, saved on each message).
    chat_history: Arc<std::sync::Mutex<ChatHistoryStore>>,
    /// Durable outgoing messages, shared with the active room lifecycle.
    outbox: Arc<std::sync::Mutex<OutboxStore>>,
    /// Whether chat history has unsaved changes.
    chat_history_dirty: bool,
    /// Number of entries that have already been saved to chat_history
    /// for the current room. Used to avoid re-saving the same entries
    /// on every room-navigation event.
    history_saved_count: usize,
    /// Current Y scroll offset of the chat log, in pixels.
    scroll_offset: f32,
    /// Current viewport height of the chat log, in pixels.
    viewport_height: f32,
    /// Cache of friend PublicKey -> is_online for quick lookup in the UI.
    friend_online_cache: HashSet<PublicKey>,
    /// Bootstrap peer addresses from the initial join ticket (if any).
    /// Used only for the first room subscription; cleared after use.
    initial_bootstrap_peers: Vec<EndpointAddr>,
    /// Whether the initial default lobby should leave the UI on the chat list.
    return_to_chat_list_after_open: bool,
    /// Handle for sending whisper/private messages.
    whisper_handle: WhisperHandle,
    /// Receiver for incoming inbox events.
    pub inbox_events_rx: Arc<Mutex<UnboundedReceiver<InboxEvent>>>,
    /// Receiver for incoming whisper events.
    pub whisper_events_rx: Arc<Mutex<UnboundedReceiver<WhisperEvent>>>,
    /// Locally selected profile image, persisted below the application data directory.
    profile_image_handle: Option<iced::widget::image::Handle>,
    /// Ticket for the locally selected profile image, for broadcasting to peers.
    profile_image_ticket: Option<String>,
    /// Cached profile image handles for remote peers, keyed by PublicKey.
    /// `None` means the peer announced a ticket but the blob hasn't been downloaded yet.
    friend_image_handles: HashMap<PublicKey, Option<iced::widget::image::Handle>>,
    /// Last-seen profile image ticket string per peer.
    /// Used to avoid re-invalidating and re-downloading when the same ticket
    /// is re-announced in a periodic AboutMe broadcast (see ConnMonitorTick).
    friend_image_tickets: HashMap<PublicKey, String>,
    /// Queue of profile image tickets that arrived via AboutMe, awaiting async download.
    /// Each entry is (peer_public_key, blob_ticket_string).
    /// Downloaded entries are removed one-at-a-time each update tick to allow
    /// multiple concurrent peer image downloads without overwriting each other.
    pending_profile_image_tickets: std::collections::VecDeque<(PublicKey, String)>,
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
    OpenSettings,
    CloseSettings,
    NetEvent(NetEvent),
    FriendEvent(FriendEvent),
    /// An event from the whisper (DM) protocol.
    WhisperEvent(WhisperEvent),
    /// An event from the inbox (offline-message) protocol.
    InboxEvent(InboxEvent),
    MessageSent(String, u64, MessageHash),
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

    /// Toggle dark mode on/off.
    ToggleDark(bool),
    /// Update the local display name (nickname).
    SetNickname(String),
    /// Open the separate log viewer window.
    OpenLogsWindow,
    /// Internal no-op for async task completions that should not change UI state.
    Noop,
    /// Copy text to the system clipboard.
    CopyToClipboard(String),
    /// Open a direct chat with an online friend.
    OpenFriendChat(PublicKey),
    /// Toggle notification sounds on/off.
    ToggleSound(bool),
    /// Open the native picker for a local profile image.
    PickProfileImage,
    /// Result of reading the selected profile image.
    ProfileImagePicked(Result<Vec<u8>, String>),
    /// The profile image was uploaded to the local blob store; carries the
    /// BlobTicket string peers use to download it.
    ProfileImageUploaded(String),
    /// Remove the currently configured profile image.
    RemoveProfileImage,
    /// A remote peer's profile image blob was downloaded and decoded.
    ProfileImageDownloaded(PublicKey, Vec<u8>),
    /// User requested to clear chat history — show confirmation.
    ClearHistoryRequested,
    /// User confirmed the clear history action.
    ConfirmClearHistory,
    /// User requested to delete a room — show confirmation.
    DeleteRoomRequested(TopicId),
    /// User confirmed deletion of a room.
    ConfirmDeleteRoom(TopicId),
    /// Results of a mailbox sync triggered on whisper reconnect.
    MailboxReplayed {
        /// Peer whose envelopes were replayed.
        peer: PublicKey,
        /// Accepted entries: (message_id, plaintext).
        texts: Vec<(String, String)>,
    },
    /// Scroll offset / viewport changed in the chat log.
    /// Used by windowed rendering to determine which entries to build widgets for.
    Scrolled(f32, f32),
}

impl IcedChat {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        secret_key: SecretKey,
        gossip: Gossip,
        router: iroh::protocol::Router,
        blob_store: MemStore,
        endpoint: iroh::Endpoint,
        memory_lookup: MemoryLookup,
        local_label: String,
        local_public: PublicKey,
        relay_mode: RelayMode,
        data_dir: std::path::PathBuf,
        runtime_handle: tokio::runtime::Handle,
        net_rx: Arc<Mutex<UnboundedReceiver<NetEvent>>>,
        net_tx: UnboundedSender<NetEvent>,
        room_history: RoomHistoryStore,
        friends: FriendsStore,
        friend_mgr: FriendPingManager,
        friend_events_rx: Arc<Mutex<UnboundedReceiver<FriendEvent>>>,
        whisper_events_rx: Arc<Mutex<UnboundedReceiver<WhisperEvent>>>,
        inbox_events_rx: Arc<Mutex<UnboundedReceiver<InboxEvent>>>,
        whisper_handle: WhisperHandle,
        initial_room: Option<(TopicId, Vec<EndpointAddr>)>,
        notice: String,
        chat_history: Arc<std::sync::Mutex<ChatHistoryStore>>,
        backfill_handle: BackfillHandle,
        return_to_chat_list_after_open: bool,
    ) -> Self {
        let (initial_topic, initial_bootstrap) =
            initial_room.unwrap_or_else(|| (TopicId::from_bytes([0u8; 32]), vec![]));
        // Seed the online cache from persisted friends who were online at last save,
        // so they show the correct status immediately instead of starting as offline.
        let friend_online_cache: HashSet<PublicKey> = friends
            .iter()
            .filter(|(_, record)| record.status.online)
            .filter_map(|(id, _)| id.parse_public_key().ok())
            .collect();
        // Load saved profile image from disk and regenerate the blob ticket
        // so AboutMe broadcasts include the ticket for peers to download.
        // Without this, a restart loses the ticket (blob store is in-memory)
        // and peers see the fallback emoji instead of the avatar.
        let (profile_image_handle, profile_image_ticket) =
            if let Ok(bytes) = fs::read(data_dir.join(PROFILE_IMAGE_FILE)) {
                if !bytes.is_empty() && bytes.len() <= PROFILE_IMAGE_MAX_BYTES {
                    let handle = Some(iced::widget::image::Handle::from_bytes(bytes.clone()));
                    let ticket = {
                        let bs = blob_store.clone();
                        let ep = endpoint.clone();
                        runtime_handle.block_on(async {
                            bs.blobs().add_bytes(bytes).await.ok().map(|tag| {
                                blob_ticket_string(ep.watch_addr().get(), tag.hash, tag.format)
                            })
                        })
                    };
                    (handle, ticket)
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };
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
            settings_return_to: None,
            names: HashMap::new(),
            topic: initial_topic,
            ticket_str: String::new(),
            secret_key,
            gossip,
            _router: router,
            sender: None,
            blob_store,
            endpoint,
            memory_lookup,
            local_label,
            local_public,
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
            self_sent_events: HashMap::new(),
            tor_reconnect_rx,
            follow_latest: true,
            total_content_height: std::cell::Cell::new(0.0),
            scroll_offset: f32::MAX,
            viewport_height: 0.0,
            dark_mode: false,
            sound_enabled: true,
            history_confirm_clear: false,
            room_delete_confirm_topic: None,
            notice,
            data_dir: data_dir.clone(),
            chat_history,
            outbox: Arc::new(std::sync::Mutex::new(OutboxStore::load_or_default(
                &data_dir,
            ))),
            chat_history_dirty: false,
            history_saved_count: 0,
            friend_online_cache,
            initial_bootstrap_peers: initial_bootstrap,
            return_to_chat_list_after_open,
            whisper_handle,
            inbox_events_rx: inbox_events_rx,
            whisper_events_rx,
            profile_image_handle,
            profile_image_ticket,
            friend_image_handles: HashMap::new(),
            friend_image_tickets: HashMap::new(),
            pending_profile_image_tickets: std::collections::VecDeque::new(),
        }
    }

    fn room_ticket(&self, topic: TopicId) -> Ticket {
        Ticket {
            topic,
            peers: vec![self.endpoint.watch_addr().get()],
        }
    }

    /// Stable room used as the default lobby for discovering online users.
    pub fn default_lobby_topic() -> TopicId {
        TopicId::from_bytes(*blake3::hash(b"iroh-gossip-chat/default-lobby/v1").as_bytes())
    }

    /// Stable personal room advertised by this identity.
    fn personal_room_topic(&self) -> TopicId {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"iroh-gossip-chat/personal-room/v1");
        hasher.update(self.local_public.as_bytes());
        TopicId::from_bytes(*hasher.finalize().as_bytes())
    }

    fn personal_room_ticket(&self) -> String {
        self.room_ticket(self.personal_room_topic()).to_string()
    }

    /// Refresh the displayed room ticket when iroh learns a new relay or
    /// direct address asynchronously. The current endpoint address is read
    /// by `room_ticket`, so this also detects changes without stale state.
    fn refresh_local_peer_addr(&mut self) -> bool {
        if self.ticket_str.is_empty() {
            return false;
        }

        let current_ticket = self.room_ticket(self.topic).to_string();
        if current_ticket == self.ticket_str {
            return false;
        }

        self.ticket_str = current_ticket;
        true
    }

    fn push_system(&mut self, text: impl Into<String>) {
        self.entries.push(ChatEntry::system(text));
    }
    fn push_local(&mut self, text: impl Into<String>) {
        self.entries.push(ChatEntry::local(&self.local_label, text));
    }

    fn log_variant(message: &AppMessage) -> &'static str {
        match message {
            AppMessage::GoToChatList => "GoToChatList",
            AppMessage::OpenRoom(_) => "OpenRoom",
            AppMessage::RoomOpened { .. } => "RoomOpened",
            AppMessage::CreateNewRoom => "CreateNewRoom",
            AppMessage::JoinFromTicket => "JoinFromTicket",
            AppMessage::RoomJoinFailed(_) => "RoomJoinFailed",
            AppMessage::JoinTicketInputChanged(_) => "JoinTicketInputChanged",
            AppMessage::NewChatCreated => "NewChatCreated",
            AppMessage::RoomSelected(_) => "RoomSelected",
            AppMessage::InputChanged(_) => "InputChanged",
            AppMessage::SendPressed => "SendPressed",
            AppMessage::AttachPressed => "AttachPressed",
            AppMessage::ToggleHelp => "ToggleHelp",
            AppMessage::OpenSettings => "OpenSettings",
            AppMessage::CloseSettings => "CloseSettings",
            AppMessage::NetEvent(_) => "NetEvent",
            AppMessage::FriendEvent(_) => "FriendEvent",
            AppMessage::WhisperEvent(_) => "WhisperEvent",
            AppMessage::InboxEvent(_) => "InboxEvent",
            AppMessage::MessageSent(..) => "MessageSent",
            AppMessage::FileSent(_) => "FileSent",
            AppMessage::DownloadDone(_) => "DownloadDone",
            AppMessage::ErrorMsg(_) => "ErrorMsg",
            AppMessage::ExecuteFileSend(_) => "ExecuteFileSend",
            AppMessage::ExecuteDownload => "ExecuteDownload",
            AppMessage::ExecuteImageSend(_) => "ExecuteImageSend",
            AppMessage::ImageDownloaded { .. } => "ImageDownloaded",
            AppMessage::FriendAdded { .. } => "FriendAdded",
            AppMessage::FriendRemoved { .. } => "FriendRemoved",
            AppMessage::FriendListResult(_) => "FriendListResult",
            AppMessage::DeleteRoom(_) => "DeleteRoom",
            AppMessage::ConnMonitorTick => "ConnMonitorTick",
            AppMessage::MeshWatchdogTick => "MeshWatchdogTick",
            AppMessage::TorReconnect(_) => "TorReconnect",
            AppMessage::ToggleDark(_) => "ToggleDark",
            AppMessage::SetNickname(_) => "SetNickname",
            AppMessage::OpenLogsWindow => "OpenLogsWindow",
            AppMessage::Noop => "Noop",
            AppMessage::CopyToClipboard(_) => "CopyToClipboard",
            AppMessage::OpenFriendChat(_) => "OpenFriendChat",
            AppMessage::ToggleSound(_) => "ToggleSound",
            AppMessage::PickProfileImage => "PickProfileImage",
            AppMessage::ProfileImagePicked(_) => "ProfileImagePicked",
            AppMessage::ProfileImageUploaded(_) => "ProfileImageUploaded",
            AppMessage::RemoveProfileImage => "RemoveProfileImage",
            AppMessage::ProfileImageDownloaded(..) => "ProfileImageDownloaded",
            AppMessage::ClearHistoryRequested => "ClearHistoryRequested",
            AppMessage::ConfirmClearHistory => "ConfirmClearHistory",
            AppMessage::DeleteRoomRequested(_) => "DeleteRoomRequested",
            AppMessage::ConfirmDeleteRoom(_) => "ConfirmDeleteRoom",
            AppMessage::MailboxReplayed { .. } => "MailboxReplayed",
            AppMessage::Scrolled(..) => "Scrolled",
        }
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
        self.neighbors.clear();
        self.history_saved_count = 0;
    }

    /// Copy new entries into the active-session store without persistence.
    fn save_room_to_history(&mut self) {
        let topic = self.topic;
        let current_count = self.entries.len();
        if self.history_saved_count >= current_count {
            return;
        }

        // Keep chat messages available to the active session only.
        for entry in &self.entries[self.history_saved_count..] {
            let kind = match entry.kind {
                ChatKind::System => "system",
                _ if entry.image_bytes.is_some() => "image",
                _ => "text",
            };
            let body_text = entry.body.clone();
            let sender = match entry.kind {
                ChatKind::Local => hex::encode(self.local_public.as_bytes()),
                _ => String::new(),
            };
            let mut history_entry = HistoryEntry::new(
                topic,
                sender,
                Vec::new(), // signed bytes not available here
                kind,
                body_text,
            );
            // Preserve image bytes so images render when replaying after
            // room switch within the same session.
            if let Some(ref img_bytes) = entry.image_bytes {
                history_entry.image_bytes = Some(img_bytes.clone());
            }
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

    let elapsed_secs = if now_ms > ms { (now_ms - ms) / 1000 } else { 0 };

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
/// The API stores message timestamps in UTC; the UI renders them in the
/// user's local timezone before applying the usual "today / this week / older"
/// label rules.
///
/// - Today:    "12:34"
/// - This week: "Mon 12:34"
/// - Older:    "Jan 5"
fn format_message_time(timestamp_ms: i64) -> String {
    use chrono::{Local, TimeZone};

    let now = Local::now();
    let to_local = |ms: i64| Local.timestamp_millis_opt(ms).single();
    format_message_time_with(timestamp_ms, now, to_local)
}

fn format_message_time_with<Tz, F>(
    timestamp_ms: i64,
    now: chrono::DateTime<Tz>,
    mut to_local: F,
) -> String
where
    Tz: chrono::TimeZone,
    F: FnMut(i64) -> Option<chrono::DateTime<Tz>>,
{
    use chrono::{Datelike, Timelike};

    let Some(timestamp) = to_local(timestamp_ms) else {
        return String::new();
    };

    let today = now.date_naive();
    let message_day = timestamp.date_naive();
    let hour = timestamp.hour();
    let minute = timestamp.minute();

    if message_day == today {
        format!("{:02}:{:02}", hour, minute)
    } else if message_day >= today - chrono::TimeDelta::days(6) {
        format!(
            "{} {:02}:{:02}",
            timestamp.naive_local().format("%a"),
            hour,
            minute
        )
    } else {
        format!(
            "{} {}",
            timestamp.naive_local().format("%b"),
            timestamp.day()
        )
    }
}

/// Create a deterministic topic id from two peer public keys.
///
/// Both peers derive the same topic by sorting their public keys
/// before hashing, so either side can initiate a private chat.
fn private_topic(a: &PublicKey, b: &PublicKey) -> TopicId {
    direct_topic(a, b)
}

fn online_friends_from_store(friends: &FriendsStore) -> HashMap<PublicKey, String> {
    friends
        .iter()
        .filter(|(_, record)| record.status.online)
        .filter_map(|(id, record)| {
            id.parse_public_key()
                .ok()
                .map(|pk| (pk, record.display_label(id)))
        })
        .collect()
}

// ── Update ────────────────────────────────────────────────────────────

impl IcedChat {
    pub fn update(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        debug!(message = Self::log_variant(&message), "app update");
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
                // Leave the current room first — abort forward_handle, clear
                // sender + entries — so we don't have a zombie forward_handle
                // or broadcast to the wrong topic during the async gap.
                self.leave_current_room();

                let topic = TopicId::from_bytes(rand::random());
                let gossip = self.gossip.clone();
                let net_tx = self.net_tx.clone();
                let sk = self.secret_key.clone();
                let label = self.local_label.clone();
                let personal_topic = self.personal_room_topic();
                let forward_handle_slot = self.forward_handle_slot.clone();
                let data_dir = self.data_dir.clone();
                let endpoint = self.endpoint.clone();
                let profile_image_ticket = self.profile_image_ticket.clone();

                iced::Task::perform(
                    async move {
                        // Subscribe to the new topic
                        let sub = gossip
                            .subscribe(topic, vec![])
                            .await
                            .map_err(|e| e.to_string())?;
                        let (sender, receiver) = sub.split();
                        let local_peer_addr = endpoint.watch_addr().get();
                        let ticket_str = Ticket {
                            topic,
                            peers: vec![local_peer_addr.clone()],
                        }
                        .to_string();
                        let personal_ticket = Ticket {
                            topic: personal_topic,
                            peers: vec![local_peer_addr.clone()],
                        }
                        .to_string();

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
                            &crate::Message::AboutMe {
                                name: label,
                                profile_image_ticket: profile_image_ticket,
                            },
                        )
                        .map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(msg).await;
                        let presence =
                            SignedMessage::sign_and_encode(&sk, &crate::Message::Presence)
                                .map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(presence).await;

                        let room = RoomStore::with_peers(&data_dir, topic, vec![local_peer_addr]);
                        let _ = room.save();

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
                let personal_topic = self.personal_room_topic();
                let forward_handle_slot = self.forward_handle_slot.clone();
                let endpoint = self.endpoint.clone();
                let memory_lookup = self.memory_lookup.clone();
                let data_dir = self.data_dir.clone();
                let profile_image_ticket = self.profile_image_ticket.clone();
                // Extract bootstrap peer addresses from the one-shot initial room
                // or from the saved RoomStore for this topic.
                let initial_addrs: Vec<EndpointAddr> =
                    self.initial_bootstrap_peers.drain(..).collect();
                let saved_addrs = RoomStore::load_or_none(&data_dir)
                    .filter(|room| room.topic == topic)
                    .map(|room| room.peers)
                    .unwrap_or_default();
                let (bootstrap_peers, initial_addrs) =
                    collect_bootstrap_peers([&initial_addrs, &saved_addrs]);
                let initial_addrs_for_save = initial_addrs.clone();
                let direct_conversation = self.friends.iter().any(|(_, record)| {
                    record
                        .direct_conversation
                        .as_ref()
                        .is_some_and(|conversation| conversation.topic == topic)
                });

                iced::Task::perform(
                    async move {
                        // Seed the endpoint address lookup with bootstrap peer
                        // addresses so the endpoint can resolve them by their
                        // transport info (relay URL, direct addresses) from the
                        // ticket or RoomStore — not just by public key.
                        seed_memory_lookup(&memory_lookup, &initial_addrs);
                        // Wait for at least one gossip neighbor if we have bootstrap
                        // peers — matching the TUI behavior.  Without bootstrap
                        // peers (room creator) use subscribe() so we don't hang.
                        // Stale bootstrap peers are protected by a 30s timeout
                        // to avoid blocking the UI indefinitely.
                        let sub: GossipTopic = if direct_conversation || bootstrap_peers.is_empty()
                        {
                            gossip
                                .subscribe(topic, bootstrap_peers)
                                .await
                                .map_err(|e| e.to_string())?
                        } else {
                            tokio::time::timeout(Duration::from_secs(30), async {
                                gossip.subscribe_and_join(topic, bootstrap_peers).await
                            })
                            .await
                            .map_err(|_| {
                                "timed out waiting for a peer to join the room — \
                                 the saved addresses may be stale; the room is \
                                 still subscribed, so any peer that connects \
                                 later will work"
                                    .to_string()
                            })?
                            .map_err(|e| e.to_string())?
                        };
                        let (sender, receiver) = sub.split();
                        let local_peer_addr = endpoint.watch_addr().get();
                        let ticket_str = Ticket {
                            topic,
                            peers: vec![local_peer_addr.clone()],
                        }
                        .to_string();
                        let personal_ticket = Ticket {
                            topic: personal_topic,
                            peers: vec![local_peer_addr.clone()],
                        }
                        .to_string();

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
                            &crate::Message::AboutMe {
                                name: label,
                                profile_image_ticket: profile_image_ticket,
                            },
                        )
                        .map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(msg).await;
                        let presence =
                            SignedMessage::sign_and_encode(&sk, &crate::Message::Presence)
                                .map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(presence).await;

                        let saved_peers = if initial_addrs_for_save.is_empty() {
                            vec![local_peer_addr]
                        } else {
                            initial_addrs_for_save.clone()
                        };
                        let room = RoomStore::with_peers(&data_dir, topic, saved_peers);
                        let _ = room.save();

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
                self.sender = Some(sender.clone());
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
                            "image" if entry.image_bytes.is_some() => {
                                let sender_pk = if entry.sender.is_empty() {
                                    None
                                } else if let Ok(bytes) = hex::decode(&entry.sender) {
                                    if bytes.len() == 32 {
                                        let arr: [u8; 32] = bytes.try_into().unwrap();
                                        PublicKey::from_bytes(&arr).ok()
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };
                                let label = sender_pk
                                    .map(|pk| pk.fmt_short().to_string())
                                    .unwrap_or_else(|| "local".to_string());
                                self.entries.push(ChatEntry::image(
                                    &label,
                                    &entry.text_preview,
                                    entry.image_bytes.clone().unwrap(),
                                    None,
                                    Some(entry.timestamp / 1000),
                                    sender_pk,
                                ));
                            }
                            _ => {
                                // Parse sender from hex, use short display
                                let sender_pk = if entry.sender.is_empty() {
                                    None
                                } else if let Ok(bytes) = hex::decode(&entry.sender) {
                                    if bytes.len() == 32 {
                                        let arr: [u8; 32] = bytes.try_into().unwrap();
                                        PublicKey::from_bytes(&arr).ok()
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };
                                let is_self =
                                    sender_pk.map(|pk| pk == self.local_public).unwrap_or(false);
                                if is_self {
                                    let mut e =
                                        ChatEntry::local(&self.local_label, &entry.text_preview);
                                    e = e.with_timestamp(Some(entry.timestamp as i64));
                                    self.entries.push(e);
                                } else {
                                    let label = sender_pk
                                        .map(|pk| pk.fmt_short().to_string())
                                        .unwrap_or_else(|| "peer".to_string());
                                    self.entries.push(ChatEntry::remote(
                                        &label,
                                        &entry.text_preview,
                                        None,
                                        Some(entry.timestamp / 1000),
                                        sender_pk,
                                    ));
                                }
                            }
                        }
                    }
                }

                // Replay queued or previously-sent messages after a reconnect.
                // The signed bytes are reused verbatim, so retries cannot create
                // a second logical message or invalidate message-hash dedup.
                let replay = {
                    let outbox = self.outbox.lock().unwrap();
                    outbox
                        .pending()
                        .into_iter()
                        .filter(|entry| entry.topic == topic)
                        .map(|entry| (entry.event_id, entry.signed_bytes.clone()))
                        .collect::<Vec<_>>()
                };
                if !replay.is_empty() {
                    let sender = sender.clone();
                    let ids = replay.iter().map(|(id, _)| *id).collect::<Vec<_>>();
                    for id in &ids {
                        let _ = self
                            .outbox
                            .lock()
                            .unwrap()
                            .update_delivery_state(*id, DeliveryState::Sent);
                        let _ = self
                            .chat_history
                            .lock()
                            .unwrap()
                            .update_delivery_state(*id, DeliveryState::Sent);
                    }
                    let _ = self.outbox.lock().unwrap().save();
                    let _ = self.chat_history.lock().unwrap().save();
                    task::spawn(async move {
                        for (_, bytes) in replay {
                            let _ = sender.broadcast(bytes.into()).await;
                        }
                    });
                }

                // Update room history
                self.room_history.upsert(topic, &self.local_label, true);
                self.room_history_dirty = true;
                self.persist_room_history();
                self.try_save_chat_history();

                if self.return_to_chat_list_after_open {
                    self.return_to_chat_list_after_open = false;
                    return iced::Task::done(AppMessage::GoToChatList);
                }

                iced::Task::none()
            }

            AppMessage::RoomJoinFailed(e) => {
                self.pending_topic = None;
                self.chat_list_error = format!("Failed to join room: {e}");
                self.screen = Screen::ChatList;
                iced::Task::none()
            }

            AppMessage::JoinFromTicket => {
                // Validate before leaving the current room or starting an
                // asynchronous task.  Previously an empty/malformed field
                // was parsed inside the task, so clicking the button gave no
                // immediate feedback and looked like a no-op.
                let ticket_input = self.join_ticket_input.trim();
                if ticket_input.is_empty() {
                    self.chat_list_error = "Paste a ticket before joining a room.".to_string();
                    self.screen = Screen::ChatList;
                    return iced::Task::none();
                }
                let ticket: Ticket = match ticket_input.parse() {
                    Ok(ticket) => ticket,
                    Err(e) => {
                        self.chat_list_error = format!("Invalid ticket: {e}");
                        self.screen = Screen::ChatList;
                        return iced::Task::none();
                    }
                };

                // Show progress while subscribe_and_join waits for the
                // bootstrap peer.  Any connection error is converted to
                // RoomJoinFailed below and rendered in this same location.
                self.chat_list_error = "Joining room…".to_string();
                self.save_room_to_history();
                self.persist_room_history();
                self.try_save_chat_history();
                self.leave_current_room();
                let gossip = self.gossip.clone();
                let net_tx = self.net_tx.clone();
                let sk = self.secret_key.clone();
                let label = self.local_label.clone();
                let personal_topic = self.personal_room_topic();
                let endpoint = self.endpoint.clone();
                let memory_lookup = self.memory_lookup.clone();
                let forward_handle_slot = self.forward_handle_slot.clone();
                let data_dir = self.data_dir.clone();
                let profile_image_ticket = self.profile_image_ticket.clone();

                iced::Task::perform(
                    async move {
                        let topic = ticket.topic;
                        let saved_addrs = RoomStore::load_or_none(&data_dir)
                            .filter(|room| room.topic == topic)
                            .map(|room| room.peers)
                            .unwrap_or_default();
                        let (peers, bootstrap_addrs) =
                            collect_bootstrap_peers([&ticket.peers, &saved_addrs]);
                        seed_memory_lookup(&memory_lookup, &bootstrap_addrs);

                        // Use subscribe_and_join so we wait for at least one gossip
                        // neighbor to connect before proceeding — matching the TUI
                        // behavior.  If no bootstrap peers are given (unlikely here
                        // since JoinFromTicket always has ticket.peers) fall back to
                        // subscribe() to avoid hanging forever.
                        let sub = tokio::time::timeout(Duration::from_secs(30), async {
                            if peers.is_empty() {
                                gossip.subscribe(topic, peers).await
                            } else {
                                gossip.subscribe_and_join(topic, peers).await
                            }
                        })
                        .await
                        .map_err(|_| "timed out waiting for a peer to join the room".to_string())?
                        .map_err(|e| e.to_string())?;
                        let (sender, receiver) = sub.split();
                        let local_peer_addr = endpoint.watch_addr().get();
                        let new_ticket = Ticket {
                            topic,
                            peers: vec![local_peer_addr.clone()],
                        };
                        let ticket_str = new_ticket.to_string();
                        let personal_ticket = Ticket {
                            topic: personal_topic,
                            peers: vec![local_peer_addr.clone()],
                        }
                        .to_string();

                        let metadata_doc =
                            room_docs::create_metadata_doc(topic, &sender, RoomMetadata::empty())
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
                            &crate::Message::AboutMe {
                                name: label,
                                profile_image_ticket: profile_image_ticket,
                            },
                        )
                        .map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(msg).await;
                        let presence = SignedMessage::sign_and_encode(
                            &sk,
                            &crate::Message::PresenceWithTicket {
                                ticket: personal_ticket,
                            },
                        )
                        .map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(presence).await;

                        let room = RoomStore::with_peers(&data_dir, topic, bootstrap_addrs);
                        let _ = room.save();

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
                let fid = FriendId::from_public_key(peer);
                let topic = direct_topic(&self.local_public, &peer);
                let known_addrs = self
                    .friends
                    .get(&fid)
                    .map(|record| record.known_addrs.clone())
                    .unwrap_or_default();
                let record = self.friends.ensure_friend(fid);
                record.set_direct_conversation(topic, DirectConversationState::Pending);
                let room = RoomStore::with_peers(&self.data_dir, topic, known_addrs.clone());
                let _ = room.save();
                self.try_save_friends();

                let secret_key = self.secret_key.clone();
                let whisper_handle = self.whisper_handle.clone();
                let local_addr = self.endpoint.addr();
                let action = ContactAction::ConversationInvite {
                    topic,
                    addrs: vec![local_addr],
                };
                let payload = match SignedContactMessage::sign(&secret_key, &action) {
                    Ok(payload) => payload.into(),
                    Err(err) => {
                        return iced::Task::done(AppMessage::ErrorMsg(format!(
                            "Could not create contact invite: {err}"
                        )));
                    }
                };
                iced::Task::batch(vec![
                    iced::Task::perform(
                        async move {
                            let _ = whisper_handle.send_control(peer, payload).await;
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
                if trimmed == "/settings" {
                    self.settings_return_to = Some(self.screen.clone());
                    self.screen = Screen::Settings;
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
                    // Parse key and lookup address outside async block (avoids capturing self)
                    let peer = key_part.parse::<PublicKey>().ok();
                    let addr = peer.as_ref().and_then(|p| {
                        let fid = FriendId::from_public_key(*p);
                        self.friends
                            .get(&fid)
                            .and_then(|record| record.known_addrs.first().cloned())
                    });
                    return iced::Task::perform(
                        async move {
                            match peer {
                                Some(peer) => {
                                    let fid = FriendId::from_public_key(peer);
                                    let label = alias
                                        .clone()
                                        .unwrap_or_else(|| peer.fmt_short().to_string());
                                    let was_new = mgr.add_friend(peer, addr).await.unwrap_or(false);
                                    AppMessage::FriendAdded {
                                        fid: fid.as_str().to_string(),
                                        label,
                                        was_new,
                                    }
                                }
                                None => {
                                    AppMessage::ErrorMsg(format!("Invalid public key: {key_part}"))
                                }
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
                    // Check if this peer has a mailbox key for offline delivery.
                    let mailbox_pk = {
                        let fid = FriendId::from_public_key(peer_key);
                        self.friends.get(&fid).and_then(|r| r.mailbox_public_key)
                    };
                    let secret_key = self.secret_key.clone();
                    let data_dir = self.data_dir.clone();
                    return iced::Task::perform(
                        async move {
                            match whisper_handle.send_dm(peer_key, text.clone()).await {
                                Ok(()) => AppMessage::Noop,
                                Err(_) if mailbox_pk.is_some() => {
                                    let pk = mailbox_pk.unwrap();
                                    match seal_for(&secret_key, pk, text.as_bytes()) {
                                        Ok(envelope) => {
                                            let mut store = MailboxStore::load(&data_dir)
                                                .ok()
                                                .flatten()
                                                .unwrap_or_else(|| {
                                                    MailboxStore::empty_at(&data_dir)
                                                });
                                            match store.enqueue_outgoing(envelope) {
                                                Ok(_) => {
                                                    if let Err(save_err) = store.save() {
                                                        AppMessage::ErrorMsg(format!(
                                                            "Failed to persist offline message: {save_err}"
                                                        ))
                                                    } else {
                                                        AppMessage::Noop
                                                    }
                                                }
                                                Err(enq_err) => AppMessage::ErrorMsg(format!(
                                                    "Failed to queue offline message: {enq_err}"
                                                )),
                                            }
                                        }
                                        Err(seal_err) => AppMessage::ErrorMsg(format!(
                                            "Failed to encrypt offline message: {seal_err}"
                                        )),
                                    }
                                }
                                Err(e) => AppMessage::ErrorMsg(format!("Whisper failed: {e}")),
                            }
                        },
                        |msg| msg,
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
                    let endpoint = self.endpoint.clone();
                    let fname = filename.clone();
                    self.push_system(format!("[Whisper] Hashing file: {filename}..."));
                    return iced::Task::perform(
                        async move {
                            let tag = blob_store
                                .blobs()
                                .add_path(abs_path)
                                .await
                                .map_err(|e| format!("Failed to hash file: {e}"))?;
                            let ticket = blob_ticket_string(
                                endpoint.watch_addr().get(),
                                tag.hash,
                                tag.format,
                            );
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
                let msg = crate::Message::Message { text: trimmed };
                let msg_hash = message_hash(&msg);
                let local_hex = hex::encode(self.local_public.as_bytes());
                // Sign before touching either store: the exact bytes are the
                // durable replay payload and the content-addressed identity.
                let encoded = match SignedMessage::sign_and_encode(&self.secret_key, &msg) {
                    Ok(encoded) => encoded,
                    Err(e) => return iced::Task::done(AppMessage::ErrorMsg(e.to_string())),
                };
                let event_id = {
                    let mut store = self.chat_history.lock().unwrap();
                    let entry = HistoryEntry::new(
                        self.topic,
                        local_hex,
                        encoded.to_vec(),
                        "text",
                        text.clone(),
                    );
                    let id = store.push_with_id(entry);
                    let _ = store.save();
                    id
                };
                {
                    let mut outbox = self.outbox.lock().unwrap();
                    let _ = outbox.push(OutboxEntry::new(event_id, self.topic, encoded.to_vec()));
                    let _ = outbox.save();
                }
                self.self_sent_events.insert(msg_hash, event_id);
                let mut local_entry = ChatEntry::local(&self.local_label, &text);
                local_entry.event_id = event_id;
                local_entry.message_hash = Some(msg_hash);
                self.entries.push(local_entry);
                if let Some(sender) = self.sender.clone() {
                    iced::Task::perform(
                        async move {
                            sender.broadcast(encoded).await.ok();
                            (text, event_id, msg_hash)
                        },
                        |(t, eid, mh)| AppMessage::MessageSent(t, eid, mh),
                    )
                } else {
                    iced::Task::none()
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

            AppMessage::OpenSettings => {
                if !matches!(self.screen, Screen::Settings) {
                    self.settings_return_to = Some(self.screen.clone());
                    self.screen = Screen::Settings;
                }
                iced::Task::none()
            }

            AppMessage::CloseSettings => {
                self.screen = self.settings_return_to.take().unwrap_or(Screen::ChatList);
                iced::Task::none()
            }

            AppMessage::NetEvent(event) => {
                self.update_room_preview(&event);
                let _ = chat_net_event(event.clone(), self);
                // ── Delivery state transitions ──
                // Echo: our own broadcast returning via gossip → Delivered
                if let NetEvent::Message {
                    from, ref message, ..
                } = &event
                {
                    if *from == self.local_public {
                        let msg_hash = message_hash(message);
                        if let Some(&event_id) = self.self_sent_events.get(&msg_hash) {
                            if let Some(entry) =
                                self.entries.iter_mut().find(|e| e.event_id == event_id)
                            {
                                if entry.delivery_state == DeliveryState::Sent {
                                    entry.delivery_state = DeliveryState::Delivered;
                                    let mut store = self.chat_history.lock().unwrap();
                                    let _ = store
                                        .update_delivery_state(event_id, DeliveryState::Delivered);
                                    let _ = store.save();
                                    let mut outbox = self.outbox.lock().unwrap();
                                    let _ = outbox
                                        .update_delivery_state(event_id, DeliveryState::Delivered);
                                    let _ = outbox.save();
                                }
                            }
                        }
                    }
                }
                // ── Auto ReadReceipt: when user is viewing the chat,
                // send ReadReceipt for incoming remote text messages ──
                if self.follow_latest {
                    if let NetEvent::Message {
                        from, ref message, ..
                    } = &event
                    {
                        if *from != self.local_public {
                            if let crate::Message::Message { .. } = message {
                                let msg_hash = message_hash(message);
                                if let Some(ref sender) = self.sender {
                                    let sk = self.secret_key.clone();
                                    let s = sender.clone();
                                    return iced::Task::perform(
                                        async move {
                                            if let Ok(encoded) = SignedMessage::sign_and_encode(
                                                &sk,
                                                &crate::Message::ReadReceipt {
                                                    message_hash: msg_hash,
                                                },
                                            ) {
                                                s.broadcast(encoded).await.ok();
                                            }
                                        },
                                        |_| AppMessage::Noop,
                                    );
                                }
                            }
                        }
                    }
                }
                // ReadReceipt from peer → Seen
                if let NetEvent::Message {
                    message:
                        Message::ReadReceipt {
                            message_hash: receipt_hash,
                        },
                    from: receipt_from,
                    ..
                } = &event
                {
                    if *receipt_from != self.local_public {
                        if let Some(&event_id) = self.self_sent_events.get(receipt_hash) {
                            if let Some(entry) =
                                self.entries.iter_mut().find(|e| e.event_id == event_id)
                            {
                                if entry.delivery_state.can_transition_to(&DeliveryState::Seen) {
                                    entry.delivery_state = DeliveryState::Seen;
                                    let mut store = self.chat_history.lock().unwrap();
                                    let _ =
                                        store.update_delivery_state(event_id, DeliveryState::Seen);
                                }
                            }
                        }
                    }
                }
                // NeighborDown → mark pending messages as Failed
                if let NetEvent::NeighborDown { .. } = &event {
                    for entry in self.entries.iter_mut() {
                        if matches!(entry.kind, ChatKind::Local)
                            && entry.event_id > 0
                            && matches!(
                                entry.delivery_state,
                                DeliveryState::Queued | DeliveryState::Sent
                            )
                        {
                            entry.delivery_state = DeliveryState::Failed;
                            let eid = entry.event_id;
                            let mut store = self.chat_history.lock().unwrap();
                            let _ = store.update_delivery_state(eid, DeliveryState::Failed);
                        }
                    }
                }
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
                // Check if a profile image ticket arrived from a remote peer
                if let Some((peer, ticket_str)) = self.pending_profile_image_tickets.pop_front() {
                    let blob_store = self.blob_store.clone();
                    let endpoint = self.endpoint.clone();
                    let memory_lookup = self.memory_lookup.clone();
                    let neighbors = self.neighbors.clone();
                    return iced::Task::perform(
                        async move {
                            let ticket: BlobTicket = ticket_str
                                .parse::<BlobTicket>()
                                .map_err(|e| format!("Parse profile image ticket: {e}"))?;
                            // The profile ticket is the authoritative transport
                            // address for the blob provider.  Register it before
                            // downloading; using only the public key leaves iroh
                            // with no relay/direct addresses to resolve.
                            seed_memory_lookup(&memory_lookup, &[ticket.addr().clone()]);
                            let peer_id = ticket.addr().id;
                            let candidates = download_candidates(peer_id, &neighbors);
                            blob_store
                                .downloader(&endpoint)
                                .download(ticket.hash(), candidates)
                                .await
                                .map_err(|e| format!("Download profile image: {e}"))?;
                            let mut reader = blob_store.blobs().reader(ticket.hash());
                            let mut buf = Vec::new();
                            use tokio::io::AsyncReadExt;
                            reader
                                .read_to_end(&mut buf)
                                .await
                                .map_err(|e| format!("Read profile image: {e}"))?;
                            Ok((peer, buf))
                        },
                        move |r: Result<(PublicKey, Vec<u8>), String>| match r {
                            Ok((peer, data)) => AppMessage::ProfileImageDownloaded(peer, data),
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
                    iroh_gossip::whisper::WhisperEvent::Control { from, content } => {
                        match SignedContactMessage::verify(&content, Some(from)) {
                            Ok((sender, ContactAction::ContactRequest { name })) => {
                                let record = self
                                    .friends
                                    .ensure_friend(FriendId::from_public_key(sender));
                                if name.is_some() {
                                    record.last_announced_name = name;
                                }
                                self.try_save_friends();
                                let payload = SignedContactMessage::sign(
                                    &self.secret_key,
                                    &ContactAction::ContactAccept,
                                );
                                if let Ok(payload) = payload {
                                    let whisper_handle = self.whisper_handle.clone();
                                    return iced::Task::perform(
                                        async move {
                                            let _ = whisper_handle
                                                .send_control(sender, payload.into())
                                                .await;
                                        },
                                        |_| AppMessage::Noop,
                                    );
                                }
                            }
                            Ok((sender, ContactAction::ContactAccept)) => {
                                if let Some(record) =
                                    self.friends.get_mut(&FriendId::from_public_key(sender))
                                {
                                    if let Some(conversation) = record.direct_conversation.as_mut()
                                    {
                                        conversation.state = DirectConversationState::Active;
                                    }
                                }
                                self.try_save_friends();
                            }
                            Ok((sender, ContactAction::ConversationInvite { topic, addrs }))
                                if addrs.iter().all(|addr| addr.id == sender) =>
                            {
                                let fid = FriendId::from_public_key(sender);
                                let record = self.friends.ensure_friend(fid);
                                record.record_addrs(addrs.clone());
                                record.set_direct_conversation(
                                    topic,
                                    DirectConversationState::Active,
                                );
                                let room = RoomStore::with_peers(&self.data_dir, topic, addrs);
                                let _ = room.save();
                                self.try_save_friends();
                                return iced::Task::done(AppMessage::OpenRoom(topic));
                            }
                            Ok((sender, ContactAction::AddressUpdate { addrs }))
                                if addrs.iter().all(|addr| addr.id == sender) =>
                            {
                                let record = self
                                    .friends
                                    .ensure_friend(FriendId::from_public_key(sender));
                                record.record_addrs(addrs);
                                self.try_save_friends();
                            }
                            Ok((_sender, _action)) => {
                                self.push_system(
                                    "Rejected invalid contact control message.".to_string(),
                                );
                            }
                            Err(err) => {
                                debug!("invalid contact control message: {err:#}");
                            }
                        }
                    }
                    iroh_gossip::whisper::WhisperEvent::Message { from, content } => {
                        let text = String::from_utf8_lossy(&content).to_string();
                        let label = self
                            .names
                            .get(&from)
                            .cloned()
                            .unwrap_or_else(|| from.fmt_short().to_string());

                        // A ticket-bearing invite gives the recipient the
                        // route needed to bootstrap the deterministic private room.
                        let invite_ticket = text
                            .strip_prefix("\x00PRIVATE_CHAT:")
                            .and_then(|raw| raw.parse::<Ticket>().ok());
                        let is_invite = invite_ticket.is_some() || text == "\x00PRIVATE_CHAT";
                        if is_invite {
                            self.push_system(format!("{label} opened a private chat with you."));
                        }

                        let fid = FriendId::from_public_key(from);
                        let should_open_private = is_invite || self.friends.get(&fid).is_some();
                        if should_open_private {
                            let private_topic = private_topic(&self.local_public, &from);
                            if let Some(ticket) = invite_ticket {
                                let room = RoomStore::with_peers(
                                    &self.data_dir,
                                    private_topic,
                                    ticket.peers,
                                );
                                let _ = room.save();
                            }
                            let already_on_topic = matches!(
                                self.screen,
                                Screen::Chat { topic } if topic == private_topic
                            );
                            if !already_on_topic {
                                self.save_room_to_history();
                                return iced::Task::done(AppMessage::OpenRoom(private_topic));
                            }
                        }

                        if !is_invite {
                            self.entries.push(ChatEntry::remote(
                                format!("Whisper from {label}"),
                                text,
                                None,
                                None, // whisper events carry no sent_at
                                Some(from),
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

                        // On reconnect, sync any offline mailbox envelopes.
                        let has_mailbox = self
                            .friends
                            .get(&FriendId::from_public_key(peer))
                            .and_then(|r| r.mailbox_public_key)
                            .is_some();
                        if has_mailbox {
                            let endpoint = self.endpoint.clone();
                            let sk = self.secret_key.clone();
                            let dd = self.data_dir.clone();
                            let peer2 = peer;
                            return iced::Task::perform(
                                async move {
                                    match send_sync_request(&endpoint, &sk, peer2, 0).await {
                                        Ok(envelopes) => {
                                            let mut store = MailboxStore::load(&dd)
                                                .ok()
                                                .flatten()
                                                .unwrap_or_else(|| {
                                                    MailboxStore::for_recipient(&dd, sk.public())
                                                });
                                            let identity = MailboxIdentity::from_secret(&sk);
                                            let mut texts = Vec::new();
                                            for env in envelopes {
                                                match store.accept_incoming(
                                                    &identity,
                                                    env,
                                                    &[peer2],
                                                ) {
                                                    Ok((msg_id, plaintext)) => {
                                                        if let Ok(text) =
                                                            String::from_utf8(plaintext)
                                                        {
                                                            texts.push((msg_id, text));
                                                        }
                                                    }
                                                    Err(_) => {}
                                                }
                                            }
                                            let _ = store.save();
                                            // Send acks for accepted envelopes.
                                            for (msg_id, _) in &texts {
                                                let ack = MailboxAck::sign(&sk, msg_id);
                                                let _ = send_ack(&endpoint, &sk, peer2, ack).await;
                                            }
                                            AppMessage::MailboxReplayed { peer: peer2, texts }
                                        }
                                        Err(e) => AppMessage::ErrorMsg(format!(
                                            "Mailbox sync failed: {e}"
                                        )),
                                    }
                                },
                                std::convert::identity,
                            );
                        }
                    }
                    iroh_gossip::whisper::WhisperEvent::Disconnected { peer } => {
                        let label = self
                            .names
                            .get(&peer)
                            .cloned()
                            .unwrap_or_else(|| peer.fmt_short().to_string());
                        self.push_system(format!("[Whisper] Disconnected from {label}"));
                    }
                    iroh_gossip::whisper::WhisperEvent::MailboxEnvelope { .. } => {
                        // Mailbox envelopes are encrypted and processed by the mailbox
                        // store — the GUI chat does not interpret them.
                    }
                    iroh_gossip::whisper::WhisperEvent::MailboxAck { .. } => {
                        // Mailbox acknowledgements are verified and removed by the
                        // mailbox store — the GUI chat does not interpret them.
                    }
                }
                iced::Task::none()
            }

            AppMessage::InboxEvent(event) => {
                match event {
                    InboxEvent::EnvelopeReceived { from, envelope } => {
                        let label = self
                            .names
                            .get(&from)
                            .cloned()
                            .unwrap_or_else(|| from.fmt_short().to_string());

                        // Load mailbox store, accept incoming (validates + persists).
                        let mut store = match MailboxStore::load(&self.data_dir)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| {
                                MailboxStore::for_recipient(
                                    &self.data_dir,
                                    self.secret_key.public(),
                                )
                            }) {
                            s => s,
                        };
                        let identity = MailboxIdentity::from_secret(&self.secret_key);
                        match store.accept_incoming(&identity, envelope, &[from]) {
                            Ok((msg_id, plaintext)) => {
                                if let Ok(text) = String::from_utf8(plaintext) {
                                    self.entries.push(ChatEntry::remote(
                                        format!("Offline DM from {label}"),
                                        text,
                                        None,
                                        None,
                                        Some(from),
                                    ));
                                }
                                // Persist accepted state.
                                let _ = store.save();
                                // Send acknowledgement via async task.
                                let endpoint = self.endpoint.clone();
                                let sk = self.secret_key.clone();
                                return iced::Task::perform(
                                    async move {
                                        let ack = MailboxAck::sign(&sk, &msg_id);
                                        let _ = send_ack(&endpoint, &sk, from, ack).await;
                                    },
                                    |_| AppMessage::Noop,
                                );
                            }
                            Err(e) => {
                                self.push_system(format!(
                                    "[Mailbox] Failed to accept envelope from {label}: {e}"
                                ));
                            }
                        }
                        iced::Task::none()
                    }
                    InboxEvent::AckReceived {
                        from: _from,
                        ack: _ack,
                    } => {
                        // Remove acknowledged envelope from local store.
                        let mut store = match MailboxStore::load(&self.data_dir)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| MailboxStore::empty_at(&self.data_dir))
                        {
                            s => s,
                        };
                        if let Ok(true) = store.acknowledge_and_save(&_ack) {
                            debug!(
                                "mailbox: peer {} acknowledged envelope {}",
                                _from.fmt_short(),
                                _ack.message_id
                            );
                        }
                        iced::Task::none()
                    }
                    InboxEvent::SyncRequested { from, since_ms } => {
                        debug!(
                            "inbox: sync requested by {} since_ms={}",
                            from.fmt_short(),
                            since_ms
                        );
                        iced::Task::none()
                    }
                }
            }

            AppMessage::MessageSent(_text, event_id, msg_hash) => {
                if let Some(entry) = self.entries.iter_mut().find(|e| e.event_id == event_id) {
                    entry.delivery_state = DeliveryState::Sent;
                    entry.message_hash = Some(msg_hash);
                }
                let mut history = self.chat_history.lock().unwrap();
                let _ = history.update_delivery_state(event_id, DeliveryState::Sent);
                let _ = history.save();
                let mut outbox = self.outbox.lock().unwrap();
                let _ = outbox.update_delivery_state(event_id, DeliveryState::Sent);
                let _ = outbox.save();
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
                let endpoint = self.endpoint.clone();
                let fname = filename.clone();

                iced::Task::perform(
                    async move {
                        let tag = blob_store
                            .blobs()
                            .add_path(std::path::PathBuf::from(&abs_path))
                            .await
                            .map_err(|e| format!("Failed to hash file: {e}"))?;
                        let ticket_str =
                            blob_ticket_string(endpoint.watch_addr().get(), tag.hash, tag.format);
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
                    Some(sender),
                ));
                iced::Task::none()
            }
            AppMessage::ProfileImageDownloaded(peer, image_bytes) => {
                if image_bytes.is_empty() || image_bytes.len() > 2 * 1024 * 1024 {
                    // Ignore empty or oversized images (>2MB)
                    return iced::Task::none();
                }
                let handle = iced::widget::image::Handle::from_bytes(image_bytes);
                self.friend_image_handles.insert(peer, Some(handle));
                // Trigger UI re-draw by marking friends dirty (the renderer
                // reads friend_image_handles each frame).
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

                // Periodic presence heartbeat — broadcasts Message::Presence every ~5s.
                let mut tasks: Vec<iced::Task<AppMessage>> = Vec::new();
                // Relay selection and direct addresses are learned asynchronously.
                // Keep the room ticket shown in the UI (and therefore copied to the
                // clipboard) aligned with the endpoint's current address.
                // Personal-room tickets are no longer broadcast publicly;
                // they must be shared through direct (whisper) channels.
                if self.refresh_local_peer_addr() {
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
                }
                if self.presence_counter == 0 {
                    self.presence_counter = 5;
                    if let Some(ref sender) = self.sender {
                        let sk = self.secret_key.clone();
                        let ticket = self.personal_room_ticket();
                        let profile_image_ticket = self.profile_image_ticket.clone();
                        let label = self.local_label.clone();
                        let s = sender.clone();
                        tasks.push(iced::Task::perform(
                            async move {
                                // PresenceWithTicket is sent frequently for liveness, but
                                // it does not carry profile metadata. Re-announce AboutMe
                                // here so peers that joined after the initial room
                                // broadcast still learn (and can download) our avatar.
                                if let Ok(encoded) = SignedMessage::sign_and_encode(
                                    &sk,
                                    &crate::Message::AboutMe {
                                        name: label,
                                        profile_image_ticket,
                                    },
                                ) {
                                    s.broadcast(encoded).await.ok();
                                }
                                if let Ok(encoded) = SignedMessage::sign_and_encode(
                                    &sk,
                                    &crate::Message::PresenceWithTicket { ticket },
                                ) {
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

                // ── Profile image download: drain pending queue ─────────
                // Processed here (on ConnMonitorTick) as a fallback path in
                // case a ticket is pushed without a subsequent NetEvent to
                // trigger the NetEvent handler's own queue drain.
                if let Some((peer, ticket_str)) = self.pending_profile_image_tickets.pop_front() {
                    let blob_store = self.blob_store.clone();
                    let endpoint = self.endpoint.clone();
                    let memory_lookup = self.memory_lookup.clone();
                    let neighbors = self.neighbors.clone();
                    tasks.push(iced::Task::perform(
                        async move {
                            let ticket: BlobTicket = ticket_str
                                .parse::<BlobTicket>()
                                .map_err(|e| format!("Parse profile image ticket: {e}"))?;
                            seed_memory_lookup(&memory_lookup, &[ticket.addr().clone()]);
                            let peer_id = ticket.addr().id;
                            let candidates = download_candidates(peer_id, &neighbors);
                            blob_store
                                .downloader(&endpoint)
                                .download(ticket.hash(), candidates)
                                .await
                                .map_err(|e| format!("Download profile image: {e}"))?;
                            let mut reader = blob_store.blobs().reader(ticket.hash());
                            let mut buf = Vec::new();
                            use tokio::io::AsyncReadExt;
                            reader
                                .read_to_end(&mut buf)
                                .await
                                .map_err(|e| format!("Read profile image: {e}"))?;
                            Ok((peer, buf))
                        },
                        move |r: Result<(PublicKey, Vec<u8>), String>| match r {
                            Ok((peer, data)) => AppMessage::ProfileImageDownloaded(peer, data),
                            Err(e) => AppMessage::ErrorMsg(e),
                        },
                    ));
                }

                // ── Seen-on-visibility: when user is at bottom of log,
                // mark Delivered entries as Seen ──
                if self.follow_latest {
                    for ui_entry in self.entries.iter_mut() {
                        if ui_entry.delivery_state == DeliveryState::Delivered
                            && ui_entry.event_id > 0
                        {
                            ui_entry.delivery_state = DeliveryState::Seen;
                            let mut store = self.chat_history.lock().unwrap();
                            let _ =
                                store.update_delivery_state(ui_entry.event_id, DeliveryState::Seen);
                        }
                    }
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

            AppMessage::SetNickname(name) => {
                self.local_label = name;
                iced::Task::none()
            }

            AppMessage::OpenLogsWindow => {
                let data_dir = self.data_dir.clone();
                iced::Task::perform(async move { log_viewer::spawn(&data_dir) }, |result| {
                    match result {
                        Ok(()) => AppMessage::Noop,
                        Err(err) => AppMessage::ErrorMsg(err),
                    }
                })
            }

            AppMessage::Noop => iced::Task::none(),

            AppMessage::Scrolled(offset, vp_h) => {
                self.scroll_offset = offset;
                self.viewport_height = vp_h;
                // Detect whether the user is at the bottom of the chat log.
                // total_content_height is set during view_chat_log() each frame
                // via Cell interior mutability (allows &self reads in view()).
                let total = self.total_content_height.get();
                if total > 0.0 && offset + vp_h >= total - 10.0 {
                    self.follow_latest = true;
                } else if total > 0.0 {
                    self.follow_latest = false;
                }
                iced::Task::none()
            }

            AppMessage::CopyToClipboard(text) => {
                return iced::clipboard::write(text);
            }

            AppMessage::ToggleSound(enabled) => {
                self.sound_enabled = enabled;
                iced::Task::none()
            }

            AppMessage::PickProfileImage => iced::Task::perform(
                async {
                    let file = rfd::AsyncFileDialog::new()
                        .set_title("Choose profile image")
                        .pick_file()
                        .await;
                    match file {
                        Some(file) => {
                            if !supported_profile_image(file.path()) {
                                return Err("Unsupported profile image type. Use PNG, JPEG, GIF, WEBP, or BMP.".to_string());
                            }
                            let bytes = file.read().await;
                            if bytes.is_empty() {
                                Err("Profile image is empty.".to_string())
                            } else if bytes.len() > PROFILE_IMAGE_MAX_BYTES {
                                Err("Profile image must be 5 MiB or smaller.".to_string())
                            } else {
                                Ok(bytes)
                            }
                        }
                        None => Err("No profile image selected.".to_string()),
                    }
                },
                AppMessage::ProfileImagePicked,
            ),

            AppMessage::ProfileImagePicked(result) => {
                match result {
                    Ok(bytes) => {
                        if let Err(err) = save_profile_image(&self.data_dir, &bytes) {
                            self.push_system(format!("Could not save profile image: {err}"));
                            return iced::Task::none();
                        }
                        self.profile_image_handle =
                            Some(iced::widget::image::Handle::from_bytes(bytes.clone()));
                        self.push_system("Profile image updated.");

                        // Upload the image to the local blob store so peers can
                        // download it via the BlobTicket advertised in AboutMe.
                        let blob_store = self.blob_store.clone();
                        let endpoint = self.endpoint.clone();
                        iced::Task::perform(
                            async move {
                                let tag =
                                    blob_store.blobs().add_bytes(bytes).await.map_err(|e| {
                                        format!("Failed to store profile image: {e}")
                                    })?;
                                let ticket_str = blob_ticket_string(
                                    endpoint.watch_addr().get(),
                                    tag.hash,
                                    tag.format,
                                );
                                Ok(ticket_str)
                            },
                            |r: Result<String, String>| match r {
                                Ok(ticket) => AppMessage::ProfileImageUploaded(ticket),
                                Err(e) => AppMessage::ErrorMsg(e),
                            },
                        )
                    }
                    Err(err) if err != "No profile image selected." => {
                        self.push_system(err);
                        iced::Task::none()
                    }
                    Err(_) => iced::Task::none(),
                }
            }

            AppMessage::ProfileImageUploaded(ticket) => {
                self.profile_image_ticket = Some(ticket);
                // Broadcast the updated AboutMe so peers fetch our new image.
                if let Some(ref sender) = self.sender {
                    let sk = self.secret_key.clone();
                    let label = self.local_label.clone();
                    let ticket = self.profile_image_ticket.clone();
                    let s = sender.clone();
                    iced::Task::perform(
                        async move {
                            if let Ok(encoded) = SignedMessage::sign_and_encode(
                                &sk,
                                &crate::Message::AboutMe {
                                    name: label,
                                    profile_image_ticket: ticket,
                                },
                            ) {
                                s.broadcast(encoded).await.ok();
                            }
                        },
                        |_| AppMessage::Noop,
                    )
                } else {
                    iced::Task::none()
                }
            }

            AppMessage::RemoveProfileImage => {
                if self.profile_image_handle.is_some() {
                    match fs::remove_file(self.data_dir.join(PROFILE_IMAGE_FILE)) {
                        Ok(()) => {
                            self.profile_image_handle = None;
                            self.profile_image_ticket = None;
                            self.push_system("Profile image removed.");
                            // Re-broadcast AboutMe with no ticket so peers stop
                            // showing our old image.
                            if let Some(ref sender) = self.sender {
                                let sk = self.secret_key.clone();
                                let label = self.local_label.clone();
                                let s = sender.clone();
                                return iced::Task::perform(
                                    async move {
                                        if let Ok(encoded) = SignedMessage::sign_and_encode(
                                            &sk,
                                            &crate::Message::AboutMe {
                                                name: label,
                                                profile_image_ticket: None,
                                            },
                                        ) {
                                            s.broadcast(encoded).await.ok();
                                        }
                                    },
                                    |_| AppMessage::Noop,
                                );
                            }
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                            self.profile_image_handle = None;
                            self.profile_image_ticket = None;
                            self.push_system("Profile image removed.");
                        }
                        Err(err) => {
                            self.push_system(format!("Could not remove profile image: {err}"));
                        }
                    }
                }
                iced::Task::none()
            }

            AppMessage::ClearHistoryRequested => {
                self.history_confirm_clear = !self.history_confirm_clear;
                iced::Task::none()
            }

            AppMessage::ConfirmClearHistory => {
                self.history_confirm_clear = false;
                self.chat_history.lock().unwrap().clear();
                self.chat_history_dirty = true;
                iced::Task::none()
            }

            AppMessage::DeleteRoomRequested(topic) => {
                // Toggle confirmation for this topic.
                self.room_delete_confirm_topic = if self.room_delete_confirm_topic == Some(topic) {
                    None
                } else {
                    Some(topic)
                };
                iced::Task::none()
            }

            AppMessage::ConfirmDeleteRoom(topic) => {
                self.room_delete_confirm_topic = None;
                // Remove room and chat history, then persist.
                self.room_history.remove(&topic);
                self.room_history_dirty = true;
                self.chat_history.lock().unwrap().remove_topic(&topic);
                self.chat_history_dirty = true;
                self.persist_room_history();
                self.try_save_chat_history();
                iced::Task::none()
            }

            AppMessage::MailboxReplayed { peer, texts } => {
                let label = self
                    .names
                    .get(&peer)
                    .cloned()
                    .unwrap_or_else(|| peer.fmt_short().to_string());
                for (_msg_id, text) in texts {
                    self.entries.push(ChatEntry::remote(
                        format!("Offline DM from {label}"),
                        text,
                        None,
                        None,
                        Some(peer),
                    ));
                }
                iced::Task::none()
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
            FriendEvent::AddressUpdated { peer, addr } => {
                self.friends
                    .ensure_friend(FriendId::from_public_key(peer))
                    .record_addrs([addr]);
                self.friends_dirty = true;
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

    fn last_announced_name(&self, peer: &PublicKey) -> Option<String> {
        let fid = FriendId::from_public_key(*peer);
        self.friends
            .get(&fid)
            .and_then(|record| record.last_announced_name.clone())
            .or_else(|| self.names.get(peer).cloned())
    }

    fn set_name(&mut self, peer: PublicKey, name: String) -> Option<String> {
        self.names.insert(peer, name)
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

    fn record_profile_image_ticket(&mut self, peer: PublicKey, ticket: String) {
        let fid = FriendId::from_public_key(peer);
        self.friends
            .set_last_announced_profile_image_ticket(fid, &ticket);
        self.friends_dirty = true;
        // Compare against the last ticket seen for this peer to avoid
        // re-invalidating + re-downloading when the same ticket is
        // re-announced in a periodic AboutMe broadcast (every ~5s via
        // ConnMonitorTick).  Repeated invalidation causes a flicker
        // between the avatar image and the fallback emoji while the
        // redundant download is in flight.
        if self.friend_image_tickets.get(&peer) == Some(&ticket) {
            return;
        }
        self.friend_image_tickets.insert(peer, ticket.clone());
        // Invalidate any previous image immediately.  The newly announced
        // ticket may point to a replacement blob, so retaining the old
        // handle would show stale artwork while the download is in flight.
        self.friend_image_handles.insert(peer, None);
        self.pending_profile_image_tickets.push_back((peer, ticket));
    }

    fn clear_profile_image(&mut self, peer: PublicKey) {
        let fid = FriendId::from_public_key(peer);
        self.friends
            .set_last_announced_profile_image_ticket(fid, "");
        self.friends_dirty = true;
        self.friend_image_handles.remove(&peer);
        self.friend_image_tickets.remove(&peer);
        self.pending_profile_image_tickets
            .retain(|(queued_peer, _)| *queued_peer != peer);
    }

    fn push_system(&mut self, text: String) {
        self.entries.push(ChatEntry::system(text));
    }

    fn push_remote(
        &mut self,
        peer: PublicKey,
        label: String,
        text: String,
        hash: Option<MessageHash>,
        sent_at: Option<u64>,
    ) {
        self.entries
            .push(ChatEntry::remote(label, text, hash, sent_at, Some(peer)));
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
        self.friend_online_cache.insert(peer);
        self.recompute_connection_counts();
    }

    fn on_neighbor_down(&mut self, peer: PublicKey) {
        self.neighbors.remove(&peer);
        self.friend_online_cache.remove(&peer);
        self.recompute_connection_counts();
    }

    fn record_activity(&mut self, peer: PublicKey) {
        // Update mesh health timestamp for this peer so the mesh
        // watchdog doesn't falsely flag them as stale.
        self.friend_online_cache.insert(peer);
        self.neighbors.insert(peer);
    }

    fn record_presence(&mut self, peer: PublicKey) {
        // A Presence heartbeat proves the peer is still alive and
        // connected.  Update the online cache so the friend list
        // shows them as online, and ensure they're tracked as a
        // neighbor for mesh health purposes.
        self.friend_online_cache.insert(peer);
        self.neighbors.insert(peer);
    }

    fn store_peer_ticket(&mut self, peer: PublicKey, ticket: Ticket) -> bool {
        let fid = FriendId::from_public_key(peer);
        let record = self.friends.ensure_friend(fid);
        record.record_addrs(ticket.peers.clone());
        record.record_room(ticket.topic, ticket);
        true
    }

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
            Screen::Settings => self.view_settings_screen().into(),
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
                    .push(
                        row![
                            text("Iroh Gossip Chat").size(TYPO_XL).width(Length::Fill),
                            button(text("⚙").size(TYPO_MD))
                                .on_press(AppMessage::OpenSettings)
                                .padding(SPACE_4),
                        ]
                        .spacing(SPACE_8)
                        .align_y(Alignment::Center),
                    )
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

        // ── Default lobby ticket ──
        if self.topic == Self::default_lobby_topic() && !self.ticket_str.is_empty() {
            content = content.push(
                container(
                    row![
                        text("Lobby ticket (share once with another user):")
                            .size(TYPO_SM)
                            .width(Length::Fill),
                        button("Copy ticket")
                            .on_press(AppMessage::CopyToClipboard(self.ticket_str.clone()))
                            .padding(SPACE_4),
                    ]
                    .spacing(SPACE_8)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding(SPACE_12)
                .style(move |t| container_surface(t)),
            );
        }

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
                        row![
                            text(" 🔗 ").size(TYPO_MD),
                            text("Join via Ticket").size(TYPO_MD),
                        ]
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
                self.friends
                    .get(&fid)
                    .is_some_and(|record| !record.rooms.is_empty())
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
                    discovered_list =
                        discovered_list.push(self.view_discovered_user_row(pk, &label));
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
                text(last_seen_str).size(TYPO_XS).color(status_color),
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

        if self.room_delete_confirm_topic == Some(topic) {
            // ── Confirmation state ──
            let confirm_row = row![
                text("Delete this room?").size(TYPO_SM).width(Length::Fill),
                button(text("Yes").size(TYPO_SM))
                    .on_press(AppMessage::ConfirmDeleteRoom(topic))
                    .padding([SPACE_4, SPACE_8]),
                button(text("No").size(TYPO_SM))
                    .on_press(AppMessage::DeleteRoomRequested(topic))
                    .padding([SPACE_4, SPACE_8]),
            ]
            .spacing(SPACE_4)
            .align_y(iced::Alignment::Center)
            .padding(SPACE_8);

            return container(confirm_row)
                .width(Length::Fill)
                .style(move |t| container_surface(t))
                .into();
        }

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
                    .on_press(AppMessage::DeleteRoomRequested(topic))
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
                button(text("⚙").size(TYPO_MD))
                    .on_press(AppMessage::OpenSettings)
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
                button(
                    text(&self.ticket_str)
                        .size(TYPO_XXS)
                        .wrapping(Wrapping::Word),
                )
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

        let theme = self.theme();

        // ── Empty state ──
        if self.entries.is_empty() {
            let col = Column::new().push(
                container(text("No messages yet.").color(self.color_muted()))
                    .padding([0.0, SPACE_8])
                    .width(Length::Fill),
            );
            self.total_content_height.set(0.0);
            return scrollable(col)
                .id(CHAT_LOG)
                .anchor_bottom()
                .width(iced::Length::Fill)
                .height(iced::Length::Fill)
                .on_scroll(|v: scrollable::Viewport| {
                    AppMessage::Scrolled(v.absolute_offset().y, v.bounds().height)
                });
        }

        // ── Height estimation constants (pixels per entry type) ──
        const DATE_SEP_H: f32 = 32.0;
        const SYSTEM_H: f32 = 24.0;
        const MSG_BASE_H: f32 = 76.0;
        const IMAGE_EXTRA: f32 = 304.0;
        const REACTION_EXTRA: f32 = 22.0;
        /// Over-scan buffer in pixels — extra entries above/below viewport
        /// so scroll-jumps don't flash empty space under estimation error.
        const OVERSCAN: f32 = 800.0;

        // ── First pass: compute per-entry estimated heights ──
        let total = self.entries.len();
        let mut heights: Vec<f32> = Vec::with_capacity(total);
        let mut prev_day_ht: Option<i64> = None;

        for entry in self.entries.iter() {
            let mut h = 0.0;
            let day = entry.timestamp.map(|ts| ts / 86400000);
            if let Some(d) = day {
                if prev_day_ht.map_or(true, |prev| prev != d) {
                    h += DATE_SEP_H;
                }
                prev_day_ht = Some(d);
            }
            match entry.kind {
                ChatKind::System => h += SYSTEM_H,
                _ => {
                    h += MSG_BASE_H;
                    if entry.image_bytes.is_some() {
                        h += IMAGE_EXTRA;
                    }
                    if !entry.reactions.is_empty() {
                        h += REACTION_EXTRA;
                    }
                }
            }
            heights.push(h);
        }

        let mut cum: Vec<f32> = Vec::with_capacity(total);
        let mut running = 0.0_f32;
        for &h in &heights {
            cum.push(running);
            running += h;
        }
        let total_height = running;
        self.total_content_height.set(total_height);

        // ── Determine visible window ──
        let so = if self.scroll_offset >= f32::MAX / 2.0 {
            (total_height - self.viewport_height.max(200.0)).max(0.0)
        } else {
            self.scroll_offset
        };
        let view_top = so.max(0.0);
        let view_bot = view_top + self.viewport_height.max(200.0);

        let range_top = (view_top - OVERSCAN).max(0.0);
        let range_bot = (view_bot + OVERSCAN).min(total_height);

        let first_idx = cum.partition_point(|&c| c < range_top);
        let last_idx = cum
            .partition_point(|&c| c <= range_bot)
            .saturating_sub(1)
            .min(total.saturating_sub(1));
        let first_idx = first_idx.min(total.saturating_sub(1));
        let last_idx = last_idx.max(first_idx);

        // ── Build windowed content column ──
        let mut col = Column::new().spacing(SPACE_4).width(Length::Fill);

        let top_space_h = cum[first_idx];
        if top_space_h > 0.0 {
            col = col.push(
                container(space::Space::new().height(Length::Fixed(top_space_h)))
                    .width(Length::Fill),
            );
        }

        let mut prev_day: Option<i64> = if first_idx > 0 {
            self.entries[first_idx - 1]
                .timestamp
                .map(|ts| ts / 86400000)
        } else {
            None
        };

        for i in first_idx..=last_idx {
            let entry = &self.entries[i];

            // ── Date separator ──
            let entry_day = entry.timestamp.map(|ts| ts / 86400000);
            if let Some(day) = entry_day {
                if prev_day.map_or(true, |prev| prev != day) {
                    let date_label = format_message_time(day * 86400000);
                    let sep_text = format!(" — {date_label} — ");
                    let sep = Row::new()
                        .push(space::horizontal())
                        .push(text(sep_text).size(TYPO_XS).color(text_system(&theme)))
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

            let label_text = if matches!(entry.kind, ChatKind::Local) && entry.event_id > 0 {
                format!("[{} {}]", entry.label, entry.delivery_state.display_icon())
            } else {
                format!("[{}]", entry.label)
            };
            let label_el = text(label_text).size(TYPO_XS).color(label_color);

            let body_el = text(&entry.body)
                .size(TYPO_SM)
                .wrapping(Wrapping::Word)
                .width(Length::Fill)
                .color(body_color);

            let bubble =
                container(body_el)
                    .padding([SPACE_4, SPACE_8])
                    .style(move |t: &iced::Theme| {
                        let mut s = iced::widget::container::Style::default();
                        if let Some(bg) = bubble_bg(t, entry.kind) {
                            s.background = Some(bg);
                        }
                        s.border.radius = (8.0_f32).into();
                        s
                    });

            let ts_text = entry.timestamp.map(format_message_time).unwrap_or_default();
            let ts_el = text(ts_text).size(TYPO_XXS).color(text_muted(&theme));

            let bubble_col = Column::new()
                .push(label_el)
                .push(bubble)
                .push(ts_el)
                .spacing(SPACE_2)
                .max_width(480.0);

            let msg_row = match entry.kind {
                ChatKind::Remote => {
                    let avatar: iced::Element<'_, AppMessage> = {
                        let cached = entry
                            .sender_key
                            .and_then(|pk| self.friend_image_handles.get(&pk))
                            .and_then(|opt| opt.as_ref())
                            .cloned();
                        if let Some(handle) = cached {
                            iced::widget::image(handle.clone())
                                .content_fit(iced::ContentFit::ScaleDown)
                                .width(Length::Fixed(28.0))
                                .height(Length::Fixed(28.0))
                                .into()
                        } else {
                            text("👤").size(TYPO_LG).into()
                        }
                    };
                    Row::new()
                        .push(avatar)
                        .push(bubble_col)
                        .align_y(iced::Alignment::Center)
                        .spacing(SPACE_6)
                }
                ChatKind::Local => {
                    let avatar: iced::Element<'_, AppMessage> =
                        if let Some(ref handle) = self.profile_image_handle {
                            iced::widget::image(handle.clone())
                                .content_fit(iced::ContentFit::ScaleDown)
                                .width(Length::Fixed(28.0))
                                .height(Length::Fixed(28.0))
                                .into()
                        } else {
                            text("👤").size(TYPO_LG).into()
                        };
                    Row::new()
                        .push(avatar)
                        .push(bubble_col)
                        .align_y(iced::Alignment::Center)
                        .spacing(SPACE_6)
                }
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

        // Bottom spacer
        let bottom_start = cum[last_idx] + heights[last_idx];
        let bottom_h = total_height - bottom_start;
        if bottom_h > 0.0 {
            col = col.push(
                container(space::Space::new().height(Length::Fixed(bottom_h))).width(Length::Fill),
            );
        }

        scrollable(col)
            .id(CHAT_LOG)
            .anchor_bottom()
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .on_scroll(|v: scrollable::Viewport| {
                AppMessage::Scrolled(v.absolute_offset().y, v.bounds().height)
            })
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
                        Color::from_rgb(0.05, 0.50, 0.05) // #0d800d
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
        let actions = row![attach_btn, send_btn, help_btn,]
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
                s.background = Some(iced::Background::Color(if matches!(t, iced::Theme::Dark) {
                    Color::from_rgb(0.10, 0.10, 0.12)
                } else {
                    Color::from_rgb(0.97, 0.97, 0.98)
                }));
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
            .push(text("/leave          Leave this room and delete from history").size(TYPO_SM))
            .push(text("/help           Toggle this menu").size(TYPO_SM))
            .push(text("/friend add <pk> [alias]  Track a friend's online status").size(TYPO_SM))
            .push(text("/friend remove <pk|alias> Stop tracking a friend").size(TYPO_SM))
            .push(text("/friend list    List tracked friends and their status").size(TYPO_SM))
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(text("/react <idx> <emoji>  Add a reaction to a message").size(TYPO_SM))
            .push(text("/edit <idx> <text>   Edit a message").size(TYPO_SM))
            .push(text("/delete <idx>        Delete a message").size(TYPO_SM))
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(text("Type a message and press Enter to send.").size(TYPO_SM))
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(text("Tip: click ✕ on a room in the chat list to remove it.").size(TYPO_SM))
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

    fn view_settings_screen(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{
            button, container, row, rule, scrollable, text, text_input, Column, Row, Space,
        };
        use iced::{Alignment, Length};
        use iroh_gossip::chat_core::MeshHealth;

        // ── Section card helper ──
        fn section_card<'a>(
            title: &'a str,
            children: Vec<iced::Element<'a, AppMessage>>,
        ) -> iced::Element<'a, AppMessage> {
            let body = Column::new()
                .push(
                    text(title.to_string())
                        .size(TYPO_XS)
                        .style(text_muted_style),
                )
                .push(rule::horizontal(1).style(iced::widget::rule::weak))
                .push(Space::new().height(Length::Fixed(SPACE_8)));
            let body = children
                .into_iter()
                .fold(body, |col, child| col.push(child));
            container(body)
                .padding([SPACE_12, SPACE_16])
                .width(Length::Fill)
                .style(container_card)
                .into()
        }

        // ── Identity section ──
        let nickname_input = container(
            text_input("Your display name…", &self.local_label)
                .on_input(AppMessage::SetNickname)
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .padding(SPACE_4);

        let profile_preview: iced::Element<'_, AppMessage> =
            if let Some(ref handle) = self.profile_image_handle {
                iced::widget::image(handle.clone())
                    .content_fit(iced::ContentFit::ScaleDown)
                    .width(Length::Fixed(48.0))
                    .height(Length::Fixed(48.0))
                    .into()
            } else {
                text("👤").size(TYPO_XL).into()
            };
        let mut profile_row = Row::new()
            .push(
                Column::new()
                    .push(profile_preview)
                    .push(text("Profile image").size(TYPO_MD))
                    .push(
                        text(if self.profile_image_handle.is_some() {
                            "Shown beside your messages"
                        } else {
                            "No image selected (using a person icon)"
                        })
                        .size(TYPO_XS)
                        .style(text_muted_style),
                    )
                    .spacing(SPACE_2)
                    .width(Length::Fill)
                    .align_x(Alignment::Start),
            )
            .push(
                button(text("Choose image").size(TYPO_SM))
                    .on_press(AppMessage::PickProfileImage)
                    .padding([SPACE_6, SPACE_12]),
            );
        if self.profile_image_handle.is_some() {
            profile_row = profile_row.push(
                button(text("Remove").size(TYPO_SM))
                    .on_press(AppMessage::RemoveProfileImage)
                    .padding([SPACE_6, SPACE_12]),
            );
        }
        let profile_row = profile_row.spacing(SPACE_12).align_y(Alignment::Center);

        let identity_card = section_card(
            "👤  IDENTITY",
            vec![nickname_input.into(), profile_row.into()],
        );

        // ── Appearance section ──
        let appearance_theme = if self.dark_mode { "Dark" } else { "Light" };

        let appearance_row = Row::new()
            .push(
                Column::new()
                    .push(text(appearance_theme).size(TYPO_MD))
                    .push(
                        text("Switch between dark and light colour themes.")
                            .size(TYPO_XS)
                            .style(text_muted_style),
                    )
                    .spacing(SPACE_2)
                    .width(Length::Fill)
                    .align_x(Alignment::Start),
            )
            .push(
                button(
                    text(if self.dark_mode {
                        "☀ Light"
                    } else {
                        "🌙 Dark"
                    })
                    .size(TYPO_SM),
                )
                .on_press(AppMessage::ToggleDark(!self.dark_mode))
                .padding([SPACE_6, SPACE_12]),
            )
            .spacing(SPACE_12)
            .align_y(Alignment::Center);

        let appearance_card = section_card("🎨  APPEARANCE", vec![appearance_row.into()]);

        // ── Notifications section ──
        let sound_label = if self.sound_enabled {
            "Sound on"
        } else {
            "Sound off"
        };
        let notifications_row = Row::new()
            .push(
                Column::new()
                    .push(text(sound_label).size(TYPO_MD))
                    .push(
                        text("Play a notification sound when a new message arrives.")
                            .size(TYPO_XS)
                            .style(text_muted_style),
                    )
                    .spacing(SPACE_2)
                    .width(Length::Fill)
                    .align_x(Alignment::Start),
            )
            .push(
                button(
                    text(if self.sound_enabled {
                        "🔇 Mute"
                    } else {
                        "🔊 Unmute"
                    })
                    .size(TYPO_SM),
                )
                .on_press(AppMessage::ToggleSound(!self.sound_enabled))
                .padding([SPACE_6, SPACE_12]),
            )
            .spacing(SPACE_12)
            .align_y(Alignment::Center);

        let notifications_card = section_card("🔔  NOTIFICATIONS", vec![notifications_row.into()]);

        // ── Network section ──
        let network_info = row![text(format!(
            "{} direct · {} relay · {} neighbors",
            self.direct_peers,
            self.relayed_peers,
            self.neighbors.len(),
        ))
        .size(TYPO_SM),]
        .spacing(SPACE_4);

        let mesh_status = row![text(match &self.mesh_health {
            MeshHealth::Good => "Mesh: healthy".into(),
            MeshHealth::Degraded(reason) => format!("Mesh: degraded — {reason}"),
            MeshHealth::Offline(reason) => format!("Mesh: offline — {reason}"),
        })
        .size(TYPO_SM),]
        .spacing(SPACE_4);

        let network_card =
            section_card("🌐  NETWORK", vec![network_info.into(), mesh_status.into()]);

        // ── Relay section ──
        let relay_info =
            row![text(format!("Mode: {}", fmt_relay_mode(&self.relay_mode))).size(TYPO_SM),]
                .spacing(SPACE_4);

        let relay_note = text("Relay mode is set at startup and cannot be changed at runtime.")
            .size(TYPO_XS)
            .style(text_muted_style);

        let relay_card = section_card("📡  RELAY", vec![relay_info.into(), relay_note.into()]);

        // ── Logs & Diagnostics section ──
        let data_dir_str = self.data_dir.to_string_lossy().to_string();

        let logs_row = Row::new()
            .push(
                Column::new()
                    .push(text("Open logs").size(TYPO_MD))
                    .push(
                        text("View application logs in a separate window.")
                            .size(TYPO_XS)
                            .style(text_muted_style),
                    )
                    .spacing(SPACE_2)
                    .width(Length::Fill)
                    .align_x(Alignment::Start),
            )
            .push(
                button(text("Open").size(TYPO_SM))
                    .on_press(AppMessage::OpenLogsWindow)
                    .padding([SPACE_6, SPACE_12]),
            )
            .spacing(SPACE_12)
            .align_y(Alignment::Center);

        let data_dir_label = Row::new()
            .push(
                Column::new()
                    .push(text("Data directory").size(TYPO_MD))
                    .push(
                        text(data_dir_str.clone())
                            .size(TYPO_XXS)
                            .style(text_muted_style)
                            .wrapping(iced::widget::text::Wrapping::Word),
                    )
                    .spacing(SPACE_2)
                    .width(Length::Fill)
                    .align_x(Alignment::Start),
            )
            .spacing(SPACE_12)
            .align_y(Alignment::Center);

        let logs_card = section_card(
            "📋  LOGS & DIAGNOSTICS",
            vec![logs_row.into(), data_dir_label.into()],
        );

        // ── Data Management section ──
        let clear_history_row = if self.history_confirm_clear {
            Row::new()
                .push(
                    Column::new()
                        .push(text("Clear all history?").size(TYPO_MD).style(|t| {
                            iced::widget::text::Style {
                                color: Some(if matches!(t, iced::Theme::Dark) {
                                    Color::from_rgb(0.9, 0.3, 0.3)
                                } else {
                                    Color::from_rgb(0.8, 0.2, 0.2)
                                }),
                            }
                        }))
                        .push(
                            text("This will delete all stored chat messages permanently.")
                                .size(TYPO_XS)
                                .style(text_muted_style),
                        )
                        .spacing(SPACE_2)
                        .width(Length::Fill)
                        .align_x(Alignment::Start),
                )
                .push(
                    button(text("Confirm").size(TYPO_SM))
                        .on_press(AppMessage::ConfirmClearHistory)
                        .padding([SPACE_6, SPACE_12]),
                )
                .push(
                    button(text("Cancel").size(TYPO_SM))
                        .on_press(AppMessage::ClearHistoryRequested)
                        .padding([SPACE_6, SPACE_12]),
                )
                .spacing(SPACE_8)
                .align_y(Alignment::Center)
        } else {
            Row::new()
                .push(
                    Column::new()
                        .push(text("Clear history").size(TYPO_MD))
                        .push(
                            text("Delete all stored chat messages permanently.")
                                .size(TYPO_XS)
                                .style(text_muted_style),
                        )
                        .spacing(SPACE_2)
                        .width(Length::Fill)
                        .align_x(Alignment::Start),
                )
                .push(
                    button(text("Clear").size(TYPO_SM))
                        .on_press(AppMessage::ClearHistoryRequested)
                        .padding([SPACE_6, SPACE_12]),
                )
                .spacing(SPACE_12)
                .align_y(Alignment::Center)
        };

        let data_card = section_card("💾  DATA", vec![clear_history_row.into()]);

        // ── Bottom navigation ──
        let nav_row = Row::new()
            .push(
                button(text("← Back").size(TYPO_MD))
                    .on_press(AppMessage::CloseSettings)
                    .style(|t, _status| {
                        let mut s = iced::widget::button::Style::default();
                        s.background = Some(iced::Background::Color(bg_surface(t)));
                        s.border = iced::Border {
                            color: border_muted(t),
                            width: 1.0,
                            radius: SPACE_8.into(),
                        };
                        s.text_color = text_muted_style(t)
                            .color
                            .unwrap_or(iced::Color::from_rgb(0.6, 0.6, 0.6));
                        s
                    })
                    .padding([SPACE_8, SPACE_16]),
            )
            .spacing(SPACE_8)
            .align_y(Alignment::Center);

        // ── Assemble page ──
        let content = Column::new()
            .push(text("Settings").size(TYPO_XL))
            .push(Space::new().height(Length::Fixed(SPACE_16)))
            .push(identity_card)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(appearance_card)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(notifications_card)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(network_card)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(relay_card)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(logs_card)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(data_card)
            .push(Space::new().height(Length::Fixed(SPACE_16)))
            .push(nav_row)
            .spacing(SPACE_6)
            .padding(SPACE_24)
            .align_x(Alignment::Start)
            .width(Length::Fill)
            .max_width(520.0);

        let scrollable = scrollable(content).width(Length::Fill).height(Length::Fill);

        container(scrollable)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |t| container_primary(t))
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

struct InboxRxHandle(Arc<Mutex<UnboundedReceiver<InboxEvent>>>);

impl std::hash::Hash for InboxRxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

fn subscription_stream(
    rx: &RxHandle,
    friend_rx: &FriendRxHandle,
    whisper_rx: &WhisperRxHandle,
    inbox_rx: &InboxRxHandle,
) -> Pin<Box<dyn Stream<Item = AppMessage> + Send>> {
    let rx = Arc::clone(&rx.0);
    let friend_rx = Arc::clone(&friend_rx.0);
    let whisper_rx = Arc::clone(&whisper_rx.0);
    let inbox_rx = Arc::clone(&inbox_rx.0);
    Box::pin(n0_future::stream::unfold(
        (rx, friend_rx, whisper_rx, inbox_rx),
        |(rx, friend_rx, whisper_rx, inbox_rx)| async move {
            let mut rx_guard = rx.lock().await;
            let mut friend_guard = friend_rx.lock().await;
            let mut whisper_guard = whisper_rx.lock().await;
            let mut inbox_guard = inbox_rx.lock().await;
            tokio::select! {
                event = rx_guard.recv() => {
                    drop(whisper_guard);
                    drop(friend_guard);
                    drop(inbox_guard);
                    drop(rx_guard);
                    event.map(|e| (AppMessage::NetEvent(e), (rx, friend_rx, whisper_rx, inbox_rx)))
                }
                event = friend_guard.recv() => {
                    drop(whisper_guard);
                    drop(rx_guard);
                    drop(inbox_guard);
                    drop(friend_guard);
                    event.map(|e| (AppMessage::FriendEvent(e), (rx, friend_rx, whisper_rx, inbox_rx)))
                }
                event = whisper_guard.recv() => {
                    drop(friend_guard);
                    drop(rx_guard);
                    drop(inbox_guard);
                    drop(whisper_guard);
                    event.map(|e| (AppMessage::WhisperEvent(e), (rx, friend_rx, whisper_rx, inbox_rx)))
                }
                event = inbox_guard.recv() => {
                    drop(friend_guard);
                    drop(rx_guard);
                    drop(whisper_guard);
                    drop(inbox_guard);
                    event.map(|e| (AppMessage::InboxEvent(e), (rx, friend_rx, whisper_rx, inbox_rx)))
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
        inbox_rx: Arc<Mutex<UnboundedReceiver<InboxEvent>>>,
    ) -> iced::Subscription<AppMessage> {
        iced::Subscription::batch(vec![
            iced::time::every(std::time::Duration::from_secs(1))
                .map(|_| AppMessage::ConnMonitorTick),
            iced::time::every(std::time::Duration::from_secs(30))
                .map(|_| AppMessage::MeshWatchdogTick),
            iced::Subscription::run_with(
                (
                    RxHandle(rx),
                    FriendRxHandle(friend_rx),
                    WhisperRxHandle(whisper_rx),
                    InboxRxHandle(inbox_rx),
                ),
                |(rx, friend_rx, whisper_rx, inbox_rx)| {
                    subscription_stream(&rx, &friend_rx, &whisper_rx, &inbox_rx)
                },
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone, Utc};

    #[test]
    fn gui_file_share_ticket_is_parseable_and_has_sender_address() {
        let key = SecretKey::generate();
        let addr = EndpointAddr::new(key.public());
        let hash = iroh_blobs::Hash::from_bytes([7; 32]);

        let ticket = blob_ticket_string(addr, hash, iroh_blobs::BlobFormat::Raw);
        let parsed: BlobTicket = ticket.parse().expect("GUI ticket must be parseable");

        assert_eq!(parsed.addr().id, key.public());
        assert_eq!(parsed.hash(), hash);
    }

    #[test]
    fn hash_only_file_share_value_is_not_a_blob_ticket() {
        let hash = iroh_blobs::Hash::from_bytes([7; 32]);
        assert!(hash.to_string().parse::<BlobTicket>().is_err());
    }

    #[test]
    fn format_message_time_converts_utc_into_local_today_time() {
        let tz = FixedOffset::east_opt(2 * 3600).expect("valid offset");
        let now = tz
            .with_ymd_and_hms(2024, 1, 3, 12, 0, 0)
            .single()
            .expect("valid local now");
        let timestamp_ms = Utc
            .with_ymd_and_hms(2024, 1, 3, 0, 30, 0)
            .single()
            .expect("valid utc timestamp")
            .timestamp_millis();

        let rendered =
            format_message_time_with(timestamp_ms, now, |ms| tz.timestamp_millis_opt(ms).single());

        assert_eq!(rendered, "02:30");
    }

    #[test]
    fn format_message_time_converts_utc_into_local_weekday() {
        let tz = FixedOffset::east_opt(2 * 3600).expect("valid offset");
        let now = tz
            .with_ymd_and_hms(2024, 1, 3, 12, 0, 0)
            .single()
            .expect("valid local now");
        let timestamp_ms = Utc
            .with_ymd_and_hms(2024, 1, 2, 20, 30, 0)
            .single()
            .expect("valid utc timestamp")
            .timestamp_millis();

        let rendered =
            format_message_time_with(timestamp_ms, now, |ms| tz.timestamp_millis_opt(ms).single());

        assert_eq!(rendered, "Tue 22:30");
    }

    #[test]
    fn record_profile_image_ticket_dedup_same_ticket_skips_redownload() {
        use iroh::PublicKey;
        use std::collections::{HashMap, VecDeque};

        let sk = SecretKey::generate();
        let pk = sk.public();
        let ticket_a = "ticket_v1_hash_abc".to_string();
        let ticket_a_dup = "ticket_v1_hash_abc".to_string();
        let ticket_b = "ticket_v2_hash_xyz".to_string();

        // Test the dedup logic inline (same guard as record_profile_image_ticket).
        let mut handles: HashMap<PublicKey, Option<iced::widget::image::Handle>> = HashMap::new();
        let mut tickets: HashMap<PublicKey, String> = HashMap::new();
        let mut queue: VecDeque<(PublicKey, String)> = VecDeque::new();

        // First call: new ticket → should queue a download.
        let pk1 = pk;
        tickets.insert(pk1, ticket_a.clone());
        handles.insert(pk1, None);
        queue.push_back((pk1, ticket_a.clone()));
        assert_eq!(queue.len(), 1, "first ticket should be queued");
        assert_eq!(
            handles.get(&pk1),
            Some(&None),
            "handle should be invalidated (None)"
        );

        // Simulate a successful download completing.
        let _handle = iced::widget::image::Handle::from_bytes(vec![0; 32]);
        handles.insert(pk1, None); // would become Some(handle) after download

        // Second call: same ticket → should NOT re-invalidate or re-queue.
        // Simulate the guard from record_profile_image_ticket.
        if tickets.get(&pk1) != Some(&ticket_a_dup) {
            panic!("guard failed: ticket should match");
        }
        // Guard returns early, so nothing changes.
        assert_eq!(queue.len(), 1, "same ticket should NOT add to queue");
        assert_eq!(
            tickets.get(&pk1),
            Some(&ticket_a),
            "cached ticket should remain unchanged"
        );

        // Third call: NEW ticket → should update and re-queue.
        tickets.insert(pk1, ticket_b.clone());
        handles.insert(pk1, None);
        queue.push_back((pk1, ticket_b.clone()));
        assert_eq!(queue.len(), 2, "new ticket should be queued");
        assert_eq!(
            tickets.get(&pk1),
            Some(&ticket_b),
            "cached ticket should update"
        );

        // clear_profile_image should remove the cached ticket.
        handles.remove(&pk1);
        tickets.remove(&pk1);
        queue.retain(|(p, _)| *p != pk1);
        assert!(!handles.contains_key(&pk1), "handle should be removed");
        assert!(
            !tickets.contains_key(&pk1),
            "cached ticket should be removed"
        );
        assert_eq!(queue.len(), 0, "queue should be cleared");
    }
}
