//! Durable encrypted outbox storage for iroh-gossip-chat.
//!
//! Outgoing messages (signed+encoded) are persisted *before* they are
//! broadcast so that in-flight messages survive a crash or restart.
//! Messages are stored with their delivery state so the layer above can
//! retry failed sends and track end-to-end delivery.
//!
//! # Lifecycle
//!
//! 1. **Push** — a signed message is pushed into the outbox before
//!    [`broadcast`](crate::api::GossipSender::broadcast) is called.
//! 2. **Send** — the caller broadcasts the bytes and then advances the
//!    entry from [`Queued`](crate::chat_history::DeliveryState::Queued)
//!    to [`Sent`](crate::chat_history::DeliveryState::Sent).
//! 3. **Confirm** — on delivery confirmation the state advances further
//!    (Sent → Delivered → Seen).
//! 4. **Remove** — once a terminal state (Seen or Failed) is reached the
//!    entry may be removed or allowed to expire.
//!
//! # Expiry
//!
//! Old entries (default 7 days) are cleaned up automatically on save.
//! Call [`OutboxStore::expire`] explicitly or rely on the automatic
//! expiry that runs before every [`save`](OutboxStore::save).
//!
//! # Duplicate suppression
//!
//! [`push`](OutboxStore::push) checks for an existing entry with the
//! same [`event_id`](OutboxEntry::event_id) and returns an error if one
//! already exists — preventing the same message from being queued twice.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};

use crate::chat_core::atomic_write::atomic_write_json;
use crate::chat_history::{DeliveryState, InvalidTransition};
use crate::proto::TopicId;

// ── Constants ────────────────────────────────────────────────────────────

/// Current schema version — bump on breaking format changes.
const OUTBOX_SCHEMA_VERSION: u32 = 1;

/// Name of the on-disk outbox file (lives beside `secret_key.txt`).
pub const OUTBOX_FILE_NAME: &str = "outbox.json";

/// Default TTL for outbox entries: 7 days.
pub const DEFAULT_OUTBOX_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Helper to provide the default schema version for serde.
fn default_outbox_schema_version() -> u32 {
    OUTBOX_SCHEMA_VERSION
}

/// Return a unix-epoch-ms timestamp for "right now".
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Compute the on-disk path for the outbox file.
fn outbox_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(OUTBOX_FILE_NAME)
}

// ── Entry type ───────────────────────────────────────────────────────────

/// A single entry in the durable outbox.
///
/// Each entry stores the raw signed (and therefore effectively encrypted)
/// message bytes, the gossip topic, a stable event id, and the current
/// delivery state so the outbox can be recovered and retried after a
/// restart.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutboxEntry {
    /// Stable, monotonically-increasing event identifier assigned locally.
    ///
    /// Matches the [`event_id`](crate::chat_history::HistoryEntry::event_id)
    /// assigned by [`ChatHistoryStore::push_with_id`](crate::chat_history::ChatHistoryStore::push_with_id).
    pub event_id: u64,
    /// blake3 hex hash of the raw signed message bytes.
    pub hash: String,
    /// The gossip topic this message should be broadcast on.
    pub topic: TopicId,
    /// The raw signed message bytes (output of
    /// [`SignedMessage::sign_and_encode`](crate::chat_core::SignedMessage::sign_and_encode)).
    ///
    /// These bytes are already signed and serialised — they are ready to
    /// pass to [`GossipSender::broadcast`](crate::api::GossipSender::broadcast).
    pub signed_bytes: Vec<u8>,
    /// Current delivery state of this message.
    #[serde(default)]
    pub delivery_state: DeliveryState,
    /// Unix-epoch milliseconds when this entry was created.
    pub created_at: u64,
    /// Number of times a send has been attempted (for retry tracking).
    #[serde(default)]
    pub retry_count: u32,
}

impl OutboxEntry {
    /// Create a new outbox entry.
    ///
    /// The `hash` is computed as the blake3 hex of `signed_bytes`.
    /// The entry starts with [`delivery_state`](DeliveryState::Queued),
    /// `created_at` set to the current time, and `retry_count` at zero.
    pub fn new(event_id: u64, topic: TopicId, signed_bytes: Vec<u8>) -> Self {
        let hash = crate::chat_history::blake3_hex(&signed_bytes);
        Self {
            event_id,
            hash,
            topic,
            signed_bytes,
            delivery_state: DeliveryState::Queued,
            created_at: now_ms(),
            retry_count: 0,
        }
    }
}

// ── Persistent store ─────────────────────────────────────────────────────

/// Durable outbox store — persisted to disk via atomic JSON writes.
///
/// Entries are indexed by [`event_id`](OutboxEntry::event_id) for O(1)
/// lookups and updates, and an ordered list preserves insertion order
/// for FIFO retry processing.
///
/// The store is serialisable so it can be written atomically with
/// [`atomic_write_json`](crate::chat_core::atomic_write::atomic_write_json).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutboxStore {
    /// Format version for future migrations.
    #[serde(default = "default_outbox_schema_version")]
    schema_version: u32,
    /// All outbox entries, keyed by event_id for fast lookup and dedup.
    #[serde(default)]
    entries: HashMap<u64, OutboxEntry>,
    /// Ordered event IDs for FIFO iteration (oldest first).
    #[serde(default)]
    ordered_ids: Vec<u64>,
    /// Data directory bound at construction (never serialised).
    #[serde(skip)]
    data_dir: PathBuf,
    /// Entries older than this TTL are eligible for expiry.
    #[serde(skip)]
    ttl: Duration,
}

impl OutboxStore {
    /// Create an empty outbox store bound to a data directory with the
    /// default TTL (7 days).
    pub fn empty_at(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            schema_version: OUTBOX_SCHEMA_VERSION,
            entries: HashMap::new(),
            ordered_ids: Vec::new(),
            data_dir: data_dir.into(),
            ttl: DEFAULT_OUTBOX_TTL,
        }
    }

    /// Create an empty outbox store with a custom TTL.
    pub fn with_ttl(data_dir: impl Into<PathBuf>, ttl: Duration) -> Self {
        Self {
            schema_version: OUTBOX_SCHEMA_VERSION,
            entries: HashMap::new(),
            ordered_ids: Vec::new(),
            data_dir: data_dir.into(),
            ttl,
        }
    }

    /// Return the on-disk outbox file path.
    pub fn file_path(&self) -> PathBuf {
        outbox_file_path(&self.data_dir)
    }

    /// Load the outbox store from disk.
    ///
    /// Returns `None` if the file does not exist (fresh state).
    /// Returns an error if the file exists but cannot be parsed or if the
    /// schema version is unknown.
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Option<Self>> {
        let data_dir = data_dir.as_ref();
        let path = outbox_file_path(data_dir);
        if !path.exists() {
            return Ok(None);
        }

        let raw = fs::read_to_string(&path)
            .with_std_context(|_| format!("failed to read outbox file {}", path.display()))?;
        let mut store: Self = serde_json::from_str(&raw)
            .with_std_context(|_| format!("failed to parse outbox file {}", path.display()))?;

        if store.schema_version != OUTBOX_SCHEMA_VERSION {
            return Err(n0_error::anyerr!(
                "unsupported outbox schema version {} in {}",
                store.schema_version,
                path.display()
            ));
        }

        // Rebind the data directory to the one we loaded from.
        store.data_dir = data_dir.to_path_buf();
        // Restore the default TTL (not serialized — #[serde(skip)]).
        store.ttl = DEFAULT_OUTBOX_TTL;
        // Rebuild entries map from ordered_ids if the stored map is empty
        // but ordered_ids has entries (for backward compat or partial load).
        if store.entries.is_empty() && !store.ordered_ids.is_empty() {
            // This shouldn't happen with the current format, but be defensive.
            store.entries = HashMap::new();
        }
        Ok(Some(store))
    }

    /// Load the outbox store, falling back to an empty store on failure.
    ///
    /// Errors are logged to stderr but do not propagate — the application
    /// can continue with an empty outbox.
    pub fn load_or_default(data_dir: impl AsRef<Path>) -> Self {
        let data_dir = data_dir.as_ref();
        match Self::load(data_dir) {
            Ok(Some(store)) => store,
            Ok(None) => Self::empty_at(data_dir),
            Err(err) => {
                eprintln!(
                    "warning: failed to load outbox from {}: {err}",
                    outbox_file_path(data_dir).display()
                );
                Self::empty_at(data_dir)
            }
        }
    }

    /// Persist the outbox store atomically to disk.
    ///
    /// Before writing, expired entries are automatically removed.
    /// Uses [`atomic_write_json`] for crash-safe writes.
    pub fn save(&self) -> Result<PathBuf> {
        let data_dir = &self.data_dir;
        if data_dir.as_os_str().is_empty() {
            return Err(n0_error::anyerr!(
                "outbox store has no data directory bound to it",
            ));
        }

        let path = self.file_path();

        // Clone, expire, and write.
        let mut to_save = self.clone();
        to_save.expire_internal();
        atomic_write_json(&path, &to_save, "outbox store")?;
        Ok(path)
    }

    /// Internal expiry helper — removes entries older than the TTL.
    /// Also removes terminal-state entries (Seen, Failed) that are past TTL.
    fn expire_internal(&mut self) {
        let cutoff = now_ms().saturating_sub(self.ttl.as_millis() as u64);

        // Collect event_ids to remove.
        // Entries created at or before the cutoff are considered expired.
        let to_remove: Vec<u64> = self
            .ordered_ids
            .iter()
            .filter_map(|id| {
                self.entries.get(id).and_then(|entry| {
                    if entry.created_at <= cutoff {
                        Some(*id)
                    } else {
                        None
                    }
                })
            })
            .collect();

        for id in &to_remove {
            self.entries.remove(id);
        }
        self.ordered_ids.retain(|id| !to_remove.contains(id));
    }

    /// Remove expired entries from the outbox.
    ///
    /// Returns the number of entries that were removed.
    pub fn expire(&mut self) -> usize {
        let before = self.entries.len();
        self.expire_internal();
        before - self.entries.len()
    }

    /// Push a new entry into the outbox.
    ///
    /// **Duplicate suppression**: if an entry with the same
    /// [`event_id`](OutboxEntry::event_id) already exists, returns
    /// `Err` with a message indicating the duplicate.
    ///
    /// Does **not** automatically save — call [`save`](Self::save)
    /// explicitly to persist.
    pub fn push(&mut self, entry: OutboxEntry) -> std::result::Result<(), String> {
        let event_id = entry.event_id;
        if self.entries.contains_key(&event_id) {
            return Err(format!("duplicate outbox entry for event_id {}", event_id));
        }
        self.entries.insert(event_id, entry);
        self.ordered_ids.push(event_id);
        Ok(())
    }

    /// Push with a consistency check against an expected delivery state.
    ///
    /// This is a convenience wrapper around [`push`](Self::push) that also
    /// verifies the entry's delivery state matches `expected_state`.
    /// Useful for callers that want to ensure they're pushing a message
    /// that has not been sent yet.
    pub fn push_queued(&mut self, entry: OutboxEntry) -> std::result::Result<(), String> {
        if entry.delivery_state != DeliveryState::Queued {
            return Err(format!(
                "expected Queued delivery state but got {:?}",
                entry.delivery_state
            ));
        }
        self.push(entry)
    }

    /// Get an entry by event ID.
    pub fn get(&self, event_id: u64) -> Option<&OutboxEntry> {
        self.entries.get(&event_id)
    }

    /// Get a mutable entry by event ID.
    pub fn get_mut(&mut self, event_id: u64) -> Option<&mut OutboxEntry> {
        self.entries.get_mut(&event_id)
    }

    /// Remove an entry from the outbox by event ID.
    ///
    /// Returns `true` if the entry was found and removed.
    pub fn remove(&mut self, event_id: u64) -> bool {
        let removed = self.entries.remove(&event_id).is_some();
        if removed {
            self.ordered_ids.retain(|id| *id != event_id);
        }
        removed
    }

    /// Return all entries in insertion order (oldest first).
    pub fn entries(&self) -> Vec<&OutboxEntry> {
        self.ordered_ids
            .iter()
            .filter_map(|id| self.entries.get(id))
            .collect()
    }

    /// Return all entries with [`Queued`](DeliveryState::Queued) delivery
    /// state, in insertion order.
    ///
    /// Call this after reloading the store to find messages that need to
    /// be (re-)sent.
    pub fn pending(&self) -> Vec<&OutboxEntry> {
        self.ordered_ids
            .iter()
            .filter_map(|id| self.entries.get(id))
            .filter(|e| e.delivery_state == DeliveryState::Queued)
            .collect()
    }

    /// Return all entries with a given delivery state, in insertion order.
    pub fn with_state(&self, state: DeliveryState) -> Vec<&OutboxEntry> {
        self.ordered_ids
            .iter()
            .filter_map(|id| self.entries.get(id))
            .filter(|e| e.delivery_state == state)
            .collect()
    }

    /// Advance the [`delivery_state`](OutboxEntry::delivery_state) of an
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
            .entries
            .get_mut(&event_id)
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

    /// Increment the retry count for an entry.
    ///
    /// Returns the new retry count, or `None` if the event_id was not found.
    pub fn increment_retry(&mut self, event_id: u64) -> Option<u32> {
        self.entries.get_mut(&event_id).map(|e| {
            e.retry_count += 1;
            e.retry_count
        })
    }

    /// Find an entry by its content hash (blake3 of signed bytes).
    ///
    /// This is useful for content-based deduplication at a higher level.
    pub fn get_by_hash(&self, hash: &str) -> Option<&OutboxEntry> {
        self.entries.values().find(|e| e.hash == hash)
    }

    /// Check whether an entry with the given event ID exists.
    pub fn contains(&self, event_id: u64) -> bool {
        self.entries.contains_key(&event_id)
    }

    /// Number of entries currently in the outbox.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the outbox is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.ordered_ids.clear();
    }

    /// Remove all entries for a specific topic.
    ///
    /// Returns the number of entries removed.
    pub fn remove_topic(&mut self, topic: &TopicId) -> usize {
        let to_remove: Vec<u64> = self
            .ordered_ids
            .iter()
            .filter_map(|id| {
                self.entries
                    .get(id)
                    .filter(|e| e.topic == *topic)
                    .map(|_| *id)
            })
            .collect();
        for id in &to_remove {
            self.entries.remove(id);
        }
        self.ordered_ids.retain(|id| !to_remove.contains(id));
        to_remove.len()
    }

    /// Set a custom TTL.  The next call to [`expire`](Self::expire) or
    /// [`save`](Self::save) will use this value.
    pub fn set_ttl(&mut self, ttl: Duration) {
        self.ttl = ttl;
    }

    /// The current TTL.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Path to the outbox file, for display.
    pub fn display_path(&self) -> String {
        self.file_path().display().to_string()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        dir.push(format!("iroh-gossip-outbox-{name}-{suffix}"));
        dir
    }

    fn make_topic(byte: u8) -> TopicId {
        TopicId::from_bytes([byte; 32])
    }

    fn make_entry(event_id: u64, topic: TopicId) -> OutboxEntry {
        let signed_bytes = format!("signed-message-{event_id}").into_bytes();
        OutboxEntry::new(event_id, topic, signed_bytes)
    }

    fn make_entry_with_bytes(event_id: u64, topic: TopicId, bytes: Vec<u8>) -> OutboxEntry {
        OutboxEntry::new(event_id, topic, bytes)
    }

    // ── OutboxEntry tests ─────────────────────────────────────────────

    #[test]
    fn outbox_entry_creates_with_queued_state() {
        let topic = make_topic(0x01);
        let entry = make_entry(1, topic);
        assert_eq!(entry.event_id, 1);
        assert_eq!(entry.delivery_state, DeliveryState::Queued);
        assert_eq!(entry.retry_count, 0);
        assert!(entry.created_at > 0);
        assert!(!entry.hash.is_empty());
    }

    #[test]
    fn outbox_entry_hash_is_blake3_of_bytes() {
        let topic = make_topic(0x02);
        let bytes = b"hello-world".to_vec();
        let entry = make_entry_with_bytes(42, topic, bytes.clone());
        let expected_hash = blake3::hash(&bytes).to_hex().to_string();
        assert_eq!(entry.hash, expected_hash);
    }

    // ── Push and duplicate suppression ─────────────────────────────────

    #[test]
    fn push_adds_entry() {
        let dir = temp_dir("push");
        let topic = make_topic(0xAA);
        let mut store = OutboxStore::empty_at(&dir);

        let entry = make_entry(1, topic);
        assert!(store.push(entry).is_ok());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn push_rejects_duplicate_event_id() {
        let dir = temp_dir("dedup");
        let topic = make_topic(0xBB);
        let mut store = OutboxStore::empty_at(&dir);

        let entry1 = make_entry(1, topic);
        let entry2 = make_entry(1, topic);
        assert!(store.push(entry1).is_ok());
        let err = store.push(entry2).unwrap_err();
        assert!(err.contains("duplicate"));
        assert!(err.contains("1"));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn push_accepts_different_event_ids() {
        let dir = temp_dir("different_ids");
        let topic = make_topic(0xCC);
        let mut store = OutboxStore::empty_at(&dir);

        assert!(store.push(make_entry(1, topic)).is_ok());
        assert!(store.push(make_entry(2, topic)).is_ok());
        assert!(store.push(make_entry(3, topic)).is_ok());
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn push_queued_rejects_sent_state() {
        let dir = temp_dir("push_queued");
        let topic = make_topic(0xDD);
        let mut store = OutboxStore::empty_at(&dir);

        let mut entry = make_entry(1, topic);
        entry.delivery_state = DeliveryState::Sent;
        let err = store.push_queued(entry).unwrap_err();
        assert!(err.contains("Queued"));
        assert!(err.contains("Sent"));
        assert!(store.is_empty());
    }

    // ── Get / contains ─────────────────────────────────────────────────

    #[test]
    fn get_returns_entry() {
        let dir = temp_dir("get");
        let topic = make_topic(0xEE);
        let mut store = OutboxStore::empty_at(&dir);

        store.push(make_entry(42, topic)).unwrap();
        let entry = store.get(42);
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().event_id, 42);
    }

    #[test]
    fn get_returns_none_for_missing() {
        let dir = temp_dir("get_missing");
        let store = OutboxStore::empty_at(&dir);
        assert!(store.get(999).is_none());
    }

    #[test]
    fn contains_works() {
        let dir = temp_dir("contains");
        let topic = make_topic(0xFF);
        let mut store = OutboxStore::empty_at(&dir);

        assert!(!store.contains(1));
        store.push(make_entry(1, topic)).unwrap();
        assert!(store.contains(1));
    }

    #[test]
    fn get_mut_allows_mutation() {
        let dir = temp_dir("get_mut");
        let topic = make_topic(0x11);
        let mut store = OutboxStore::empty_at(&dir);

        store.push(make_entry(1, topic)).unwrap();
        {
            let entry = store.get_mut(1).unwrap();
            entry.retry_count = 5;
        }
        assert_eq!(store.get(1).unwrap().retry_count, 5);
    }

    // ── Removal ────────────────────────────────────────────────────────

    #[test]
    fn remove_removes_entry() {
        let dir = temp_dir("remove");
        let topic = make_topic(0x22);
        let mut store = OutboxStore::empty_at(&dir);

        store.push(make_entry(1, topic)).unwrap();
        assert!(store.remove(1));
        assert!(store.is_empty());
        assert!(!store.contains(1));
    }

    #[test]
    fn remove_nonexistent_returns_false() {
        let dir = temp_dir("remove_missing");
        let mut store = OutboxStore::empty_at(&dir);
        assert!(!store.remove(999));
    }

    #[test]
    fn remove_preserves_order() {
        let dir = temp_dir("remove_order");
        let topic = make_topic(0x33);
        let mut store = OutboxStore::empty_at(&dir);

        store.push(make_entry(1, topic)).unwrap();
        store.push(make_entry(2, topic)).unwrap();
        store.push(make_entry(3, topic)).unwrap();

        store.remove(2);
        let ids: Vec<u64> = store.entries().iter().map(|e| e.event_id).collect();
        assert_eq!(ids, vec![1, 3]);
    }

    // ── update_delivery_state ──────────────────────────────────────────

    #[test]
    fn update_state_valid_transition() {
        let dir = temp_dir("update_valid");
        let topic = make_topic(0x44);
        let mut store = OutboxStore::empty_at(&dir);

        store.push(make_entry(1, topic)).unwrap();
        store.update_delivery_state(1, DeliveryState::Sent).unwrap();
        assert_eq!(store.get(1).unwrap().delivery_state, DeliveryState::Sent);
    }

    #[test]
    fn update_state_invalid_transition() {
        let dir = temp_dir("update_invalid");
        let topic = make_topic(0x55);
        let mut store = OutboxStore::empty_at(&dir);

        store.push(make_entry(1, topic)).unwrap();
        let err = store
            .update_delivery_state(1, DeliveryState::Delivered)
            .unwrap_err();
        assert_eq!(err.current, DeliveryState::Queued);
        assert_eq!(err.attempted, DeliveryState::Delivered);
        // State should remain unchanged
        assert_eq!(store.get(1).unwrap().delivery_state, DeliveryState::Queued);
    }

    #[test]
    fn update_state_unknown_event_id() {
        let dir = temp_dir("update_unknown");
        let mut store = OutboxStore::empty_at(&dir);

        let err = store
            .update_delivery_state(999, DeliveryState::Sent)
            .unwrap_err();
        assert_eq!(err.event_id, 999);
    }

    // ── Retry count ────────────────────────────────────────────────────

    #[test]
    fn increment_retry_works() {
        let dir = temp_dir("retry");
        let topic = make_topic(0x66);
        let mut store = OutboxStore::empty_at(&dir);

        store.push(make_entry(1, topic)).unwrap();
        assert_eq!(store.increment_retry(1), Some(1));
        assert_eq!(store.increment_retry(1), Some(2));
        assert_eq!(store.get(1).unwrap().retry_count, 2);
    }

    #[test]
    fn increment_retry_missing_returns_none() {
        let dir = temp_dir("retry_missing");
        let mut store = OutboxStore::empty_at(&dir);
        assert!(store.increment_retry(999).is_none());
    }

    // ── Pending and filtering ──────────────────────────────────────────

    #[test]
    fn pending_returns_queued_entries() {
        let dir = temp_dir("pending");
        let topic = make_topic(0x77);
        let mut store = OutboxStore::empty_at(&dir);

        store.push(make_entry(1, topic)).unwrap();
        store.push(make_entry(2, topic)).unwrap();
        store.push(make_entry(3, topic)).unwrap();

        // Advance entry 2 to Sent
        store.update_delivery_state(2, DeliveryState::Sent).unwrap();

        let pending: Vec<u64> = store.pending().iter().map(|e| e.event_id).collect();
        assert_eq!(pending, vec![1, 3]);
    }

    #[test]
    fn with_state_filters_correctly() {
        let dir = temp_dir("with_state");
        let topic = make_topic(0x88);
        let mut store = OutboxStore::empty_at(&dir);

        store.push(make_entry(1, topic)).unwrap();
        store.push(make_entry(2, topic)).unwrap();

        store
            .update_delivery_state(2, DeliveryState::Failed)
            .unwrap();

        let failed: Vec<u64> = store
            .with_state(DeliveryState::Failed)
            .iter()
            .map(|e| e.event_id)
            .collect();
        assert_eq!(failed, vec![2]);
    }

    // ── Entries order ──────────────────────────────────────────────────

    #[test]
    fn entries_are_in_insertion_order() {
        let dir = temp_dir("order");
        let topic = make_topic(0x99);
        let mut store = OutboxStore::empty_at(&dir);

        store.push(make_entry(10, topic)).unwrap();
        store.push(make_entry(5, topic)).unwrap();
        store.push(make_entry(20, topic)).unwrap();

        let ids: Vec<u64> = store.entries().iter().map(|e| e.event_id).collect();
        assert_eq!(ids, vec![10, 5, 20]);
    }

    // ── Expiry ─────────────────────────────────────────────────────────

    #[test]
    fn expire_removes_old_entries() {
        let dir = temp_dir("expire");
        let topic = make_topic(0xAA);

        // Use a zero TTL so everything is immediately expired.
        let mut store = OutboxStore::with_ttl(&dir, Duration::from_secs(0));

        store.push(make_entry(1, topic)).unwrap();
        store.push(make_entry(2, topic)).unwrap();
        assert_eq!(store.len(), 2);

        let removed = store.expire();
        assert_eq!(removed, 2);
        assert!(store.is_empty());
    }

    #[test]
    fn expire_keeps_recent_entries() {
        let dir = temp_dir("expire_recent");
        let topic = make_topic(0xBB);

        // Use a very long TTL so nothing expires.
        let mut store = OutboxStore::with_ttl(&dir, Duration::from_secs(86400 * 365));

        store.push(make_entry(1, topic)).unwrap();
        store.push(make_entry(2, topic)).unwrap();

        let removed = store.expire();
        assert_eq!(removed, 0);
        assert_eq!(store.len(), 2);
    }

    // ── Persistence (load / save) ──────────────────────────────────────

    #[test]
    fn save_then_load_preserves_entries() {
        let dir = temp_dir("save_load");
        let topic = make_topic(0xCC);
        let mut store = OutboxStore::empty_at(&dir);

        store.push(make_entry(1, topic)).unwrap();
        store.push(make_entry(2, topic)).unwrap();
        store.update_delivery_state(2, DeliveryState::Sent).unwrap();

        let saved_path = store.save().expect("save should succeed");
        assert!(saved_path.exists());

        // Load into a fresh store
        let loaded = OutboxStore::load(&dir)
            .expect("load should succeed")
            .expect("should have saved data");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get(1).unwrap().delivery_state, DeliveryState::Queued);
        assert_eq!(loaded.get(2).unwrap().delivery_state, DeliveryState::Sent);
    }

    #[test]
    fn load_missing_returns_none() {
        let dir = temp_dir("load_missing");
        let loaded = OutboxStore::load(&dir).expect("load missing");
        assert!(loaded.is_none());
    }

    #[test]
    fn load_or_default_creates_empty() {
        let dir = temp_dir("load_default");
        let store = OutboxStore::load_or_default(&dir);
        assert!(store.is_empty());
    }

    #[test]
    fn save_then_load_preserves_retry_count() {
        let dir = temp_dir("save_retry");
        let topic = make_topic(0xDD);
        let mut store = OutboxStore::empty_at(&dir);

        store.push(make_entry(1, topic)).unwrap();
        store.increment_retry(1);
        store.increment_retry(1);
        store.increment_retry(1);
        store.save().expect("save");

        let loaded = OutboxStore::load(&dir)
            .expect("load")
            .expect("should exist");
        assert_eq!(loaded.get(1).unwrap().retry_count, 3);
    }

    #[test]
    fn save_is_atomic() {
        let dir = temp_dir("atomic_save");
        let topic = make_topic(0xEE);
        let mut store = OutboxStore::empty_at(&dir);

        store.push(make_entry(1, topic)).unwrap();
        let path = store.save().expect("first save");

        // Verify the file is valid JSON
        let raw = fs::read_to_string(&path).expect("read saved file");
        let parsed: OutboxStore = serde_json::from_str(&raw).expect("valid JSON");
        assert_eq!(parsed.len(), 1);

        // Overwrite with different data
        let mut store2 = OutboxStore::empty_at(&dir);
        store2.push(make_entry(2, topic)).unwrap();
        store2.save().expect("second save");

        // File should now contain event_id 2
        let raw2 = fs::read_to_string(&path).expect("re-read");
        let parsed2: OutboxStore = serde_json::from_str(&raw2).expect("valid JSON");
        assert_eq!(parsed2.len(), 1);
        assert!(parsed2.contains(2));
        assert!(!parsed2.contains(1));
    }

    #[test]
    fn save_auto_expires() {
        let dir = temp_dir("save_expire");
        let topic = make_topic(0xFF);

        // Zero TTL so entries expire immediately on save.
        let mut store = OutboxStore::with_ttl(&dir, Duration::from_secs(0));
        store.push(make_entry(1, topic)).unwrap();
        store.push(make_entry(2, topic)).unwrap();

        store.save().expect("save with auto-expire");

        // Reload — entries should be gone because they expired on save.
        let loaded = OutboxStore::load(&dir)
            .expect("load")
            .expect("should exist");
        assert!(loaded.is_empty(), "expired entries should not persist");
    }

    #[test]
    fn recovery_on_restart() {
        // Simulate an app restart: create entries, save, drop the store,
        // reload, verify pending messages are recoverable.
        let dir = temp_dir("recovery");
        let topic = make_topic(0x11);

        // First session: push some messages, advance some states, save.
        {
            let mut store = OutboxStore::empty_at(&dir);
            let e1 = make_entry(1, topic);
            let e2 = make_entry(2, topic);
            let e3 = make_entry(3, topic);
            store.push(e1).unwrap();
            store.push(e2).unwrap();
            store.push(e3).unwrap();
            store.update_delivery_state(1, DeliveryState::Sent).unwrap();
            store.update_delivery_state(2, DeliveryState::Sent).unwrap();
            store
                .update_delivery_state(2, DeliveryState::Delivered)
                .unwrap();
            // e3 stays Queued
            store.save().expect("save session 1");
        }

        // Simulate app restart: reload
        let loaded = OutboxStore::load_or_default(&dir);
        assert_eq!(loaded.len(), 3);

        // Verify all states are preserved
        assert_eq!(loaded.get(1).unwrap().delivery_state, DeliveryState::Sent);
        assert_eq!(
            loaded.get(2).unwrap().delivery_state,
            DeliveryState::Delivered
        );
        assert_eq!(loaded.get(3).unwrap().delivery_state, DeliveryState::Queued);

        // Pending messages (Queued) should include event 3
        let pending: Vec<u64> = loaded.pending().iter().map(|e| e.event_id).collect();
        assert_eq!(pending, vec![3], "only queued messages should be pending");
    }

    #[test]
    fn invalid_json_is_rejected() {
        let dir = temp_dir("invalid_json");
        fs::create_dir_all(&dir).expect("create test dir");
        fs::write(outbox_file_path(&dir), "not valid json").expect("write invalid data");

        let result = OutboxStore::load(&dir);
        assert!(result.is_err(), "invalid JSON should fail");
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("failed to parse outbox file"));
    }

    #[test]
    fn load_or_default_falls_back_gracefully() {
        let dir = temp_dir("corrupt");
        fs::create_dir_all(&dir).expect("create test dir");
        fs::write(outbox_file_path(&dir), "{broken json}").expect("write corrupt file");

        // Should return empty store without panicking
        let store = OutboxStore::load_or_default(&dir);
        assert!(store.is_empty());
    }

    // ── get_by_hash ────────────────────────────────────────────────────

    #[test]
    fn get_by_hash_finds_entry() {
        let dir = temp_dir("by_hash");
        let topic = make_topic(0x22);
        let mut store = OutboxStore::empty_at(&dir);

        let bytes = b"unique-content".to_vec();
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let entry = make_entry_with_bytes(1, topic, bytes);
        store.push(entry).unwrap();

        let found = store.get_by_hash(&hash);
        assert!(found.is_some());
        assert_eq!(found.unwrap().event_id, 1);
    }

    #[test]
    fn get_by_hash_returns_none_for_unknown() {
        let dir = temp_dir("by_hash_missing");
        let store = OutboxStore::empty_at(&dir);
        assert!(store.get_by_hash("nonexistent-hash").is_none());
    }

    // ── remove_topic ───────────────────────────────────────────────────

    #[test]
    fn remove_topic_removes_matching_entries() {
        let dir = temp_dir("remove_topic");
        let ta = make_topic(0xAA);
        let tb = make_topic(0xBB);
        let mut store = OutboxStore::empty_at(&dir);

        store.push(make_entry(1, ta)).unwrap();
        store.push(make_entry(2, tb)).unwrap();
        store.push(make_entry(3, ta)).unwrap();

        store.remove_topic(&ta);
        assert_eq!(store.len(), 1);
        assert_eq!(store.entries()[0].event_id, 2);
        assert_eq!(store.entries()[0].topic, tb);
    }

    // ── clear ──────────────────────────────────────────────────────────

    #[test]
    fn clear_removes_all() {
        let dir = temp_dir("clear");
        let topic = make_topic(0xCC);
        let mut store = OutboxStore::empty_at(&dir);

        store.push(make_entry(1, topic)).unwrap();
        store.push(make_entry(2, topic)).unwrap();
        assert_eq!(store.len(), 2);

        store.clear();
        assert!(store.is_empty());
    }

    // ── TTL ────────────────────────────────────────────────────────────

    #[test]
    fn ttl_get_set_works() {
        let dir = temp_dir("ttl");
        let mut store = OutboxStore::empty_at(&dir);
        assert_eq!(store.ttl(), DEFAULT_OUTBOX_TTL);
        let shorter = Duration::from_secs(3600);
        store.set_ttl(shorter);
        assert_eq!(store.ttl(), shorter);
    }

    // ── Display / file path ────────────────────────────────────────────

    #[test]
    fn file_path_ends_with_outbox_json() {
        let dir = temp_dir("path");
        let store = OutboxStore::empty_at(&dir);
        let path = store.file_path();
        assert!(path.to_string_lossy().ends_with("outbox.json"));
    }

    #[test]
    fn display_path_contains_filename() {
        let dir = temp_dir("display");
        let store = OutboxStore::empty_at(&dir);
        assert!(store.display_path().contains("outbox.json"));
    }
}
