//! Transient chat message state for iroh-gossip-chat.
//!
//! Chat messages are intentionally never persisted.  This type remains as a
//! small in-memory compatibility layer for the active-session backfill code;
//! its disk-facing methods discard legacy files and never write new ones.
//!
//! Callers may use the in-memory entries while the process is running. No
//! state survives process exit and no history is replayed on room open.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

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

    /// Discard a legacy on-disk history file and return no history.
    ///
    /// Existing `chat_history.json` files are deleted on discovery so old
    /// chat content is not retained or replayed after upgrading.
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Option<Self>> {
        let data_dir = data_dir.as_ref();
        let path = history_file_path(data_dir);
        if path.exists() {
            fs::remove_file(&path).with_std_context(|_| {
                format!("failed to remove legacy history file {}", path.display())
            })?;
        }
        Ok(None)
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

    /// Do not persist chat history.
    ///
    /// The path is returned for API compatibility, but no file or temporary
    /// file is created.
    pub fn save(&self) -> Result<PathBuf> {
        let data_dir = &self.data_dir;
        if data_dir.as_os_str().is_empty() {
            return Err(n0_error::anyerr!(
                "chat history store has no data directory bound to it",
            ));
        }
        Ok(self.file_path())
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
        self.entries.iter().filter(|e| e.topic == *topic).collect()
    }

    /// Return up to `count` of the most recent entries, oldest-first.
    pub fn get_recent_messages(&self, count: usize) -> Vec<&HistoryEntry> {
        if count == 0 {
            return Vec::new();
        }
        let start = self.entries.len().saturating_sub(count);
        self.entries[start..].iter().collect()
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
    fn load_removes_legacy_history_file() {
        let dir = temp_dir("legacy");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(HISTORY_FILE_NAME);
        std::fs::write(&path, b"legacy chat content").unwrap();

        let store = ChatHistoryStore::load_or_default(&dir);
        assert!(store.is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn save_does_not_persist_entries() {
        let dir = temp_dir("roundtrip");
        let mut store = ChatHistoryStore::empty_at(&dir);

        let topic = make_topic(0xAA);
        store.push(make_entry(topic, 1));
        store.push(make_entry(topic, 2));
        store.save().expect("save");

        assert!(!store.file_path().exists());
        let loaded = ChatHistoryStore::load_or_default(&dir);
        assert!(loaded.is_empty());
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
    fn save_does_not_create_history_file() {
        let dir = temp_dir("atomic");
        let topic = make_topic(0xAA);
        let mut store = ChatHistoryStore::empty_at(&dir);

        store.push(make_entry(topic, 1));
        let path = store.save().expect("first save");
        assert_eq!(path, dir.join(HISTORY_FILE_NAME));
        assert!(!path.exists());

        store.push(make_entry(topic, 2));
        store.save().expect("second save");
        assert!(!path.exists());
    }

    #[test]
    fn hash_is_computed_from_bytes() {
        let data = b"test message";
        let topic = make_topic(0x01);
        let entry = HistoryEntry::new(topic, "alice", data.to_vec(), "text", "test message");
        assert_eq!(entry.hash, blake3_hex(data));
    }
}
