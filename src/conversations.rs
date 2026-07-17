//! Durable conversation records for boru-chat.
//!
//! A conversation is a persisted record keyed by gossip [`TopicId`] that
//! survives application restarts.  Each entry tracks the direct one-to-one
//! conversations the user has engaged in — distinct from the transient
//! room-history list (which is deliberately not persisted).
//!
//! The on-disk file `conversations.json` lives beside `secret_key.txt` in the
//! user's data directory.
//!
//! # Relationship to other stores
//!
//! | Store | Persisted? | Purpose |
//! |-------|-----------|---------|
//! | [`ConversationStore`] | ✓ | Durable conversation records (this module) |
//! | [`RoomHistoryStore`](crate::room_history::RoomHistoryStore) | ✗ | Transient in-process room list for navigation |
//! | [`RoomStore`](crate::room::RoomStore) | ✓ | Current active room's topic and bootstrap peers |
//! | [`FriendsStore`](crate::friends::FriendsStore) | ✓ | Friend/contact list with relationship state |

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};

use crate::chat_core::atomic_write::atomic_write_json;
use crate::proto::TopicId;

const SCHEMA_VERSION: u32 = 1;
/// Name of the on-disk conversations file (lives beside `secret_key.txt`).
pub const CONVERSATIONS_FILE_NAME: &str = "conversations.json";

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn conversations_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CONVERSATIONS_FILE_NAME)
}

// ── Conversation kind ───────────────────────────────────────────────────

/// The kind of a conversation — either a direct one-to-one chat with a peer
/// or a group room with a shared topic.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConversationKind {
    /// Direct one-to-one conversation with a peer.
    /// The topic is deterministically derived from the two public keys.
    #[default]
    Direct,
    /// Group/room conversation on a shared gossip topic.
    Group,
}

// ── Network event tagged by topic ───────────────────────────────────────

/// A [`crate::chat_core::NetEvent`] tagged with the [`TopicId`] of the
/// conversation it belongs to.
///
/// Created by per-conversation forwarder tasks so the frontend can route
/// incoming events to the correct conversation state.
#[derive(Clone, Debug)]
pub struct ConversationNetEvent {
    /// The gossip topic this event arrived on.
    pub topic: TopicId,
    /// The decoded network event.
    pub event: crate::chat_core::NetEvent,
}

impl ConversationNetEvent {
    /// Wrap a [`NetEvent`] with the topic it arrived on.
    pub fn new(topic: TopicId, event: crate::chat_core::NetEvent) -> Self {
        Self { topic, event }
    }
}

// ── On-disk conversation entry ──────────────────────────────────────────

/// A single persisted conversation record.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversationEntry {
    /// The gossip topic for this conversation.
    pub topic: TopicId,
    /// Hex-encoded public key of the other participant (empty for group
    /// conversations that lack a single peer identifier).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub peer_id: String,
    /// Human-readable display name for the conversation.
    #[serde(default)]
    pub name: String,
    /// What kind of conversation this is.
    #[serde(default)]
    pub kind: ConversationKind,
    /// Unix-epoch milliseconds when the conversation was first created.
    pub created_at_unix_ms: u64,
    /// Unix-epoch milliseconds of the most recent activity.
    #[serde(default)]
    pub last_seen_at_unix_ms: u64,
    /// Whether the conversation is archived and should not appear in the
    /// default conversation list.
    #[serde(default)]
    pub archived: bool,
}

impl ConversationEntry {
    /// Create a new conversation entry with the current timestamp.
    pub fn new(topic: TopicId, peer_id: impl Into<String>, name: impl Into<String>) -> Self {
        let now = now_unix_ms();
        Self {
            topic,
            peer_id: peer_id.into(),
            name: name.into(),
            kind: ConversationKind::Direct,
            created_at_unix_ms: now,
            last_seen_at_unix_ms: now,
            archived: false,
        }
    }

    /// Create a new group conversation entry.
    pub fn new_group(topic: TopicId, name: impl Into<String>) -> Self {
        let now = now_unix_ms();
        Self {
            topic,
            peer_id: String::new(),
            name: name.into(),
            kind: ConversationKind::Group,
            created_at_unix_ms: now,
            last_seen_at_unix_ms: now,
            archived: false,
        }
    }

    /// Bump the last-seen timestamp to now.
    pub fn touch(&mut self) {
        self.last_seen_at_unix_ms = now_unix_ms();
    }

    /// Display label for the conversation.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            if self.peer_id.is_empty() {
                "Unknown"
            } else {
                &self.peer_id[..self.peer_id.len().min(16)]
            }
        } else {
            &self.name
        }
    }
}

// ── On-disk conversation store ──────────────────────────────────────────

/// Versioned persistent conversation store.
///
/// Conversations are serialised as a JSON vec (since `TopicId` cannot serve as
/// a JSON map key) and indexed internally via a [`BTreeMap`] for O(log n)
/// lookups.  The in-memory index is rebuilt on load.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversationStore {
    /// Format version for future migrations.
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    /// Conversations serialised as a vec (serde cannot use [`TopicId`] as a
    /// JSON object key).
    #[serde(default)]
    conversations: Vec<ConversationEntry>,
    /// Fast topic → entry index, rebuilt on load.
    #[serde(skip)]
    by_topic: BTreeMap<TopicId, usize>,
    /// Data directory used for load/save operations.
    #[serde(skip)]
    data_dir: PathBuf,
}

impl Default for ConversationStore {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            conversations: Vec::new(),
            by_topic: BTreeMap::new(),
            data_dir: PathBuf::new(),
        }
    }
}

impl ConversationStore {
    fn rebuild_index(&mut self) {
        self.by_topic.clear();
        for (i, entry) in self.conversations.iter().enumerate() {
            self.by_topic.insert(entry.topic, i);
        }
    }

    /// Sort the conversation list most-recent-first by `last_seen_at_unix_ms`.
    fn sort_by_recency(&mut self) {
        self.conversations
            .sort_by(|a, b| b.last_seen_at_unix_ms.cmp(&a.last_seen_at_unix_ms));
        self.rebuild_index();
    }

    /// Bubble an entry at `idx` upward (toward index 0) after a
    /// `last_seen_at_unix_ms` increase, keeping the list sorted
    /// most-recent-first.  Updates `by_topic` indices for swapped entries.
    fn bubble_up(&mut self, mut idx: usize) {
        while idx > 0 {
            if self.conversations[idx].last_seen_at_unix_ms
                <= self.conversations[idx - 1].last_seen_at_unix_ms
            {
                break;
            }
            let ts = self.conversations[idx].topic;
            let ts_prev = self.conversations[idx - 1].topic;
            self.conversations.swap(idx, idx - 1);
            self.by_topic.insert(ts, idx - 1);
            self.by_topic.insert(ts_prev, idx);
            idx -= 1;
        }
    }

    /// Bubble an entry at `idx` downward after a `last_seen_at_unix_ms`
    /// decrease, keeping the list sorted most-recent-first.
    fn bubble_down(&mut self, mut idx: usize) {
        let len = self.conversations.len();
        while idx + 1 < len {
            if self.conversations[idx].last_seen_at_unix_ms
                >= self.conversations[idx + 1].last_seen_at_unix_ms
            {
                break;
            }
            let ts = self.conversations[idx].topic;
            let ts_next = self.conversations[idx + 1].topic;
            self.conversations.swap(idx, idx + 1);
            self.by_topic.insert(ts, idx + 1);
            self.by_topic.insert(ts_next, idx);
            idx += 1;
        }
    }

    fn insert_or_update(&mut self, entry: ConversationEntry) -> Option<ConversationEntry> {
        if let Some(&idx) = self.by_topic.get(&entry.topic) {
            let old = std::mem::replace(&mut self.conversations[idx], entry);
            // Re-position if the recency changed
            if self.conversations[idx].last_seen_at_unix_ms > old.last_seen_at_unix_ms {
                self.bubble_up(idx);
            } else if self.conversations[idx].last_seen_at_unix_ms < old.last_seen_at_unix_ms {
                self.bubble_down(idx);
            }
            Some(old)
        } else {
            // Insert at the correct sorted position (most-recent-first)
            let pos = self
                .conversations
                .binary_search_by(|e| entry.last_seen_at_unix_ms.cmp(&e.last_seen_at_unix_ms))
                .unwrap_or_else(|e| e);
            self.conversations.insert(pos, entry);
            // Update indices for entries at `pos` and above
            for i in pos..self.conversations.len() {
                self.by_topic.insert(self.conversations[i].topic, i);
            }
            None
        }
    }

    fn remove_by_topic(&mut self, topic: &TopicId) -> Option<ConversationEntry> {
        if let Some(idx) = self.by_topic.remove(topic) {
            let removed = self.conversations.remove(idx);
            // Update indices for entries that shifted down
            self.rebuild_index();
            Some(removed)
        } else {
            None
        }
    }

    /// Construct an empty store bound to a data directory.
    pub fn empty_at(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            ..Self::default()
        }
    }

    /// Return the data directory used by this store.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Return the on-disk conversations file path.
    pub fn file_path(&self) -> PathBuf {
        conversations_file_path(&self.data_dir)
    }

    /// Load the conversation store from disk.
    ///
    /// Missing files are treated as an empty store.  Corrupt JSON returns an
    /// error so callers can decide whether to fall back.
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref();
        let path = conversations_file_path(data_dir);
        if !path.exists() {
            return Ok(Self::empty_at(data_dir));
        }

        let raw = fs::read_to_string(&path).with_std_context(|_| {
            format!("failed to read conversations file {}", path.display())
        })?;
        let mut store: Self = serde_json::from_str(&raw).with_std_context(|_| {
            format!("failed to parse conversations file {}", path.display())
        })?;

        if !(1..=SCHEMA_VERSION).contains(&store.schema_version) {
            return Err(n0_error::anyerr!(
                "unsupported conversations schema version {} in {}",
                store.schema_version,
                path.display()
            ));
        }
        store.schema_version = SCHEMA_VERSION;
        store.data_dir = data_dir.to_path_buf();
        store.rebuild_index();
        store.sort_by_recency();
        Ok(store)
    }

    /// Load a store, logging and falling back to an empty store on failure.
    pub fn load_or_default(data_dir: impl AsRef<Path>) -> Self {
        let data_dir = data_dir.as_ref();
        match Self::load(data_dir) {
            Ok(store) => store,
            Err(err) => {
                eprintln!(
                    "warning: starting with an empty conversation list; \
                     failed to load {}: {err}",
                    conversations_file_path(data_dir).display()
                );
                Self::empty_at(data_dir)
            }
        }
    }

    /// Persist the store atomically to `conversations.json`.
    pub fn save(&self) -> Result<PathBuf> {
        let data_dir = self.data_dir();
        if data_dir.as_os_str().is_empty() {
            return Err(n0_error::anyerr!(
                "conversation store has no data directory bound to it",
            ));
        }
        let path = self.file_path();
        atomic_write_json(&path, self, "conversation store")?;
        Ok(path)
    }

    /// Number of conversations in the store.
    pub fn len(&self) -> usize {
        self.conversations.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.conversations.is_empty()
    }

    /// Immutable iterator over all conversations (in insertion order).
    pub fn iter(&self) -> impl Iterator<Item = &ConversationEntry> {
        self.conversations.iter()
    }

    /// Find a conversation by topic.
    pub fn find(&self, topic: &TopicId) -> Option<&ConversationEntry> {
        self.by_topic
            .get(topic)
            .and_then(|&idx| self.conversations.get(idx))
    }

    /// Find a conversation by topic, mutably.
    pub fn find_mut(&mut self, topic: &TopicId) -> Option<&mut ConversationEntry> {
        if let Some(&idx) = self.by_topic.get(topic) {
            self.conversations.get_mut(idx)
        } else {
            None
        }
    }

    /// Insert or update a conversation entry.
    ///
    /// Returns the previous entry for the same topic, if any.
    pub fn upsert(&mut self, entry: ConversationEntry) -> Option<ConversationEntry> {
        self.insert_or_update(entry)
    }

    /// Remove a conversation by topic.
    ///
    /// Returns the removed entry, if any.
    pub fn remove(&mut self, topic: &TopicId) -> Option<ConversationEntry> {
        self.remove_by_topic(topic)
    }

    /// Remove all conversations.
    pub fn clear(&mut self) {
        self.conversations.clear();
        self.by_topic.clear();
    }

    /// Bump the `last_seen_at_unix_ms` of a conversation and re-position
    /// it in the sorted list (most-recent-first).  Returns the entry's
    /// previous timestamp, or `None` if the topic doesn't exist.
    ///
    /// This is O(k) where k is the number of positions the entry moves —
    /// typically 0 or 1 for a conversation that was already recent.
    pub fn touch_and_bump(&mut self, topic: &TopicId) -> Option<u64> {
        let idx = *self.by_topic.get(topic)?;
        let old_ts = self.conversations[idx].last_seen_at_unix_ms;
        let now = now_unix_ms();
        self.conversations[idx].last_seen_at_unix_ms = now;
        if now > old_ts {
            self.bubble_up(idx);
        }
        Some(old_ts)
    }

    /// Return an iterator over non-archived conversations, most-recently-seen
    /// first.
    ///
    /// The list is already maintained in sorted order internally, so this
    /// is O(n) without any sorting overhead.
    pub fn active_iter(&self) -> Vec<&ConversationEntry> {
        self.conversations.iter().filter(|e| !e.archived).collect()
    }

    /// Return all archived conversations, most-recently-seen first.
    pub fn archived_iter(&self) -> Vec<&ConversationEntry> {
        self.conversations.iter().filter(|e| e.archived).collect()
    }
}

// ── Topic-tagged event forwarding ───────────────────────────────────────

/// Spawn a background task that forwards gossip events for a conversation,
/// tagging each event with the conversation's topic.
///
/// The resulting [`ConversationNetEvent`]s are pushed to `net_tx` so the
/// frontend can route them to the correct conversation state.
///
/// Returns a [`JoinHandle`] that can be stored in the conversation's
/// `forward_handle` field for lifecycle tracking.  Dropping the handle
/// does **not** abort the task — the task runs until the gossip receiver
/// closes or the `net_tx` channel is dropped.
#[cfg(feature = "net")]
pub fn spawn_conversation_forwarder(
    topic: TopicId,
    metadata_doc: crate::room_docs::RoomMetadataDoc,
    roster_doc: crate::room_docs::RosterDoc,
    receiver: crate::api::GossipReceiver,
    net_tx: tokio::sync::mpsc::UnboundedSender<ConversationNetEvent>,
    safety: Option<std::sync::Arc<crate::public_room_safety::PublicRoomSafety>>,
) -> n0_future::task::JoinHandle<()> {
    n0_future::task::spawn(async move {
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        // Spawn the room-doc-aware forwarder to push raw NetEvents to inner_tx
        let forward_handle =
            n0_future::task::spawn(crate::room_docs::forward_room_events_for_chat(
                metadata_doc,
                roster_doc,
                receiver,
                inner_tx,
                safety,
            ));
        // Bridge: tag each NetEvent with the topic and forward to the shared channel
        while let Some(event) = inner_rx.recv().await {
            if net_tx
                .send(ConversationNetEvent::new(topic, event))
                .is_err()
            {
                break;
            }
        }
        // Wait for the underlying forwarder to finish (it will when the receiver closes)
        let _ = forward_handle.await;
    })
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        dir.push(format!("boru-conversations-{name}-{suffix}"));
        dir
    }

    fn make_topic(byte: u8) -> TopicId {
        TopicId::from_bytes([byte; 32])
    }

    fn make_entry(topic: TopicId, peer: &str, name: &str) -> ConversationEntry {
        ConversationEntry::new(topic, peer, name)
    }

    // ── ConversationKind ─────────────────────────────────────────────

    #[test]
    fn conversation_kind_default_is_direct() {
        let entry = ConversationEntry::new(make_topic(0xAA), "peer", "name");
        assert_eq!(entry.kind, ConversationKind::Direct);
    }

    #[test]
    fn conversation_kind_group_is_preserved() {
        let entry = ConversationEntry::new_group(make_topic(0xBB), "Room");
        assert_eq!(entry.kind, ConversationKind::Group);
    }

    // ── Load / save ──────────────────────────────────────────────────────

    #[test]
    fn load_missing_returns_empty_store() {
        let dir = temp_dir("missing");
        let store = ConversationStore::load(&dir).expect("load missing");
        assert!(store.is_empty());
        assert_eq!(store.data_dir(), dir.as_path());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir("roundtrip");
        let mut store = ConversationStore::empty_at(&dir);
        let topic = make_topic(0xAA);
        store.upsert(make_entry(topic, "alice", "Alice"));
        store.save().expect("save");

        let loaded = ConversationStore::load(&dir).expect("load");
        assert_eq!(loaded.len(), 1);
        let entry = loaded.find(&topic).expect("entry exists");
        assert_eq!(entry.peer_id, "alice");
        assert_eq!(entry.name, "Alice");
    }

    #[test]
    fn load_or_default_returns_empty_on_missing_file() {
        let dir = temp_dir("default-missing");
        let store = ConversationStore::load_or_default(&dir);
        assert!(store.is_empty());
    }

    #[test]
    fn load_or_default_fallback_on_corrupt() {
        let dir = temp_dir("corrupt");
        fs::create_dir_all(&dir).expect("create dir");
        fs::write(conversations_file_path(&dir), "not valid json").expect("write corrupt file");
        let store = ConversationStore::load_or_default(&dir);
        // Should fall back to empty store, not panic
        assert!(store.is_empty());
    }

    #[test]
    fn save_then_load_preserves_multiple_conversations() {
        let dir = temp_dir("multi");
        let mut store = ConversationStore::empty_at(&dir);
        let t1 = make_topic(0x01);
        let t2 = make_topic(0x02);
        store.upsert(make_entry(t1, "bob", "Bob"));
        store.upsert(make_entry(t2, "carol", "Carol"));
        store.save().expect("save");

        let loaded = ConversationStore::load(&dir).expect("load");
        assert_eq!(loaded.len(), 2);
        assert!(loaded.find(&t1).is_some());
        assert!(loaded.find(&t2).is_some());
    }

    #[test]
    fn save_then_load_preserves_kind() {
        let dir = temp_dir("kind");
        let mut store = ConversationStore::empty_at(&dir);
        let t = make_topic(0xCC);
        store.upsert(ConversationEntry::new_group(t, "The Room"));
        store.save().expect("save");

        let loaded = ConversationStore::load(&dir).expect("load");
        let entry = loaded.find(&t).expect("entry exists");
        assert_eq!(entry.kind, ConversationKind::Group);
    }

    // ── upsert / remove / clear ──────────────────────────────────────────

    #[test]
    fn upsert_adds_new_entry() {
        let dir = temp_dir("upsert-new");
        let mut store = ConversationStore::empty_at(&dir);
        let topic = make_topic(0xBB);
        assert!(store.find(&topic).is_none());

        store.upsert(make_entry(topic, "dave", "Dave"));
        assert!(store.find(&topic).is_some());
    }

    #[test]
    fn upsert_replaces_existing() {
        let dir = temp_dir("upsert-replace");
        let mut store = ConversationStore::empty_at(&dir);
        let topic = make_topic(0xCC);
        store.upsert(make_entry(topic, "eve", "Eve"));
        let entry = make_entry(topic, "eve", "Eve (updated)");
        let old = store.upsert(entry);
        assert!(old.is_some());
        assert_eq!(store.find(&topic).unwrap().name, "Eve (updated)");
    }

    #[test]
    fn remove_removes_entry() {
        let dir = temp_dir("remove");
        let mut store = ConversationStore::empty_at(&dir);
        let topic = make_topic(0xDD);
        store.upsert(make_entry(topic, "frank", "Frank"));
        assert_eq!(store.len(), 1);

        let removed = store.remove(&topic);
        assert!(removed.is_some());
        assert!(store.is_empty());
    }

    #[test]
    fn remove_nonexistent_returns_none() {
        let dir = temp_dir("remove-nonexist");
        let mut store = ConversationStore::empty_at(&dir);
        let topic = make_topic(0xFF);
        assert!(store.remove(&topic).is_none());
    }

    #[test]
    fn clear_empties_store() {
        let dir = temp_dir("clear");
        let mut store = ConversationStore::empty_at(&dir);
        store.upsert(make_entry(make_topic(0x01), "a", "A"));
        store.upsert(make_entry(make_topic(0x02), "b", "B"));
        assert_eq!(store.len(), 2);
        store.clear();
        assert!(store.is_empty());
    }

    // ── Iteration ────────────────────────────────────────────────────────

    #[test]
    fn active_iter_skips_archived_and_sorts_by_recency() {
        let dir = temp_dir("active-iter");
        let mut store = ConversationStore::empty_at(&dir);

        let t_old = make_topic(0x01);
        let t_new = make_topic(0x02);
        let t_archived = make_topic(0x03);

        // Create oldest conversation first
        store.upsert(make_entry(t_old, "old", "Old"));

        // Ensure distinct timestamps
        std::thread::sleep(std::time::Duration::from_millis(2));

        // Create newest active conversation
        store.upsert(make_entry(t_new, "new", "New"));

        // Create and archive a conversation
        std::thread::sleep(std::time::Duration::from_millis(2));
        let mut archived = make_entry(t_archived, "archived", "Archived");
        archived.archived = true;
        store.upsert(archived);

        let active = store.active_iter();
        assert_eq!(active.len(), 2);
        // Newest first
        assert_eq!(active[0].topic, t_new);
        assert_eq!(active[1].topic, t_old);

        let archived_list = store.archived_iter();
        assert_eq!(archived_list.len(), 1);
        assert_eq!(archived_list[0].topic, t_archived);
    }

    #[test]
    fn display_name_falls_back_to_peer_id() {
        let topic = make_topic(0xEE);
        let entry = ConversationEntry::new(topic, "abcdef1234567890", "");
        let display = entry.display_name();
        assert_eq!(display, "abcdef1234567890");
    }

    #[test]
    fn display_name_uses_name_when_set() {
        let topic = make_topic(0xAA);
        let entry = ConversationEntry::new(topic, "peer", "My Friend");
        assert_eq!(entry.display_name(), "My Friend");
    }

    // ── touch_and_bump ────────────────────────────────────────────────

    #[test]
    fn touch_and_bump_moves_conversation_to_top() {
        let dir = temp_dir("touch-bump");
        let mut store = ConversationStore::empty_at(&dir);

        // Use entries with explicit, well-separated timestamps
        let t1 = make_topic(0x01);
        let t2 = make_topic(0x02);
        let t3 = make_topic(0x03);

        let mut e1 = make_entry(t1, "a", "A");
        e1.last_seen_at_unix_ms = 1000;
        let mut e2 = make_entry(t2, "b", "B");
        e2.last_seen_at_unix_ms = 2000;
        let mut e3 = make_entry(t3, "c", "C");
        e3.last_seen_at_unix_ms = 3000;

        store.upsert(e1);
        store.upsert(e2);
        store.upsert(e3);

        // Sorted: t3 (3000), t2 (2000), t1 (1000)
        let active = store.active_iter();
        assert_eq!(active.len(), 3);
        assert_eq!(active[0].topic, t3);
        assert_eq!(active[1].topic, t2);
        assert_eq!(active[2].topic, t1);

        // Bump the oldest conversation to a timestamp newer than all others
        {
            let entry = store.find_mut(&t1).unwrap();
            entry.last_seen_at_unix_ms = 4000;
        }
        let old_ts = store.touch_and_bump(&t1).expect("t1 exists");
        // The store bumps to now() which is > 4000, so old_ts is whatever we set above
        assert!(old_ts > 0, "should return the previous timestamp");

        // After bump, t1 should be at the top
        let active = store.active_iter();
        assert_eq!(active.len(), 3);
        assert_eq!(active[0].topic, t1, "t1 should move to top after bump");
    }

    #[test]
    fn touch_and_bump_returns_none_for_unknown() {
        let dir = temp_dir("touch-bump-unknown");
        let mut store = ConversationStore::empty_at(&dir);
        assert!(store.touch_and_bump(&make_topic(0xFF)).is_none());
    }

    #[test]
    fn touch_updates_last_seen() {
        let topic = make_topic(0xBB);
        let mut entry = ConversationEntry::new(topic, "peer", "Name");
        let original = entry.last_seen_at_unix_ms;
        std::thread::sleep(std::time::Duration::from_millis(2));
        entry.touch();
        assert!(entry.last_seen_at_unix_ms > original);
    }

    #[test]
    fn upsert_reuses_entry_on_same_topic() {
        let dir = temp_dir("upsert-same-topic");
        let mut store = ConversationStore::empty_at(&dir);
        let topic = make_topic(0x10);
        store.upsert(make_entry(topic, "grace", "Grace"));
        store.upsert(make_entry(topic, "grace", "Grace (updated)"));
        assert_eq!(store.len(), 1);
        assert_eq!(store.find(&topic).unwrap().name, "Grace (updated)");
    }
}
