//! Durable multi-room history for iroh-gossip-chat.
//!
//! Stores a list of known chat rooms (topics) the user has joined or
//! created, along with a display name and last-active timestamp so the
//! UI can show a recent-chat list on startup — the way Telegram and
//! Signal show your conversation list before you join a specific chat.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::chat_core::atomic_write::atomic_write_json;
use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};

use crate::proto::TopicId;

const SCHEMA_VERSION: u32 = 1;
/// Name of the on-disk room history file (lives beside `secret_key.txt`).
pub const ROOM_HISTORY_FILE_NAME: &str = "rooms.json";

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

fn default_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn rooms_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(ROOM_HISTORY_FILE_NAME)
}

/// A single entry in the room history — one chat the user has visited.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomHistoryEntry {
    /// The gossip topic for this room.
    pub topic: TopicId,
    /// Human-readable display name (may be empty, derived from topic).
    pub name: String,
    /// Unix-epoch seconds of the last activity / visit.
    pub last_seen: u64,
    /// Optional last message preview (first few chars of latest message).
    pub last_preview: String,
    /// Whether the user created this room (vs. joined someone else's).
    pub is_owner: bool,
}

impl RoomHistoryEntry {
    /// Create a new entry for a room the user is opening or joining.
    pub fn new(topic: TopicId, name: impl Into<String>, is_owner: bool) -> Self {
        Self {
            topic,
            name: name.into(),
            last_seen: default_now(),
            last_preview: String::new(),
            is_owner,
        }
    }

    /// Bump the last_seen timestamp (call on room entry / message activity).
    pub fn touch(&mut self) {
        self.last_seen = default_now();
    }

    /// Display label: use the name if set, otherwise a short topic preview.
    pub fn display_name(&self) -> String {
        if self.name.is_empty() {
            let hex = format!("{:.16}", self.topic);
            format!("room-{}", &hex[..8])
        } else {
            self.name.clone()
        }
    }
}

/// Persistent store for the user's room/chat history.
///
/// Saved as `rooms.json` alongside the secret key and single-room
/// metadata.  Loaded at startup to populate the chat list.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomHistoryStore {
    /// Format version for future migrations.
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    /// All known rooms, newest-first (by last_seen).
    pub rooms: Vec<RoomHistoryEntry>,
    /// Data directory used for load/save operations.
    #[serde(skip)]
    data_dir: PathBuf,
}

impl RoomHistoryStore {
    /// Create an empty store bound to a data directory.
    pub fn empty_at(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            rooms: Vec::new(),
            data_dir: data_dir.into(),
        }
    }

    /// Return the on-disk file path.
    pub fn file_path(&self) -> PathBuf {
        rooms_file_path(&self.data_dir)
    }

    /// Load room history from disk.  Missing file returns `None`.
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Option<Self>> {
        let data_dir = data_dir.as_ref();
        let path = rooms_file_path(data_dir);
        if !path.exists() {
            return Ok(None);
        }

        let raw = fs::read_to_string(&path)
            .with_std_context(|_| format!("failed to read rooms file {}", path.display()))?;
        let mut store: Self = serde_json::from_str(&raw)
            .with_std_context(|_| format!("failed to parse rooms file {}", path.display()))?;

        if store.schema_version != SCHEMA_VERSION {
            return Err(n0_error::anyerr!(
                "unsupported rooms schema version {} in {}",
                store.schema_version,
                path.display()
            ));
        }

        store.data_dir = data_dir.to_path_buf();
        Ok(Some(store))
    }

    /// Load room history, falling back to empty store on failure.
    pub fn load_or_default(data_dir: impl AsRef<Path>) -> Self {
        let data_dir = data_dir.as_ref();
        match Self::load(data_dir) {
            Ok(Some(store)) => store,
            Ok(None) => Self::empty_at(data_dir),
            Err(err) => {
                eprintln!(
                    "warning: failed to load room history from {}: {err}",
                    rooms_file_path(data_dir).display()
                );
                Self::empty_at(data_dir)
            }
        }
    }

    /// Persist the room history atomically to `rooms.json`.
    pub fn save(&self) -> Result<PathBuf> {
        let data_dir = &self.data_dir;
        if data_dir.as_os_str().is_empty() {
            return Err(n0_error::anyerr!(
                "room history store has no data directory bound to it",
            ));
        }
        let path = self.file_path();
        atomic_write_json(&path, self, "room history store")?;
        Ok(path)
    }

    /// Find a room by topic, or return `None`.
    pub fn find(&self, topic: &TopicId) -> Option<&RoomHistoryEntry> {
        self.rooms.iter().find(|r| r.topic == *topic)
    }

    /// Find a room by topic, mutably.
    pub fn find_mut(&mut self, topic: &TopicId) -> Option<&mut RoomHistoryEntry> {
        self.rooms.iter_mut().find(|r| r.topic == *topic)
    }

    /// Add or update a room entry.  If the topic already exists, updates
    /// its name and bumps last_seen.  Otherwise inserts a new entry
    /// (as newest).
    pub fn upsert(&mut self, topic: TopicId, name: impl Into<String>, is_owner: bool) {
        if let Some(existing) = self.find_mut(&topic) {
            existing.touch();
            let n = name.into();
            if !n.is_empty() {
                existing.name = n;
            }
        } else {
            self.rooms
                .push(RoomHistoryEntry::new(topic, name, is_owner));
        }
        // Sort: newest first
        self.rooms.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
    }

    /// Update the last message preview for a room.
    pub fn update_preview(&mut self, topic: &TopicId, preview: impl Into<String>) {
        if let Some(entry) = self.find_mut(topic) {
            entry.last_preview = preview.into();
        }
    }

    /// Remove a room from history.
    pub fn remove(&mut self, topic: &TopicId) {
        self.rooms.retain(|r| r.topic != *topic);
    }

    /// Number of rooms in history.
    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

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
        dir.push(format!("iroh-gossip-room-history-{name}-{suffix}"));
        dir
    }

    fn make_topic(byte: u8) -> TopicId {
        TopicId::from_bytes([byte; 32])
    }

    #[test]
    fn load_missing_returns_empty() {
        let dir = temp_dir("missing");
        let store = RoomHistoryStore::load_or_default(&dir);
        assert!(store.is_empty());
    }

    #[test]
    fn save_then_load_preserves_rooms() {
        let dir = temp_dir("roundtrip");
        let mut store = RoomHistoryStore::empty_at(&dir);

        let t1 = make_topic(0xAA);
        let t2 = make_topic(0xBB);
        store.upsert(t1, "Friends Chat", true);
        store.upsert(t2, "Work Chat", false);
        store.save().expect("save");

        let loaded = RoomHistoryStore::load_or_default(&dir);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.find(&t1).unwrap().display_name(), "Friends Chat");
        assert_eq!(loaded.find(&t2).unwrap().display_name(), "Work Chat");
    }

    #[test]
    fn upsert_updates_existing() {
        let dir = temp_dir("upsert");
        let mut store = RoomHistoryStore::empty_at(&dir);

        let t = make_topic(0xAA);
        store.upsert(t, "Old Name", true);
        assert_eq!(store.len(), 1);

        // Same topic, new name
        store.upsert(t, "New Name", true);
        assert_eq!(store.len(), 1);
        assert_eq!(store.find(&t).unwrap().name, "New Name");
    }

    #[test]
    fn remove_entry() {
        let dir = temp_dir("remove");
        let mut store = RoomHistoryStore::empty_at(&dir);

        let t = make_topic(0xAA);
        store.upsert(t, "Test", true);
        assert_eq!(store.len(), 1);

        store.remove(&t);
        assert!(store.is_empty());
    }

    #[test]
    fn order_is_newest_first() {
        let dir = temp_dir("order");
        let mut store = RoomHistoryStore::empty_at(&dir);

        let t1 = make_topic(0x01);
        let t2 = make_topic(0x02);
        let t3 = make_topic(0x03);

        store.upsert(t1, "Oldest", true);
        std::thread::sleep(std::time::Duration::from_millis(2));
        store.upsert(t2, "Middle", false);
        std::thread::sleep(std::time::Duration::from_millis(2));
        store.upsert(t3, "Newest", true);

        assert_eq!(store.rooms[0].topic, t3);
        assert_eq!(store.rooms[1].topic, t2);
        assert_eq!(store.rooms[2].topic, t1);

        // Touching t1 should move it to front
        std::thread::sleep(std::time::Duration::from_millis(2));
        let _ = store.find_mut(&t1).map(|r| r.touch());
        store.rooms.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        assert_eq!(store.rooms[0].topic, t1);
    }
}
