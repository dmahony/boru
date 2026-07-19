//! Durable room metadata for boru-chat.
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
use tracing::warn;

use crate::chat_core::atomic_write::atomic_write_json;
use crate::discovery_secret::DiscoverySecret;
use crate::proto::TopicId;

const SCHEMA_VERSION: u32 = 3;
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
    /// Per-room discovery secret for private-room DHT isolation.
    ///
    /// Generated at room creation time and persisted across restarts so
    /// that the same DHT namespace is reused.  `None` for legacy rooms
    /// or rooms that do not use DHT discovery.
    #[serde(default)]
    pub discovery_secret: Option<DiscoverySecret>,
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
            discovery_secret: None,
            data_dir: data_dir.into(),
        }
    }

    /// Create a store with a known topic and empty peer list, bound to a data directory.
    pub fn new(data_dir: impl Into<PathBuf>, topic: TopicId) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            topic,
            peers: Vec::new(),
            discovery_secret: None,
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
            discovery_secret: None,
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

        if store.schema_version < SCHEMA_VERSION {
            // ── v2 → v3 migration: add discovery_secret field ─────────
            // Older files lack the discovery_secret field (it is
            // `#[serde(default)]` so deserializing a v2 file as v3 would
            // default it to None anyway), but we bump the version to v3
            // so that future migrations have a consistent base.
            store.discovery_secret = None;
            store.schema_version = SCHEMA_VERSION;
            store.data_dir = data_dir.to_path_buf();
            // Persist the migrated store so future loads skip migration.
            if let Err(err) = store.save() {
                warn!(error = %err, "failed to persist v3 room migration");
            }
            Ok(Some(store))
        } else if store.schema_version > SCHEMA_VERSION {
            Err(n0_error::anyerr!(
                "unsupported room schema version {} in {} (expected version 3 or lower)",
                store.schema_version,
                path.display()
            ))
        } else {
            store.data_dir = data_dir.to_path_buf();
            Ok(Some(store))
        }
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

    /// Set the discovery secret and persist to disk.
    ///
    /// Pass `None` to clear a previously stored secret.
    pub fn set_discovery_secret(&mut self, secret: Option<DiscoverySecret>) -> Result<()> {
        self.discovery_secret = secret;
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

    /// Remove the persisted room file for a data directory, if present.
    ///
    /// Returns `true` when a file was removed.
    pub fn delete(data_dir: impl AsRef<Path>) -> Result<bool> {
        let data_dir = data_dir.as_ref();
        let path = room_file_path(data_dir);
        if !path.exists() {
            return Ok(false);
        }

        fs::remove_file(&path)
            .with_std_context(|_| format!("failed to remove room file {}", path.display()))?;
        Ok(true)
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
        dir.push(format!("boru-room-{name}-{suffix}"));
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
    fn delete_removes_saved_file() {
        let dir = temp_dir("delete");
        let topic = TopicId::from_bytes([0x11u8; 32]);
        let store = RoomStore::new(&dir, topic);
        let path = store.save().expect("save room store");
        assert!(path.exists());

        assert!(RoomStore::delete(&dir).expect("delete room file"));
        assert!(!path.exists());
        assert!(!RoomStore::delete(&dir).expect("delete room file again"));
    }

    // -----------------------------------------------------------------------
    // v2 → v3 migration
    // -----------------------------------------------------------------------

    #[test]
    fn save_load_round_trip_preserves_discovery_secret() {
        let dir = temp_dir("secret-roundtrip");
        let mut store = RoomStore::empty_at(&dir);
        let secret = DiscoverySecret::from_bytes([0xAAu8; 32]);
        store
            .set_discovery_secret(Some(secret))
            .expect("set discovery secret");

        let reloaded = RoomStore::load(&dir)
            .expect("load saved store")
            .expect("should have a saved room");
        assert_eq!(reloaded.discovery_secret, Some(secret));
    }

    #[test]
    fn v2_room_auto_migrates_to_v3_with_no_secret() {
        let dir = temp_dir("v2-migration");
        fs::create_dir_all(&dir).expect("create test dir");

        // Write a v2 room.json — no discovery_secret field.
        let topic_bytes: [u8; 32] = [0x42u8; 32];
        let v2_json = serde_json::json!({
            "schema_version": 2,
            "topic": topic_bytes,
            "peers": []
        });
        fs::write(room_file_path(&dir), v2_json.to_string()).expect("write v2 room file");

        // Load should succeed and auto-migrate.
        let loaded = RoomStore::load(&dir)
            .expect("load v2 room")
            .expect("should load v2 room file");
        assert_eq!(loaded.schema_version, 3, "should migrate to v3");
        assert_eq!(
            loaded.discovery_secret, None,
            "migrated room should have no discovery secret"
        );
        assert_eq!(
            loaded.topic,
            TopicId::from_bytes([0x42u8; 32]),
            "topic preserved"
        );

        // The migrated file on disk should now be v3.
        let raw = fs::read_to_string(room_file_path(&dir)).expect("read migrated file");
        let reread: serde_json::Value = serde_json::from_str(&raw).expect("parse migrated file");
        assert_eq!(reread["schema_version"], 3, "persisted schema should be v3");
    }

    #[test]
    fn unsupported_schema_version_rejected() {
        let dir = temp_dir("unsupported");
        fs::create_dir_all(&dir).expect("create test dir");

        // Write a room.json with version 99 (unsupported).
        let topic_bytes: [u8; 32] = [0xFFu8; 32];
        let v99_json = serde_json::json!({
            "schema_version": 99,
            "topic": topic_bytes,
            "peers": []
        });
        fs::write(room_file_path(&dir), v99_json.to_string()).expect("write v99 room file");

        let result = RoomStore::load(&dir);
        assert!(result.is_err(), "unsupported version should fail");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unsupported room schema version 99"),
            "error should mention version 99: {err}"
        );
        assert!(
            err.contains("expected version 3 or lower"),
            "error should mention expected max version: {err}"
        );
    }

    #[test]
    fn delete_works_on_migrated_v2_file() {
        let dir = temp_dir("delete-migrated");
        fs::create_dir_all(&dir).expect("create test dir");

        // Write a v2 room file.
        let topic_bytes: [u8; 32] = [0x33u8; 32];
        let v2_json = serde_json::json!({
            "schema_version": 2,
            "topic": topic_bytes,
            "peers": []
        });
        fs::write(room_file_path(&dir), v2_json.to_string()).expect("write v2 room file");

        // Load triggers migration and persist.
        let _loaded = RoomStore::load(&dir)
            .expect("load v2 room")
            .expect("should load v2 file");
        assert!(room_file_path(&dir).exists(), "migrated file should exist");

        // Delete should still work.
        assert!(RoomStore::delete(&dir).expect("delete migrated file"));
        assert!(!room_file_path(&dir).exists());
    }

    #[test]
    fn empty_at_works_with_new_schema() {
        let dir = temp_dir("empty-at");
        let store = RoomStore::empty_at(&dir);
        assert_eq!(store.schema_version, 3);
        assert_eq!(store.discovery_secret, None);
        assert_eq!(store.topic, TopicId::from_bytes([0u8; 32]));
        assert!(store.peers.is_empty());

        // Save and re-load should preserve everything.
        store.save().expect("save empty store");
        let loaded = RoomStore::load(&dir)
            .expect("load empty store")
            .expect("should have a saved room");
        assert_eq!(loaded.schema_version, 3);
        assert_eq!(loaded.discovery_secret, None);
    }
}
