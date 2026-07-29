//! Public-room directory topic and advertisement store.
//!
//! The directory topic is a deterministic gossip topic derived from the relay
//! URL.  Peers on the same relay can discover each other's public rooms by
//! subscribing to this topic and sharing [`RoomAdvertisement`] messages.
//!
//! # Security
//!
//! Advertisements carry a signature from the room creator's node key so
//! receivers can verify authenticity.  The [`DirectoryStore`] does not verify
//! signatures — that is the caller's responsibility.  Stale advertisements
//! are evicted by the [`evict_stale`](DirectoryStore::evict_stale) method.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use crate::chat_core::RoomAdvertisement;
use crate::proto::TopicId;
use anyhow::{anyhow, Result};
use iroh::PublicKey;
use rusqlite::{params, Connection};

// ---------------------------------------------------------------------------
// Directory topic derivation
// ---------------------------------------------------------------------------

/// Domain separator for the public-room directory gossip topic.
///
/// Deliberately distinct from other boru-chat domain separators
/// ([`PUBLIC_ROOM_DOMAIN_SEPARATOR`](crate::topic_derivation::PUBLIC_ROOM_DOMAIN_SEPARATOR),
/// [`DISCOVERY_KEY_DOMAIN_SEPARATOR`](crate::public_room::DISCOVERY_KEY_DOMAIN_SEPARATOR),
/// etc.) so that the same bytes never produce a gossip topic, discovery key,
/// or any other namespace value — preventing cross-protocol confusion.
const DIRECTORY_DOMAIN_SEPARATOR: &[u8] = b"boru-chat/public-room-directory/v1";

/// Deterministically derive the directory gossip topic from a relay URL.
///
/// Peers connected to the same relay derive the same topic, making the
/// directory a shared mesh for room discovery within that relay's cohort.
///
/// # Derivation
///
/// ```text
/// TopicId = BLAKE3("boru-chat/public-room-directory/v1" || relay_url_bytes)
/// ```
pub fn directory_topic(relay_url: &str) -> TopicId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(DIRECTORY_DOMAIN_SEPARATOR);
    hasher.update(relay_url.as_bytes());
    TopicId::from_bytes(*hasher.finalize().as_bytes())
}

// ---------------------------------------------------------------------------
// DirectoryStore
// ---------------------------------------------------------------------------

/// A lightweight in-memory store for room advertisements received over the
/// directory gossip topic.
///
/// Advertisements are keyed by (room topic, author public key) so that each
/// author can publish at most one advertisement per room.  A later
/// advertisement from the same author replaces an earlier one (upsert).
///
/// Old entries should be periodically evicted with [`evict_stale`] to keep
/// the store size bounded.
#[derive(Debug)]
pub struct DirectoryStore {
    /// Active advertisements keyed by (topic, author).
    /// Each entry holds the ad + received timestamp for eviction.
    ads: HashMap<(TopicId, PublicKey), (RoomAdvertisement, Instant)>,
}

impl DirectoryStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            ads: HashMap::new(),
        }
    }

    /// Insert or update an advertisement for a room by a specific author.
    ///
    /// If an advertisement already exists for the same (topic, author) pair,
    /// it is replaced with the new value and the received timestamp is reset
    /// to the current time.
    pub fn upsert(&mut self, ad: RoomAdvertisement, author: PublicKey) {
        self.ads.insert((ad.topic, author), (ad, Instant::now()));
    }

    /// Persist all current advertisements to the SQLite directory table.
    pub fn save_to_db(&self, conn: &Connection) -> Result<()> {
        conn.execute("DELETE FROM directory_ads", [])?;
        for ((topic, author), (ad, received)) in &self.ads {
            let received_at_ms = std::time::SystemTime::now()
                .checked_sub(received.elapsed())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .ok_or_else(|| anyhow!("directory advertisement timestamp overflow"))?
                .as_millis() as i64;
            conn.execute(
                "INSERT INTO directory_ads
                    (topic, author, room_name, description, ticket, member_count,
                     last_activity, received_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    topic.as_bytes(),
                    author.as_bytes(),
                    ad.room_name,
                    ad.description,
                    ad.ticket,
                    ad.member_count as i64,
                    ad.last_activity as i64,
                    received_at_ms,
                ],
            )?;
        }
        Ok(())
    }

    /// Load persisted advertisements, replacing the in-memory contents.
    pub fn load_from_db(&mut self, conn: &Connection) -> Result<()> {
        let mut stmt = conn.prepare(
            "SELECT topic, author, room_name, description, ticket, member_count,
                    last_activity, received_at_ms
             FROM directory_ads",
        )?;
        let rows = stmt.query_map([], |row| {
            let topic: Vec<u8> = row.get(0)?;
            let author: Vec<u8> = row.get(1)?;
            let topic: [u8; 32] = topic.try_into().map_err(|_| {
                rusqlite::Error::InvalidColumnType(0, "topic".into(), rusqlite::types::Type::Blob)
            })?;
            let author: [u8; 32] = author.try_into().map_err(|_| {
                rusqlite::Error::InvalidColumnType(1, "author".into(), rusqlite::types::Type::Blob)
            })?;
            let author = PublicKey::from_bytes(&author).map_err(|_| {
                rusqlite::Error::InvalidColumnType(1, "author".into(), rusqlite::types::Type::Blob)
            })?;
            Ok((
                RoomAdvertisement {
                    room_name: row.get(2)?,
                    description: row.get(3)?,
                    topic: TopicId::from_bytes(topic),
                    ticket: row.get(4)?,
                    member_count: row.get::<_, i64>(5)? as u32,
                    last_activity: row.get::<_, i64>(6)? as u64,
                },
                author,
                row.get::<_, i64>(7)?,
            ))
        })?;

        let now = Instant::now();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| anyhow!("system clock is before UNIX epoch"))?
            .as_millis() as i64;
        self.ads.clear();
        for row in rows {
            let (ad, author, received_at_ms) = row?;
            let age_ms = now_ms.saturating_sub(received_at_ms).max(0) as u64;
            self.ads.insert(
                (ad.topic, author),
                (
                    ad,
                    now.checked_sub(Duration::from_millis(age_ms))
                        .unwrap_or(now),
                ),
            );
        }
        Ok(())
    }

    /// Return all active advertisements paired with their author.
    pub fn list_active(&self) -> Vec<(RoomAdvertisement, PublicKey)> {
        self.ads
            .iter()
            .map(|((_topic, author), (ad, _))| (ad.clone(), *author))
            .collect()
    }

    /// Remove all advertisements for a room topic and return the number removed.
    pub fn remove_topic(&mut self, topic: TopicId) -> usize {
        let before = self.ads.len();
        self.ads
            .retain(|(stored_topic, _), _| *stored_topic != topic);
        before - self.ads.len()
    }

    /// Remove one advertisement and return whether it was present.
    pub fn remove(&mut self, topic: TopicId, author: PublicKey) -> bool {
        self.ads.remove(&(topic, author)).is_some()
    }

    /// Remove advertisements older than `max_age`.
    ///
    /// Call this periodically (e.g. every 60 seconds) to keep the store
    /// from accumulating stale entries from peers that have gone offline.
    pub fn evict_stale(&mut self, max_age: Duration) -> Vec<(TopicId, PublicKey)> {
        let cutoff = Instant::now() - max_age;
        let evicted: Vec<_> = self
            .ads
            .iter()
            .filter(|(_, (_, received))| *received < cutoff)
            .map(|(key, _)| *key)
            .collect();
        self.ads.retain(|key, _| !evicted.contains(key));
        evicted
    }

    /// Return the number of stored advertisements.
    pub fn len(&self) -> usize {
        self.ads.len()
    }

    /// Returns `true` if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.ads.is_empty()
    }
}

impl Default for DirectoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The directory topic must be deterministic — same relay URL always
    /// produces the same topic.
    #[test]
    fn directory_topic_is_deterministic() {
        let url = "https://relay.example.com:8443";
        let a = directory_topic(url);
        let b = directory_topic(url);
        assert_eq!(a, b);
    }

    /// Different relay URLs produce different directory topics.
    #[test]
    fn different_relays_produce_different_topics() {
        let a = directory_topic("https://relay-a.example.com:8443");
        let b = directory_topic("https://relay-b.example.com:8443");
        assert_ne!(a, b);
    }

    /// The directory topic must differ from the canonical public lobby topic
    /// (domain separation sanity check).
    #[test]
    fn directory_topic_differs_from_lobby() {
        let relay = "https://boru.chat:8443";
        let dir_topic = directory_topic(relay);
        let lobby_topic =
            crate::public_room::public_lobby_topic(crate::public_room::PublicNetwork::Mainnet);
        assert_ne!(dir_topic, lobby_topic);
    }

    /// Non-zero output (avalanche sanity check).
    #[test]
    fn directory_topic_is_nonzero() {
        let topic = directory_topic("https://boru.chat:8443");
        assert!(topic.as_bytes().iter().any(|&b| b != 0));
    }

    // ── DirectoryStore tests ───────────────────────────────────────────

    fn make_topic(id: u8) -> TopicId {
        TopicId::from_bytes([id; 32])
    }

    fn make_public_key(id: u8) -> PublicKey {
        for candidate in id..=u8::MAX {
            let bytes = [candidate; 32];
            if let Ok(key) = PublicKey::from_bytes(&bytes) {
                return key;
            }
        }
        panic!("no valid test public key found");
    }

    fn make_ad(room_name: &str, topic: TopicId) -> RoomAdvertisement {
        RoomAdvertisement {
            room_name: room_name.to_string(),
            description: "A test room".to_string(),
            topic,
            ticket: format!("ticket-{room_name}"),
            member_count: 5,
            last_activity: 0,
        }
    }

    #[test]
    fn directory_store_new_is_empty() {
        let store = DirectoryStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn directory_store_upsert_and_list() {
        let mut store = DirectoryStore::new();
        let topic_a = make_topic(1);
        let topic_b = make_topic(2);
        let author = make_public_key(42);

        let ad_a = make_ad("room-a", topic_a);
        let ad_b = make_ad("room-b", topic_b);

        store.upsert(ad_a.clone(), author);
        assert_eq!(store.len(), 1);

        let active = store.list_active();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].0.room_name, "room-a");
        assert_eq!(active[0].1, author);

        store.upsert(ad_b.clone(), author);
        assert_eq!(store.len(), 2);

        let active = store.list_active();
        assert_eq!(active.len(), 2);
    }

    #[test]
    fn directory_store_upsert_replaces_same_key() {
        let mut store = DirectoryStore::new();
        let topic = make_topic(1);
        let author = make_public_key(42);

        let ad_old = RoomAdvertisement {
            room_name: "old-name".to_string(),
            description: "original".to_string(),
            topic,
            ticket: "ticket".to_string(),
            member_count: 1,
            last_activity: 0,
        };
        let ad_new = RoomAdvertisement {
            room_name: "new-name".to_string(),
            description: "updated".to_string(),
            topic,
            ticket: "ticket".to_string(),
            member_count: 10,
            last_activity: 1000,
        };

        store.upsert(ad_old, author);
        store.upsert(ad_new.clone(), author);

        assert_eq!(store.len(), 1);
        let active = store.list_active();
        assert_eq!(active[0].0.room_name, "new-name");
        assert_eq!(active[0].0.member_count, 10);
    }

    #[test]
    fn directory_store_remove_deletes_only_requested_author() {
        let mut store = DirectoryStore::new();
        let topic = make_topic(1);
        let author_a = make_public_key(42);
        let author_b = make_public_key(43);

        store.upsert(make_ad("room-a", topic), author_a);
        store.upsert(make_ad("room-b", topic), author_b);

        assert!(store.remove(topic, author_a));
        assert_eq!(store.len(), 1);
        assert_eq!(store.list_active()[0].1, author_b);
        assert!(!store.remove(topic, author_a));
    }

    #[test]
    fn directory_store_evict_stale() {
        let mut store = DirectoryStore::new();
        let topic = make_topic(1);
        let author = make_public_key(42);
        let ad = make_ad("room", topic);

        store.upsert(ad, author);

        assert_eq!(store.evict_stale(Duration::from_secs(0)).len(), 1);
        assert!(store.is_empty());
    }

    /// Different authors advertising the same topic are stored separately.
    #[test]
    fn same_topic_different_authors() {
        let mut store = DirectoryStore::new();
        let topic = make_topic(1);
        let author_a = make_public_key(42);
        let author_b = make_public_key(43);

        store.upsert(make_ad("room-alpha", topic), author_a);
        store.upsert(make_ad("room-beta", topic), author_b);

        assert_eq!(store.len(), 2);
    }

    #[test]
    fn directory_store_round_trips_sqlite() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE directory_ads (
                topic BLOB NOT NULL, author BLOB NOT NULL, room_name TEXT NOT NULL,
                description TEXT NOT NULL, ticket TEXT NOT NULL, member_count INTEGER NOT NULL,
                last_activity INTEGER NOT NULL, received_at_ms INTEGER NOT NULL,
                PRIMARY KEY (topic, author)
            )",
        )
        .unwrap();
        let mut original = DirectoryStore::new();
        let ad = make_ad("persisted", make_topic(9));
        let author = make_public_key(42);
        original.upsert(ad.clone(), author);
        original.save_to_db(&conn).unwrap();

        let mut restored = DirectoryStore::new();
        restored.load_from_db(&conn).unwrap();
        assert_eq!(restored.list_active(), vec![(ad, author)]);
    }
}
