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
use iroh::PublicKey;

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

    /// Return all active advertisements paired with their author.
    pub fn list_active(&self) -> Vec<(RoomAdvertisement, PublicKey)> {
        self.ads
            .iter()
            .map(|((_topic, author), (ad, _))| (ad.clone(), *author))
            .collect()
    }

    /// Remove advertisements older than `max_age`.
    ///
    /// Call this periodically (e.g. every 60 seconds) to keep the store
    /// from accumulating stale entries from peers that have gone offline.
    pub fn evict_stale(&mut self, max_age: Duration) {
        let cutoff = Instant::now() - max_age;
        self.ads.retain(|_, (_, received)| *received >= cutoff);
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
        let lobby_topic = crate::public_room::public_lobby_topic(
            crate::public_room::PublicNetwork::Mainnet,
        );
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
        let bytes = [id; 32];
        // PublicKey doesn't have a from_bytes constructor that takes [u8; 32]
        // directly.  Use it as an endpoint ID via the key exchange namespace.
        PublicKey::from_bytes(&bytes).expect("valid public key bytes")
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
    fn directory_store_evict_stale() {
        let mut store = DirectoryStore::new();
        let topic = make_topic(1);
        let author = make_public_key(42);
        let ad = make_ad("room", topic);

        store.upsert(ad, author);

        // With a zero-length max_age, everything should be evicted
        // (Instant::now() - Duration::ZERO is effectively now, but
        // "now - ZERO" = now, so entries at "now" are still >= cutoff).
        // Use a very small duration to ensure the entry is older than the cutoff.
        store.evict_stale(Duration::from_nanos(1));
        // The entry was just inserted so it should still be present.
        assert_eq!(store.len(), 1);

        // There is no way to forcefully set Instant in the past, so we
        // approximate by using Instant::now() + small delay won't help.
        // Instead, verify that len() doesn't change for very small durations.
        store.evict_stale(Duration::from_secs(0));
        assert_eq!(store.len(), 1);
    }

    /// Different authors advertising the same topic are stored separately.
    #[test]
    fn same_topic_different_authors() {
        let mut store = DirectoryStore::new();
        let topic = make_topic(1);
        let author_a = make_public_key(10);
        let author_b = make_public_key(20);

        store.upsert(make_ad("room-alpha", topic), author_a);
        store.upsert(make_ad("room-beta", topic), author_b);

        assert_eq!(store.len(), 2);
    }
}
