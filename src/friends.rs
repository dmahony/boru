//! Durable friends list storage for iroh-gossip-chat.
//!
//! This module owns the on-disk `friends.json` file that lives beside the
//! persistent `secret_key.txt` identity file.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::chat_core::{atomic_write::atomic_write_json, Ticket};
use crate::proto::TopicId;
use iroh::{EndpointAddr, PublicKey};
use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};

const SCHEMA_VERSION: u32 = 2;
const MAX_KNOWN_ADDRS: usize = 5;
/// Name of the on-disk friends list file (lives beside `secret_key.txt`).
pub const FRIENDS_FILE_NAME: &str = "friends.json";

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn friends_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FRIENDS_FILE_NAME)
}

/// Stable identifier for a friend record.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FriendId(String);

impl FriendId {
    /// Construct an id from a parsed iroh public key.
    pub fn from_public_key(public_key: PublicKey) -> Self {
        Self(public_key.to_string())
    }

    /// Construct an id from a raw string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Return the underlying string form.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parse the id back into a public key.
    pub fn parse_public_key(&self) -> Result<PublicKey> {
        PublicKey::from_str(&self.0).std_context("parse friend public key")
    }
}

/// Persisted presence status for a friend.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendStatus {
    /// Whether the peer was last observed online.
    #[serde(default)]
    pub online: bool,
    /// Last time we observed the peer online, stored as unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at_unix_ms: Option<u64>,
    /// Last time we observed the peer offline, stored as unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_offline_at_unix_ms: Option<u64>,
}

/// Persisted friend metadata.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendRecord {
    /// User-chosen display label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Most recently announced self-name from the peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_announced_name: Option<String>,
    /// Durably stored online/offline observations.
    #[serde(default)]
    pub status: FriendStatus,
    /// Recently observed endpoint addresses, newest first.
    #[serde(default)]
    pub known_addrs: Vec<EndpointAddr>,
    /// Last time the durable address list changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addrs_updated_at_unix_ms: Option<u64>,
    /// Rooms for which we have exchanged a ticket with this friend.
    #[serde(default, with = "topic_ticket_map")]
    pub rooms: BTreeMap<TopicId, Ticket>,
}

mod topic_ticket_map {
    use super::*;

    pub fn serialize<S>(
        map: &BTreeMap<TopicId, Ticket>,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        map.iter().collect::<Vec<_>>().serialize(serializer)
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> std::result::Result<BTreeMap<TopicId, Ticket>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let entries = Vec::<(TopicId, Ticket)>::deserialize(deserializer)?;
        Ok(entries.into_iter().collect())
    }
}

impl FriendRecord {
    /// Merge observed endpoint addresses, keeping newest entries first and bounded.
    pub fn record_addrs(&mut self, addrs: impl IntoIterator<Item = EndpointAddr>) {
        let mut merged = Vec::new();
        for addr in addrs {
            if !merged
                .iter()
                .any(|existing: &EndpointAddr| existing == &addr)
            {
                merged.push(addr);
            }
        }
        for addr in self.known_addrs.drain(..) {
            if !merged.iter().any(|existing| existing == &addr) {
                merged.push(addr);
            }
        }
        merged.truncate(MAX_KNOWN_ADDRS);
        if merged != self.known_addrs {
            self.known_addrs = merged;
            self.addrs_updated_at_unix_ms = Some(now_unix_ms());
        }
    }

    /// Record a room ticket exchanged with this friend.
    pub fn record_room(&mut self, topic: TopicId, ticket: Ticket) {
        self.rooms.insert(topic, ticket);
    }

    /// Human-friendly label for display.
    pub fn display_label(&self, id: &FriendId) -> String {
        self.label
            .clone()
            .or_else(|| self.last_announced_name.clone())
            .unwrap_or_else(|| id.as_str().chars().take(12).collect())
    }
}

/// Versioned persistent friends list.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FriendsStore {
    /// Format version for future migrations.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Friends indexed by their stable public key string.
    #[serde(default)]
    pub friends: BTreeMap<FriendId, FriendRecord>,
    /// Data directory used for load/save operations.
    #[serde(skip)]
    data_dir: PathBuf,
}

impl Default for FriendsStore {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            friends: BTreeMap::new(),
            data_dir: PathBuf::new(),
        }
    }
}

impl FriendsStore {
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

    /// Return the on-disk friends file path.
    pub fn file_path(&self) -> PathBuf {
        friends_file_path(&self.data_dir)
    }

    /// Load the friends store from disk.
    ///
    /// Missing files are treated as an empty store. Corrupt JSON or invalid
    /// friend ids return an error so callers can decide whether to fall back.
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref();
        let path = friends_file_path(data_dir);
        if !path.exists() {
            return Ok(Self::empty_at(data_dir));
        }

        let raw = fs::read_to_string(&path)
            .with_std_context(|_| format!("failed to read friends file {}", path.display()))?;
        let mut store: Self = serde_json::from_str(&raw)
            .with_std_context(|_| format!("failed to parse friends file {}", path.display()))?;

        if store.schema_version != 1 && store.schema_version != SCHEMA_VERSION {
            return Err(n0_error::anyerr!(
                "unsupported friends schema version {} in {}",
                store.schema_version,
                path.display()
            ));
        }
        // Version 1 had no durable addressing or room fields. They are
        // serde-defaulted above; normalise the version on the next save.
        store.schema_version = SCHEMA_VERSION;

        for id in store.friends.keys() {
            id.parse_public_key().with_std_context(|_| {
                format!("invalid friend id in {}: {}", path.display(), id.as_str())
            })?;
        }

        store.data_dir = data_dir.to_path_buf();
        Ok(store)
    }

    /// Load a store, logging and falling back to an empty store on failure.
    pub fn load_or_default(data_dir: impl AsRef<Path>) -> Self {
        let data_dir = data_dir.as_ref();
        match Self::load(data_dir) {
            Ok(store) => store,
            Err(err) => {
                eprintln!(
                    "warning: starting with an empty friends list; failed to load {}: {err}",
                    friends_file_path(data_dir).display()
                );
                Self::empty_at(data_dir)
            }
        }
    }

    /// Persist the store atomically to `friends.json`.
    pub fn save(&self) -> Result<PathBuf> {
        let data_dir = self.data_dir();
        if data_dir.as_os_str().is_empty() {
            return Err(n0_error::anyerr!(
                "friends store has no data directory bound to it",
            ));
        }
        let path = self.file_path();
        atomic_write_json(&path, self, "friends store")?;
        Ok(path)
    }

    /// Number of friends in the store.
    pub fn len(&self) -> usize {
        self.friends.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.friends.is_empty()
    }

    /// Immutable iterator over all friends.
    pub fn iter(&self) -> impl Iterator<Item = (&FriendId, &FriendRecord)> {
        self.friends.iter()
    }

    /// Insert or update a friend record.
    pub fn upsert(&mut self, id: FriendId, record: FriendRecord) {
        self.friends.insert(id, record);
    }

    /// Remove a friend by id.
    pub fn remove(&mut self, id: &FriendId) -> Option<FriendRecord> {
        self.friends.remove(id)
    }

    /// Get a friend record by id.
    pub fn get(&self, id: &FriendId) -> Option<&FriendRecord> {
        self.friends.get(id)
    }

    /// Get a mutable friend record by id.
    pub fn get_mut(&mut self, id: &FriendId) -> Option<&mut FriendRecord> {
        self.friends.get_mut(id)
    }

    /// Ensure a friend exists and return a mutable record reference.
    pub fn ensure_friend(&mut self, id: FriendId) -> &mut FriendRecord {
        self.friends.entry(id).or_default()
    }

    /// Mark a peer online and update its last-seen timestamp.
    pub fn mark_online(&mut self, id: FriendId) -> &mut FriendRecord {
        let record = self.ensure_friend(id);
        record.status.online = true;
        record.status.last_seen_at_unix_ms = Some(now_unix_ms());
        record
    }

    /// Mark a peer offline and update its last-offline timestamp.
    pub fn mark_offline(&mut self, id: FriendId) -> &mut FriendRecord {
        let record = self.ensure_friend(id);
        record.status.online = false;
        record.status.last_offline_at_unix_ms = Some(now_unix_ms());
        record
    }

    /// Update the user-facing label for a peer.
    pub fn set_label(&mut self, id: FriendId, label: impl Into<String>) -> &mut FriendRecord {
        let record = self.ensure_friend(id);
        record.label = Some(label.into());
        record
    }

    /// Update the last announced name from a peer.
    pub fn set_last_announced_name(
        &mut self,
        id: FriendId,
        name: impl Into<String>,
    ) -> &mut FriendRecord {
        let record = self.ensure_friend(id);
        record.last_announced_name = Some(name.into());
        record
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
        dir.push(format!("iroh-gossip-friends-{name}-{suffix}"));
        dir
    }

    #[test]
    fn load_missing_returns_empty_store() {
        let dir = temp_dir("missing");
        let store = FriendsStore::load(&dir).expect("load missing");
        assert!(store.is_empty());
        assert_eq!(store.data_dir(), dir.as_path());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir("roundtrip");
        let mut store = FriendsStore::empty_at(&dir);
        let pk = iroh::SecretKey::generate().public();
        let id = FriendId::from_public_key(pk);
        store.set_label(id.clone(), "Bob");
        store.mark_online(id.clone());
        store.save().expect("save friends store");

        let reloaded = FriendsStore::load(&dir).expect("load saved store");
        assert_eq!(reloaded.len(), 1);
        let record = reloaded.get(&id).expect("friend exists");
        assert_eq!(record.label.as_deref(), Some("Bob"));
        assert!(record.status.online);
        assert!(record.status.last_seen_at_unix_ms.is_some());
    }

    #[test]
    fn known_addrs_are_deduplicated_and_capped() {
        let mut record = FriendRecord::default();
        let addrs: Vec<_> = (0..7)
            .map(|_| EndpointAddr::new(iroh::SecretKey::generate().public()))
            .collect();
        record.record_addrs(addrs[..5].iter().cloned());
        record.record_addrs(addrs[4..].iter().cloned());
        assert_eq!(record.known_addrs.len(), 5);
        assert_eq!(record.known_addrs[0], addrs[4]);
        assert!(record.addrs_updated_at_unix_ms.is_some());
    }

    #[test]
    fn old_schema_without_new_fields_loads() {
        let dir = temp_dir("migration");
        fs::create_dir_all(&dir).expect("create test dir");
        let pk = iroh::SecretKey::generate().public();
        let raw = serde_json::json!({
            "schema_version": 1,
            "friends": { pk.to_string(): { "label": "Old" } }
        });
        fs::write(friends_file_path(&dir), raw.to_string()).expect("write old file");
        let store = FriendsStore::load(&dir).expect("load old friends file");
        let record = store
            .get(&FriendId::from_public_key(pk))
            .expect("friend exists");
        assert!(record.known_addrs.is_empty());
        assert!(record.rooms.is_empty());
        assert_eq!(store.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn save_then_load_preserves_address_and_room_data() {
        let dir = temp_dir("rich-roundtrip");
        let mut store = FriendsStore::empty_at(&dir);
        let pk = iroh::SecretKey::generate().public();
        let id = FriendId::from_public_key(pk);
        let topic = TopicId::from_bytes([7; 32]);
        let ticket = Ticket {
            topic,
            peers: vec![EndpointAddr::new(pk)],
        };
        store
            .ensure_friend(id.clone())
            .record_addrs(ticket.peers.clone());
        store
            .ensure_friend(id.clone())
            .record_room(topic, ticket.clone());
        store.save().expect("save");
        let loaded = FriendsStore::load(&dir).expect("load");
        let record = loaded.get(&id).expect("friend");
        assert_eq!(record.known_addrs, ticket.peers);
        assert_eq!(record.rooms.get(&topic), Some(&ticket));
    }

    #[test]
    fn invalid_json_is_rejected() {
        let dir = temp_dir("invalid");
        fs::create_dir_all(&dir).expect("create test dir");
        fs::write(friends_file_path(&dir), "not json").expect("write invalid file");
        let err = FriendsStore::load(&dir).expect_err("invalid friends file should fail");
        assert!(err.to_string().contains("failed to parse friends file"));
    }
}
