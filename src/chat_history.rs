//! Durable chat message history for iroh-gossip-chat.
//!
//! Stores each signed message as a persistent entry so that late-joiners
//! can catch up and messages survive restarts.  Every message is also
//! content-addressed: its blake3 hash acts as a stable key that peers
//! can use to request and verify the raw bytes via iroh blobs.
//!
//! History is saved as a JSON file (`chat_history.json`) alongside the
//! identity key — one per data directory.  The file is written atomically
//! (write to .tmp, fsync, rename) so partial writes never corrupt the log.
//!
//! ## Gossip-level sync (out of band)
//!
//! This module provides the local persistent store.  The gossip protocol
//! is extended with a `Message::HistoryTip { hash }` variant so peers
//! can announce the latest blob hash.  Callers (forward_gossip_events /
//! handle_net_event) are responsible for:
//!
//! 1. Storing every outgoing and incoming signed message via [`add_entry`].
//! 2. Broadcasting a `HistoryTip` after each message so others learn the
//!    latest hash.
//! 3. On `NeighborUp`, requesting the missing chain from the new peer
//!    and replaying any blobs they don't yet have locally.
//! 4. Loading history on room open and replaying it into the UI.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::chat_core::atomic_write::atomic_write_json;
use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};

use crate::proto::TopicId;

/// Current schema version — bump on breaking format changes.
const SCHEMA_VERSION: u32 = 1;

/// Name of the on-disk history file (lives beside `secret_key.txt`).
pub const HISTORY_FILE_NAME: &str = "chat_history.json";

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
/// Each entry records the content-addressed hash (blake3 of the signed
/// message bytes), the sender's public key, a millisecond-precision
/// timestamp, a human-readable message kind, and a short text preview.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
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
}

impl HistoryEntry {
    /// Create a new history entry from the raw signed message bytes.
    ///
    /// The hash is computed as blake3 of `signed_bytes`.  `kind` and
    /// `text_preview` should be derived from the decoded `Message`.
    pub fn new(
        topic: TopicId,
        sender: impl Into<String>,
        signed_bytes: Vec<u8>,
        kind: impl Into<String>,
        text_preview: impl Into<String>,
    ) -> Self {
        let hash = blake3_hex(&signed_bytes);
        Self {
            hash,
            sender: sender.into(),
            timestamp: default_now(),
            kind: kind.into(),
            topic,
            text_preview: text_preview.into(),
            signed_bytes,
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

/// Persistent, append-only chat message history.
///
/// Saved as a JSON array at `chat_history.json`.  The file is rewritten
/// atomically on every call to [`save`](Self::save).  This is safe for
/// the moderate message volumes of a chat application (thousands, not
/// millions), but would not scale to high-throughput logging.
///
/// ## Limitations
///
/// - The entire file is rewritten on every save.  For very large chat
///   logs (10k+ messages) this could become slow.  A future optimisation
///   would switch to an append-only log format.
/// - No deduplication: if the same message is received twice (e.g. from
///   two peers relaying it), two entries will be stored.
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
}

impl ChatHistoryStore {
    /// Create an empty store bound to a data directory.
    pub fn empty_at(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            entries: Vec::new(),
            data_dir: data_dir.into(),
        }
    }

    /// Return the on-disk history file path.
    pub fn file_path(&self) -> PathBuf {
        history_file_path(&self.data_dir)
    }

    /// Load the history store from disk.
    ///
    /// - Missing file → `Ok(None)`
    /// - Corrupt JSON → error
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Option<Self>> {
        let data_dir = data_dir.as_ref();
        let path = history_file_path(data_dir);
        if !path.exists() {
            return Ok(None);
        }

        let raw = fs::read_to_string(&path).with_std_context(|_| {
            format!("failed to read history file {}", path.display())
        })?;
        let mut store: Self = serde_json::from_str(&raw).with_std_context(|_| {
            format!("failed to parse history file {}", path.display())
        })?;

        if store.schema_version != SCHEMA_VERSION {
            return Err(n0_error::anyerr!(
                "unsupported history schema version {} in {}",
                store.schema_version,
                path.display()
            ));
        }

        store.data_dir = data_dir.to_path_buf();
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

    /// Persist the history store atomically to `chat_history.json`.
    pub fn save(&self) -> Result<PathBuf> {
        let data_dir = &self.data_dir;
        if data_dir.as_os_str().is_empty() {
            return Err(n0_error::anyerr!(
                "chat history store has no data directory bound to it",
            ));
        }
        let path = self.file_path();
        atomic_write_json(&path, self, "chat history store")?;
        Ok(path)
    }

    /// Append a new entry.  Does **not** automatically save — call
    /// [`save`](Self::save) explicitly to persist.
    pub fn push(&mut self, entry: HistoryEntry) {
        self.entries.push(entry);
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
    pub fn remove_topic(&mut self, topic: &TopicId) {
        self.entries.retain(|e| e.topic != *topic);
    }

    /// Filter entries by topic, returning matching entries in insertion
    /// order.
    pub fn for_topic(&self, topic: &TopicId) -> Vec<&HistoryEntry> {
        self.entries
            .iter()
            .filter(|e| e.topic == *topic)
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
        dir.push(format!("iroh-gossip-chat-history-{name}-{suffix}"));
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

    #[test]
    fn load_missing_returns_empty() {
        let dir = temp_dir("missing");
        let store = ChatHistoryStore::load_or_default(&dir);
        assert!(store.is_empty());
    }

    #[test]
    fn save_then_load_preserves_entries() {
        let dir = temp_dir("roundtrip");
        let mut store = ChatHistoryStore::empty_at(&dir);

        let topic = make_topic(0xAA);
        store.push(make_entry(topic, 1));
        store.push(make_entry(topic, 2));
        store.save().expect("save");

        let loaded = ChatHistoryStore::load_or_default(&dir);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.entries[0].kind, "text");
        assert_eq!(loaded.entries[1].text_preview, "hello 2");
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
    fn save_is_atomic() {
        let dir = temp_dir("atomic");
        let topic = make_topic(0xAA);
        let mut store = ChatHistoryStore::empty_at(&dir);

        store.push(make_entry(topic, 1));
        let path = store.save().expect("first save");

        // Verify the file is valid JSON
        let raw = fs::read_to_string(&path).expect("read saved");
        let parsed: ChatHistoryStore = serde_json::from_str(&raw).expect("valid JSON");
        assert_eq!(parsed.len(), 1);

        // Overwrite
        store.push(make_entry(topic, 2));
        store.save().expect("second save");

        let raw2 = fs::read_to_string(&path).expect("re-read");
        let parsed2: ChatHistoryStore = serde_json::from_str(&raw2).expect("valid JSON");
        assert_eq!(parsed2.len(), 2);
    }

    #[test]
    fn hash_is_computed_from_bytes() {
        let data = b"test message";
        let topic = make_topic(0x01);
        let entry = HistoryEntry::new(topic, "alice", data.to_vec(), "text", "test message");
        assert_eq!(entry.hash, blake3_hex(data));
    }
}
