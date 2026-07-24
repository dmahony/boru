//! Durable chat history storage for Boru.
//!
//! Chat messages are persisted atomically so room history remains available
//! after restart. Outgoing messages additionally live in the durable outbox
//! until transport delivery has been observed.
use std::{
    fmt, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};

use crate::chat_core::atomic_write::atomic_write_json;
use crate::proto::TopicId;

/// Current schema version — bump on breaking format changes.
const SCHEMA_VERSION: u32 = 1;

/// Name of the on-disk history file (lives beside `secret_key.txt`).
pub const HISTORY_FILE_NAME: &str = "chat_history.json";

// ── Delivery state ─────────────────────────────────────────────────────

/// Delivery status of a chat message.
///
/// Messages begin in [`Queued`](DeliveryState::Queued) and proceed through
/// a directed acyclic graph of transitions:
///
/// ```text
/// Queued ──→ Sent ──→ Delivered ──→ Seen
///   │                    │
///   └──→ Failed          │
///                        └──→ Failed  (delivery explicitly failed)
/// ```
///
/// Key semantic: **Delivered** means the peer's transport confirmed receipt
/// (i.e. the message reached the peer's node), while **Seen** means the
/// peer acknowledged the user actually viewed/read the message.  This
/// distinction lets frontends show a two-tick (Delivered) vs. two-blue-tick
/// (Seen) read-receipt pattern.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DeliveryState {
    /// Message has been composed and queued for sending.
    #[default]
    Queued,
    /// Message was handed off to the transport layer (gossip / QUIC).
    Sent,
    /// Peer confirmed receipt of the message payload.
    Delivered,
    /// Peer confirmed the recipient viewed the message.
    Seen,
    /// Sending or delivery failed permanently.
    Failed,
}

impl DeliveryState {
    /// Return `true` if a transition from `self` to `other` is valid.
    ///
    /// The rules, in order:
    ///
    /// | From        | To          | Valid? |
    /// |-------------|-------------|--------|
    /// | Queued      | Sent        | ✓      |
    /// | Queued      | Failed      | ✓      |
    /// | Sent        | Delivered   | ✓      |
    /// | Sent        | Seen        | ✓      |
    /// | Sent        | Failed      | ✓      |
    /// | Delivered   | Seen        | ✓      |
    /// | Delivered   | Failed      | ✓      |
    /// | anything    | same state  | ✓ (no-op, useful for idempotent updates) |
    /// | anything    | anything else | ✗    |
    ///
    /// A transition to the current state (identity) is always valid so
    /// callers may safely re-apply known state without error handling.
    pub fn can_transition_to(&self, other: &DeliveryState) -> bool {
        if self == other {
            return true; // identity is always a no-op
        }
        matches!(
            (self, other),
            (DeliveryState::Queued, DeliveryState::Sent)
                | (DeliveryState::Queued, DeliveryState::Failed)
                | (DeliveryState::Sent, DeliveryState::Delivered)
                | (DeliveryState::Sent, DeliveryState::Seen)
                | (DeliveryState::Sent, DeliveryState::Failed)
                | (DeliveryState::Delivered, DeliveryState::Seen)
                | (DeliveryState::Delivered, DeliveryState::Failed)
        )
    }
}

impl DeliveryState {
    /// Return a short display-style icon for the delivery state.
    /// Suitable for appending to message labels in a chat UI.
    pub fn display_icon(&self) -> &'static str {
        match self {
            DeliveryState::Queued => "🔄",
            DeliveryState::Sent => "✓",
            DeliveryState::Delivered => "✓✓",
            DeliveryState::Seen => "👁",
            DeliveryState::Failed => "✗",
        }
    }
}

/// Error returned when an invalid delivery-state transition is attempted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidTransition {
    /// The event id whose state could not be updated.
    pub event_id: u64,
    /// The current state.
    pub current: DeliveryState,
    /// The attempted new state.
    pub attempted: DeliveryState,
}

impl fmt::Display for InvalidTransition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid delivery-state transition for event {}: {:?} → {:?}",
            self.event_id, self.current, self.attempted,
        )
    }
}

impl std::error::Error for InvalidTransition {}

// ── Helpers ────────────────────────────────────────────────────────────

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

fn default_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn history_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(HISTORY_FILE_NAME)
}

/// Compute the blake3 hex hash of a byte slice.
pub fn blake3_hex(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

// ── Entry type ─────────────────────────────────────────────────────────

/// One stored message in the chat history.
///
/// Each entry records a stable locally-assigned event id, the
/// content-addressed hash (blake3 of the signed message bytes), the
/// sender's public key, a millisecond-precision timestamp, a human-readable
/// message kind, a short text preview, and the current delivery state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Stable, monotonically-increasing event identifier assigned locally.
    ///
    /// Unlike the content-addressed `hash`, this id is unique per event in
    /// the local session and stable across state transitions.
    pub event_id: u64,
    /// blake3 hex hash of the raw signed message bytes.
    pub hash: String,
    /// Hex-encoded public key of the sender.
    pub sender: String,
    /// Unix-epoch milliseconds when this entry was *stored locally*.
    pub timestamp: u64,
    /// Human-readable kind: "text", "file", "about_me", "goodbye", "system".
    pub kind: String,
    /// The gossip topic this message belongs to.
    pub topic: TopicId,
    /// Short text preview (first 120 chars, or empty for binary/system).
    pub text_preview: String,
    /// The raw signed message bytes that were broadcast over gossip.
    /// Stored so peers can replay the exact bytes through
    /// `SignedMessage::verify_and_decode`.
    pub signed_bytes: Vec<u8>,
    /// Current delivery state of this message.
    #[serde(default)]
    pub delivery_state: DeliveryState,
    /// Decoded image bytes for inline rendering, if this is an image message.
    /// Stored so images persist when switching rooms within the same session.
    #[serde(skip)]
    pub image_bytes: Option<Vec<u8>>,
    /// Storage identifier returned by the [`ImageStore`](crate::image_store::ImageStore)
    /// for this image, if it was stored via the per-user image storage system.
    /// The identifier has the form `<user-hash>/<content-hash>.<ext>` and is
    /// a relative path within the store's files root — never an absolute
    /// filesystem path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_identifier: Option<String>,
}

impl HistoryEntry {
    /// Create a new history entry from the raw signed message bytes.
    ///
    /// The hash is computed as blake3 of `signed_bytes`.  `kind` and
    /// `text_preview` should be derived from the decoded `Message`.
    /// The entry starts with `event_id = 0` and `delivery_state = Queued`;
    /// use [`ChatHistoryStore::push_with_id`] to assign a proper id.
    pub fn new(
        topic: TopicId,
        sender: impl Into<String>,
        signed_bytes: Vec<u8>,
        kind: impl Into<String>,
        text_preview: impl Into<String>,
    ) -> Self {
        let hash = blake3_hex(&signed_bytes);
        Self {
            event_id: 0,
            hash,
            sender: sender.into(),
            timestamp: default_now(),
            kind: kind.into(),
            topic,
            text_preview: text_preview.into(),
            signed_bytes,
            delivery_state: DeliveryState::Queued,
            image_bytes: None,
            image_identifier: None,
        }
    }

    /// Short one-line summary for `/history` command output.
    pub fn summary(&self) -> String {
        let preview = if self.text_preview.len() > 60 {
            format!("{}…", &self.text_preview[..60])
        } else {
            self.text_preview.clone()
        };
        format!(
            "{:16} {:8} {}",
            &self.sender[..16.min(self.sender.len())],
            self.kind,
            preview
        )
    }
}

// ── Persistent store ───────────────────────────────────────────────────

/// In-memory chat message entries for the active process only.
///
/// The `data_dir` field is retained only to preserve the API used by the
/// frontends; it is never used for writing chat data.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatHistoryStore {
    /// Format version for future migrations.
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    /// All stored history entries, in insertion order (oldest first).
    pub entries: Vec<HistoryEntry>,
    /// Data directory used for load/save.
    #[serde(skip)]
    data_dir: PathBuf,
    /// Monotonically-increasing counter for stable [`HistoryEntry::event_id`]
    /// assignment.  Each call to [`push_with_id`](Self::push_with_id)
    /// advances this counter and assigns the next value.
    #[serde(default)]
    next_event_id: u64,
}

impl ChatHistoryStore {
    /// Create an empty store bound to a data directory.
    pub fn empty_at(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            entries: Vec::new(),
            data_dir: data_dir.into(),
            next_event_id: 1,
        }
    }

    /// Return the on-disk history file path.
    pub fn file_path(&self) -> PathBuf {
        history_file_path(&self.data_dir)
    }

    /// Load the persisted chat history, if present.
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Option<Self>> {
        let data_dir = data_dir.as_ref();
        let path = history_file_path(data_dir);
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path)
            .with_std_context(|_| format!("failed to read history file {}", path.display()))?;
        let mut store: Self = serde_json::from_str(&raw)
            .with_std_context(|_| format!("failed to parse history file {}", path.display()))?;
        if store.schema_version != SCHEMA_VERSION {
            return Err(n0_error::anyerr!(
                "unsupported history schema version {} in {}",
                store.schema_version,
                path.display()
            ));
        }
        store.data_dir = data_dir.to_path_buf();
        store.next_event_id = store
            .entries
            .iter()
            .map(|entry| entry.event_id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        Ok(Some(store))
    }

    /// Load history, falling back to an empty store on any failure.
    pub fn load_or_default(data_dir: impl AsRef<Path>) -> Self {
        let data_dir = data_dir.as_ref();
        match Self::load(data_dir) {
            Ok(Some(store)) => store,
            Ok(None) => Self::empty_at(data_dir),
            Err(err) => {
                eprintln!(
                    "warning: failed to load chat history from {}: {err}",
                    history_file_path(data_dir).display()
                );
                Self::empty_at(data_dir)
            }
        }
    }

    /// Persist chat history using an atomic replacement.
    pub fn save(&self) -> Result<PathBuf> {
        let data_dir = &self.data_dir;
        if data_dir.as_os_str().is_empty() {
            return Err(n0_error::anyerr!(
                "chat history store has no data directory bound to it",
            ));
        }
        let path = self.file_path();
        atomic_write_json(&path, self, "chat history")?;
        Ok(path)
    }

    /// Append a new entry.  Does **not** automatically save — call
    /// [`save`](Self::save) explicitly to persist.
    pub fn push(&mut self, entry: HistoryEntry) {
        self.entries.push(entry);
    }

    /// Append a new entry, assigning a stable [`event_id`](HistoryEntry::event_id).
    ///
    /// Returns the assigned event id.  The entry is initialised with
    /// [`delivery_state`](HistoryEntry::delivery_state) set to
    /// [`Queued`](DeliveryState::Queued) by [`HistoryEntry::new`]; call
    /// [`update_delivery_state`](Self::update_delivery_state) to advance it.
    ///
    /// Does **not** automatically save — call [`save`](Self::save)
    /// explicitly to persist.
    pub fn push_with_id(&mut self, mut entry: HistoryEntry) -> u64 {
        let id = self.next_event_id;
        self.next_event_id += 1;
        entry.event_id = id;
        self.entries.push(entry);
        id
    }

    /// Find an entry by its stable [`event_id`](HistoryEntry::event_id).
    pub fn get_by_event_id(&self, event_id: u64) -> Option<&HistoryEntry> {
        self.entries.iter().find(|e| e.event_id == event_id)
    }

    /// Find a mutable entry by its stable [`event_id`](HistoryEntry::event_id).
    pub fn get_by_event_id_mut(&mut self, event_id: u64) -> Option<&mut HistoryEntry> {
        self.entries.iter_mut().find(|e| e.event_id == event_id)
    }

    /// Advance the [`delivery_state`](HistoryEntry::delivery_state) of an
    /// entry identified by `event_id`.
    ///
    /// Returns an [`InvalidTransition`] error if the entry cannot be found
    /// or if the transition is not allowed by the state machine (see
    /// [`DeliveryState::can_transition_to`]).
    ///
    /// A no-op transition (current → current) is always accepted and
    /// returns `Ok(())` without modifying anything.
    pub fn update_delivery_state(
        &mut self,
        event_id: u64,
        new_state: DeliveryState,
    ) -> std::result::Result<(), InvalidTransition> {
        let entry = self
            .get_by_event_id_mut(event_id)
            .ok_or_else(|| InvalidTransition {
                event_id,
                current: DeliveryState::Queued,
                attempted: new_state.clone(),
            })?;
        if entry.delivery_state.can_transition_to(&new_state) {
            entry.delivery_state = new_state;
            Ok(())
        } else {
            Err(InvalidTransition {
                event_id,
                current: entry.delivery_state.clone(),
                attempted: new_state,
            })
        }
    }

    /// Return all entries (oldest first).
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Return the latest blob hash, or `None` if the history is empty.
    pub fn latest_hash(&self) -> Option<&str> {
        self.entries.last().map(|e| e.hash.as_str())
    }

    /// Number of stored entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Remove all entries for a topic.
    ///
    /// Returns the number of entries removed.
    pub fn remove_topic(&mut self, topic: &TopicId) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| e.topic != *topic);
        before - self.entries.len()
    }

    /// Return entries that are our own outgoing messages (Queued or Sent state)
    /// for a given topic, in insertion order.
    ///
    /// These are messages that should be replayed when reconnecting.
    pub fn get_outgoing_queue(&self, topic: &TopicId, local_hex: &str) -> Vec<&HistoryEntry> {
        self.entries
            .iter()
            .filter(|e| {
                e.topic == *topic
                    && e.sender == local_hex
                    && matches!(
                        e.delivery_state,
                        DeliveryState::Queued | DeliveryState::Sent
                    )
            })
            .collect()
    }

    /// Find an entry by its blake3 hex hash.
    pub fn find_by_hash(&self, hash: &str) -> Option<&HistoryEntry> {
        self.entries.iter().find(|e| e.hash == hash)
    }

    /// Filter entries by topic, returning matching entries in insertion
    /// order.
    pub fn for_topic(&self, topic: &TopicId) -> Vec<&HistoryEntry> {
        self.entries.iter().filter(|e| e.topic == *topic).collect()
    }

    /// Return up to `count` of the most recent entries for a given topic,
    /// oldest-first.
    ///
    /// Unlike [`for_topic`], this scans backwards from newest to oldest
    /// and stops once `count` matching entries have been found, making it
    /// suitable for backfill queries against large topic histories.
    pub fn get_recent_messages_for_topic(
        &self,
        topic: &TopicId,
        count: usize,
    ) -> Vec<&HistoryEntry> {
        if count == 0 {
            return Vec::new();
        }
        let mut result = Vec::with_capacity(count);
        for entry in self.entries.iter().rev() {
            if entry.topic == *topic {
                result.push(entry);
                if result.len() >= count {
                    break;
                }
            }
        }
        result.reverse(); // restore chronological order
        result
    }

    /// Count the total number of entries for a given topic.
    pub fn count_for_topic(&self, topic: &TopicId) -> usize {
        self.entries.iter().filter(|e| e.topic == *topic).count()
    }

    /// Return up to `count` of the most recent entries, oldest-first.
    pub fn get_recent_messages(&self, count: usize) -> Vec<&HistoryEntry> {
        if count == 0 {
            return Vec::new();
        }
        let start = self.entries.len().saturating_sub(count);
        self.entries[start..].iter().collect()
    }

    /// Return all entries whose delivery state matches the given state.
    ///
    /// Useful for finding messages that need to be replayed on reconnection
    /// (e.g. Queued or Sent messages that never got a Delivered confirmation).
    pub fn get_by_delivery_state(&self, state: DeliveryState) -> Vec<&HistoryEntry> {
        self.entries
            .iter()
            .filter(|e| e.delivery_state == state)
            .collect()
    }

    /// The path to the history file, for display purposes.
    pub fn display_path(&self) -> String {
        self.file_path().display().to_string()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        dir.push(format!("boru-chat-history-{name}-{suffix}"));
        dir
    }

    fn make_topic(byte: u8) -> TopicId {
        TopicId::from_bytes([byte; 32])
    }

    fn make_entry(topic: TopicId, idx: u8) -> HistoryEntry {
        let bytes = vec![idx; 64];
        HistoryEntry::new(
            topic,
            format!("sender{idx}"),
            bytes,
            "text",
            format!("hello {idx}"),
        )
    }

    // ── DeliveryState tests ─────────────────────────────────────────────

    #[test]
    fn delivery_state_queued_to_sent() {
        assert!(DeliveryState::Queued.can_transition_to(&DeliveryState::Sent));
    }

    #[test]
    fn delivery_state_queued_to_failed() {
        assert!(DeliveryState::Queued.can_transition_to(&DeliveryState::Failed));
    }

    #[test]
    fn delivery_state_sent_to_delivered() {
        assert!(DeliveryState::Sent.can_transition_to(&DeliveryState::Delivered));
    }

    #[test]
    fn delivery_state_sent_to_seen() {
        assert!(DeliveryState::Sent.can_transition_to(&DeliveryState::Seen));
    }

    #[test]
    fn delivery_state_sent_to_failed() {
        assert!(DeliveryState::Sent.can_transition_to(&DeliveryState::Failed));
    }

    #[test]
    fn delivery_state_delivered_to_seen() {
        assert!(DeliveryState::Delivered.can_transition_to(&DeliveryState::Seen));
    }

    #[test]
    fn delivery_state_delivered_to_failed() {
        assert!(DeliveryState::Delivered.can_transition_to(&DeliveryState::Failed));
    }

    #[test]
    fn delivery_state_identity_is_noop() {
        for state in &[
            DeliveryState::Queued,
            DeliveryState::Sent,
            DeliveryState::Delivered,
            DeliveryState::Seen,
            DeliveryState::Failed,
        ] {
            assert!(
                state.can_transition_to(state),
                "identity transition {:?} → {:?} should be valid",
                state,
                state,
            );
        }
    }

    // ── Invalid transitions ────────────────────────────────────────────

    #[test]
    fn delivery_state_queued_cannot_skip_sent() {
        // Queued → Delivered is invalid
        assert!(!DeliveryState::Queued.can_transition_to(&DeliveryState::Delivered));
        // Queued → Seen is invalid
        assert!(!DeliveryState::Queued.can_transition_to(&DeliveryState::Seen));
    }

    #[test]
    fn delivery_state_cannot_go_backwards() {
        assert!(!DeliveryState::Sent.can_transition_to(&DeliveryState::Queued));
        assert!(!DeliveryState::Delivered.can_transition_to(&DeliveryState::Sent));
        assert!(!DeliveryState::Delivered.can_transition_to(&DeliveryState::Queued));
        assert!(!DeliveryState::Seen.can_transition_to(&DeliveryState::Delivered));
        assert!(!DeliveryState::Seen.can_transition_to(&DeliveryState::Sent));
        assert!(!DeliveryState::Seen.can_transition_to(&DeliveryState::Queued));
    }

    #[test]
    fn delivery_state_terminal_states_are_final() {
        // Seen is terminal — no valid transitions (except identity)
        assert!(!DeliveryState::Seen.can_transition_to(&DeliveryState::Failed));
        assert!(!DeliveryState::Seen.can_transition_to(&DeliveryState::Sent));

        // Failed is terminal — no valid transitions (except identity)
        assert!(!DeliveryState::Failed.can_transition_to(&DeliveryState::Queued));
        assert!(!DeliveryState::Failed.can_transition_to(&DeliveryState::Sent));
        assert!(!DeliveryState::Failed.can_transition_to(&DeliveryState::Delivered));
        assert!(!DeliveryState::Failed.can_transition_to(&DeliveryState::Seen));
    }

    #[test]
    fn delivery_state_all_transitions_mapped() {
        // Explicitly enumerate every (from, to) pair to catch gaps.
        let states = [
            DeliveryState::Queued,
            DeliveryState::Sent,
            DeliveryState::Delivered,
            DeliveryState::Seen,
            DeliveryState::Failed,
        ];

        // Expected valid transitions (from → to): true means valid.
        // Columns: Queued, Sent, Delivered, Seen, Failed
        let expected: [[bool; 5]; 5] = [
            // from Queued
            [true, true, false, false, true],
            // from Sent
            [false, true, true, true, true],
            // from Delivered
            [false, false, true, true, true],
            // from Seen
            [false, false, false, true, false],
            // from Failed
            [false, false, false, false, true],
        ];

        for (i, from) in states.iter().enumerate() {
            for (j, to) in states.iter().enumerate() {
                let actual = from.can_transition_to(to);
                assert_eq!(
                    actual, expected[i][j],
                    "transition mismatch: {:?} → {:?} expected {} but got {}",
                    from, to, expected[i][j], actual,
                );
            }
        }
    }

    // ── InvalidTransition tests ─────────────────────────────────────────

    #[test]
    fn invalid_transition_display() {
        let err = InvalidTransition {
            event_id: 42,
            current: DeliveryState::Queued,
            attempted: DeliveryState::Delivered,
        };
        let msg = err.to_string();
        assert!(msg.contains("42"));
        assert!(msg.contains("Queued"));
        assert!(msg.contains("Delivered"));
    }

    #[test]
    fn invalid_transition_implements_error() {
        let err = InvalidTransition {
            event_id: 1,
            current: DeliveryState::Queued,
            attempted: DeliveryState::Seen,
        };
        // Should implement std::error::Error
        let _: &dyn std::error::Error = &err;
    }

    // ── Event ID tests ──────────────────────────────────────────────────

    #[test]
    fn push_with_id_assigns_monotonic_ids() {
        let dir = temp_dir("event_ids");
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0xAA);

        let id1 = store.push_with_id(make_entry(topic, 1));
        let id2 = store.push_with_id(make_entry(topic, 2));
        let id3 = store.push_with_id(make_entry(topic, 3));

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
        assert_eq!(store.entries.len(), 3);
    }

    #[test]
    fn push_with_id_starts_entry_at_queued() {
        let dir = temp_dir("starts_queued");
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0xBB);

        let id = store.push_with_id(make_entry(topic, 1));
        let entry = store.get_by_event_id(id).unwrap();
        assert_eq!(entry.delivery_state, DeliveryState::Queued);
        assert_eq!(entry.event_id, id);
    }

    #[test]
    fn event_ids_are_unique_across_entries() {
        let dir = temp_dir("unique_ids");
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0xCC);

        let mut ids = Vec::new();
        for i in 0..100 {
            ids.push(store.push_with_id(make_entry(topic, i)));
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 100);
    }

    #[test]
    fn get_by_event_id_finds_entry() {
        let dir = temp_dir("get_by_id");
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0xDD);

        store.push_with_id(make_entry(topic, 1));
        let id2 = store.push_with_id(make_entry(topic, 2));
        store.push_with_id(make_entry(topic, 3));

        let entry = store.get_by_event_id(id2).unwrap();
        assert_eq!(entry.event_id, id2);
        assert_eq!(entry.text_preview, "hello 2");
    }

    #[test]
    fn get_by_event_id_returns_none_for_missing() {
        let dir = temp_dir("missing_id");
        let store = ChatHistoryStore::empty_at(&dir);
        assert!(store.get_by_event_id(999).is_none());
    }

    #[test]
    fn get_by_event_id_mut_allows_mutation() {
        let dir = temp_dir("mut_by_id");
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0xEE);

        let id = store.push_with_id(make_entry(topic, 1));
        {
            let entry = store.get_by_event_id_mut(id).unwrap();
            entry.text_preview = "modified".to_string();
        }
        assert_eq!(store.get_by_event_id(id).unwrap().text_preview, "modified");
    }

    #[test]
    fn get_by_event_id_mut_returns_none_for_missing() {
        let dir = temp_dir("mut_missing");
        let mut store = ChatHistoryStore::empty_at(&dir);
        assert!(store.get_by_event_id_mut(999).is_none());
    }

    // ── update_delivery_state tests ─────────────────────────────────────

    #[test]
    fn update_delivery_state_valid_transition() {
        let dir = temp_dir("update_valid");
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0x11);

        let id = store.push_with_id(make_entry(topic, 1));

        // Queued → Sent
        store
            .update_delivery_state(id, DeliveryState::Sent)
            .unwrap();
        assert_eq!(
            store.get_by_event_id(id).unwrap().delivery_state,
            DeliveryState::Sent
        );

        // Sent → Delivered
        store
            .update_delivery_state(id, DeliveryState::Delivered)
            .unwrap();
        assert_eq!(
            store.get_by_event_id(id).unwrap().delivery_state,
            DeliveryState::Delivered
        );

        // Delivered → Seen
        store
            .update_delivery_state(id, DeliveryState::Seen)
            .unwrap();
        assert_eq!(
            store.get_by_event_id(id).unwrap().delivery_state,
            DeliveryState::Seen
        );
    }

    #[test]
    fn update_delivery_state_queued_to_failed() {
        let dir = temp_dir("queued_failed");
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0x22);

        let id = store.push_with_id(make_entry(topic, 1));
        store
            .update_delivery_state(id, DeliveryState::Failed)
            .unwrap();
        assert_eq!(
            store.get_by_event_id(id).unwrap().delivery_state,
            DeliveryState::Failed
        );
    }

    #[test]
    fn update_delivery_state_sent_to_failed() {
        let dir = temp_dir("sent_failed");
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0x33);

        let id = store.push_with_id(make_entry(topic, 1));
        store
            .update_delivery_state(id, DeliveryState::Sent)
            .unwrap();
        store
            .update_delivery_state(id, DeliveryState::Failed)
            .unwrap();
        assert_eq!(
            store.get_by_event_id(id).unwrap().delivery_state,
            DeliveryState::Failed
        );
    }

    #[test]
    fn update_delivery_state_delivered_to_failed() {
        let dir = temp_dir("delivered_failed");
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0x44);

        let id = store.push_with_id(make_entry(topic, 1));
        store
            .update_delivery_state(id, DeliveryState::Sent)
            .unwrap();
        store
            .update_delivery_state(id, DeliveryState::Delivered)
            .unwrap();
        store
            .update_delivery_state(id, DeliveryState::Failed)
            .unwrap();
        assert_eq!(
            store.get_by_event_id(id).unwrap().delivery_state,
            DeliveryState::Failed
        );
    }

    #[test]
    fn update_delivery_state_identity_is_ok() {
        let dir = temp_dir("identity_ok");
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0x55);

        let id = store.push_with_id(make_entry(topic, 1));
        // Re-applying Queued should be fine
        store
            .update_delivery_state(id, DeliveryState::Queued)
            .unwrap();
        assert_eq!(
            store.get_by_event_id(id).unwrap().delivery_state,
            DeliveryState::Queued
        );
    }

    #[test]
    fn update_delivery_state_rejects_backwards() {
        let dir = temp_dir("reject_back");
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0x66);

        let id = store.push_with_id(make_entry(topic, 1));
        store
            .update_delivery_state(id, DeliveryState::Sent)
            .unwrap();

        // Sent → Queued is invalid
        let err = store
            .update_delivery_state(id, DeliveryState::Queued)
            .unwrap_err();
        assert_eq!(err.event_id, id);
        assert_eq!(err.current, DeliveryState::Sent);
        assert_eq!(err.attempted, DeliveryState::Queued);
        // State should remain unchanged
        assert_eq!(
            store.get_by_event_id(id).unwrap().delivery_state,
            DeliveryState::Sent
        );
    }

    #[test]
    fn update_delivery_state_rejects_skip_sent() {
        let dir = temp_dir("reject_skip");
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0x77);

        let id = store.push_with_id(make_entry(topic, 1));

        // Queued → Delivered is invalid
        let err = store
            .update_delivery_state(id, DeliveryState::Delivered)
            .unwrap_err();
        assert_eq!(err.current, DeliveryState::Queued);
        assert_eq!(err.attempted, DeliveryState::Delivered);
        assert_eq!(
            store.get_by_event_id(id).unwrap().delivery_state,
            DeliveryState::Queued
        );
    }

    #[test]
    fn update_delivery_state_terminal_is_final() {
        let dir = temp_dir("terminal_final");
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0x88);

        let id = store.push_with_id(make_entry(topic, 1));
        store
            .update_delivery_state(id, DeliveryState::Sent)
            .unwrap();
        store
            .update_delivery_state(id, DeliveryState::Delivered)
            .unwrap();
        store
            .update_delivery_state(id, DeliveryState::Seen)
            .unwrap();

        // Seen → anything (except Seen) is invalid
        let err = store
            .update_delivery_state(id, DeliveryState::Failed)
            .unwrap_err();
        assert_eq!(err.current, DeliveryState::Seen);
        assert_eq!(err.attempted, DeliveryState::Failed);
        assert_eq!(
            store.get_by_event_id(id).unwrap().delivery_state,
            DeliveryState::Seen
        );

        // Failed is similarly terminal
        let id2 = store.push_with_id(make_entry(topic, 2));
        store
            .update_delivery_state(id2, DeliveryState::Failed)
            .unwrap();
        let err2 = store
            .update_delivery_state(id2, DeliveryState::Sent)
            .unwrap_err();
        assert_eq!(err2.current, DeliveryState::Failed);
    }

    #[test]
    fn update_delivery_state_unknown_event_id() {
        let dir = temp_dir("unknown_event");
        let mut store = ChatHistoryStore::empty_at(&dir);

        let err = store
            .update_delivery_state(999, DeliveryState::Sent)
            .unwrap_err();
        assert_eq!(err.event_id, 999);
        assert_eq!(err.attempted, DeliveryState::Sent);
    }

    #[test]
    fn update_delivery_state_does_not_affect_other_entries() {
        let dir = temp_dir("no_side_effects");
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0x99);

        let id1 = store.push_with_id(make_entry(topic, 1));
        let id2 = store.push_with_id(make_entry(topic, 2));
        let id3 = store.push_with_id(make_entry(topic, 3));

        store
            .update_delivery_state(id2, DeliveryState::Sent)
            .unwrap();
        store
            .update_delivery_state(id2, DeliveryState::Delivered)
            .unwrap();

        assert_eq!(
            store.get_by_event_id(id1).unwrap().delivery_state,
            DeliveryState::Queued
        );
        assert_eq!(
            store.get_by_event_id(id2).unwrap().delivery_state,
            DeliveryState::Delivered
        );
        assert_eq!(
            store.get_by_event_id(id3).unwrap().delivery_state,
            DeliveryState::Queued
        );
    }

    // ── Existing tests (unchanged) ──────────────────────────────────────

    #[test]
    fn load_missing_returns_empty() {
        let dir = temp_dir("missing");
        let store = ChatHistoryStore::load_or_default(&dir);
        assert!(store.is_empty());
    }

    #[test]
    fn load_roundtrip_preserves_entries() {
        let dir = temp_dir("roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = ChatHistoryStore::empty_at(&dir);

        let topic = make_topic(0xAA);
        store.push(make_entry(topic, 1));
        store.push(make_entry(topic, 2));
        store.save().expect("save");

        let loaded = ChatHistoryStore::load_or_default(&dir);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.entries()[0].text_preview, "hello 1");
    }

    #[test]
    fn latest_hash_works() {
        let dir = temp_dir("latest_hash");
        let mut store = ChatHistoryStore::empty_at(&dir);
        assert!(store.latest_hash().is_none());

        let topic = make_topic(0xBB);
        store.push(make_entry(topic, 1));
        assert!(store.latest_hash().is_some());
    }

    #[test]
    fn clear_removes_all() {
        let dir = temp_dir("clear");
        let mut store = ChatHistoryStore::empty_at(&dir);

        let topic = make_topic(0xCC);
        store.push(make_entry(topic, 1));
        store.push(make_entry(topic, 2));
        assert_eq!(store.len(), 2);

        store.clear();
        assert!(store.is_empty());
    }

    #[test]
    fn remove_topic_removes_matching_entries() {
        let dir = temp_dir("remove_topic");
        let mut store = ChatHistoryStore::empty_at(&dir);

        let ta = make_topic(0xAA);
        let tb = make_topic(0xBB);
        store.push(make_entry(ta, 1));
        store.push(make_entry(tb, 2));
        store.push(make_entry(ta, 3));

        store.remove_topic(&ta);
        assert_eq!(store.len(), 1);
        assert_eq!(store.entries[0].topic, tb);
    }

    #[test]
    fn for_topic_filters_correctly() {
        let dir = temp_dir("filter");
        let mut store = ChatHistoryStore::empty_at(&dir);

        let ta = make_topic(0xAA);
        let tb = make_topic(0xBB);
        store.push(make_entry(ta, 1));
        store.push(make_entry(tb, 2));
        store.push(make_entry(ta, 3));

        let a_entries = store.for_topic(&ta);
        assert_eq!(a_entries.len(), 2);
        assert_eq!(a_entries[0].text_preview, "hello 1");
        assert_eq!(a_entries[1].text_preview, "hello 3");

        let b_entries = store.for_topic(&tb);
        assert_eq!(b_entries.len(), 1);
    }

    #[test]
    fn blake3_hex_is_deterministic() {
        let data = b"hello world";
        let h1 = blake3_hex(data);
        let h2 = blake3_hex(data);
        assert_eq!(h1, h2);
    }

    #[test]
    fn blake3_hex_differs_for_diff_data() {
        let h1 = blake3_hex(b"hello");
        let h2 = blake3_hex(b"world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn save_persists_entries() {
        let dir = temp_dir("atomic");
        let topic = make_topic(0xAA);
        let mut store = ChatHistoryStore::empty_at(&dir);

        store.push(make_entry(topic, 1));
        let path = store.save().expect("first save");
        assert_eq!(path, dir.join(HISTORY_FILE_NAME));
        assert!(path.exists());

        store.push(make_entry(topic, 2));
        store.save().expect("second save");
        let loaded = ChatHistoryStore::load_or_default(&dir);
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn hash_is_computed_from_bytes() {
        let data = b"test message";
        let topic = make_topic(0x01);
        let entry = HistoryEntry::new(topic, "alice", data.to_vec(), "text", "test message");
        assert_eq!(entry.hash, blake3_hex(data));
    }

    #[test]
    fn new_entry_defaults_to_queued() {
        let topic = make_topic(0x02);
        let entry = HistoryEntry::new(topic, "bob", vec![1, 2, 3], "text", "default test");
        assert_eq!(entry.delivery_state, DeliveryState::Queued);
        assert_eq!(entry.event_id, 0);
    }

    // ── Image identifier persistence tests ─────────────────────────────

    #[test]
    fn image_identifier_is_preserved_across_save_and_load() {
        let dir = temp_dir("image_id_persist");
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0xF0);

        let mut entry = make_entry(topic, 1);
        entry.image_identifier = Some("abc123hash/def456hash.png".to_string());
        entry.image_bytes = Some(vec![1, 2, 3]);
        store.push(entry);
        store.save().expect("save");

        let loaded = ChatHistoryStore::load_or_default(&dir);
        assert_eq!(loaded.len(), 1);
        let loaded_entry = &loaded.entries()[0];
        assert_eq!(
            loaded_entry.image_identifier,
            Some("abc123hash/def456hash.png".to_string())
        );
        // image_bytes is intentionally skipped in serde to prevent multi-megabyte JSON files.
        assert_eq!(loaded_entry.image_bytes, None);
    }

    #[test]
    fn image_identifier_is_none_by_default() {
        let topic = make_topic(0xF1);
        let entry = HistoryEntry::new(topic, "carol", vec![], "text", "no image");
        assert!(entry.image_identifier.is_none());
        assert!(entry.image_bytes.is_none());
    }

    #[test]
    fn image_identifier_not_written_when_none() {
        let dir = temp_dir("image_id_none");
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0xF2);

        store.push(make_entry(topic, 1));
        store.push(make_entry(topic, 2));
        store.save().expect("save");

        let raw = std::fs::read_to_string(dir.join(HISTORY_FILE_NAME)).unwrap();
        assert!(
            !raw.contains("image_identifier"),
            "JSON should not contain image_identifier key when None: {raw}"
        );
    }

    #[test]
    fn image_identifier_written_when_some() {
        let dir = temp_dir("image_id_some");
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = ChatHistoryStore::empty_at(&dir);
        let topic = make_topic(0xF3);

        let mut entry = make_entry(topic, 1);
        entry.image_identifier = Some("hash1/hash2.jpg".to_string());
        store.push(entry);
        store.save().expect("save");

        let raw = std::fs::read_to_string(dir.join(HISTORY_FILE_NAME)).unwrap();
        assert!(
            raw.contains("image_identifier"),
            "JSON should contain image_identifier key when Some"
        );
        assert!(raw.contains("hash1/hash2.jpg"));
    }
}
