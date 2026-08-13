//! Public-room directory topic and advertisement store.
//!
//! The directory topic is a deterministic gossip topic derived from the relay
//! URL.  Peers on the same relay can discover each other's public rooms by
//! subscribing to this topic and sharing [`RoomAdvertisement`](crate::chat_core::RoomAdvertisement) messages.
//!
//! # Security
//!
//! Advertisements carry a signature from the room creator's node key so
//! receivers can verify authenticity.  The [`DirectoryStore`](crate::directory::DirectoryStore) does not verify
//! signatures — that is the caller's responsibility.  Stale advertisements
//! are evicted by the [`evict_stale`](crate::directory::DirectoryStore::evict_stale) method.

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
/// Old entries should be periodically evicted with [`evict_stale`](crate::directory::DirectoryStore::evict_stale) to keep
/// the store size bounded.
#[derive(Debug)]
pub struct DirectoryStore {
    /// Active advertisements keyed by (topic, author).
    /// Each entry holds the ad + received timestamp for eviction.
    ads: HashMap<(TopicId, PublicKey), (RoomAdvertisement, Instant)>,
}

/// How long an advertisement stays active after it was (re)ceived
/// (BORU-DIR-08).  Uses the advertisement's own TTL field
/// (`expires_after_secs`), clamped to at least 1 second.  Advertisements
/// that predate the TTL field decode with
/// [`DEFAULT_ADVERT_TTL_SECS`](crate::chat_core::DEFAULT_ADVERT_TTL_SECS).
fn ad_lifetime(ad: &RoomAdvertisement) -> Duration {
    Duration::from_secs(u64::from(ad.expires_after_secs.max(1)))
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
    /// to the current time — a **refresh** (BORU-DIR-08).  The entry's
    /// lifetime restarts from `expires_after_secs` (see
    /// [`evict_expired`](Self::evict_expired)).
    pub fn upsert(&mut self, ad: RoomAdvertisement, author: PublicKey) {
        self.ads.insert((ad.topic, author), (ad, Instant::now()));
    }

    /// Return `true` if an advertisement already exists for the given room
    /// and author.
    ///
    /// Useful for callers that want to distinguish a genuinely new
    /// announcement from a periodic refresh (the same author re-broadcasts
    /// every ~60 s, and that should not count as a fresh event).
    pub fn contains(&self, topic: TopicId, author: PublicKey) -> bool {
        self.ads.contains_key(&(topic, author))
    }

    /// Persist all current (non-expired) advertisements to the SQLite
    /// directory table.
    ///
    /// Expired advertisements are skipped so 'currently active' is never
    /// persisted forever (PDF Task 3.2 step 6): a room whose advertiser
    /// went offline is not resurrected by the next restart.
    pub fn save_to_db(&self, conn: &Connection) -> Result<()> {
        conn.execute("DELETE FROM directory_ads", [])?;
        let now = Instant::now();
        for ((topic, author), (ad, received)) in &self.ads {
            if now.duration_since(*received) >= ad_lifetime(ad) {
                // Expired — do not persist as active (BORU-DIR-08).
                continue;
            }
            let received_at_ms = std::time::SystemTime::now()
                .checked_sub(received.elapsed())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .ok_or_else(|| anyhow!("directory advertisement timestamp overflow"))?
                .as_millis() as i64;
            conn.execute(
                "INSERT INTO directory_ads
                    (topic, author, room_name, description, ticket, member_count,
                     last_activity, received_at_ms, expires_after_secs)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    topic.as_bytes(),
                    author.as_bytes(),
                    ad.room_name,
                    ad.description,
                    ad.ticket,
                    ad.member_count as i64,
                    ad.last_activity as i64,
                    received_at_ms,
                    ad.expires_after_secs as i64,
                ],
            )?;
        }
        Ok(())
    }

    /// Load persisted advertisements, replacing the in-memory contents.
    ///
    /// Entries whose TTL already elapsed while the application was stopped
    /// are **not** restored (PDF Task 3.2 step 6 — do not persist 'currently
    /// active' forever across restarts).  Live advertisers re-announce on
    /// startup (BORU-DIR-07) and refresh periodically, so a still-active
    /// room reappears quickly even if its persisted row was dropped.
    pub fn load_from_db(&mut self, conn: &Connection) -> Result<()> {
        let mut stmt = conn.prepare(
            "SELECT topic, author, room_name, description, ticket, member_count,
                    last_activity, received_at_ms, expires_after_secs
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
                    expires_after_secs: row.get::<_, i64>(8)? as u32,
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
            // BORU-DIR-08: drop rows that already expired while offline so a
            // stale advertisement cannot stay 'live' across a restart.
            if age_ms >= ad_lifetime(&ad).as_millis() as u64 {
                continue;
            }
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
    ///
    /// Expired advertisements (received longer than `expires_after_secs`
    /// ago without a refresh) are excluded — mirroring
    /// [`evict_expired`](Self::evict_expired) on read paths so a stale
    /// entry can never be presented as live (BORU-DIR-08, PDF Task 3.2
    /// step 4).
    pub fn list_active(&self) -> Vec<(RoomAdvertisement, PublicKey)> {
        let now = Instant::now();
        self.ads
            .iter()
            .filter(|(_, (ad, received))| now.duration_since(*received) < ad_lifetime(ad))
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
    /// Legacy fixed-window eviction.  Prefer [`evict_expired`](Self::evict_expired),
    /// which honours each advertisement's own TTL (BORU-DIR-08).
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

    /// Remove advertisements whose TTL has elapsed since they were last
    /// refreshed (BORU-DIR-08, PDF Task 3.2 step 4).
    ///
    /// A room whose advertiser disappears eventually leaves the active
    /// directory: the entry expires `expires_after_secs` after the last
    /// valid refresh.  Because the refresh interval is much shorter than
    /// the TTL, a few lost refreshes (temporary network loss) never expire
    /// a room — it only leaves after the advertiser stops refreshing for
    /// the full TTL.  Returns the evicted `(topic, author)` keys so callers
    /// can refresh UI state.
    ///
    /// Call this periodically (e.g. on the GUI's 1 s monitor tick).
    pub fn evict_expired(&mut self) -> Vec<(TopicId, PublicKey)> {
        let now = Instant::now();
        let expired: Vec<_> = self
            .ads
            .iter()
            .filter(|(_, (ad, received))| now.duration_since(*received) >= ad_lifetime(ad))
            .map(|(key, _)| *key)
            .collect();
        self.ads.retain(|key, _| !expired.contains(key));
        expired
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
            expires_after_secs: crate::chat_core::DEFAULT_ADVERT_TTL_SECS,
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

    /// `contains` distinguishes a new announcement from a periodic refresh
    /// from the same author (the re-broadcast dedup used by the recent
    /// activity feed).
    #[test]
    fn directory_store_contains_tracks_room_author_pairs() {
        let mut store = DirectoryStore::new();
        let topic = make_topic(1);
        let author_a = make_public_key(42);
        let author_b = make_public_key(43);

        assert!(!store.contains(topic, author_a));
        store.upsert(make_ad("room-a", topic), author_a);
        assert!(store.contains(topic, author_a));
        // A different author announcing the same topic is still new.
        assert!(!store.contains(topic, author_b));
        // A different topic by the same author is new.
        assert!(!store.contains(make_topic(2), author_a));
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
            expires_after_secs: 300,
        };
        let ad_new = RoomAdvertisement {
            room_name: "new-name".to_string(),
            description: "updated".to_string(),
            topic,
            ticket: "ticket".to_string(),
            member_count: 10,
            last_activity: 1000,
            expires_after_secs: 300,
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
                expires_after_secs INTEGER NOT NULL DEFAULT 300,
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

    // ── BORU-DIR-08 (PDF Task 3.2): TTL refresh and expiry ─────────────

    /// An advertisement whose TTL elapses without a refresh is evicted from
    /// the active directory — a room whose advertiser disappears eventually
    /// leaves (PDF Task 3.2 acceptance criterion).
    #[test]
    fn directory_store_evicts_expired_ads_without_refresh() {
        let mut store = DirectoryStore::new();
        let topic = make_topic(1);
        let author = make_public_key(42);
        let mut ad = make_ad("vanishing-room", topic);
        ad.expires_after_secs = 1; // 1-second TTL for the test

        store.upsert(ad.clone(), author);
        assert_eq!(store.list_active().len(), 1, "fresh ad is active");

        std::thread::sleep(Duration::from_millis(1_200));
        let evicted = store.evict_expired();
        assert_eq!(evicted, vec![(topic, author)], "expired ad is evicted");
        assert!(
            store.is_empty(),
            "stale room cannot remain permanently live"
        );
    }

    /// `list_active` never presents an expired advertisement as live, even
    /// before the periodic eviction sweep runs.
    #[test]
    fn directory_store_list_active_excludes_expired() {
        let mut store = DirectoryStore::new();
        let topic = make_topic(1);
        let author = make_public_key(42);
        let mut ad = make_ad("expiring-room", topic);
        ad.expires_after_secs = 1;

        store.upsert(ad, author);
        std::thread::sleep(Duration::from_millis(1_200));
        assert!(
            store.list_active().is_empty(),
            "expired ad must not appear in the active directory"
        );
    }

    /// A room whose advertiser refreshes within the TTL stays active —
    /// temporary gaps (packet loss) shorter than the TTL never flicker the
    /// room out of the directory (PDF Task 3.2 step 5: refresh interval
    /// significantly shorter than TTL).
    #[test]
    fn directory_store_refresh_within_ttl_keeps_ad_active() {
        let mut store = DirectoryStore::new();
        let topic = make_topic(1);
        let author = make_public_key(42);
        let mut ad = make_ad("steady-room", topic);
        ad.expires_after_secs = 2;

        store.upsert(ad.clone(), author);
        // Simulate one lost refresh (a 1 s gap < 2 s TTL) then a refresh.
        std::thread::sleep(Duration::from_millis(1_000));
        store.upsert(ad.clone(), author);
        std::thread::sleep(Duration::from_millis(1_000));
        // The refresh keeps the entry alive past what the original receipt
        // would have allowed.
        assert_eq!(store.list_active().len(), 1, "refreshed room stays active");
        assert!(
            store.evict_expired().is_empty(),
            "no eviction while refreshes keep arriving"
        );
    }

    /// `save_to_db` does not persist ads that already expired, and
    /// `load_from_db` drops rows whose TTL elapsed while the app was
    /// stopped — 'currently active' must not survive a restart forever
    /// (PDF Task 3.2 step 6).
    #[test]
    fn directory_store_expired_rows_not_persisted_or_resurrected() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE directory_ads (
                topic BLOB NOT NULL, author BLOB NOT NULL, room_name TEXT NOT NULL,
                description TEXT NOT NULL, ticket TEXT NOT NULL, member_count INTEGER NOT NULL,
                last_activity INTEGER NOT NULL, received_at_ms INTEGER NOT NULL,
                expires_after_secs INTEGER NOT NULL DEFAULT 300,
                PRIMARY KEY (topic, author)
            )",
        )
        .unwrap();

        // Insert a row that expired 10 minutes ago (2-minute TTL) directly,
        // simulating a stale row persisted before the app stopped.
        let topic = make_topic(1);
        let author = make_public_key(42);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        conn.execute(
            "INSERT INTO directory_ads
                (topic, author, room_name, description, ticket, member_count,
                 last_activity, received_at_ms, expires_after_secs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                topic.as_bytes(),
                author.as_bytes(),
                "stale-room",
                "stale".to_string(),
                "ticket".to_string(),
                0i64,
                0i64,
                now_ms - 600_000, // received 10 minutes ago
                120i64,           // 2-minute TTL → already expired
            ],
        )
        .unwrap();

        let mut store = DirectoryStore::new();
        store.load_from_db(&conn).unwrap();
        assert!(
            store.list_active().is_empty(),
            "an ad that expired while offline must not be resurrected"
        );
    }
}
