//! UI-independent chat application state.
//!
//! [`AppState`] is the backend-agnostic chat state machine shared by the TUI
//! and GUI frontends.  Extracted from `chat_core` so the state machine and its
//! [`ChatCallbacks`] implementation can be tested without network or storage.

use std::collections::HashMap;
use std::time::Instant;

use iroh::PublicKey;

use crate::chat_callbacks::ChatCallbacks;
use crate::chat_core::protocol::MessageHash;
use crate::chat_core::{ChatEntry, Composer, StatusContext, Ticket};
use crate::friends::{FriendId, FriendsStore};
use crate::chat_core::typing::TypingState;

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
    /// Pending external catalogue GIF shares: (payload, sender_pk).
    pub pending_gif: Vec<(crate::gif_provider::SharedGif, PublicKey)>,
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
    /// Ephemeral typing leases; never included in durable history.
    pub typing: TypingState,
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
            pending_gif: Vec::new(),
            friends,
            friends_dirty: false,
            names,
            local_public,
            self_sent_events: HashMap::new(),
            typing: TypingState::default(),
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

    /// Append a remote message while retaining stable mention metadata.
    pub fn push_remote_with_mentions(
        &mut self,
        label: impl Into<String>,
        text: impl Into<String>,
        mentions: Vec<crate::mentions::Mention>,
        hash: MessageHash,
    ) {
        self.push_entry(
            ChatEntry::remote(label, text)
                .with_message_hash(hash)
                .with_mentions(mentions),
            true,
        );
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
        // Priority: friend label > friend's last announced name > session name
        //           > compact peer-ID suffix (last 5 hex chars).
        let fid = FriendId::from_public_key(*peer);
        let friend_label = self.friends.get(&fid).and_then(|r| r.label.as_deref());
        let friend_announced = self
            .friends
            .get(&fid)
            .and_then(|r| r.last_announced_name.as_deref());
        let session_name = self.names.get(peer).map(|s| s.as_str());
        crate::peer_names::resolve_peer_name(
            peer,
            friend_label,
            None, // profile display name not available here
            friend_announced,
            session_name,
        )
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

    fn push_remote_with_mentions(
        &mut self,
        _peer: PublicKey,
        label: String,
        text: String,
        mentions: Vec<crate::mentions::Mention>,
        hash: Option<MessageHash>,
        sent_at_secs: Option<u64>,
    ) {
        let mut entry = ChatEntry::remote(label, text).with_mentions(mentions);
        if let Some(secs) = sent_at_secs {
            entry = entry.with_timestamp(Some(secs * 1000));
        }
        if let Some(h) = hash {
            entry = entry.with_message_hash(h);
        }
        self.push_entry(entry, true);
    }

    fn set_pending_file(
        &mut self,
        name: String,
        ticket: String,
        _size: u64,
        thumbnail_hash: Option<MessageHash>,
        _sender_label: Option<String>,
    ) {
        self.pending_file = Some((name, ticket));
        let _ = thumbnail_hash;
    }

    fn set_pending_image(&mut self, name: String, hash: MessageHash, from: PublicKey) {
        self.pending_image.push((name, hash, from));
    }

    fn set_pending_gif(
        &mut self,
        gif: crate::gif_provider::SharedGif,
        from: PublicKey,
        _message_hash: MessageHash,
    ) {
        self.pending_gif.push((gif, from));
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

    fn on_typing(
        &mut self,
        topic: Option<crate::proto::TopicId>,
        peer: PublicKey,
        active: bool,
    ) {
        let Some(topic) = topic else { return };
        if active {
            self.typing.set(topic, peer, Instant::now());
        } else {
            self.typing.clear(topic, &peer);
        }
    }

    fn clear_typing_peer(&mut self, peer: &PublicKey) {
        self.typing.clear_peer(peer);
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


