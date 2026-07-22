//! Shared chat core — reusable state machine, protocol types, and network event handling.
//!
//! This module contains the protocol types (`Message`, `SignedMessage`, `Ticket`),
//! the chat state machine (`AppState`, `Composer`, `ChatEntry`, `StatusContext`),
//! and network event processing (`handle_net_event`, `forward_gossip_events`).
//!
//! It has **no** terminal/ratatui/crossterm dependencies, making it usable from
//! any frontend (TUI, GUI, headless).
//!
//! The [`ChatCallbacks`] trait is defined in [`crate::chat_callbacks`].

pub mod atomic_write;
pub mod friend_ping;

use std::{
    collections::{HashMap, HashSet},
    fmt,
    str::FromStr,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use bytes::Bytes;
use iroh::{Endpoint, EndpointAddr, EndpointId, PublicKey, RelayMode, SecretKey};
use n0_error::{bail_any, Result, StdResultExt};
use n0_future::StreamExt;
use serde::{Deserialize, Serialize};
use serde_byte_array::ByteArray;

use crate::api::{Event, GossipReceiver};
use crate::chat_history::DeliveryState;
use crate::diagnostics::{DiagnosticEventKind, Diagnostics, ReceivedProbe};
use crate::discovery_secret::DiscoverySecret;
use crate::friends::{FriendId, FriendsStore};
use crate::proto::TopicId;
use crate::public_room_safety::PublicRoomSafety;
use crate::transfer_telemetry::TransferTelemetry;
use crate::user_profile::UserProfile;

/// Global diagnostics store for recording network events and probes.
///
/// Lazily initialised on first access with default capacities
/// (5 000 events, 1 000 received probes).
pub static DIAGNOSTICS: LazyLock<Diagnostics> = LazyLock::new(Diagnostics::new);

/// Global transfer lifecycle telemetry store.
///
/// Lazily initialised on first access.  Wraps the shared [`DIAGNOSTICS`]
/// store and adds per-transfer sequence tracking.
pub static TRANSFER_TELEMETRY: LazyLock<TransferTelemetry> =
    LazyLock::new(|| TransferTelemetry::new(DIAGNOSTICS.clone()));

/// Maximum clock-skew tolerance for future-dated messages (5 minutes).
const MAX_FUTURE_SKEW_SECS: u64 = 300;

/// Shared progress callback used by blob download functions with progress tracking.
pub(crate) type TransferProgressCallback =
    Arc<Mutex<Option<Box<dyn FnMut(TransferProgress) + Send>>>>;

// ── Bootstrap peer resolution ─────────────────────────────────────────────────

/// Collect unique bootstrap peer IDs from multiple address sources, preserving
/// the EndpointAddr information for seeding the endpoint address lookup.
///
/// Takes multiple slices of [`EndpointAddr`] values (e.g. from a ticket and
/// from a RoomStore), deduplicates them, and returns the peer IDs (for
/// `subscribe_and_join`) plus the full addresses (for seeding a MemoryLookup).
pub fn collect_bootstrap_peers(
    sources: impl IntoIterator<Item = impl AsRef<[EndpointAddr]>>,
) -> (Vec<EndpointId>, Vec<EndpointAddr>) {
    let mut seen_ids = HashSet::new();
    let mut peer_ids = Vec::new();
    let mut all_addrs = Vec::new();
    let mut seen_addrs = HashSet::new();

    for source in sources {
        for addr in source.as_ref() {
            if seen_ids.insert(addr.id) {
                peer_ids.push(addr.id);
            }
            if seen_addrs.insert(addr.id) {
                all_addrs.push(addr.clone());
            }
        }
    }

    (peer_ids, all_addrs)
}

/// Seed an [`iroh::address_lookup::memory::MemoryLookup`] with every
/// [`EndpointAddr`] from a deduplicated address list, so that
/// `endpoint.connect()` can resolve the peers by their addresses.
///
/// Call this **before** `subscribe_and_join()` so the address resolution
/// chain has the ticket/room-store peer addresses available.
pub fn seed_memory_lookup(
    memory_lookup: &iroh::address_lookup::memory::MemoryLookup,
    addrs: &[EndpointAddr],
) {
    for addr in addrs {
        memory_lookup.set_endpoint_info(addr.clone());
    }
}

/// Refresh the stored bootstrap peers in a [`RoomStore`] using the
/// endpoint's current remote info for a set of known peer IDs.
///
/// Call this **after** joining a room so that future reconnections
/// have up-to-date address information, even if the original ticket
/// creator is offline.
///
/// Returns `true` if the peers list changed.
pub async fn refresh_bootstrap_peers(
    room_store: &mut crate::room::RoomStore,
    peer_ids: &std::collections::HashSet<iroh::PublicKey>,
    endpoint: &iroh::Endpoint,
) -> bool {
    let mut refreshed: Vec<iroh::EndpointAddr> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for pk in peer_ids {
        if !seen.insert(*pk) {
            continue;
        }
        if let Some(info) = endpoint.remote_info(*pk).await {
            let addr =
                iroh::EndpointAddr::from_parts(info.id(), info.into_addrs().map(|a| a.into_addr()));
            refreshed.push(addr);
        }
    }

    if refreshed.is_empty() {
        return false;
    }

    let changed = room_store.peers != refreshed;
    if changed {
        room_store.peers = refreshed;
    }
    changed
}

/// Re-export the callback trait for convenience — existing import paths
/// (`iroh_gossip::chat_core::ChatCallbacks`) continue to work.
pub use crate::chat_callbacks::ChatCallbacks;
pub use crate::chat_callbacks::{TransferId, TransferKind, TransferProgress};

// ── Composer ─────────────────────────────────────────────────────────────────

/// A text buffer with cursor tracking, suitable for a message composer / input line.
#[derive(Clone, Debug, Default)]
pub struct Composer {
    text: String,
    cursor: usize,
}

impl From<&str> for Composer {
    fn from(text: &str) -> Self {
        Self {
            text: text.to_string(),
            cursor: text.len(),
        }
    }
}

impl Composer {
    /// The current text content.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Byte offset of the cursor.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Visual column (character count up to cursor) for rendering.
    pub fn cursor_column(&self) -> u16 {
        self.text[..self.cursor].chars().count() as u16
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Insert a character at the cursor position.
    pub fn insert_char(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    /// Insert a string at the cursor position.
    pub fn insert_str(&mut self, text: &str) {
        for ch in text.chars() {
            self.insert_char(ch);
        }
    }

    /// Move cursor one character left.
    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = prev_char_boundary(&self.text, self.cursor);
        }
    }

    /// Move cursor one character right.
    pub fn move_right(&mut self) {
        if self.cursor < self.text.len() {
            self.cursor = next_char_boundary(&self.text, self.cursor);
        }
    }

    /// Move cursor to the start.
    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to the end.
    pub fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let start = prev_char_boundary(&self.text, self.cursor);
            self.text.drain(start..self.cursor);
            self.cursor = start;
        }
    }

    /// Delete the character at the cursor.
    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            let end = next_char_boundary(&self.text, self.cursor);
            self.text.drain(self.cursor..end);
        }
    }

    /// Take the buffer contents and reset.
    pub fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        text
    }
}

fn prev_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .char_indices()
        .nth(1)
        .map(|(idx, _)| cursor + idx)
        .unwrap_or(text.len())
}

// ── Chat entry types ─────────────────────────────────────────────────────────

/// Whether a chat message originated locally, from a remote peer, or is a system notice.
#[derive(Clone, Debug)]
pub enum ChatKind {
    /// System notification (join/leave, errors, info).
    System,
    /// A message we sent ourselves.
    Local,
    /// A message from a remote peer.
    Remote,
}

/// A single entry in the chat log.
#[derive(Clone, Debug)]
pub struct ChatEntry {
    /// Kind of entry (system, local, remote).
    pub kind: ChatKind,
    /// Display label (e.g. nickname or "System").
    pub label: String,
    /// The message body text.
    pub body: String,
    /// Hash of the protocol message that produced this entry, when known.
    pub message_hash: Option<MessageHash>,
    /// Whether this entry has been edited after initial delivery.
    pub edited: bool,
    /// Emoji reactions attached to this entry.
    pub reactions: Vec<String>,
    /// Stable event id mapping to ChatHistoryStore entry (0 = unassigned).
    pub event_id: u64,
    /// Current delivery state of this message (only meaningful for Local kind).
    pub delivery_state: DeliveryState,
    /// Unix epoch milliseconds when this entry was created (UTC).
    /// None for entries created before this field was added.
    pub timestamp: Option<u64>,
}

impl ChatEntry {
    /// Create a system notification entry.
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            kind: ChatKind::System,
            label: "System".to_string(),
            body: text.into(),
            message_hash: None,
            edited: false,
            reactions: Vec::new(),
            event_id: 0,
            delivery_state: DeliveryState::default(),
            timestamp: Some(crate::chat_core::now_ms()),
        }
    }

    /// Create a local (self-sent) message entry.
    pub fn local(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind: ChatKind::Local,
            label: label.into(),
            body: text.into(),
            message_hash: None,
            edited: false,
            reactions: Vec::new(),
            event_id: 0,
            delivery_state: DeliveryState::default(),
            timestamp: Some(crate::chat_core::now_ms()),
        }
    }

    /// Create a remote (received) message entry.
    pub fn remote(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind: ChatKind::Remote,
            label: label.into(),
            body: text.into(),
            message_hash: None,
            edited: false,
            reactions: Vec::new(),
            event_id: 0,
            delivery_state: DeliveryState::default(),
            timestamp: Some(crate::chat_core::now_ms()),
        }
    }

    /// Attach a protocol message hash to this entry.
    pub fn with_message_hash(mut self, hash: MessageHash) -> Self {
        self.message_hash = Some(hash);
        self
    }

    /// Override the timestamp with a specific Unix epoch millisecond value.
    pub fn with_timestamp(mut self, timestamp_ms: Option<u64>) -> Self {
        self.timestamp = timestamp_ms;
        self
    }
}

// ── Status context ────────────────────────────────────────────────────────────

/// Overall mesh health summary shown in the status panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshHealth {
    /// The mesh looks healthy right now.
    Good,
    /// The mesh is connected but some peers have gone quiet.
    Degraded(String),
    /// The transport is offline.
    Offline(String),
}

/// Connection status information displayed in the status panel.
#[derive(Clone, Debug)]
pub struct StatusContext {
    /// Human-readable transport status message.
    pub transport_status: String,
    /// The gossip topic for this chat room.
    pub topic: TopicId,
    /// Relay configuration.
    pub relay_mode: RelayMode,
    /// Whether we are connected to peers.
    pub connected: bool,
    /// Number of known peers.
    pub peer_count: usize,
    /// Our display name / label.
    pub identity_label: String,
    /// A notice about the transport (shown in the status panel).
    pub transport_notice: String,
    /// Number of peers with a direct (hole-punched) connection.
    pub direct_peers: usize,
    /// Number of peers connected through a relay server.
    pub relayed_peers: usize,
    /// Set of peer PublicKeys that are currently gossip neighbors.
    pub neighbors: HashSet<PublicKey>,
    /// Cached per-peer connection type (direct vs relay).
    pub peer_connection_types: HashMap<PublicKey, ConnectionType>,
    /// Last time we saw any gossip activity from each peer.
    pub last_activity: HashMap<PublicKey, Instant>,
    /// Current mesh health summary for the UI.
    pub mesh_health: MeshHealth,
    /// Whether private-room DHT discovery is enabled.
    pub dht_enabled: bool,
    /// Number of peers discovered via DHT.
    pub dht_peer_count: usize,
}

impl StatusContext {
    /// Recompute the mesh health from the latest gossip activity and transport state.
    pub async fn recompute_mesh_health(&mut self, endpoint: &Endpoint) {
        let now = Instant::now();
        let stale_threshold = Duration::from_secs(120);
        let stale_peer = self.neighbors.iter().find_map(|peer| {
            self.last_activity.get(peer).and_then(|seen_at| {
                let age = now.saturating_duration_since(*seen_at);
                (age > stale_threshold).then_some((*peer, age))
            })
        });

        let online = tokio::time::timeout(Duration::from_millis(0), endpoint.online())
            .await
            .is_ok();

        let new_health = if !online {
            MeshHealth::Offline("iroh endpoint is offline".to_string())
        } else if let Some((peer, age)) = stale_peer {
            MeshHealth::Degraded(format!(
                "peer {} has been quiet for {}s",
                peer.fmt_short(),
                age.as_secs()
            ))
        } else {
            MeshHealth::Good
        };

        if new_health != self.mesh_health {
            match &new_health {
                MeshHealth::Good => {}
                MeshHealth::Degraded(reason) | MeshHealth::Offline(reason) => {
                    tracing::warn!("mesh health degraded: {reason}");
                }
            }
        }

        self.mesh_health = new_health;
    }

    /// Check the current mesh health against a previously observed state and
    /// return an optional user-facing notification message on transition.
    ///
    /// Returns `Some(notification)` when the mesh health has changed since
    /// `last_health` was recorded, or `None` on the first call or when the
    /// state has not changed.
    ///
    /// The caller should display the returned message to the user (e.g. as a
    /// system notification in the chat log) and persist the updated
    /// `last_health` for future calls.
    pub fn check_mesh_quiescence(&self, last_health: &mut Option<MeshHealth>) -> Option<String> {
        let current_health = &self.mesh_health;
        let notification = match (last_health.as_ref(), current_health) {
            // Good → Degraded: warn the user
            (Some(MeshHealth::Good), MeshHealth::Degraded(reason)) => {
                Some(format!("⚠ Mesh health degraded: {reason}"))
            }
            // Good → Offline: warn the user
            (Some(MeshHealth::Good), MeshHealth::Offline(reason)) => {
                Some(format!("⚠ Mesh offline: {reason}"))
            }
            // Degraded → Good: recovery
            (Some(MeshHealth::Degraded(_)), MeshHealth::Good) => {
                Some("✓ Mesh health recovered: all peers are active.".to_string())
            }
            // Offline → Good: recovery
            (Some(MeshHealth::Offline(_)), MeshHealth::Good) => {
                Some("✓ Mesh health recovered: endpoint is back online.".to_string())
            }
            // First check: don't notify
            (None, _) => None,
            // Same state or other transitions: no notification
            _ => None,
        };
        *last_health = Some(current_health.clone());
        notification
    }
}

/// Whether a peer's connection goes through a relay server or directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionType {
    /// Peer has at least one direct (IP-based) address.
    Direct,
    /// Peer is reachable only via a relay server.
    Relayed,
    /// Connection type is unknown (not a neighbor, or no info yet).
    Unknown,
}

// ── App state ─────────────────────────────────────────────────────────────────

/// The complete chat application state, independent of any rendering backend.
#[derive(Debug)]
pub struct AppState {
    /// Connection status context.
    pub status: StatusContext,
    /// All chat log entries.
    pub entries: Vec<ChatEntry>,
    /// The composer / input buffer.
    pub composer: Composer,
    /// Whether to auto-scroll to the latest message.
    pub follow_latest: bool,
    /// Current scroll offset (in lines).
    pub scroll_offset: u16,
    /// Last measured log height (updated by the renderer).
    pub last_log_height: u16,
    /// Whether the user has requested to quit.
    pub should_quit: bool,
    /// Whether the help overlay is visible.
    pub help_visible: bool,
    /// Pending file download info: (filename, ticket_string).
    pub pending_file: Option<(String, String)>,
    /// Pending image downloads queue: (filename, blob_hash, sender_pk).
    /// Vec so rapid ImageShare events are all queued (multi-image burst fix).
    pub pending_image: Vec<(String, MessageHash, PublicKey)>,
    /// Durable friends list store.
    pub friends: FriendsStore,
    /// Whether the friends store has unsaved changes.
    pub friends_dirty: bool,
    /// Display name cache: peer PublicKey → last announced display name.
    pub names: HashMap<PublicKey, String>,
    /// Our own public key — used to filter self-messages on echo.
    pub local_public: PublicKey,
    /// Map from content hash to stable event id for all self-sent messages.
    ///
    /// Populated when a local message is broadcast; used by
    /// [`event_id_for_hash`](ChatCallbacks::event_id_for_hash) to resolve
    /// delivery-state updates from network events.
    pub self_sent_events: HashMap<MessageHash, u64>,
}

impl AppState {
    /// Create a new chat state with the given status context, friends store,
    /// and an initial name entry for our own identity.
    pub fn new(
        status: StatusContext,
        friends: FriendsStore,
        local_public: PublicKey,
        local_label: Option<String>,
    ) -> Self {
        let mut names = HashMap::new();
        if let Some(label) = local_label {
            names.insert(local_public, label);
        }
        Self {
            status,
            entries: Vec::new(),
            composer: Composer::default(),
            follow_latest: true,
            scroll_offset: 0,
            last_log_height: 10,
            should_quit: false,
            help_visible: false,
            pending_file: None,
            pending_image: Vec::new(),
            friends,
            friends_dirty: false,
            names,
            local_public,
            self_sent_events: HashMap::new(),
        }
    }

    /// Append a system notification.
    pub fn push_system(&mut self, text: impl Into<String>) {
        self.push_entry(ChatEntry::system(text), true);
    }

    /// Append a local (self-sent) message.
    pub fn push_local(&mut self, label: impl Into<String>, text: impl Into<String>) {
        self.push_entry(ChatEntry::local(label, text), true);
    }

    /// Append a remote (received) message.
    pub fn push_remote(&mut self, label: impl Into<String>, text: impl Into<String>) {
        self.push_entry(ChatEntry::remote(label, text), true);
    }

    /// Append a remote (received) message and remember its protocol hash.
    pub fn push_remote_with_hash(
        &mut self,
        label: impl Into<String>,
        text: impl Into<String>,
        hash: MessageHash,
    ) {
        self.push_entry(ChatEntry::remote(label, text).with_message_hash(hash), true);
    }

    /// Push a raw [`ChatEntry`].
    pub fn push_entry(&mut self, entry: ChatEntry, follow_latest: bool) {
        self.entries.push(entry);
        if follow_latest {
            self.follow_latest = true;
        }
    }

    /// Maximum scroll offset given the visible height.
    pub fn max_scroll_offset(&self, visible_height: u16) -> u16 {
        let visible_height = visible_height as usize;
        self.entries.len().saturating_sub(visible_height) as u16
    }

    /// The rendered scroll offset, clamped and respecting follow-latest mode.
    pub fn rendered_scroll_offset(&self, visible_height: u16) -> u16 {
        let max = self.max_scroll_offset(visible_height);
        if self.follow_latest {
            max
        } else {
            self.scroll_offset.min(max)
        }
    }

    /// Scroll up by `amount` lines.
    pub fn scroll_up(&mut self, amount: u16, visible_height: u16) {
        let max = self.max_scroll_offset(visible_height);
        self.follow_latest = false;
        if self.scroll_offset == 0 {
            self.scroll_offset = max.saturating_sub(amount);
        } else {
            self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        }
    }

    /// Scroll down by `amount` lines.
    pub fn scroll_down(&mut self, amount: u16, visible_height: u16) {
        let max = self.max_scroll_offset(visible_height);
        self.scroll_offset = self.scroll_offset.saturating_add(amount).min(max);
        self.follow_latest = self.scroll_offset >= max;
    }
}

// ── ChatCallbacks impl for AppState ──────────────────────────────────────────

impl ChatCallbacks for AppState {
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

    fn store_peer_ticket(&mut self, peer: PublicKey, ticket: Ticket) -> bool {
        let fid = FriendId::from_public_key(peer);
        let record = self.friends.ensure_friend(fid);
        record.record_addrs(ticket.peers.clone());
        record.record_room(ticket.topic, ticket);
        true
    }

    fn record_activity(&mut self, peer: PublicKey) {
        self.status.last_activity.insert(peer, Instant::now());
    }

    fn push_system(&mut self, text: String) {
        self.push_entry(ChatEntry::system(text), true);
    }

    fn push_remote(
        &mut self,
        _peer: PublicKey,
        label: String,
        text: String,
        hash: Option<MessageHash>,
        sent_at_secs: Option<u64>,
    ) {
        let mut entry = ChatEntry::remote(label, text);
        // Override the default local-time timestamp with the protocol's
        // sent_at value (Unix epoch seconds, UTC) converted to milliseconds.
        if let Some(secs) = sent_at_secs {
            entry = entry.with_timestamp(Some(secs * 1000));
        }
        if let Some(h) = hash {
            entry = entry.with_message_hash(h);
        }
        self.push_entry(entry, true);
    }

    fn set_pending_file(&mut self, name: String, ticket: String) {
        self.pending_file = Some((name, ticket));
    }

    fn set_pending_image(&mut self, name: String, hash: MessageHash, from: PublicKey) {
        self.pending_image.push((name, hash, from));
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
            entry.body = new_text;
            entry.edited = true;
        }
    }

    fn delete_message(&mut self, hash: &MessageHash) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.message_hash == Some(*hash))
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
            .find(|entry| entry.message_hash == Some(*hash))
        {
            entry.reactions.push(emoji);
        }
    }

    fn on_neighbor_up(&mut self, peer: PublicKey) {
        self.record_activity(peer);
        self.status.neighbors.insert(peer);
    }

    fn on_neighbor_down(&mut self, peer: PublicKey) {
        self.record_activity(peer);
        self.status.neighbors.remove(&peer);
    }

    fn request_quit(&mut self) {
        self.should_quit = true;
    }

    fn event_id_for_hash(&self, hash: &MessageHash) -> Option<u64> {
        self.self_sent_events.get(hash).copied()
    }

    fn update_delivery_state(&mut self, event_id: u64, state: crate::chat_history::DeliveryState) {
        // Update the state in the AppState's self_sent_events tracking.
        // The actual history store update happens in the frontend event loop.
        tracing::debug!(?event_id, ?state, "AppState::update_delivery_state called");
        // This method exists so handle_net_event can be wired without
        // knowing about ChatHistoryStore. The frontend event loop
        // will read the updated state and apply it to the store.
        let _ = (event_id, state);
    }
}

// ── Network event types ──────────────────────────────────────────────────────

/// An event received from the gossip network (decoded from the wire).
#[derive(Debug, Clone)]
pub enum NetEvent {
    /// A decoded message from a peer.
    Message {
        /// Public key of the sender.
        from: PublicKey,
        /// The decoded message payload.
        message: Message,
        /// Unix epoch seconds when the message was sent.
        sent_at: u64,
    },
    /// A peer has joined the gossip mesh (new neighbor connection).
    NeighborUp {
        /// Public key of the peer that joined.
        peer: PublicKey,
    },
    /// A peer has left the gossip mesh (connection dropped or app closed).
    NeighborDown {
        /// Public key of the peer that left.
        peer: PublicKey,
    },
    /// The gossip receiver stream closed.
    Closed,
    /// A fatal network error occurred.
    Error(String),
}

// ── Protocol types ───────────────────────────────────────────────────────────

/// Messages that can be sent between peers in the chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    /// Announce or change your display name.
    AboutMe {
        /// The new display name.
        name: String,
        /// Optional BlobTicket for the profile image.
        #[serde(default)]
        profile_image_ticket: Option<String>,
    },
    /// A regular text message.
    Message {
        /// The message text.
        text: String,
    },
    /// Announce a file available for download.
    FileShare {
        /// The file name (basename only, no path).
        name: String,
        /// BlobTicket serialized to string.
        ticket: String,
    },
    /// Graceful goodbye — the sender is leaving the chat.
    /// This is a best-effort notification: the gossip protocol also
    /// detects disconnection via NeighborDown events.
    Leave,
    /// Periodic presence heartbeat.
    Presence,
    /// Presence heartbeat plus a ticket for opening a chat with this peer.
    ///
    /// This is additive to [`Message::Presence`] so older peers can still
    /// participate in the presence protocol without understanding tickets.
    PresenceWithTicket {
        /// Serialized chat-room ticket advertised by the sender.
        ticket: String,
    },
    /// Acknowledge that the sender read a message.
    ReadReceipt {
        /// Hash of the message being acknowledged.
        message_hash: MessageHash,
    },
    /// Replace the text of a previously sent message.
    Edit {
        /// Hash of the original message being edited.
        original_hash: MessageHash,
        /// Replacement message text.
        new_text: String,
    },
    /// Mark a previously sent message as deleted.
    Delete {
        /// Hash of the message being deleted.
        message_hash: MessageHash,
    },
    /// Add an emoji reaction to a previously sent message.
    Reaction {
        /// Hash of the message being reacted to.
        message_hash: MessageHash,
        /// Reaction emoji.
        emoji: String,
    },
    /// Announce an image available for download and inline display.
    ImageShare {
        /// The image file name (basename only, no path).
        name: String,
        /// Blob hash for the image content, for blob-store lookup and download.
        hash: MessageHash,
    },
    /// Invisible keepalive heartbeat — keeps connections warm and updates
    /// mesh health timestamps without producing any chat log entry or UI
    /// notification.
    ///
    /// Frontends broadcast this periodically (every 2–3 seconds) as a
    /// lightweight gossip message.  Peers receive it and update their
    /// `last_activity` timestamp for the sender, preventing the mesh
    /// health from decaying to "Degraded" or "Offline."
    ///
    /// This is intentionally separate from `Presence`, which is a
    /// *visible* status indicator.
    Heartbeat,
    /// A diagnostic probe sent through the normal gossip path — not displayed
    /// as an ordinary chat message by default.
    DiagnosticProbe(crate::diagnostics::DiagnosticProbe),
    /// Opaque contact-layer control message (friend requests, chat invites).
    ContactControl {
        /// Serialised SignedContactMessage payload.
        payload: Vec<u8>,
    },
    /// Publish the sender's profile and metadata over gossip.
    ProfileUpdate(UserProfile),
}

/// Metadata for a file advertised by a peer in [`Message::ProfileUpdate`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedFileMeta {
    /// Stable identifier assigned by the publisher.
    pub id: String,
    /// Basename shown to peers (never a local path).
    pub filename: String,
    /// File size in bytes.
    pub size: u64,
    /// MIME type, if known.
    pub mime_type: String,
    /// Unix timestamp in milliseconds of the source file's modification time.
    pub modified_time: u64,
    /// Content hash used to identify the file.
    pub hash: MessageHash,
}

/// Content hash used by richer interaction messages to refer to a chat message.
pub type Hash = [u8; 32];

/// Descriptive alias for message reference hashes.
pub type MessageHash = Hash;

/// Calculate the stable content hash for a protocol message.
pub fn message_hash(message: &Message) -> MessageHash {
    let bytes = postcard::to_stdvec(message).expect("postcard::to_stdvec is infallible");
    *blake3::hash(&bytes).as_bytes()
}

const SIGNATURE_LENGTH: usize = iroh::Signature::LENGTH;
type Signature = ByteArray<SIGNATURE_LENGTH>;

/// A signed message envelope with sender identity and signature.
#[derive(Debug, Serialize, Deserialize)]
pub struct SignedMessage {
    from: PublicKey,
    data: Bytes,
    signature: Signature,
    /// Unix epoch seconds when the message was sent.
    sent_at: u64,
}

impl SignedMessage {
    /// Verify a signed message and decode the inner [`Message`].
    pub fn verify_and_decode(bytes: &[u8]) -> Result<(PublicKey, Message, u64)> {
        let signed_message: Self =
            postcard::from_bytes(bytes).std_context("decode signed message")?;
        let key: PublicKey = signed_message.from;
        key.verify(
            &signed_message.data,
            &iroh::Signature::from_bytes(&signed_message.signature),
        )
        .std_context("verify signature")?;
        let message: Message =
            postcard::from_bytes(&signed_message.data).std_context("decode message")?;
        Ok((signed_message.from, message, signed_message.sent_at))
    }

    /// Sign a [`Message`] and encode it into a `Bytes` payload ready for gossip broadcast.
    pub fn sign_and_encode(secret_key: &SecretKey, message: &Message) -> Result<Bytes> {
        let data: Bytes = postcard::to_stdvec(&message)
            .std_context("encode message")?
            .into();
        let signature = secret_key.sign(&data);
        let key: PublicKey = secret_key.public();
        let sent_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let signed_message = Self {
            from: key,
            data,
            signature: ByteArray::new(signature.to_bytes()),
            sent_at,
        };
        let encoded = postcard::to_stdvec(&signed_message).std_context("encode signed message")?;
        Ok(encoded.into())
    }
}

/// A chat-room ticket that peers use to join a topic.
///
/// The optional [`DiscoverySecret`] enables DHT-based private-room discovery:
/// when present, the holder can publish and look up discovery records under
/// the room's encrypted namespace. Legacy tickets without a secret use their
/// endpoint-bearing bootstrap peers only; no replacement secret is generated.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    /// The gossip topic to join.
    pub topic: TopicId,
    /// Known peers to bootstrap from.
    pub peers: Vec<EndpointAddr>,
    /// Optional bearer capability for DHT-based private-room lookup.
    ///
    /// Holders can derive the room's private discovery namespace, so this
    /// value must be protected like the ticket itself. It is not message
    /// encryption and does not authenticate room membership. `#[serde(default)]`
    /// ensures legacy tickets (serialised without this field) deserialise to
    /// `None` instead of failing.
    #[serde(default)]
    pub discovery_secret: Option<DiscoverySecret>,
}

impl fmt::Debug for Ticket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ticket")
            .field("topic", &self.topic)
            .field("peers", &self.peers)
            .field(
                "discovery_secret",
                &self.discovery_secret.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

impl Ticket {
    /// Create a ticket from a topic, bootstrap peers, and an optional
    /// discovery secret.
    pub fn new(topic: TopicId, peers: Vec<EndpointAddr>) -> Self {
        Self {
            topic,
            peers,
            discovery_secret: None,
        }
    }

    /// Create a ticket with a discovery secret for DHT-based private-room
    /// discovery.
    pub fn with_discovery(
        topic: TopicId,
        peers: Vec<EndpointAddr>,
        secret: DiscoverySecret,
    ) -> Self {
        Self {
            topic,
            peers,
            discovery_secret: Some(secret),
        }
    }

    /// Decode a ticket from serialized bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        postcard::from_bytes(bytes).std_context("decode chat ticket")
    }

    /// Encode this ticket into bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("postcard::to_stdvec is infallible")
    }
}

impl fmt::Display for Ticket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = data_encoding::BASE32_NOPAD.encode(&self.to_bytes()[..]);
        text.make_ascii_lowercase();
        write!(f, "{text}")
    }
}

impl FromStr for Ticket {
    type Err = n0_error::AnyError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let bytes = data_encoding::BASE32_NOPAD
            .decode(s.to_ascii_uppercase().as_bytes())
            .std_context("decode chat ticket base32")?;
        Self::from_bytes(&bytes)
    }
}

/// The invitation formats accepted by the room join path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoomInvitation {
    /// Stable, endpoint-free invitation carrying the shared discovery secret.
    Stable(RoomInviteV2),
    /// Legacy postcard ticket. Bootstrap peers are preserved. A missing
    /// discovery secret explicitly disables DHT discovery for this room.
    Legacy(Ticket),
}

impl RoomInvitation {
    /// Detect and decode an invitation. A malformed `boru1:` string never
    /// falls through into the legacy decoder.
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        if trimmed.starts_with(RoomInviteV2::PREFIX) {
            return Ok(Self::Stable(RoomInviteV2::parse(trimmed)?));
        }
        Ok(Self::Legacy(trimmed.parse()?))
    }

    /// Return the room topic represented by this invitation.
    pub fn topic(&self) -> TopicId {
        match self {
            Self::Stable(invite) => invite.topic,
            Self::Legacy(ticket) => ticket.topic,
        }
    }

    /// Return the endpoint-bearing bootstrap peers, if any.
    pub fn bootstrap_peers(&self) -> &[EndpointAddr] {
        match self {
            Self::Stable(_) => &[],
            Self::Legacy(ticket) => &ticket.peers,
        }
    }

    /// Return the shared DHT discovery capability, if present.
    pub fn discovery_secret(&self) -> Option<&DiscoverySecret> {
        match self {
            Self::Stable(invite) => Some(&invite.discovery_secret),
            Self::Legacy(ticket) => ticket.discovery_secret.as_ref(),
        }
    }
}

// ── RoomInviteV2 — stable, versioned, compact invitation ────────────────────

/// A stable versioned room invitation with no endpoint/relay/creator identity.
///
/// Unlike the legacy [`Ticket`], this format carries only the room identity
/// (topic) and a bearer discovery secret — no transport address information.
/// It produces shorter, copy-paste-safe strings prefixed with `boru1:`.
///
/// # Format
///
/// `boru1:` + base32-nopad-lowercase of `[version: u8, topic: [u8; 32], discovery_secret: [u8; 32]]`
///
/// Total payload: 1 + 32 + 32 = 65 bytes.  Encoded string: ~105 chars + `boru1:` prefix.
///
/// # Safety
///
/// The discovery secret is redacted in the [`Debug`] implementation so it
/// never appears in logs or terminal output.
#[derive(Clone, PartialEq, Eq)]
pub struct RoomInviteV2 {
    /// The gossip topic to join (32 bytes).
    pub topic: TopicId,
    /// Bearer capability for DHT-based private-room discovery.
    /// Redacted in Debug output.
    pub discovery_secret: DiscoverySecret,
}

impl fmt::Debug for RoomInviteV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoomInviteV2")
            .field("topic", &self.topic)
            .field("discovery_secret", &"[redacted]")
            .finish()
    }
}

impl RoomInviteV2 {
    /// Current wire version for serialisation.
    /// Increment this when the payload layout changes (e.g. adding a field).
    const VERSION: u8 = 1;

    /// The human-readable prefix that identifies this invitation format.
    const PREFIX: &'static str = "boru1:";

    /// Create a new invitation from a topic and discovery secret.
    pub fn new(topic: TopicId, discovery_secret: DiscoverySecret) -> Self {
        Self {
            topic,
            discovery_secret,
        }
    }

    /// Serialise this invitation into a compact string with the `boru1:` prefix.
    ///
    /// Payload layout: `[version: u8, topic: [u8; 32], discovery_secret: [u8; 32]]`
    pub fn encode(&self) -> String {
        let mut payload = Vec::with_capacity(65);
        payload.push(Self::VERSION);
        payload.extend_from_slice(self.topic.as_ref());
        payload.extend_from_slice(self.discovery_secret.as_bytes());
        let encoded = data_encoding::BASE32_NOPAD.encode(&payload);
        let mut lower = encoded.to_ascii_lowercase();
        lower.insert_str(0, Self::PREFIX);
        lower
    }

    /// Parse an invitation string, accepting only the `boru1:` prefix.
    ///
    /// Returns an error for wrong prefix, version mismatch, or incorrect payload
    /// length.
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if !s.starts_with(Self::PREFIX) {
            bail_any!(
                "invalid invitation: expected prefix '{}', got '{}'",
                Self::PREFIX,
                &s[..s.len().min(10)]
            );
        }
        let encoded = &s[Self::PREFIX.len()..];
        if encoded.len() < 104 {
            // 65 bytes → 104 base32-nopad chars
            bail_any!(
                "invalid invitation: payload too short ({} chars, need ≥104)",
                encoded.len()
            );
        }
        let bytes = data_encoding::BASE32_NOPAD
            .decode(encoded.to_ascii_uppercase().as_bytes())
            .std_context("decode invitation base32")?;
        if bytes.len() != 65 {
            bail_any!(
                "invalid invitation: expected 65 payload bytes, got {}",
                bytes.len()
            );
        }
        let version = bytes[0];
        if version != Self::VERSION {
            bail_any!(
                "unsupported invitation version {} (expected {})",
                version,
                Self::VERSION
            );
        }
        let topic_bytes: [u8; 32] = bytes[1..33]
            .try_into()
            .map_err(|_| n0_error::anyerr!("invitation topic is not 32 bytes"))?;
        let secret_bytes: [u8; 32] = bytes[33..65]
            .try_into()
            .map_err(|_| n0_error::anyerr!("invitation secret is not 32 bytes"))?;
        Ok(Self {
            topic: TopicId::from_bytes(topic_bytes),
            discovery_secret: DiscoverySecret::from_bytes(secret_bytes),
        })
    }
}

// ── Formatting helpers ───────────────────────────────────────────────────────

/// Format a [`RelayMode`] into a human-readable string.
pub fn fmt_relay_mode(relay_mode: &RelayMode) -> String {
    match relay_mode {
        RelayMode::Disabled => "None".to_string(),
        RelayMode::Default => "Default Relay (production) servers".to_string(),
        RelayMode::Staging => "Default Relay (staging) servers".to_string(),
        RelayMode::Custom(map) => map
            .urls::<Vec<_>>()
            .into_iter()
            .map(|url| url.to_string())
            .collect::<Vec<_>>()
            .join(", "),
    }
}

// ── Bootstrap peer resolution ─────────────────────────────────────────────────

// ── Network event dispatch ───────────────────────────────────────────────────

/// Key used for message deduplication: (sender, content_hash, sent_at_seconds).
type DedupKey = (PublicKey, MessageHash, u64);

/// How long we remember a message for deduplication.
///
/// Must be at least as long as the maximum TTL to cover the gossip-storm and
/// backfill window.  Default message TTL is 1 hour; we use 2 hours to safely
/// cover reconnection + backfill scenarios.
const DEDUP_TTL: Duration = Duration::from_secs(7200);

/// Trigger a cleanup sweep when the seen set grows beyond this size.
const DEDUP_SWEEP_THRESHOLD: usize = 10_000;

/// Set of already-processed messages, keyed by (sender, content_hash, sent_at).
///
/// The value is the [`Instant`] when we first saw the message, used for TTL-based
/// eviction.  Entries older than [`DEDUP_TTL`] are periodically pruned.
static SEEN_MESSAGES: LazyLock<Mutex<HashMap<DedupKey, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Prune entries older than [`DEDUP_TTL`] from the seen-messages set.
fn prune_seen_messages() {
    let now = Instant::now();
    if let Ok(mut seen) = SEEN_MESSAGES.lock() {
        seen.retain(|_, first_seen| now.duration_since(*first_seen) < DEDUP_TTL);
    }
}

/// Key used for diagnostics event deduplication: (content_hash, sender_key).
/// Unlike [`SEEN_MESSAGES`] this does NOT include sent_at, so replayed
/// messages with different timestamps are still collapsed into one
/// diagnostic event.
type DiagDedupKey = (MessageHash, PublicKey);

/// Cooldown for diagnostic `MessageReceived` events: prevents the 5,000-entry
/// buffer from being saturated by repeated identical message hashes from the
/// same sender (e.g. stale messages replayed through the gossip mesh).  TTL
/// is 60 seconds — generous enough to catch bursts of gossip replays while
/// short enough that a genuine new message with the same hash from the same
/// sender (extremely unlikely) would eventually be recorded.
static DIAGNOSTIC_SEEN_MESSAGES: LazyLock<Mutex<HashMap<DiagDedupKey, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Apply public-room safety checks to a [`NetEvent`].
///
/// Returns `Some(event)` if the event passes all checks, or `None` if the
/// event should be silently dropped (message too large, rate limited, peer
/// exceeded blob announcement limit, etc.).
///
/// Non-message events (NeighborUp, NeighborDown, Closed, Error) always pass
/// through unchanged — they are control-plane signals, not untrusted content.
///
/// Pass `&PublicRoomSafety` for public rooms, or simply do not call this
/// function for private rooms to skip every check.
///
/// # Tracing
///
/// Each dropped event is logged at `debug` level with the reason, so operators
/// can monitor whether a peer is abusing the room without noise from legitimate
/// traffic.
pub fn filter_net_event_with_safety(
    event: NetEvent,
    safety: &PublicRoomSafety,
) -> Option<NetEvent> {
    match event {
        NetEvent::Message {
            from,
            message,
            sent_at,
        } => {
            // ── Message size check (text messages) ─────────────
            if let Message::Message { ref text } = message {
                if !safety.check_message_size(text.as_bytes()) {
                    tracing::debug!(
                        "safety: dropped oversized message ({} B) from {}",
                        text.len(),
                        from.fmt_short(),
                    );
                    return None;
                }
            }

            // ── Nickname length check ──────────────────────────
            if let Message::AboutMe { ref name, .. } = message {
                if name.len() > safety.config().nickname_length_limit {
                    tracing::debug!(
                        "safety: dropped long nickname ({} B) from {}",
                        name.len(),
                        from.fmt_short(),
                    );
                    return None;
                }
            }

            // ── Blob announcement check ────────────────────────
            let is_blob = matches!(
                message,
                Message::ImageShare { .. } | Message::FileShare { .. }
            );
            if is_blob && !safety.check_blob_announcement(&from) {
                tracing::debug!(
                    "safety: dropped blob announcement from {}",
                    from.fmt_short(),
                );
                return None;
            }

            // ── Per-peer rate limit ───────────────────────────
            if !safety.check_rate_limit(&from) {
                tracing::debug!("safety: rate-limited message from {}", from.fmt_short(),);
                return None;
            }

            Some(NetEvent::Message {
                from,
                message,
                sent_at,
            })
        }
        // ── Control-plane events always pass through ───────────
        other => Some(other),
    }
}

/// Process a decoded [`NetEvent`] against a [`ChatCallbacks`] implementor,
/// optionally applying public-room safety checks first.
///
/// When `safety` is `Some(...)`, the event is first run through
/// [`filter_net_event_with_safety`]; if it is rejected (rate-limited,
/// oversized, etc.) the function returns `Ok(())` without calling the
/// callback — the event is silently dropped.
///
/// When `safety` is `None` (private-room path), every event is forwarded
/// to `handle_net_event` unchanged.
///
/// `topic` is the optional room/topic context used when recording
/// diagnostic events (`PeerJoinedRoom`/`PeerLeftRoom`) so queries
/// scoped to a specific room can find them.
pub fn handle_net_event_with_safety(
    event: NetEvent,
    cb: &mut impl ChatCallbacks,
    safety: Option<&PublicRoomSafety>,
) -> Result<()> {
    handle_net_event_with_safety_for_topic(event, cb, safety, None)
}

/// Process an event with safety checks and explicit room/topic context.
pub fn handle_net_event_with_safety_for_topic(
    event: NetEvent,
    cb: &mut impl ChatCallbacks,
    safety: Option<&PublicRoomSafety>,
    topic: Option<TopicId>,
) -> Result<()> {
    let event = match safety {
        Some(s) => match filter_net_event_with_safety(event, s) {
            Some(ev) => ev,
            None => return Ok(()),
        },
        None => event,
    };
    handle_net_event_for_topic(event, cb, topic)
}

/// Process a decoded [`NetEvent`] against a [`ChatCallbacks`] implementor.
///
/// Handles common logic: friend tracking, name resolution, message
/// modification (edit/delete/reaction), typing indicators, and file
/// sharing. Frontend-specific side-effects (persistence, connection
/// counting, room previews) are delegated to the callbacks.
///
/// `topic` is the optional room/topic context used when recording
/// diagnostic events (`PeerJoinedRoom`/`PeerLeftRoom`) so queries
/// scoped to a specific room can find them.
pub fn handle_net_event(event: NetEvent, cb: &mut impl ChatCallbacks) -> Result<()> {
    handle_net_event_for_topic(event, cb, None)
}

/// Process a decoded event with explicit room/topic context.
pub fn handle_net_event_for_topic(
    event: NetEvent,
    cb: &mut impl ChatCallbacks,
    topic: Option<TopicId>,
) -> Result<()> {
    let event_label = match &event {
        NetEvent::Message { .. } => "Message",
        NetEvent::NeighborUp { .. } => "NeighborUp",
        NetEvent::NeighborDown { .. } => "NeighborDown",
        NetEvent::Closed => "Closed",
        NetEvent::Error(_) => "Error",
    };
    let _timer = crate::perf::PerfTracker::timer("handle_net_event", event_label);
    match event {
        NetEvent::Message {
            from,
            message,
            sent_at,
        } => {
            let incoming_hash = message_hash(&message);

            // ── Deduplication ──────────────────────────────────────────
            // Suppress duplicate deliveries from gossip fan-out, backfill,
            // and reconnection paths without dropping legitimate new messages.
            let dedup_key = (from, incoming_hash, sent_at);
            {
                let mut seen = SEEN_MESSAGES.lock().unwrap();
                if seen.insert(dedup_key, Instant::now()).is_none() {
                    // First time — continue processing below.
                } else {
                    // Dedup detected — record diagnostic event and suppress.
                    DIAGNOSTICS.record_with_peer(
                        None,
                        Some(from.to_string()),
                        DiagnosticEventKind::DuplicateMessage,
                    );
                    tracing::debug!(
                        "dedup: duplicate message from {} (hash={}, sent_at={})",
                        from.fmt_short(),
                        hex::encode(incoming_hash),
                        sent_at,
                    );
                    return Ok(());
                }
                // Periodic eviction of stale entries to bound memory growth.
                if seen.len() >= DEDUP_SWEEP_THRESHOLD {
                    drop(seen);
                    prune_seen_messages();
                }
            }

            cb.record_activity(from);

            // ── Blocked peer check ──────────────────────────────
            // Silently drop all messages from blocked peers.
            if cb.is_blocked(&from) {
                tracing::debug!("dropping message from blocked peer {}", from.fmt_short(),);
                return Ok(());
            }

            // ── Muted peer check ────────────────────────────────
            // Muted peers still have text messages shown, but
            // system notifications (name changes, file shares, image
            // shares) are suppressed.
            let is_muted = if from != cb.local_public() {
                cb.is_muted(&from)
            } else {
                false
            };

            if from != cb.local_public() {
                let now_secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let ttl_secs = cb.message_ttl().as_secs();

                // Check for past-expired messages.
                let age_secs = now_secs.saturating_sub(sent_at);
                if age_secs > ttl_secs {
                    tracing::debug!(
                        "dropping stale message from {} (age {}s > TTL {}s)",
                        from.fmt_short(),
                        age_secs,
                        ttl_secs,
                    );
                    return Ok(());
                }

                // Check for future-dated messages beyond clock-skew tolerance.
                let skew_secs = sent_at.saturating_sub(now_secs);
                if skew_secs > MAX_FUTURE_SKEW_SECS {
                    tracing::debug!(
                        "dropping future-dated message from {} (skew {}s > MAX_FUTURE_SKEW_SECS {}s)",
                        from.fmt_short(),
                        skew_secs,
                        MAX_FUTURE_SKEW_SECS,
                    );
                    return Ok(());
                }
            }
            // ── Diagnostics message-received dedup ────────────────────────
            // Suppress duplicate diagnostic events for the same
            // (message_hash, sender_id) — regardless of sent_at — to
            // prevent the diagnostics buffer from being saturated by
            // replayed stale messages bouncing through the gossip mesh.
            // TTL is 60 seconds (generous for catching bursts).
            let mut diag_had_hash = false;
            const DIAG_DEDUP_TTL_S: u64 = 60;
            {
                let mut diag_seen = DIAGNOSTIC_SEEN_MESSAGES.lock().unwrap();
                let diag_key = (incoming_hash, from);
                let now = Instant::now();
                if let Some(prev) = diag_seen.get_mut(&diag_key) {
                    let expired = now.duration_since(*prev).as_secs() > DIAG_DEDUP_TTL_S;
                    if !expired {
                        diag_had_hash = true;
                    } else {
                        *prev = now; // refresh for the new window
                    }
                } else {
                    diag_seen.insert(diag_key, now);
                }
                // Periodic eviction of stale entries to bound memory growth.
                if diag_seen.len() >= 256 {
                    diag_seen.retain(|_, seen_at| {
                        now.duration_since(*seen_at).as_secs() <= DIAG_DEDUP_TTL_S
                    });
                }
            }

            match message {
                Message::AboutMe {
                    name,
                    profile_image_ticket,
                } => {
                    let prior_announced_name = cb.last_announced_name(&from);
                    let old_name = cb.set_name(from, name.clone());
                    let old_name = prior_announced_name.or(old_name);
                    match profile_image_ticket {
                        Some(ticket) => cb.record_profile_image_ticket(from, ticket),
                        None => cb.clear_profile_image(from),
                    }
                    if from != cb.local_public() {
                        let fid = FriendId::from_public_key(from);
                        if cb.is_friend(&from) {
                            cb.friend_set_name(fid, name.clone());
                            cb.mark_friends_dirty();
                        }
                        if old_name.as_deref() != Some(&name) && !is_muted {
                            cb.push_system(format!(
                                "{} is now known as {}",
                                from.fmt_short(),
                                name
                            ));
                        }
                    }
                }
                Message::ProfileUpdate(profile) => {
                    if from != cb.local_public() {
                        cb.on_profile_update(from, profile);
                    }
                }
                Message::Message { text } => {
                    if from != cb.local_public() {
                        // Record diagnostic event for real chat messages from
                        // remote peers, subject to the per-key cooldown.
                        if !diag_had_hash {
                            DIAGNOSTICS.record_with_peer(
                                topic,
                                Some(from.to_string()),
                                DiagnosticEventKind::MessageReceived {
                                    message_id: Some(hex::encode(incoming_hash)),
                                    message_hash: Some(hex::encode(incoming_hash)),
                                    probe_id: None,
                                    sender_id: Some(from.to_string()),
                                },
                            );
                        }
                        let fid = FriendId::from_public_key(from);
                        if cb.is_friend(&from) {
                            cb.friend_mark_online(fid);
                            // NOT mark_friends_dirty — online status is
                            // determined by the dedicated friend ping manager
                            // (FriendPingManager), not by gossip activity.
                        }
                        let display_name = cb.resolve_name(&from);
                        cb.push_remote(
                            from,
                            display_name,
                            text,
                            Some(incoming_hash),
                            Some(sent_at),
                        );
                    }
                }
                Message::FileShare { name, ticket } => {
                    if from != cb.local_public() {
                        let fid = FriendId::from_public_key(from);
                        if cb.is_friend(&from) {
                            cb.friend_mark_online(fid);
                        }
                        if !is_muted {
                            let sender_name = cb.resolve_name(&from);
                            cb.push_system(format!("{} shared a file: {}", sender_name, name));
                            cb.set_pending_file(name, ticket);
                        }
                    }
                }
                Message::ImageShare { name, hash } => {
                    if from != cb.local_public() {
                        let fid = FriendId::from_public_key(from);
                        if cb.is_friend(&from) {
                            cb.friend_mark_online(fid);
                        }
                        if !is_muted {
                            let sender_name = cb.resolve_name(&from);
                            cb.push_system(format!("{} shared an image: {}", sender_name, name));
                            cb.set_pending_image(name, hash, from);
                        }
                    }
                }
                Message::Leave => {
                    // Handled via NetEvent::NeighborDown, which fires for
                    // both clean (Leave) and unclean (crash/disconnect)
                    // departures.
                }
                Message::Presence => {
                    cb.record_presence(from);
                }
                Message::PresenceWithTicket { ticket } => {
                    cb.record_presence(from);
                    cb.record_peer_ticket(from, ticket);
                }
                Message::Heartbeat => {
                    // Heartbeat is invisible — record activity to update
                    // mesh health timestamps, but never push to the chat log.
                    cb.record_activity(from);
                }
                Message::DiagnosticProbe(ref probe) => {
                    // Diagnostic probes travel through the normal gossip path
                    // but are NOT displayed as ordinary chat messages.  They
                    // are recorded in the diagnostics store with full metadata
                    // (latency, message hash, duplicate tracking).
                    let received_at_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64;

                    // Compute the message hash from the probe content
                    let hash = message_hash(&message);

                    // Compute latency; return None if clock skew produces
                    // a negative value.
                    let latency_ms = if received_at_ms >= probe.sent_at_ms {
                        Some(received_at_ms - probe.sent_at_ms)
                    } else {
                        None
                    };

                    let hash_hex = hex::encode(hash);

                    // Record the received probe in diagnostics storage
                    let received = ReceivedProbe {
                        probe_id: probe.probe_id.clone(),
                        room_id: probe.room_id.clone(),
                        sender_id: probe.sender_id.clone(),
                        sent_at_ms: probe.sent_at_ms,
                        received_at_ms,
                        latency_ms,
                        message_hash: hash_hex.clone(),
                        duplicate_count: 0,
                        timestamp: chrono::Utc::now(),
                        room_id_opt: None,
                    };
                    DIAGNOSTICS.record_received_probe_enhanced(received);

                    // Emit diagnostic events
                    DIAGNOSTICS.record(
                        None,
                        DiagnosticEventKind::ProbeReceived {
                            probe_id: probe.probe_id.clone(),
                            message_hash: hash_hex.clone(),
                            sender_id: probe.sender_id.clone(),
                        },
                    );
                }
                Message::ReadReceipt { message_hash } => {
                    if from != cb.local_public() && cb.has_message(&message_hash) {
                        let name = cb.resolve_name(&from);
                        cb.push_system(format!("{name} read a message"));
                    }
                }
                Message::Edit {
                    original_hash,
                    new_text,
                } => {
                    if from != cb.local_public() {
                        cb.edit_message(&original_hash, new_text);
                    }
                }
                Message::Delete { message_hash } => {
                    if from != cb.local_public() {
                        cb.delete_message(&message_hash);
                    }
                }
                Message::Reaction {
                    message_hash,
                    emoji,
                } => {
                    if from != cb.local_public() {
                        cb.add_reaction(&message_hash, emoji);
                    }
                }
                Message::ContactControl { .. } => {
                    // Handled at the frontend layer.
                }
            }
        }
        NetEvent::NeighborUp { peer } => {
            // NeighborUp is the first reliable application-level indication
            // that the gossip transport has discovered and admitted a peer
            // to this topic.  Address lookup/source details are deliberately
            // left unreported here (they are owned by iroh), but the
            // connection, subscription, and topic-membership stages are
            // observable and should be reflected in diagnostics.
            DIAGNOSTICS.record_with_peer(
                topic,
                Some(peer.to_string()),
                DiagnosticEventKind::PeerDiscovered,
            );
            DIAGNOSTICS.record_with_peer(
                topic,
                Some(peer.to_string()),
                DiagnosticEventKind::ConnectionEstablished {
                    remote_address: None,
                    transport: None,
                    used_relay: None,
                },
            );
            DIAGNOSTICS.record_with_peer(
                topic,
                Some(peer.to_string()),
                DiagnosticEventKind::RoomSubscriptionJoined,
            );
            DIAGNOSTICS.record_with_peer(
                topic,
                Some(peer.to_string()),
                DiagnosticEventKind::PeerAddedToTopic,
            );
            DIAGNOSTICS.record_with_peer(
                topic,
                Some(peer.to_string()),
                DiagnosticEventKind::PeerJoinedRoom,
            );
            cb.on_neighbor_status_change(peer, true);
        }
        NetEvent::NeighborDown { peer } => {
            DIAGNOSTICS.record_with_peer(
                topic,
                Some(peer.to_string()),
                DiagnosticEventKind::PeerLeftRoom,
            );
            cb.on_neighbor_status_change(peer, false);
        }
        NetEvent::Closed => {
            DIAGNOSTICS.record(
                None,
                DiagnosticEventKind::Error("gossip receiver closed".to_string()),
            );
            cb.push_system("The gossip receiver closed.".into());
            cb.request_quit();
        }
        NetEvent::Error(err) => {
            DIAGNOSTICS.record(None, DiagnosticEventKind::Error(err.to_string()));
            cb.push_system(format!("Network error: {err}"));
            cb.request_quit();
        }
    }
    Ok(())
}

/// Room-doc messages on the gossip topic use marker prefixes.
/// Metadata updates start with 0xFE, roster updates start with 0xFF.
/// These are handled by the room_docs layer and are not SignedMessages.
const METADATA_MARKER: u8 = 0xFE;
const ROSTER_MARKER: u8 = 0xFF;

/// Default maximum age of a received message before it is rejected as stale.
pub const DEFAULT_MESSAGE_TTL: Duration = Duration::from_secs(3600);

/// Return the current Unix epoch time in seconds.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Return the current Unix epoch time in milliseconds.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Create and sign a diagnostic probe message suitable for gossip broadcast.
///
/// Returns the signed, encoded bytes ready to send via `gossip.broadcast()`.
/// The probe reuses the same serialisation and signing path as ordinary
/// room messages — no separate protocol is needed.
///
/// When `payload` is `None`, the probe carries no diagnostic text.  The
/// `probe_id` is auto-generated via [`crate::diagnostics::generate_probe_id`]
/// unless one is supplied.
pub fn broadcast_diagnostic_probe(
    secret_key: &SecretKey,
    room_id: &str,
    payload: Option<String>,
    probe_id_override: Option<String>,
) -> Result<Bytes> {
    let probe = crate::diagnostics::DiagnosticProbe {
        probe_id: probe_id_override.unwrap_or_else(crate::diagnostics::generate_probe_id),
        sender_id: secret_key.public().to_string(),
        room_id: room_id.to_string(),
        sent_at_ms: now_ms() as i64,
        payload,
    };

    // Record the broadcast event in diagnostics
    let hash_hex = {
        let msg = Message::DiagnosticProbe(probe.clone());
        let hash = message_hash(&msg);
        hex::encode(hash)
    };
    DIAGNOSTICS.record(
        None,
        DiagnosticEventKind::ProbeBroadcast {
            probe_id: probe.probe_id.clone(),
            message_hash: hash_hex,
        },
    );

    let message = Message::DiagnosticProbe(probe);
    SignedMessage::sign_and_encode(secret_key, &message)
}

/// Forward raw gossip events into a [`NetEvent`] channel.
///
/// Spawn this as a background task to bridge the gossip receiver
/// into a `NetEvent` stream.  Private-room callers use this; public-room
/// callers should use [`forward_gossip_events_with_safety`] instead.
pub async fn forward_gossip_events(
    receiver: GossipReceiver,
    net_tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
) {
    forward_gossip_events_with_safety(receiver, net_tx, None).await
}

/// Forward raw gossip events into a [`NetEvent`] channel, applying public-room
/// safety checks when a [`PublicRoomSafety`] is provided.
///
/// When `safety` is `None`, every decoded event passes through unchanged
/// (private-room path).  When `Some(...)`, each event is run through
/// [`filter_net_event_with_safety`] and silently dropped if it violates
/// the room's size, rate, or announcement limits.
pub async fn forward_gossip_events_with_safety(
    mut receiver: GossipReceiver,
    net_tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
    safety: Option<Arc<PublicRoomSafety>>,
) {
    while let Ok(Some(event)) = receiver.try_next().await {
        match event {
            Event::Received(msg) => {
                // Skip room-doc messages (metadata 0xFE, roster 0xFF) —
                // they are not SignedMessages and would fail decode.
                if let Some(&marker) = msg.content.first() {
                    if marker == METADATA_MARKER || marker == ROSTER_MARKER {
                        continue;
                    }
                }
                let _decode_timer =
                    crate::perf::PerfTracker::timer("forward_gossip_decode", "verify_and_decode");
                match SignedMessage::verify_and_decode(&msg.content) {
                    Ok((from, message, sent_at)) => {
                        let net_event = NetEvent::Message {
                            from,
                            message,
                            sent_at,
                        };
                        // Apply safety filter for public rooms.
                        let net_event = match &safety {
                            Some(s) => match filter_net_event_with_safety(net_event, s) {
                                Some(ev) => ev,
                                None => continue,
                            },
                            None => net_event,
                        };
                        if net_tx.send(net_event).is_err() {
                            return;
                        }
                    }
                    Err(err) => {
                        // Log the error but keep running — a single bad
                        // message should not kill the network bridge task.
                        tracing::warn!("forward_gossip_events: decode error (dropped): {err}");
                        continue;
                    }
                }
            }
            Event::NeighborUp(id) => {
                if net_tx.send(NetEvent::NeighborUp { peer: id }).is_err() {
                    return;
                }
            }
            Event::NeighborDown(id) => {
                if net_tx.send(NetEvent::NeighborDown { peer: id }).is_err() {
                    return;
                }
            }
            Event::Lagged => {
                // Lagged warnings are protocol-level backpressure signals;
                // not forwarded to the frontend.
            }
        }
    }
    let _ = net_tx.send(NetEvent::Closed);
}

/// Update `StatusContext.direct_peers` and `.relayed_peers` by querying the
/// iroh [`Endpoint`] for each known neighbor.
///
/// For each peer in `status.neighbors` we ask the endpoint for remote info.
/// A peer with at least one direct (IP-based) transport address is counted
/// as `direct`; a peer reachable only via relay is counted as `relayed`.
///
/// Also populates `status.peer_connection_types` with per-peer granularity.
pub async fn update_connection_counts(endpoint: &Endpoint, status: &mut StatusContext) {
    let mut direct = 0usize;
    let mut relayed = 0usize;
    let peers: Vec<iroh::PublicKey> = status.neighbors.iter().copied().collect();
    for peer in &peers {
        let ctype = check_peer_connection_type(endpoint, *peer).await;
        match ctype {
            ConnectionType::Direct => direct += 1,
            ConnectionType::Relayed => relayed += 1,
            ConnectionType::Unknown => {}
        }
        if ctype != ConnectionType::Unknown {
            status.peer_connection_types.insert(*peer, ctype);
        }
    }
    status.direct_peers = direct;
    status.relayed_peers = relayed;
}

/// Build a list of blob download candidates: the original sender first, then
/// any online gossip neighbors (deduplicated).
///
/// Pass the result as the `providers` argument to
/// [`Downloader::download`][iroh_blobs::api::downloader::Downloader::download]
/// so the download can fall back to other peers that may have the blob
/// if the original sender is offline.
///
/// The original sender is always placed first so the primary peer is tried
/// before fallback candidates.
pub fn download_candidates(original: PublicKey, neighbors: &HashSet<PublicKey>) -> Vec<PublicKey> {
    let mut candidates: Vec<PublicKey> = Vec::with_capacity(neighbors.len() + 1);
    candidates.push(original);
    for n in neighbors {
        if *n != original {
            candidates.push(*n);
        }
    }
    candidates
}

/// Query the iroh [`Endpoint`] for a single peer and return its connection type.
///
/// Returns:
/// - [`ConnectionType::Direct`] if the peer has at least one direct (IP-based) address.
/// - [`ConnectionType::Relayed`] if the peer is reachable only via relay.
/// - [`ConnectionType::Unknown`] if the peer is not known to the endpoint.
pub async fn check_peer_connection_type(
    endpoint: &Endpoint,
    peer: iroh::PublicKey,
) -> ConnectionType {
    match endpoint.remote_info(peer).await {
        Some(info) => {
            let has_direct = info
                .addrs()
                .any(|a| matches!(a.addr(), iroh::TransportAddr::Ip(_)));
            if has_direct {
                ConnectionType::Direct
            } else {
                ConnectionType::Relayed
            }
        }
        None => ConnectionType::Unknown,
    }
}

// ── CancelGuard ──────────────────────────────────────────────────────────
//
// A RAII guard that emits TransferProgress::Cancelled via the shared callback
// wrapper when dropped without being disarmed first.
struct CancelGuard {
    callback: TransferProgressCallback,
    id: TransferId,
    kind: TransferKind,
    name: String,
    armed: bool,
}

impl CancelGuard {
    fn new(
        id: TransferId,
        kind: TransferKind,
        name: String,
        callback: TransferProgressCallback,
    ) -> Self {
        Self {
            callback,
            id,
            kind,
            name,
            armed: true,
        }
    }

    /// Disarm the guard so Drop does not emit Cancelled.
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Ok(mut guard) = self.callback.lock() {
                if let Some(cb) = guard.as_mut() {
                    cb(TransferProgress::Cancelled {
                        id: self.id,
                        kind: self.kind,
                        name: self.name.clone(),
                    });
                }
            }
        }
    }
}

/// Download a blob from the iroh network with observable progress events.
///
/// Wraps the iroh-blobs streaming download API to emit [`TransferProgress`]
/// events — `Started`, `Progress`, `Completed`, `Failed`, or `Cancelled` —
/// through the provided `on_progress` callback.  No events are emitted after
/// a terminal state (completed, failed, or cancelled).
///
/// `total` on `Progress` events is `None` because iroh-blobs does not expose
/// the total blob size before the download is complete.
///
/// If the future is dropped before the download finishes (e.g. the caller
/// cancels via a timeout, `select!`, or component unmount), a `Cancelled`
/// event is emitted automatically via the shared callback wrapper.
///
/// On success the blob bytes are returned.  The caller must ensure the blob
/// was actually stored (the stream only confirms the download completed).
///
/// When `max_bytes` is `Some(limit)` the download is aborted as soon as the
/// cumulative progress exceeds the limit — the stream is abandoned and the
/// partially-stored blob is never loaded into memory.  When `None` (the
/// private-room path) no size enforcement is applied.
#[expect(clippy::too_many_arguments)]
pub async fn download_blob_with_progress(
    blob_store: &iroh_blobs::api::Store,
    endpoint: &Endpoint,
    hash: iroh_blobs::Hash,
    candidates: Vec<PublicKey>,
    name: String,
    kind: TransferKind,
    on_progress: impl FnMut(TransferProgress) + Send + 'static,
    max_bytes: Option<u64>,
) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let id = TransferId::next();

    // Wrap the callback in a shared Mutex so the CancelGuard can reach it.
    let shared_cb: TransferProgressCallback = Arc::new(Mutex::new(Some(Box::new(on_progress))));

    // Helper: lock and call the callback.
    let emit = |ev: TransferProgress| {
        if let Ok(mut guard) = shared_cb.lock() {
            if let Some(cb) = guard.as_mut() {
                cb(ev);
            }
        }
    };

    emit(TransferProgress::Started {
        id,
        kind,
        name: name.clone(),
        total: None,
    });

    // Drop-guard: emit Cancelled if the future is dropped before
    // completion or failure.
    let cancel_guard = CancelGuard::new(id, kind, name.clone(), shared_cb.clone());

    let downloader = blob_store.downloader(endpoint);
    let progress = downloader.download(hash, candidates);

    // Stream the download progress items.
    let mut stream = progress.stream().await?;

    loop {
        use iroh_blobs::api::downloader::DownloadProgressItem;
        match stream.next().await {
            Some(DownloadProgressItem::Progress(n)) => {
                // Enforce streaming size cap before forwarding the event.
                if let Some(max) = max_bytes {
                    if n > max {
                        cancel_guard.disarm();
                        emit(TransferProgress::Failed {
                            id,
                            name,
                            error: format!("blob too large ({} bytes, limit {max} bytes)", n,),
                        });
                        return Err(n0_error::anyerr!(
                            "blob too large: {n} bytes, limit {max} bytes",
                        ));
                    }
                }
                emit(TransferProgress::Progress {
                    id,
                    kind,
                    name: name.clone(),
                    bytes: n,
                    total: None,
                });
            }
            Some(DownloadProgressItem::Error(e)) => {
                cancel_guard.disarm();
                emit(TransferProgress::Failed {
                    id,
                    name,
                    error: format!("{e}"),
                });
                return Err(e);
            }
            Some(DownloadProgressItem::DownloadError) => {
                cancel_guard.disarm();
                emit(TransferProgress::Failed {
                    id,
                    name,
                    error: "Download error".into(),
                });
                return Err(n0_error::anyerr!("Download error"));
            }
            Some(_) => {
                // Ignore TryProvider, ProviderFailed, PartComplete
            }
            None => {
                // Stream ended → download completed successfully.
                break;
            }
        }
    }

    cancel_guard.disarm();
    emit(TransferProgress::Completed {
        id,
        kind,
        name: name.clone(),
    });

    // Read back the blob.
    let mut reader = blob_store.blobs().reader(hash);
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await?;
    Ok(buf)
}

/// Download a blob with progress reporting, streaming directly to a file.
///
/// The blob is downloaded to the local store, then streamed from the store
/// to `save_path` in fixed-size chunks.  No whole-file buffer is allocated.
/// Progress events (`TransferProgress`) are emitted via `on_progress`.
#[allow(clippy::too_many_arguments)]
pub async fn download_blob_to_file(
    blob_store: &iroh_blobs::api::Store,
    endpoint: &Endpoint,
    hash: iroh_blobs::Hash,
    candidates: Vec<PublicKey>,
    name: String,
    kind: TransferKind,
    save_path: &std::path::Path,
    on_progress: impl FnMut(TransferProgress) + Send + 'static,
    max_bytes: Option<u64>,
) -> Result<()> {
    let id = TransferId::next();
    let shared_cb: TransferProgressCallback = Arc::new(Mutex::new(Some(Box::new(on_progress))));
    let emit = |ev: TransferProgress| {
        if let Ok(mut guard) = shared_cb.lock() {
            if let Some(cb) = guard.as_mut() {
                cb(ev);
            }
        }
    };
    emit(TransferProgress::Started {
        id,
        kind,
        name: name.clone(),
        total: None,
    });
    let cancel_guard = CancelGuard::new(id, kind, name.clone(), shared_cb.clone());

    // Phase 1: download to the local blob store
    let downloader = blob_store.downloader(endpoint);
    let progress = downloader.download(hash, candidates);
    let mut stream = progress.stream().await?;
    use iroh_blobs::api::downloader::DownloadProgressItem;
    loop {
        match stream.next().await {
            Some(DownloadProgressItem::Progress(n)) => {
                if let Some(max) = max_bytes {
                    if n > max {
                        emit(TransferProgress::Failed {
                            id,
                            name: name.clone(),
                            error: format!("blob too large ({} bytes, limit {} bytes)", n, max),
                        });
                        return Err(n0_error::anyerr!("blob too large"));
                    }
                }
                emit(TransferProgress::Progress {
                    id,
                    kind,
                    name: name.clone(),
                    bytes: n,
                    total: None,
                });
            }
            Some(DownloadProgressItem::Error(e)) => {
                cancel_guard.disarm();
                emit(TransferProgress::Failed {
                    id,
                    name,
                    error: format!("{e}"),
                });
                return Err(e);
            }
            Some(DownloadProgressItem::DownloadError) => {
                cancel_guard.disarm();
                emit(TransferProgress::Failed {
                    id,
                    name,
                    error: "Download error".into(),
                });
                return Err(n0_error::anyerr!("Download error"));
            }
            None => break,
            _ => {}
        }
    }
    cancel_guard.disarm();

    // Phase 2: stream from the local store to the output file
    use tokio::io::AsyncReadExt;
    let mut reader = blob_store.blobs().reader(hash);
    let mut file = tokio::fs::File::create(save_path).await?;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        tokio::io::AsyncWriteExt::write_all(&mut file, &buf[..n]).await?;
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;

    emit(TransferProgress::Completed { id, kind, name });
    Ok(())
}

/// Download a blob with public-room safety admission control and
/// blob-size enforcement.
///
/// When `safety` is `Some(...)`, this function first calls
/// [`PublicRoomSafety::try_acquire_download`] for the `original_sender`.
/// If the per-peer download queue is full, the function returns an error
/// without starting the download.  On completion the downloaded blob is
/// read back with a [`max_blob_size_bytes`] cap so objects exceeding the
/// configured limit are rejected without allocating beyond the cap.
/// On success or failure, [`PublicRoomSafety::release_download`] is called
/// to free the slot.
///
/// When `safety` is `None`, this is equivalent to
/// [`download_blob_with_progress`] (no size enforcement).
///
/// [`max_blob_size_bytes`]: PublicRoomConfig::max_blob_size_bytes
#[expect(clippy::too_many_arguments)]
pub async fn download_blob_with_safety(
    blob_store: &iroh_blobs::api::Store,
    endpoint: &Endpoint,
    hash: iroh_blobs::Hash,
    candidates: Vec<PublicKey>,
    name: String,
    kind: TransferKind,
    on_progress: impl FnMut(TransferProgress) + Send + 'static,
    safety: Option<&PublicRoomSafety>,
    original_sender: PublicKey,
) -> Result<Vec<u8>> {
    // ── Admission control for public rooms ───────────────────────
    if let Some(s) = safety {
        if !s.try_acquire_download(&original_sender) {
            tracing::debug!(
                "safety: download from {} rejected (queue full)",
                original_sender.fmt_short(),
            );
            return Err(n0_error::anyerr!(
                "download queue full for peer {}",
                original_sender.fmt_short()
            ));
        }
    }

    let max_size = safety.map(|s| s.config().max_blob_size_bytes as u64);
    let result = download_blob_with_progress(
        blob_store,
        endpoint,
        hash,
        candidates,
        name,
        kind,
        on_progress,
        max_size,
    )
    .await;

    // ── Release the download slot ───────────────────────────────
    if let Some(s) = safety {
        s.release_download(&original_sender);
    }

    // ── Blob size enforcement ──────────────────────────────
    if let Some(s) = safety {
        if let Ok(ref bytes) = result {
            if !s.check_blob_size(bytes.len()) {
                let max = s.config().max_blob_size_bytes;
                tracing::debug!(
                    "safety: blob from {} exceeds size limit ({} > {})",
                    original_sender.fmt_short(),
                    bytes.len(),
                    max,
                );
                return Err(n0_error::anyerr!(
                    "blob from {} exceeds size limit ({} > {} bytes)",
                    original_sender.fmt_short(),
                    bytes.len(),
                    max,
                ));
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::TopicId;

    // ── Composer tests ───────────────────────────────────────────────────

    #[test]
    fn composer_default_is_empty() {
        let c = Composer::default();
        assert!(c.is_empty());
        assert_eq!(c.text(), "");
        assert_eq!(c.cursor(), 0);
        assert_eq!(c.cursor_column(), 0);
    }

    #[test]
    fn composer_from_str_sets_text_and_cursor_at_end() {
        let c = Composer::from("hello");
        assert_eq!(c.text(), "hello");
        assert_eq!(c.cursor(), 5);
        assert!(!c.is_empty());
    }

    #[test]
    fn composer_insert_char_at_cursor() {
        let mut c = Composer::from("ab");
        c.move_home();
        c.insert_char('X');
        assert_eq!(c.text(), "Xab");
        assert_eq!(c.cursor(), 1);
    }

    #[test]
    fn composer_insert_str_at_cursor() {
        let mut c = Composer::from("ab");
        c.insert_str("XY");
        assert_eq!(c.text(), "abXY");
        assert_eq!(c.cursor(), 4);
    }

    #[test]
    fn composer_insert_str_mid_buffer() {
        let mut c = Composer::from("ab");
        c.move_home();
        c.insert_str("12");
        assert_eq!(c.text(), "12ab");
        assert_eq!(c.cursor(), 2);
    }

    #[test]
    fn composer_move_left_and_right() {
        let mut c = Composer::from("abc");
        c.move_left();
        assert_eq!(c.cursor(), 2);
        c.move_left();
        assert_eq!(c.cursor(), 1);
        c.move_left();
        assert_eq!(c.cursor(), 0);
        c.move_left(); // no-op at start
        assert_eq!(c.cursor(), 0);
        c.move_right();
        assert_eq!(c.cursor(), 1);
        c.move_right();
        assert_eq!(c.cursor(), 2);
        c.move_right();
        assert_eq!(c.cursor(), 3);
        c.move_right(); // no-op at end
        assert_eq!(c.cursor(), 3);
    }

    #[test]
    fn composer_move_home_and_end() {
        let mut c = Composer::from("hello world");
        c.move_home();
        assert_eq!(c.cursor(), 0);
        c.move_end();
        assert_eq!(c.cursor(), 11);
    }

    #[test]
    fn composer_backspace_removes_before_cursor() {
        let mut c = Composer::from("abcd");
        c.move_left();
        c.backspace();
        assert_eq!(c.text(), "abd");
        assert_eq!(c.cursor(), 2);
    }

    #[test]
    fn composer_backspace_at_start_does_nothing() {
        let mut c = Composer::from("test");
        c.move_home();
        c.backspace();
        assert_eq!(c.text(), "test");
        assert_eq!(c.cursor(), 0);
    }

    #[test]
    fn composer_delete_removes_after_cursor() {
        // "abcd" cursor at end → move_left → cursor before 'd'
        // delete removes 'd' → "abc", cursor at end (3)
        let mut c = Composer::from("abcd");
        c.move_left();
        c.delete();
        assert_eq!(c.text(), "abc");
        assert_eq!(c.cursor(), 3);
    }

    #[test]
    fn composer_delete_at_end_does_nothing() {
        let mut c = Composer::from("abc");
        c.delete();
        assert_eq!(c.text(), "abc");
        assert_eq!(c.cursor(), 3);
    }

    #[test]
    fn composer_take_clears_buffer() {
        let mut c = Composer::from("hello");
        let taken = c.take();
        assert_eq!(taken, "hello");
        assert!(c.is_empty());
        assert_eq!(c.cursor(), 0);
    }

    #[test]
    fn composer_cursor_column_is_unicode_aware() {
        let mut c = Composer::default();
        c.insert_char('é'); // 2 bytes, 1 column
        c.insert_char('☃'); // 3 bytes, 1 column
        assert_eq!(c.cursor_column(), 2);
        c.move_home();
        assert_eq!(c.cursor_column(), 0);
        c.move_right();
        assert_eq!(c.cursor_column(), 1);
        c.move_right();
        assert_eq!(c.cursor_column(), 2);
    }

    #[test]
    fn composer_insert_unicode_at_cursor() {
        let mut c = Composer::from("a");
        c.move_home();
        c.insert_char('é');
        assert_eq!(c.text(), "éa");
        assert_eq!(c.cursor(), 2);
    }

    // ── ChatEntry tests ──────────────────────────────────────────────────

    #[test]
    fn chat_entry_system_uses_system_label() {
        let e = ChatEntry::system("hello");
        assert!(matches!(e.kind, ChatKind::System));
        assert_eq!(e.label, "System");
        assert_eq!(e.body, "hello");
    }

    #[test]
    fn chat_entry_local_uses_given_label() {
        let e = ChatEntry::local("alice", "hey");
        assert!(matches!(e.kind, ChatKind::Local));
        assert_eq!(e.label, "alice");
        assert_eq!(e.body, "hey");
    }

    #[test]
    fn chat_entry_remote_uses_given_label() {
        let e = ChatEntry::remote("bob", "hi");
        assert!(matches!(e.kind, ChatKind::Remote));
        assert_eq!(e.label, "bob");
        assert_eq!(e.body, "hi");
    }

    // ── StatusContext tests ──────────────────────────────────────────────

    fn test_status() -> StatusContext {
        StatusContext {
            transport_status: "ready".into(),
            topic: TopicId::from_bytes([0u8; 32]),
            relay_mode: RelayMode::Default,
            connected: true,
            peer_count: 0,
            identity_label: "tester".into(),
            transport_notice: "notice".into(),
            direct_peers: 0,
            relayed_peers: 0,
            neighbors: HashSet::new(),
            peer_connection_types: HashMap::new(),
            last_activity: HashMap::new(),
            mesh_health: MeshHealth::Good,
            dht_enabled: false,
            dht_peer_count: 0,
        }
    }

    fn test_app() -> AppState {
        AppState::new(
            test_status(),
            FriendsStore::default(),
            SecretKey::generate().public(),
            Some("tester".into()),
        )
    }

    #[test]
    fn status_context_fields_are_accessible() {
        let s = test_status();
        assert_eq!(s.transport_status, "ready");
        assert_eq!(s.identity_label, "tester");
        assert!(s.connected);
    }

    // ── AppState tests ───────────────────────────────────────────────────

    #[test]
    fn app_state_new_creates_empty_state() {
        let app = test_app();
        assert!(app.entries.is_empty());
        assert!(app.composer.is_empty());
        assert!(app.follow_latest);
        assert!(!app.should_quit);
    }

    #[test]
    fn app_state_push_system_adds_entry_and_sets_follow() {
        let mut app = test_app();
        app.follow_latest = false;
        app.push_system("system msg");
        assert_eq!(app.entries.len(), 1);
        assert!(matches!(app.entries[0].kind, ChatKind::System));
        assert_eq!(app.entries[0].body, "system msg");
        assert!(app.follow_latest);
    }

    #[test]
    fn app_state_push_local_adds_local_entry() {
        let mut app = test_app();
        app.push_local("alice", "hello");
        assert!(matches!(app.entries[0].kind, ChatKind::Local));
        assert_eq!(app.entries[0].label, "alice");
        assert_eq!(app.entries[0].body, "hello");
    }

    #[test]
    fn app_state_push_remote_adds_remote_entry() {
        let mut app = test_app();
        app.push_remote("bob", "hi");
        assert!(matches!(app.entries[0].kind, ChatKind::Remote));
        assert_eq!(app.entries[0].label, "bob");
        assert_eq!(app.entries[0].body, "hi");
    }

    #[test]
    fn app_state_entries_maintain_insertion_order() {
        let mut app = test_app();
        app.push_system("sys");
        app.push_local("A", "local");
        app.push_remote("B", "remote");
        assert_eq!(app.entries.len(), 3);
        assert!(matches!(app.entries[0].kind, ChatKind::System));
        assert!(matches!(app.entries[1].kind, ChatKind::Local));
        assert!(matches!(app.entries[2].kind, ChatKind::Remote));
    }

    #[test]
    fn default_record_peer_ticket_ignores_invalid_ticket() {
        let peer = SecretKey::generate().public();
        let mut app = test_app();

        ChatCallbacks::record_peer_ticket(&mut app, peer, "not-a-ticket".into());

        assert!(app.friends.is_empty());
        assert!(!app.friends_dirty);
    }

    #[test]
    fn default_record_peer_ticket_ignores_self_ticket() {
        let mut app = test_app();
        let local_public = app.local_public;
        let ticket = Ticket {
            topic: TopicId::from_bytes([9; 32]),
            peers: vec![EndpointAddr::new(local_public)],
            discovery_secret: None,
        };

        ChatCallbacks::record_peer_ticket(&mut app, local_public, ticket.to_string());

        assert!(app.friends.is_empty());
        assert!(!app.friends_dirty);
    }

    #[test]
    fn default_record_peer_ticket_persists_valid_ticket() {
        let peer = SecretKey::generate().public();
        let mut app = test_app();
        let ticket = Ticket {
            topic: TopicId::from_bytes([8; 32]),
            peers: vec![EndpointAddr::new(peer)],
            discovery_secret: None,
        };

        ChatCallbacks::record_peer_ticket(&mut app, peer, ticket.to_string());

        let record = app
            .friends
            .get(&FriendId::from_public_key(peer))
            .expect("peer ticket creates friend record");
        assert_eq!(record.known_addrs, ticket.peers);
        assert_eq!(record.rooms.get(&ticket.topic), Some(&ticket));
        assert!(app.friends_dirty);
    }

    #[test]
    fn app_state_max_scroll_offset_zero_when_fewer_entries_than_height() {
        let mut app = test_app();
        assert_eq!(app.max_scroll_offset(10), 0);
        for i in 0..5 {
            app.push_system(format!("m{i}"));
        }
        assert_eq!(app.max_scroll_offset(10), 0);
    }

    #[test]
    fn app_state_max_scroll_offset_non_zero_when_more_entries_than_height() {
        let mut app = test_app();
        for i in 0..15 {
            app.push_system(format!("m{i}"));
        }
        assert_eq!(app.max_scroll_offset(10), 5);
    }

    #[test]
    fn app_state_rendered_scroll_following_returns_max() {
        let mut app = test_app();
        for i in 0..20 {
            app.push_system(format!("m{i}"));
        }
        app.follow_latest = true;
        assert_eq!(app.rendered_scroll_offset(10), 10);
    }

    #[test]
    fn app_state_rendered_scroll_not_following_uses_scroll_offset() {
        let mut app = test_app();
        for i in 0..20 {
            app.push_system(format!("m{i}"));
        }
        app.follow_latest = false;
        app.scroll_offset = 3;
        assert_eq!(app.rendered_scroll_offset(10), 3);
        // Clamped to max (10) when scroll_offset exceeds
        app.scroll_offset = 100;
        assert_eq!(app.rendered_scroll_offset(10), 10);
    }

    #[test]
    fn app_state_scroll_up_from_top_wraps() {
        let mut app = test_app();
        for i in 0..10 {
            app.push_system(format!("m{i}"));
        }
        app.scroll_up(3, 5);
        assert!(!app.follow_latest);
        // max = 10 - 5 = 5, scroll_offset was 0 => wraps to 5 - 3 = 2
        assert_eq!(app.scroll_offset, 2);
    }

    #[test]
    fn app_state_scroll_up_from_mid() {
        let mut app = test_app();
        for i in 0..10 {
            app.push_system(format!("m{i}"));
        }
        app.scroll_offset = 5;
        app.scroll_up(2, 5);
        assert_eq!(app.scroll_offset, 3);
    }

    #[test]
    fn app_state_scroll_down_re_enables_follow_at_bottom() {
        let mut app = test_app();
        for i in 0..10 {
            app.push_system(format!("m{i}"));
        }
        app.follow_latest = false;
        app.scroll_offset = 0;
        app.scroll_down(10, 5); // max=5, so should land at 5
        assert_eq!(app.scroll_offset, 5);
        assert!(app.follow_latest);
    }

    #[test]
    fn app_state_scroll_down_does_not_follow_when_not_at_bottom() {
        let mut app = test_app();
        for i in 0..10 {
            app.push_system(format!("m{i}"));
        }
        app.follow_latest = false;
        app.scroll_offset = 0;
        app.scroll_down(2, 5);
        assert_eq!(app.scroll_offset, 2);
        assert!(!app.follow_latest);
    }

    #[test]
    fn app_state_push_entry_without_follow_does_not_change_flag() {
        let mut app = test_app();
        app.follow_latest = false;
        app.push_entry(ChatEntry::system("test"), false);
        assert!(
            !app.follow_latest,
            "push_entry with false should not change flag"
        );
    }

    #[test]
    fn app_state_push_entry_with_follow_sets_flag() {
        let mut app = test_app();
        app.follow_latest = false;
        app.push_entry(ChatEntry::system("test"), true);
        assert!(app.follow_latest);
    }

    // ── Message serialization tests ──────────────────────────────────────

    #[test]
    fn message_serialization_roundtrip_about_me() {
        let msg = Message::AboutMe {
            name: "alice".into(),
            profile_image_ticket: None,
        };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        assert!(
            matches!(decoded, Message::AboutMe { ref name, profile_image_ticket: _ } if name == "alice")
        );
    }

    #[test]
    fn message_serialization_roundtrip_profile_update() {
        use crate::user_profile::UserProfile;
        let mut profile = UserProfile::new(
            PublicKey::from_bytes(&[1u8; 32]).expect("32 one-bytes is a valid ed25519 public key"),
        );
        profile.display_name = "alice".into();
        profile.bio = "hello".into();
        profile.file_sharing_enabled = true;
        profile.max_file_size = 1024 * 1024;
        profile.allowed_extensions = vec!["jpg".into(), "txt".into()];
        profile.avatar_identifier = Some("avatar-id".into());
        profile.shared_folder_path = std::path::PathBuf::from("/tmp/shared");
        profile.allow_downloads = true;
        let msg = Message::ProfileUpdate(profile);
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        match decoded {
            Message::ProfileUpdate(profile) => {
                assert_eq!(profile.display_name, "alice");
                assert_eq!(profile.bio, "hello");
                assert!(profile.file_sharing_enabled);
                assert!(profile.is_download_allowed());
            }
            _ => panic!("expected ProfileUpdate"),
        }
    }

    #[test]
    fn message_serialization_roundtrip_text() {
        let msg = Message::Message {
            text: "hello world".into(),
        };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        assert!(matches!(decoded, Message::Message { ref text } if text == "hello world"));
    }

    #[test]
    fn message_serialization_roundtrip_file_share() {
        let msg = Message::FileShare {
            name: "photo.png".into(),
            ticket: "ticket123".into(),
        };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        match decoded {
            Message::FileShare { name, ticket } => {
                assert_eq!(name, "photo.png");
                assert_eq!(ticket, "ticket123");
            }
            _ => panic!("expected FileShare"),
        }
    }

    #[test]
    fn message_serialization_roundtrip_image_share() {
        let msg = Message::ImageShare {
            name: "cat.jpg".into(),
            hash: [0xab; 32],
        };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        match decoded {
            Message::ImageShare { name, hash } => {
                assert_eq!(name, "cat.jpg");
                assert_eq!(hash, [0xab; 32]);
            }
            _ => panic!("expected ImageShare"),
        }
    }

    #[test]
    fn signed_message_sign_and_verify_roundtrip() {
        let key = SecretKey::generate();
        let msg = Message::Message {
            text: "secure chat".into(),
        };
        let encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap();
        let (pk, decoded, sent_at) = SignedMessage::verify_and_decode(&encoded).unwrap();
        assert_eq!(pk, key.public());
        assert!(sent_at > 0);
        assert!(matches!(decoded, Message::Message { ref text } if text == "secure chat"));
    }

    #[test]
    fn signed_message_rejects_tampered_data() {
        let key = SecretKey::generate();
        let msg = Message::Message {
            text: "original".into(),
        };
        let mut encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap().to_vec();
        if let Some(b) = encoded.last_mut() {
            *b ^= 0xff;
        }
        let result = SignedMessage::verify_and_decode(&encoded);
        assert!(result.is_err(), "tampered message should fail verification");
    }

    #[test]
    fn signed_message_wrong_key_fails_verification() {
        let key_a = SecretKey::generate();
        let _key_b = SecretKey::generate();
        let msg = Message::Message {
            text: "secret".into(),
        };
        let encoded = SignedMessage::sign_and_encode(&key_a, &msg).unwrap();
        // Verification should still succeed because the signed message
        // contains the claimed public key — the signature matches key_a
        // and the protocol trusts the claimed key.  This test verifies
        // that a message signed by one key cannot be claimed as having
        // come from a different key after verification.
        let (_pk, _, _sent_at) = SignedMessage::verify_and_decode(&encoded).unwrap();
    }

    // ── Ticket serialization tests ───────────────────────────────────────

    #[test]
    fn ticket_roundtrip_through_base32() {
        let ticket = Ticket {
            topic: TopicId::from_bytes([9u8; 32]),
            peers: vec![EndpointAddr::new(SecretKey::generate().public())],
            discovery_secret: None,
        };
        let encoded = ticket.to_string();
        let decoded = Ticket::from_str(&encoded).unwrap();
        assert_eq!(decoded, ticket);
    }

    #[test]
    fn ticket_is_deterministic() {
        let key = SecretKey::generate();
        let topic = TopicId::from_bytes([42u8; 32]);
        let peer = EndpointAddr::new(key.public());
        let a = Ticket {
            topic,
            peers: vec![peer.clone()],
            discovery_secret: None,
        };
        let b = Ticket {
            topic,
            peers: vec![peer],
            discovery_secret: None,
        };
        assert_eq!(a.to_string(), b.to_string());
        assert_eq!(a.to_bytes(), b.to_bytes());
    }

    #[test]
    fn ticket_to_bytes_and_from_bytes_roundtrip() {
        let ticket = Ticket {
            topic: TopicId::from_bytes([1u8; 32]),
            peers: vec![],
            discovery_secret: None,
        };
        let bytes = ticket.to_bytes();
        let decoded = Ticket::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, ticket);
    }

    // ── fmt_relay_mode tests ─────────────────────────────────────────────

    #[test]
    fn fmt_relay_mode_disabled() {
        assert_eq!(fmt_relay_mode(&RelayMode::Disabled), "None");
    }

    #[test]
    fn fmt_relay_mode_default() {
        let rendered = fmt_relay_mode(&RelayMode::Default);
        assert!(rendered.contains("Default Relay"));
    }

    #[test]
    fn fmt_relay_mode_staging() {
        let rendered = fmt_relay_mode(&RelayMode::Staging);
        assert!(rendered.contains("staging"));
    }

    // ── handle_net_event tests ──────────────────────────────────────────

    #[test]
    fn handle_net_event_message_appends_remote_entry() {
        let key = SecretKey::generate();
        let mut app = test_app();

        let event = NetEvent::Message {
            from: key.public(),
            message: Message::Message { text: "hi".into() },
            sent_at: now_secs(),
        };

        handle_net_event(event, &mut app).unwrap();
        assert_eq!(app.entries.len(), 1);
        assert!(matches!(app.entries[0].kind, ChatKind::Remote));
        assert_eq!(app.entries[0].body, "hi");
    }

    #[test]
    fn handle_net_event_about_me_stores_name_and_notifies() {
        let remote_key = SecretKey::generate();
        let _local_key = SecretKey::generate();
        let mut app = test_app();

        let event = NetEvent::Message {
            from: remote_key.public(),
            message: Message::AboutMe {
                name: "bob".into(),
                profile_image_ticket: None,
            },
            sent_at: now_secs(),
        };

        handle_net_event(event, &mut app).unwrap();
        // Name should be stored
        assert_eq!(app.names.get(&remote_key.public()).unwrap(), "bob");
        // Should have a system notification about the name
        assert!(app.entries.iter().any(|e| e.body.contains("bob")));
    }

    #[test]
    fn handle_net_event_about_me_same_name_suppresses_duplicate_system_message() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        let msg = Message::AboutMe {
            name: "bob".into(),
            profile_image_ticket: None,
        };

        handle_net_event(
            NetEvent::Message {
                from: remote_key.public(),
                message: msg.clone(),
                sent_at: now_secs(),
            },
            &mut app,
        )
        .unwrap();
        handle_net_event(
            NetEvent::Message {
                from: remote_key.public(),
                message: msg,
                sent_at: now_secs(),
            },
            &mut app,
        )
        .unwrap();

        let matching = app
            .entries
            .iter()
            .filter(|entry| {
                entry.body == format!("{} is now known as bob", remote_key.public().fmt_short())
            })
            .count();
        assert_eq!(
            matching, 1,
            "same AboutMe should only emit one name-change notice"
        );
        assert_eq!(app.names.get(&remote_key.public()).unwrap(), "bob");
    }

    #[test]
    fn handle_net_event_about_me_reconnect_uses_persisted_friend_name_to_skip_duplicate_notice() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.set_last_announced_name(fid.clone(), "bob");
        app.names.clear();

        handle_net_event(
            NetEvent::Message {
                from: remote_key.public(),
                message: Message::AboutMe {
                    name: "bob".into(),
                    profile_image_ticket: None,
                },
                sent_at: now_secs(),
            },
            &mut app,
        )
        .unwrap();

        assert_eq!(
            app.names.get(&remote_key.public()).map(String::as_str),
            Some("bob")
        );
        assert_eq!(
            app.entries
                .iter()
                .filter(|entry| {
                    entry.body == format!("{} is now known as bob", remote_key.public().fmt_short())
                })
                .count(),
            0,
            "persisted friend name should suppress reconnect duplicate notice"
        );
    }

    #[test]
    fn handle_net_event_own_message_is_skipped() {
        let mut app = test_app();
        let own_key = app.local_public;
        let event = NetEvent::Message {
            from: own_key,
            message: Message::Message {
                text: "echo".into(),
            },
            sent_at: 0,
        };
        handle_net_event(event, &mut app).unwrap();
        // Own messages should not appear as remote entries
        assert!(app.entries.is_empty());
    }

    #[test]
    fn handle_net_event_image_share_sets_pending() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        let event = NetEvent::Message {
            from: remote_key.public(),
            message: Message::ImageShare {
                name: "photo.jpg".into(),
                hash: [0xab; 32],
            },
            sent_at: now_secs(),
        };
        handle_net_event(event, &mut app).unwrap();
        assert_eq!(
            app.pending_image,
            vec![("photo.jpg".into(), [0xab; 32], remote_key.public())]
        );
        assert!(app.entries.iter().any(|e| e.body.contains("photo.jpg")));
    }

    #[test]
    fn handle_net_event_two_image_shares_both_pending() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        let event1 = NetEvent::Message {
            from: remote_key.public(),
            message: Message::ImageShare {
                name: "sunset.jpg".into(),
                hash: [0xaa; 32],
            },
            sent_at: now_secs(),
        };
        let event2 = NetEvent::Message {
            from: remote_key.public(),
            message: Message::ImageShare {
                name: "puppy.jpg".into(),
                hash: [0xbb; 32],
            },
            sent_at: now_secs(),
        };
        handle_net_event(event1, &mut app).unwrap();
        handle_net_event(event2, &mut app).unwrap();
        assert_eq!(
            app.pending_image.len(),
            2,
            "both image shares must be queued"
        );
        assert_eq!(app.pending_image[0].0, "sunset.jpg");
        assert_eq!(app.pending_image[1].0, "puppy.jpg");
        assert!(app.entries.iter().any(|e| e.body.contains("sunset.jpg")));
        assert!(app.entries.iter().any(|e| e.body.contains("puppy.jpg")));
    }

    #[test]
    fn handle_net_event_five_image_shares_all_pending() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        let names = ["img1.png", "img2.png", "img3.png", "img4.png", "img5.png"];
        for (i, name) in names.iter().enumerate() {
            let event = NetEvent::Message {
                from: remote_key.public(),
                message: Message::ImageShare {
                    name: name.to_string(),
                    hash: [i as u8; 32],
                },
                sent_at: now_secs(),
            };
            handle_net_event(event, &mut app).unwrap();
        }
        assert_eq!(
            app.pending_image.len(),
            5,
            "all five image shares must be queued"
        );
        for (i, name) in names.iter().enumerate() {
            assert_eq!(app.pending_image[i].0, *name, "image {} order preserved", i);
        }
        for name in &names {
            assert!(
                app.entries.iter().any(|e| e.body.contains(name)),
                "system message for {name} must be present"
            );
        }
    }

    #[test]
    fn handle_net_event_image_share_self_is_skipped() {
        let mut app = test_app();
        let local_pk = app.local_public;

        let event = NetEvent::Message {
            from: local_pk,
            message: Message::ImageShare {
                name: "selfie.jpg".into(),
                hash: [0xcc; 32],
            },
            sent_at: now_secs(),
        };
        handle_net_event(event, &mut app).unwrap();
        assert!(
            app.pending_image.is_empty(),
            "self-shared images must not be queued for download"
        );
    }

    #[test]
    fn handle_net_event_file_share_from_unknown_peer_is_displayed() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        let event = NetEvent::Message {
            from: remote_key.public(),
            message: Message::FileShare {
                name: "doc.pdf".into(),
                ticket: "abc123".into(),
            },
            sent_at: now_secs(),
        };
        handle_net_event(event, &mut app).unwrap();
        assert!(
            app.pending_file.is_some(),
            "file share from a non-muted remote peer must queue a download"
        );
        assert!(app.entries.iter().any(|e| e.body.contains("doc.pdf")));
    }

    #[test]
    fn handle_net_event_closed_sets_quit() {
        let mut app = test_app();
        handle_net_event(NetEvent::Closed, &mut app).unwrap();
        assert!(app.should_quit);
        assert!(app.entries.iter().any(|e| e.body.contains("closed")));
    }

    #[test]
    fn handle_net_event_error_sets_quit() {
        let mut app = test_app();
        handle_net_event(NetEvent::Error("timeout".into()), &mut app).unwrap();
        assert!(app.should_quit);
        assert!(app.entries.iter().any(|e| e.body.contains("timeout")));
    }

    #[test]
    fn handle_net_event_neighbor_down_uses_display_name() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        app.names.insert(remote_key.public(), "alice".to_string());

        handle_net_event(
            NetEvent::NeighborDown {
                peer: remote_key.public(),
            },
            &mut app,
        )
        .unwrap();
        assert!(app.entries.iter().any(|e| e.body == "alice left the chat"));
    }

    #[test]
    fn handle_net_event_neighbor_down_falls_back_to_short_key() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        handle_net_event(
            NetEvent::NeighborDown {
                peer: remote_key.public(),
            },
            &mut app,
        )
        .unwrap();
        // Without a display name, it formats the short public key.
        let short = remote_key.public().fmt_short();
        assert!(
            app.entries
                .iter()
                .any(|e| e.body == format!("{short} left the chat")),
            "expected '{} left the chat' but got: {:?}",
            short,
            app.entries
        );
    }

    #[test]
    fn handle_net_event_neighbor_up_marks_friend_online() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        // Add the peer as a friend first.
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.ensure_friend(fid.clone());
        app.friends.mark_offline(fid);
        app.friends_dirty = false;

        app.names.insert(remote_key.public(), "alice".to_string());

        handle_net_event(
            NetEvent::NeighborUp {
                peer: remote_key.public(),
            },
            &mut app,
        )
        .unwrap();

        // Friend should be marked online (in memory), but DURTY flag is not
        // set — online status persistence is left to the friend ping manager.
        let fid = FriendId::from_public_key(remote_key.public());
        assert!(
            app.friends
                .get(&fid)
                .map(|r| r.status.online)
                .unwrap_or(false),
            "friend should be marked online"
        );
        assert!(
            !app.friends_dirty,
            "friends should NOT be marked dirty from gossip-level NeighborUp alone"
        );
        assert!(app
            .entries
            .iter()
            .any(|e| e.body == "alice joined the chat"));
    }

    #[test]
    fn handle_net_event_neighbor_up_falls_back_to_short_key() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        handle_net_event(
            NetEvent::NeighborUp {
                peer: remote_key.public(),
            },
            &mut app,
        )
        .unwrap();

        // Without a display name, it formats the short public key.
        let short = remote_key.public().fmt_short();
        assert!(
            app.entries
                .iter()
                .any(|e| e.body == format!("{short} joined the chat")),
            "expected '{} joined the chat' but got: {:?}",
            short,
            app.entries
        );
    }

    #[test]
    fn handle_net_event_neighbor_up_non_friend_not_marked() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        // Don't add the peer as a friend.

        handle_net_event(
            NetEvent::NeighborUp {
                peer: remote_key.public(),
            },
            &mut app,
        )
        .unwrap();

        // Should NOT have a friend record (only friend presence is updated).
        let fid = FriendId::from_public_key(remote_key.public());
        assert!(
            app.friends.get(&fid).is_none(),
            "non-friend should not get a friend record"
        );
        // But we still show a system message.
        let short = remote_key.public().fmt_short();
        assert!(
            app.entries
                .iter()
                .any(|e| e.body == format!("{short} joined the chat")),
            "should show join message even for non-friends"
        );
    }

    // ── handle_net_event dedup tests ───────────────────────────────────

    /// Clear the global seen-messages set so tests start fresh.
    fn clear_seen_messages() {
        if let Ok(mut seen) = SEEN_MESSAGES.lock() {
            seen.clear();
        }
    }

    /// Clear the diagnostics message-received cooldown set so tests start fresh.
    #[allow(dead_code)]
    fn clear_diagnostic_seen_messages() {
        if let Ok(mut seen) = DIAGNOSTIC_SEEN_MESSAGES.lock() {
            seen.clear();
        }
    }

    #[test]
    fn handle_net_event_dedup_exact_duplicate_is_suppressed() {
        let key = SecretKey::generate();
        let mut app = test_app();

        let event = NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: "hello".into(),
            },
            sent_at: now_secs(),
        };

        // First delivery produces one entry.
        handle_net_event(event.clone(), &mut app).unwrap();
        assert_eq!(app.entries.len(), 1);

        // Second delivery (same from, same content, same sent_at) is suppressed.
        handle_net_event(event, &mut app).unwrap();
        assert_eq!(
            app.entries.len(),
            1,
            "duplicate message should not add a second entry"
        );
    }

    #[test]
    fn handle_net_event_dedup_different_text_passes() {
        let key = SecretKey::generate();
        let mut app = test_app();

        let event_a = NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: "first".into(),
            },
            sent_at: now_secs(),
        };
        let event_b = NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: "second".into(),
            },
            sent_at: now_secs() + 1,
        };

        handle_net_event(event_a, &mut app).unwrap();
        handle_net_event(event_b, &mut app).unwrap();
        assert_eq!(
            app.entries.len(),
            2,
            "different messages from same sender should both appear"
        );
        assert_eq!(app.entries[0].body, "first");
        assert_eq!(app.entries[1].body, "second");
    }

    #[test]
    fn handle_net_event_dedup_different_sender_passes() {
        let key_a = SecretKey::generate();
        let key_b = SecretKey::generate();
        let mut app = test_app();

        // Both send the same text at the same time — different senders,
        // so both are legitimate new messages.
        let identical_text = "same text".to_string();
        let event_a = NetEvent::Message {
            from: key_a.public(),
            message: Message::Message {
                text: identical_text.clone(),
            },
            sent_at: now_secs(),
        };
        let event_b = NetEvent::Message {
            from: key_b.public(),
            message: Message::Message {
                text: identical_text,
            },
            sent_at: now_secs(),
        };

        handle_net_event(event_a, &mut app).unwrap();
        handle_net_event(event_b, &mut app).unwrap();
        assert_eq!(
            app.entries.len(),
            2,
            "same content from different senders should both appear"
        );
    }

    #[test]
    fn handle_net_event_dedup_different_sent_at_passes() {
        let key = SecretKey::generate();
        let mut app = test_app();

        // Same content from same sender at different timestamps is a
        // legitimate re-send and should NOT be deduped.
        let event_t1 = NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: "hello".into(),
            },
            sent_at: now_secs(),
        };
        let event_t2 = NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: "hello".into(),
            },
            sent_at: now_secs() + 2,
        };

        handle_net_event(event_t1, &mut app).unwrap();
        handle_net_event(event_t2, &mut app).unwrap();
        assert_eq!(
            app.entries.len(),
            2,
            "same content from same sender at different timestamps should both appear"
        );
    }

    #[test]
    fn handle_net_event_dedup_self_message_is_recorded() {
        // Self-messages are normally skipped for push_remote but should
        // still be tracked in the dedup set so duplicate gossip deliveries
        // of our own messages are suppressed.
        let local_key = SecretKey::generate();
        let mut app = AppState::new(
            test_status(),
            FriendsStore::default(),
            local_key.public(),
            Some("self".into()),
        );

        let event = NetEvent::Message {
            from: local_key.public(),
            message: Message::Message {
                text: "self-msg".into(),
            },
            sent_at: now_secs(),
        };

        // Self-message produces no remote entry.
        handle_net_event(event.clone(), &mut app).unwrap();
        assert!(app.entries.is_empty());

        // Duplicate self-message is still suppressed at the dedup layer.
        handle_net_event(event, &mut app).unwrap();
        assert!(app.entries.is_empty());
    }

    #[test]
    fn handle_net_event_dedup_about_me_is_deduped() {
        let key = SecretKey::generate();
        let mut app = test_app();

        let event = NetEvent::Message {
            from: key.public(),
            message: Message::AboutMe {
                name: "bob".into(),
                profile_image_ticket: None,
            },
            sent_at: now_secs(),
        };

        handle_net_event(event.clone(), &mut app).unwrap();
        // First delivery: one system notification.
        let system_count_before = app
            .entries
            .iter()
            .filter(|e| e.body.contains("bob"))
            .count();
        assert_eq!(system_count_before, 1);

        // Second delivery: suppressed.
        handle_net_event(event, &mut app).unwrap();
        let system_count_after = app
            .entries
            .iter()
            .filter(|e| e.body.contains("bob"))
            .count();
        assert_eq!(
            system_count_after, 1,
            "duplicate AboutMe should not produce a second notification"
        );
    }

    // ── resolve_name with friends store tests ────────────────────────────

    #[test]
    fn resolve_name_prefers_friend_label_over_session_name() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        // Set a session name.
        app.names
            .insert(remote_key.public(), "session_alice".to_string());
        // Add as friend with a label.
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.set_label(fid, "Friend Alice");

        let display = app.resolve_name(&remote_key.public());
        assert_eq!(
            display, "Friend Alice",
            "friend label should override session name"
        );
    }

    #[test]
    fn resolve_name_prefers_friend_announced_name_over_session_name() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        // Give them a session name.
        app.names
            .insert(remote_key.public(), "session_bob".to_string());
        // Add as friend with last_announced_name but no label.
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.set_last_announced_name(fid, "friend_bob");

        let display = app.resolve_name(&remote_key.public());
        assert_eq!(
            display, "friend_bob",
            "friend's last announced name should override session name"
        );
    }

    #[test]
    fn resolve_name_prefers_friend_label_over_friend_announced_name() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends
            .set_last_announced_name(fid.clone(), "auto_name");
        app.friends.set_label(fid, "Label");

        let display = app.resolve_name(&remote_key.public());
        assert_eq!(
            display, "Label",
            "friend label should take priority over last_announced_name"
        );
    }

    #[test]
    fn resolve_name_falls_back_to_session_name_when_not_a_friend() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        app.names
            .insert(remote_key.public(), "session_carol".to_string());

        // Not a friend — should use session name.
        let display = app.resolve_name(&remote_key.public());
        assert_eq!(display, "session_carol");
    }

    #[test]
    fn resolve_name_falls_back_to_short_pk_when_no_name_or_friend() {
        let remote_key = SecretKey::generate();
        let app = test_app();
        // No name, no friend — should fall back to short key.
        let display = app.resolve_name(&remote_key.public());
        let short = format!("{}", remote_key.public().fmt_short());
        assert_eq!(display, short);
    }

    #[test]
    fn resolve_name_falls_back_to_short_pk_when_friend_has_no_named_fields() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        let fid = FriendId::from_public_key(remote_key.public());
        // Ensure the friend exists, but with no label and no last_announced_name.
        app.friends.ensure_friend(fid);

        // No session name either — should fall back to short key.
        let display = app.resolve_name(&remote_key.public());
        let short = format!("{}", remote_key.public().fmt_short());
        assert_eq!(display, short);
    }

    #[test]
    fn handle_net_event_message_shows_friend_label() {
        clear_seen_messages();
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        // Add as friend with a label.
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.set_label(fid, "Best Friend");

        let event = NetEvent::Message {
            from: remote_key.public(),
            message: Message::Message {
                text: "hello!".into(),
            },
            sent_at: now_secs(),
        };
        handle_net_event(event, &mut app).unwrap();
        assert_eq!(app.entries.len(), 1);
        assert_eq!(app.entries[0].label, "Best Friend");
        assert_eq!(app.entries[0].body, "hello!");
    }

    #[test]
    fn handle_net_event_neighbor_up_shows_friend_label() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        // Add as friend with a label.
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.set_label(fid, "Buddy");

        handle_net_event(
            NetEvent::NeighborUp {
                peer: remote_key.public(),
            },
            &mut app,
        )
        .unwrap();

        assert!(app
            .entries
            .iter()
            .any(|e| e.body == "Buddy joined the chat"));
    }

    #[test]
    fn handle_net_event_neighbor_down_shows_friend_label() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.set_label(fid, "Pal");

        handle_net_event(
            NetEvent::NeighborDown {
                peer: remote_key.public(),
            },
            &mut app,
        )
        .unwrap();

        assert!(app.entries.iter().any(|e| e.body == "Pal left the chat"));
    }

    #[test]
    fn handle_net_event_profile_update_calls_on_profile_update() {
        let remote_key = SecretKey::generate();
        let local_key = SecretKey::generate().public();
        let mut app = test_app();
        app.local_public = local_key;

        let mut profile = UserProfile::new(remote_key.public());
        profile.display_name = "alice".into();
        profile.bio = "hello world".into();
        profile.file_sharing_enabled = true;

        let event = NetEvent::Message {
            from: remote_key.public(),
            message: Message::ProfileUpdate(profile.clone()),
            sent_at: 1000,
        };

        // Process the event. The ProfileUpdate handler calls on_profile_update
        // on the callback (AppState's implementation is a no-op, but the method
        // is called without error).
        handle_net_event(event, &mut app).unwrap();

        // Our own messages should be skipped (from == local_public)
        let self_event = NetEvent::Message {
            from: local_key,
            message: Message::ProfileUpdate(profile),
            sent_at: 1001,
        };
        handle_net_event(self_event, &mut app).unwrap();
        // No system message should appear (ProfileUpdate doesn't generate entries)
        assert!(
            app.entries.is_empty(),
            "ProfileUpdate should not create chat entries"
        );
    }

    // ── SignedMessage roundtrip helper ──────────────────────────────────

    fn assert_signed_message_roundtrip(msg: Message, predicate: impl FnOnce(&Message) -> bool) {
        let key = SecretKey::generate();
        let encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap();
        let (pk, decoded, sent_at) = SignedMessage::verify_and_decode(&encoded).unwrap();
        assert!(sent_at > 0);
        assert_eq!(pk, key.public());
        assert!(
            predicate(&decoded),
            "unexpected decoded message: {decoded:?}"
        );
    }

    // ── Basic roundtrip tests for each new interaction type ─────────────

    #[test]
    fn signed_message_roundtrip_read_receipt() {
        let hash = [1u8; 32];
        assert_signed_message_roundtrip(
            Message::ReadReceipt { message_hash: hash },
            |decoded| matches!(decoded, Message::ReadReceipt { message_hash } if *message_hash == hash),
        );
    }

    #[test]
    fn signed_message_roundtrip_edit() {
        let hash = [2u8; 32];
        assert_signed_message_roundtrip(
            Message::Edit {
                original_hash: hash,
                new_text: "updated".into(),
            },
            |decoded| {
                matches!(decoded, Message::Edit { original_hash, new_text }
                    if *original_hash == hash && new_text == "updated")
            },
        );
    }

    #[test]
    fn signed_message_roundtrip_delete() {
        let hash = [3u8; 32];
        assert_signed_message_roundtrip(
            Message::Delete { message_hash: hash },
            |decoded| matches!(decoded, Message::Delete { message_hash } if *message_hash == hash),
        );
    }

    #[test]
    fn signed_message_roundtrip_reaction() {
        let hash = [4u8; 32];
        assert_signed_message_roundtrip(
            Message::Reaction {
                message_hash: hash,
                emoji: "👍".into(),
            },
            |decoded| {
                matches!(decoded, Message::Reaction { message_hash, emoji }
                    if *message_hash == hash && emoji == "👍")
            },
        );
    }

    // ── Edge case roundtrip tests ───────────────────────────────────────

    #[test]
    fn signed_message_roundtrip_reaction_empty_emoji() {
        let hash = [5u8; 32];
        assert_signed_message_roundtrip(
            Message::Reaction {
                message_hash: hash,
                emoji: String::new(),
            },
            |decoded| {
                matches!(decoded, Message::Reaction { message_hash, emoji }
                    if *message_hash == hash && emoji.is_empty())
            },
        );
    }

    #[test]
    fn signed_message_roundtrip_reaction_various_emoji() {
        let hash = [6u8; 32];
        for emoji in &[
            "🔥", // fire - single codepoint
            "👍🏿", // thumbs up dark skin tone
            "👨‍👩‍👧‍👦", // family ZWJ
            "🇦🇺", // AU flag
            "1⃣",  // keycap 1
            "❤️", // heart + VS16
            "😀", // grinning face
            "🎉", // party popper
        ] {
            assert_signed_message_roundtrip(
                Message::Reaction {
                    message_hash: hash,
                    emoji: (*emoji).to_string(),
                },
                |decoded| {
                    matches!(decoded, Message::Reaction { message_hash, emoji }
                        if *message_hash == hash && emoji.as_str() == *emoji)
                },
            );
        }
    }

    #[test]
    fn signed_message_roundtrip_reaction_long_emoji_string() {
        let hash = [7u8; 32];
        let many_hearts: String = "❤️".repeat(50);
        assert_signed_message_roundtrip(
            Message::Reaction {
                message_hash: hash,
                emoji: many_hearts.clone(),
            },
            |decoded| {
                matches!(decoded, Message::Reaction { message_hash, emoji }
                    if *message_hash == hash && *emoji == many_hearts)
            },
        );
    }

    #[test]
    fn signed_message_roundtrip_edit_empty_text() {
        let hash = [8u8; 32];
        assert_signed_message_roundtrip(
            Message::Edit {
                original_hash: hash,
                new_text: String::new(),
            },
            |decoded| {
                matches!(decoded, Message::Edit { original_hash, new_text }
                    if *original_hash == hash && new_text.is_empty())
            },
        );
    }

    #[test]
    fn signed_message_roundtrip_edit_long_text() {
        let hash = [9u8; 32];
        let long_text: String = "A".repeat(10_000);
        assert_signed_message_roundtrip(
            Message::Edit {
                original_hash: hash,
                new_text: long_text.clone(),
            },
            |decoded| {
                matches!(decoded, Message::Edit { original_hash, new_text }
                    if *original_hash == hash && *new_text == long_text)
            },
        );
    }

    #[test]
    fn signed_message_roundtrip_edit_unicode_text() {
        let hash = [10u8; 32];
        let unicode_text = "日本語 русский العربية 😊👋".to_string();
        assert_signed_message_roundtrip(
            Message::Edit {
                original_hash: hash,
                new_text: unicode_text.clone(),
            },
            |decoded| {
                matches!(decoded, Message::Edit { original_hash, new_text }
                    if *original_hash == hash && *new_text == unicode_text)
            },
        );
    }

    #[test]
    fn signed_message_roundtrip_read_receipt_zero_hash() {
        let hash = [0u8; 32];
        assert_signed_message_roundtrip(
            Message::ReadReceipt { message_hash: hash },
            |decoded| matches!(decoded, Message::ReadReceipt { message_hash } if *message_hash == hash),
        );
    }

    #[test]
    fn signed_message_roundtrip_delete_zero_hash() {
        let hash = [0u8; 32];
        assert_signed_message_roundtrip(
            Message::Delete { message_hash: hash },
            |decoded| matches!(decoded, Message::Delete { message_hash } if *message_hash == hash),
        );
    }

    // ── download_candidates ──────────────────────────────────────────────

    #[test]
    fn test_download_candidates_original_first() {
        let pk_a = SecretKey::generate().public();
        let pk_b = SecretKey::generate().public();
        let pk_c = SecretKey::generate().public();
        let mut neighbors = HashSet::new();
        neighbors.insert(pk_b);
        neighbors.insert(pk_c);

        let candidates = download_candidates(pk_a, &neighbors);
        assert_eq!(candidates.len(), 3, "should have 3 candidates");
        assert_eq!(candidates[0], pk_a, "original sender should be first");
        assert!(candidates.contains(&pk_b), "should include neighbor B");
        assert!(candidates.contains(&pk_c), "should include neighbor C");
    }

    #[test]
    fn test_download_candidates_deduplicates_original() {
        let pk_a = SecretKey::generate().public();
        let mut neighbors = HashSet::new();
        neighbors.insert(pk_a); // original is also a neighbor

        let candidates = download_candidates(pk_a, &neighbors);
        assert_eq!(candidates.len(), 1, "should deduplicate");
        assert_eq!(candidates[0], pk_a, "original should be the only entry");
    }

    #[test]
    fn test_download_candidates_no_neighbors() {
        let pk_a = SecretKey::generate().public();
        let neighbors = HashSet::new();

        let candidates = download_candidates(pk_a, &neighbors);
        assert_eq!(candidates.len(), 1, "should have just the original");
        assert_eq!(candidates[0], pk_a);
    }

    // ── collect_bootstrap_peers tests ──────────────────────────────────────

    #[test]
    fn test_collect_bootstrap_peers_dedup() {
        let sk1 = SecretKey::generate();
        let sk2 = SecretKey::generate();
        let pk1 = sk1.public();
        let pk2 = sk2.public();

        let addr1 = EndpointAddr::new(pk1);
        let addr2 = EndpointAddr::new(pk2);
        let addr1_dup = EndpointAddr::new(pk1); // same pk1

        let ticket_peers = [addr1, addr2.clone()];
        let room_peers = [addr1_dup];

        let (peer_ids, all_addrs) = collect_bootstrap_peers([&ticket_peers[..], &room_peers[..]]);

        assert_eq!(peer_ids.len(), 2, "should have 2 unique peer IDs");
        assert!(peer_ids.contains(&pk1), "pk1 should be in peer_ids");
        assert!(peer_ids.contains(&pk2), "pk2 should be in peer_ids");

        assert_eq!(all_addrs.len(), 2, "should have 2 unique addresses");
    }

    #[test]
    fn test_collect_bootstrap_peers_empty() {
        let (ids, addrs) = collect_bootstrap_peers([&[] as &[EndpointAddr]]);
        assert!(ids.is_empty(), "empty sources → empty peer_ids");
        assert!(addrs.is_empty(), "empty sources → empty addrs");
    }

    #[test]
    fn test_collect_bootstrap_peers_single_source() {
        let sk = SecretKey::generate();
        let pk = sk.public();
        let addr = EndpointAddr::new(pk);

        let (ids, addrs) = collect_bootstrap_peers([&[addr.clone()][..]]);
        assert_eq!(ids, vec![pk], "single source should produce its peer ID");
        assert_eq!(addrs.len(), 1, "single source should produce its addr");
    }

    #[test]
    fn test_seed_memory_lookup_adds_addresses() {
        let sk = SecretKey::generate();
        let pk = sk.public();
        let addr = EndpointAddr::new(pk);

        let lookup = iroh::address_lookup::memory::MemoryLookup::new();
        seed_memory_lookup(&lookup, &[addr]);

        let resolved = lookup.get_endpoint_info(pk);
        assert!(
            resolved.is_some(),
            "seed_memory_lookup should add the address"
        );
    }

    #[test]
    fn test_seed_memory_lookup_empty() {
        let lookup = iroh::address_lookup::memory::MemoryLookup::new();
        seed_memory_lookup(&lookup, &[]);
        // Should not panic — verify by checking nothing was added
        assert!(lookup
            .get_endpoint_info(SecretKey::generate().public())
            .is_none());
    }

    // ── TransferProgress lifecycle tests ────────────────────────────────

    #[test]
    fn test_transfer_id_unique() {
        let a = TransferId::next();
        let b = TransferId::next();
        let c = TransferId::next();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn test_cancel_guard_emits_cancelled_on_drop() {
        use std::sync::Mutex;
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let id = TransferId::next();
        let cb: TransferProgressCallback = Arc::new(Mutex::new(Some(Box::new(move |ev| {
            events_clone.lock().unwrap().push(ev);
        }))));

        // Create the guard and let it drop without disarming.
        {
            let _guard = CancelGuard::new(id, TransferKind::File, "test.txt".into(), cb.clone());
        }

        let emitted = events.lock().unwrap();
        assert_eq!(emitted.len(), 1, "should emit exactly one event on drop");
        match &emitted[0] {
            TransferProgress::Cancelled {
                id: emitted_id,
                kind,
                name,
            } => {
                assert_eq!(*emitted_id, id);
                assert_eq!(*kind, TransferKind::File);
                assert_eq!(name, "test.txt");
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    #[test]
    fn test_cancel_guard_disarm_suppresses_cancelled() {
        use std::sync::Mutex;
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let id = TransferId::next();
        let cb: TransferProgressCallback = Arc::new(Mutex::new(Some(Box::new(move |ev| {
            events_clone.lock().unwrap().push(ev);
        }))));

        // Create the guard, disarm it, then let it drop.
        {
            let guard = CancelGuard::new(id, TransferKind::Image, "photo.png".into(), cb.clone());
            guard.disarm();
        }

        let emitted = events.lock().unwrap();
        assert!(
            emitted.is_empty(),
            "should NOT emit Cancelled after disarm, got {emitted:?}"
        );
    }

    #[test]
    fn test_transfer_progress_progress_variant_has_name() {
        // Verify the Progress variant accepts kind + name fields.
        let id = TransferId::next();
        let ev = TransferProgress::Progress {
            id,
            kind: TransferKind::File,
            name: "report.pdf".into(),
            bytes: 512,
            total: None,
        };
        match ev {
            TransferProgress::Progress {
                id: _,
                kind,
                ref name,
                bytes,
                total: _,
            } => {
                assert_eq!(kind, TransferKind::File);
                assert_eq!(name, "report.pdf");
                assert_eq!(bytes, 512);
            }
            other => panic!("expected Progress, got {other:?}"),
        }
    }

    #[test]
    fn test_transfer_progress_cancelled_variant_has_name_and_kind() {
        let id = TransferId::next();
        let ev = TransferProgress::Cancelled {
            id,
            kind: TransferKind::Image,
            name: "avatar.png".into(),
        };
        match ev {
            TransferProgress::Cancelled {
                id: _,
                kind,
                ref name,
            } => {
                assert_eq!(kind, TransferKind::Image);
                assert_eq!(name, "avatar.png");
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    // ── Wire compatibility tests ──────────────────────────────────────────────
    //
    // These tests verify that serialized messages from older versions (where
    // the Message enum had 13 variants without ProfileUpdate) can be decoded
    // correctly by the current code.  ProfileUpdate was inserted mid-enum
    // at commit bf3c0cb3, shifting all later discriminants.  The fix must
    // re-map ProfileUpdate to an explicit high discriminant (13) so all
    // original variants keep their original values (0-12).

    #[test]
    fn old_wire_format_message_decodes_correctly() {
        // Old wire format for Message::Message { text: "hello" }:
        // postcard externally tagged: varint discriminant (1), then struct fields
        //   discriminant: varint(1) = 0x01
        //   text field (String): varint length (5) + UTF-8 bytes
        let old_bytes = [0x01, 0x05, 0x68, 0x65, 0x6c, 0x6c, 0x6f];
        let decoded: Message = postcard::from_bytes(&old_bytes)
            .expect("old-format Message::Message should decode correctly");
        assert!(
            matches!(decoded, Message::Message { ref text } if text == "hello"),
            "expected Message::Message(\"hello\"), got {decoded:?}"
        );
    }

    #[test]
    fn old_wire_format_about_me_decodes_correctly() {
        // Old wire format for Message::AboutMe { name: "alice", profile_image_ticket: None }
        // discriminant: varint(0) = 0x00
        //   name: varint(5) + b"alice"
        //   profile_image_ticket: Option<String> → serde None (postcard: 0x00 for None, or just missing?)
        let old_bytes = [
            0x00, // discriminant 0 = AboutMe
            0x05, // name length
            0x61, 0x6c, 0x69, 0x63, 0x65, // "alice"
            0x00, // profile_image_ticket: None (0 = false for Option)
        ];
        let decoded: Message =
            postcard::from_bytes(&old_bytes).expect("old-format AboutMe should decode correctly");
        assert!(
            matches!(decoded, Message::AboutMe { ref name, profile_image_ticket: None } if name == "alice"),
            "expected AboutMe(\"alice\", None), got {decoded:?}"
        );
    }

    #[test]
    fn old_wire_format_file_share_decodes_correctly() {
        // Old wire format for Message::FileShare { name: "doc.pdf", ticket: "tkt" }
        // discriminant: varint(2) = 0x02
        let old_bytes = [
            0x02, // discriminant 2 = FileShare
            0x07, // name length
            0x64, 0x6f, 0x63, 0x2e, 0x70, 0x64, 0x66, // "doc.pdf"
            0x03, // ticket length
            0x74, 0x6b, 0x74, // "tkt"
        ];
        let decoded: Message =
            postcard::from_bytes(&old_bytes).expect("old-format FileShare should decode correctly");
        match decoded {
            Message::FileShare {
                ref name,
                ref ticket,
            } => {
                assert_eq!(name, "doc.pdf");
                assert_eq!(ticket, "tkt");
            }
            other => panic!("expected FileShare, got {other:?}"),
        }
    }

    #[test]
    fn old_wire_format_presence_decodes_correctly() {
        // Old wire format for Message::Presence (unit variant)
        // discriminant: varint(4) = 0x04
        let old_bytes = [0x04];
        let decoded: Message =
            postcard::from_bytes(&old_bytes).expect("old-format Presence should decode correctly");
        assert!(
            matches!(decoded, Message::Presence),
            "expected Presence, got {decoded:?}"
        );
    }

    #[test]
    fn new_profile_update_wire_format_not_confusable_with_old_message() {
        // ProfileUpdate must NOT use discriminant 1 (the old Message position).
        // Serialize a ProfileUpdate and verify the discriminant byte is NOT 0x01.
        use crate::user_profile::UserProfile;
        let profile = UserProfile::new(PublicKey::from_bytes(&[1u8; 32]).expect("valid key"));
        let msg = Message::ProfileUpdate(profile);
        let bytes = postcard::to_stdvec(&msg).unwrap();
        assert_ne!(
            bytes[0], 0x01,
            "ProfileUpdate MUST NOT use discriminant 1 (old Message position); first byte = {:#04x}",
            bytes[0]
        );
    }

    // ── Ticket tests ────────────────────────────────────────────────────────

    /// Legacy binary ticket (topic + peers only) deserialises to
    /// discovery_secret=None.
    #[test]
    fn test_ticket_legacy_binary_no_secret() {
        let topic = TopicId::from_bytes([0xAAu8; 32]);
        let pk = iroh::SecretKey::generate().public();
        let addr = iroh::EndpointAddr::new(pk);
        let legacy = Ticket {
            topic,
            peers: vec![addr],
            discovery_secret: None,
        };
        let bytes = postcard::to_stdvec(&legacy).unwrap();
        // Decode into the new Ticket struct — should produce None.
        let restored: Ticket = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(restored.topic, topic);
        assert_eq!(restored.peers.len(), 1);
        assert_eq!(restored.discovery_secret, None);
    }

    /// New binary ticket (topic + peers + Some(secret)) round-trips correctly.
    #[test]
    fn test_ticket_with_secret_roundtrip() {
        let topic = TopicId::from_bytes([0xBBu8; 32]);
        let pk = iroh::SecretKey::generate().public();
        let addr = iroh::EndpointAddr::new(pk);
        let secret = DiscoverySecret::generate();
        let ticket = Ticket::with_discovery(topic, vec![addr], secret);
        let bytes = ticket.to_bytes();
        let restored = Ticket::from_bytes(&bytes).unwrap();
        assert_eq!(restored.topic, ticket.topic);
        assert_eq!(restored.peers, ticket.peers);
        assert_eq!(restored.discovery_secret, ticket.discovery_secret);
    }

    /// Display/FromStr round-trip for a ticket without discovery secret.
    #[test]
    fn test_ticket_display_fromstr_no_secret() {
        let topic = TopicId::from_bytes([0xCCu8; 32]);
        let pk = iroh::SecretKey::generate().public();
        let addr = iroh::EndpointAddr::new(pk);
        let ticket = Ticket::new(topic, vec![addr]);
        let display = ticket.to_string();
        let parsed: Ticket = display.parse().unwrap();
        assert_eq!(parsed.topic, ticket.topic);
        assert_eq!(parsed.peers, ticket.peers);
        assert_eq!(parsed.discovery_secret, None);
    }

    /// Display/FromStr round-trip for a ticket with discovery secret.
    #[test]
    fn test_ticket_display_fromstr_with_secret() {
        let topic = TopicId::from_bytes([0xDDu8; 32]);
        let pk = iroh::SecretKey::generate().public();
        let addr = iroh::EndpointAddr::new(pk);
        let secret = DiscoverySecret::from_bytes([0x42u8; 32]);
        let ticket = Ticket::with_discovery(topic, vec![addr], secret);
        let display = ticket.to_string();
        let parsed: Ticket = display.parse().unwrap();
        assert_eq!(parsed.topic, ticket.topic);
        assert_eq!(parsed.peers, ticket.peers);
        assert_eq!(parsed.discovery_secret, ticket.discovery_secret);
    }

    /// Ticket is Send + Sync (compile-time check).
    #[test]
    fn test_ticket_is_send_sync() {
        fn assert_send<T: Send>(_: &T) {}
        fn assert_sync<T: Sync>(_: &T) {}
        let ticket = Ticket::new(
            TopicId::from_bytes([0xEEu8; 32]),
            vec![iroh::EndpointAddr::new(
                iroh::SecretKey::generate().public(),
            )],
        );
        assert_send(&ticket);
        assert_sync(&ticket);
    }
}
