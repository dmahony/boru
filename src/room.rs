//! Durable room metadata for iroh-gossip-chat.
//!
//! This module owns the on-disk `room.json` file that lives beside the
//! persistent `secret_key.txt` identity file.  When a user runs `open`
//! without specifying a topic, the saved topic is reused so that
//! reopening the room produces a stable ticket, with peers serving only
//! as bootstrap hints.

use std::{
    fs,
    path::{Path, PathBuf},
};

use iroh::EndpointAddr;
use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};

use crate::chat_core::atomic_write::atomic_write_json;
use crate::proto::TopicId;

const SCHEMA_VERSION: u32 = 2;
/// Name of the on-disk room metadata file (lives beside `secret_key.txt`).
pub const ROOM_FILE_NAME: &str = "room.json";

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

fn room_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(ROOM_FILE_NAME)
}

/// Durable room metadata that survives restarts.
///
/// At minimum this stores the gossip [`TopicId`] so that a subsequent
/// `cargo ... open` (without an explicit topic) reuses the same room
/// instead of generating a fresh random topic.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomStore {
    /// Format version for future migrations.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// The gossip topic for the room — stable across reopen.
    pub topic: TopicId,
    /// Known bootstrap peer addresses, persisted so reopening a room can
    /// seed the address lookup without a fresh ticket.
    #[serde(default)]
    pub peers: Vec<EndpointAddr>,
    /// Data directory used for load/save operations.
    #[serde(skip)]
    data_dir: PathBuf,
}

impl RoomStore {
    /// Construct an empty (uninitialised) store bound to a data directory.
    /// Has `topic = [0; 32]` as a placeholder; callers should set the
    /// real topic before saving.
    pub fn empty_at(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            topic: TopicId::from_bytes([0u8; 32]),
            peers: Vec::new(),
            data_dir: data_dir.into(),
        }
    }

    /// Create a store with a known topic and empty peer list, bound to a data directory.
    pub fn new(data_dir: impl Into<PathBuf>, topic: TopicId) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            topic,
            peers: Vec::new(),
            data_dir: data_dir.into(),
        }
    }

    /// Create a store with a known topic and bootstrap peers, bound to a data directory.
    pub fn with_peers(
        data_dir: impl Into<PathBuf>,
        topic: TopicId,
        peers: Vec<EndpointAddr>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            topic,
            peers,
            data_dir: data_dir.into(),
        }
    }

    /// Return the data directory used by this store.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Return the on-disk room file path.
    pub fn file_path(&self) -> PathBuf {
        room_file_path(&self.data_dir)
    }

    /// Load the room store from disk.
    ///
    /// Missing files are treated as no saved room and return `None`.
    /// Corrupt JSON returns an error so callers can decide how to handle it.
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Option<Self>> {
        let data_dir = data_dir.as_ref();
        let path = room_file_path(data_dir);
        if !path.exists() {
            return Ok(None);
        }

        let raw = fs::read_to_string(&path)
            .with_std_context(|_| format!("failed to read room file {}", path.display()))?;
        let mut store: Self = serde_json::from_str(&raw)
            .with_std_context(|_| format!("failed to parse room file {}", path.display()))?;

        if store.schema_version != SCHEMA_VERSION {
            return Err(n0_error::anyerr!(
                "unsupported room schema version {} in {}",
                store.schema_version,
                path.display()
            ));
        }

        store.data_dir = data_dir.to_path_buf();
        Ok(Some(store))
    }

    /// Load a room store, logging and falling back to `None` on failure.
    pub fn load_or_none(data_dir: impl AsRef<Path>) -> Option<Self> {
        let data_dir = data_dir.as_ref();
        match Self::load(data_dir) {
            Ok(maybe) => maybe,
            Err(err) => {
                eprintln!(
                    "warning: no saved room data; failed to load {}: {err}",
                    room_file_path(data_dir).display()
                );
                None
            }
        }
    }

    /// Replace the full peers list and persist to disk.
    pub fn set_peers(&mut self, peers: Vec<EndpointAddr>) -> Result<()> {
        self.peers = peers;
        self.save()?;
        Ok(())
    }

    /// Clear the peers list and persist to disk.
    pub fn clear_peers(&mut self) -> Result<()> {
        self.peers.clear();
        self.save()?;
        Ok(())
    }

    /// Persist the room store atomically to `room.json`.
    pub fn save(&self) -> Result<PathBuf> {
        let data_dir = self.data_dir();
        if data_dir.as_os_str().is_empty() {
            return Err(n0_error::anyerr!(
                "room store has no data directory bound to it",
            ));
        }
        let path = self.file_path();
        atomic_write_json(&path, self, "room store")?;
        Ok(path)
    }
}

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
        dir.push(format!("iroh-gossip-room-{name}-{suffix}"));
        dir
    }

    #[test]
    fn load_missing_returns_none() {
        let dir = temp_dir("missing");
        let store = RoomStore::load(&dir).expect("load missing");
        assert!(store.is_none());
    }

    #[test]
    fn save_then_load_preserves_topic() {
        let dir = temp_dir("roundtrip");
        let topic = TopicId::from_bytes([0xABu8; 32]);
        let store = RoomStore::new(&dir, topic);
        store.save().expect("save room store");

        let reloaded = RoomStore::load(&dir)
            .expect("load saved store")
            .expect("should have a saved room");
        assert_eq!(reloaded.topic, topic);
    }

    #[test]
    fn reopening_generates_same_topic() {
        let dir = temp_dir("reopen");
        let topic = TopicId::from_bytes([0x42u8; 32]);
        let store = RoomStore::new(&dir, topic);
        store.save().expect("save room store");

        // Simulate "open" without a topic — load saved room
        let loaded = RoomStore::load_or_none(&dir).expect("should find saved room");
        assert_eq!(loaded.topic, topic);

        // The ticket string derived from this topic is deterministic,
        // already verified by ticket_is_deterministic in chat_core.
    }

    #[test]
    fn invalid_json_is_rejected() {
        let dir = temp_dir("invalid");
        fs::create_dir_all(&dir).expect("create test dir");
        fs::write(room_file_path(&dir), "not json").expect("write invalid file");
        let result = RoomStore::load(&dir);
        assert!(result.is_err(), "invalid room file should fail");
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("failed to parse room file"));
    }

    #[test]
    fn load_or_none_falls_back_gracefully() {
        let dir = temp_dir("corrupt");
        fs::create_dir_all(&dir).expect("create test dir");
        fs::write(room_file_path(&dir), "{broken json}").expect("write corrupt file");
        // Should return None without panicking.
        let store = RoomStore::load_or_none(&dir);
        assert!(
            store.is_none(),
            "load_or_none should return None on failure"
        );
    }

    #[test]
    fn save_is_atomic() {
        let dir = temp_dir("atomic");
        let topic = TopicId::from_bytes([0x01u8; 32]);
        let store = RoomStore::new(&dir, topic);
        let path = store.save().expect("first save");

        // Verify the file is valid JSON
        let raw = fs::read_to_string(&path).expect("read saved file");
        let parsed: RoomStore = serde_json::from_str(&raw).expect("valid JSON");
        assert_eq!(parsed.topic, topic);

        // Overwrite
        let topic2 = TopicId::from_bytes([0x02u8; 32]);
        let store2 = RoomStore::new(&dir, topic2);
        store2.save().expect("second save");

        // File should now contain topic2, not topic1
        let raw2 = fs::read_to_string(&path).expect("re-read");
        let parsed2: RoomStore = serde_json::from_str(&raw2).expect("valid JSON");
        assert_eq!(parsed2.topic, topic2);
    }
}
