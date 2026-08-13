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
use crate::control_plane::advertisement::{
    DEFAULT_MAX_ADVERT_TTL_SECS, DEFAULT_MIN_ADVERT_TTL_SECS,
};
use crate::control_plane::privacy::ControlPlaneRateLimiter;
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
// Legacy advertisement abuse controls (BORU-DIR-19, PDF Task 7.1)
// ---------------------------------------------------------------------------
//
// The legacy `RoomAdvertisement` gossip path is a separate, unauthenticated-
// by-policy receive surface: any peer on the directory topic can broadcast
// advertisements, and the receive loop must not let a peer grow our memory,
// churn the UI, or force subscriptions without bound. The constants below
// mirror the control-plane bounds (BORU-DIR-02) so both advertisement
// pipelines enforce the same resource limits.

/// Maximum room-name length (Unicode characters) for legacy directory
/// advertisements — matches `DEFAULT_MAX_ROOM_NAME_LEN` (BORU-DIR-02).
pub const LEGACY_MAX_ROOM_NAME_LEN: usize = 64;

/// Maximum description length (Unicode characters) for legacy directory
/// advertisements — matches `DEFAULT_MAX_DESCRIPTION_LEN` (BORU-DIR-02).
pub const LEGACY_MAX_DESCRIPTION_LEN: usize = 256;

/// Maximum serialized ticket length. A serialized `Ticket` is well under
/// this; the cap prevents an attacker from smuggling an oversized payload
/// through the join-ticket field.
pub const LEGACY_MAX_TICKET_LEN: usize = 512;

/// Maximum number of entries in the legacy [`DirectoryStore`]. The store is
/// a bounded cache, not a database: beyond this the store evicts expired
/// entries first, then the least-recently-received entry (bounded memory,
/// mirrors [`crate::room_directory::MAX_DIRECTORY_ENTRIES`]).
pub const MAX_DIRECTORY_STORE_ENTRIES: usize = 1024;

/// Default per-author legacy advertisement rate limit: advertisements per
/// sliding window. The legitimate refresh cadence is one ad per room per
/// ~60 s, so this allows a peer to advertise a large room list while
/// bounding a broadcast flood.
pub const LEGACY_AD_RATE_LIMIT_MAX: u32 = 60;

/// Default per-author legacy advertisement rate-limit window.
pub const LEGACY_AD_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

/// Why a legacy [`RoomAdvertisement`] was rejected at the receive boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyAdViolation {
    /// The room name is longer than [`LEGACY_MAX_ROOM_NAME_LEN`].
    RoomNameTooLong {
        /// Length of the offending name (Unicode chars).
        len: usize,
        /// Maximum allowed.
        max: usize,
    },
    /// The description is longer than [`LEGACY_MAX_DESCRIPTION_LEN`].
    DescriptionTooLong {
        /// Length of the offending description (Unicode chars).
        len: usize,
        /// Maximum allowed.
        max: usize,
    },
    /// The serialized ticket is longer than [`LEGACY_MAX_TICKET_LEN`].
    TicketTooLong {
        /// Length of the offending ticket.
        len: usize,
        /// Maximum allowed.
        max: usize,
    },
}

/// Check a legacy [`RoomAdvertisement`] against the receive-boundary bounds
/// (BORU-DIR-19, PDF Task 7.1 step 1/5). The control-plane advertisement
/// path enforces the same limits at decode via
/// [`crate::control_plane::privacy::ControlAdvertPolicy`]; this is the
/// equivalent gate for the legacy directory gossip path.
///
/// TTLs are intentionally **not** part of this check: an absurd
/// `expires_after_secs` is *clamped* by [`clamp_legacy_ttl`] (the store
/// keeps the advertisement with a bounded lifetime) rather than rejected,
/// so a quirky or legacy advertiser never nukes a valid listing.
pub fn legacy_advertisement_bounds_check(ad: &RoomAdvertisement) -> Result<(), LegacyAdViolation> {
    let name_len = ad.room_name.chars().count();
    if name_len > LEGACY_MAX_ROOM_NAME_LEN {
        return Err(LegacyAdViolation::RoomNameTooLong {
            len: name_len,
            max: LEGACY_MAX_ROOM_NAME_LEN,
        });
    }
    let desc_len = ad.description.chars().count();
    if desc_len > LEGACY_MAX_DESCRIPTION_LEN {
        return Err(LegacyAdViolation::DescriptionTooLong {
            len: desc_len,
            max: LEGACY_MAX_DESCRIPTION_LEN,
        });
    }
    let ticket_len = ad.ticket.len();
    if ticket_len > LEGACY_MAX_TICKET_LEN {
        return Err(LegacyAdViolation::TicketTooLong {
            len: ticket_len,
            max: LEGACY_MAX_TICKET_LEN,
        });
    }
    Ok(())
}

/// Clamp a legacy advertisement's TTL into the protocol-defined range
/// (BORU-DIR-19, PDF Task 7.1 step 5: reject absurd TTL values and clamp to
/// protocol-defined limits). An absurd `expires_after_secs` (e.g.
/// `u32::MAX`) would otherwise keep a stale room in the directory for
/// ~136 years; the clamp makes the effective lifetime bounded regardless of
/// what the peer sends. The control-plane path rejects out-of-range TTLs at
/// decode ([`AdvertisementViolation::TtlTooSmall`](crate::control_plane::advertisement::AdvertisementViolation::TtlTooSmall) /
/// [`TtlTooLarge`](crate::control_plane::advertisement::AdvertisementViolation::TtlTooLarge));
/// the legacy path clamps instead so a quirky advertiser's listing survives
/// with a bounded lifetime.
pub fn clamp_legacy_ttl(ad: &mut RoomAdvertisement) {
    ad.expires_after_secs = ad
        .expires_after_secs
        .clamp(DEFAULT_MIN_ADVERT_TTL_SECS, DEFAULT_MAX_ADVERT_TTL_SECS);
}

/// Outcome of admitting a legacy advertisement through
/// [`DirectoryStore::receive`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyAdmitOutcome {
    /// A genuinely new `(topic, author)` advertisement was stored — the UI
    /// may announce it and (legacy behaviour) subscribe once.
    Added,
    /// A known `(topic, author)` refreshed with **changed** metadata — the
    /// UI should refresh the card but not re-announce or re-subscribe.
    Refreshed,
    /// The advertisement is byte-identical in its user-visible metadata to
    /// the cached one — a pure liveness refresh. No UI event (no constant
    /// re-rendering).
    Duplicate,
    /// The author exceeded the per-author advertisement rate limit — the
    /// advertisement is dropped (bounded logging, no UI churn).
    RateLimited,
    /// The advertisement violates the receive-boundary bounds — it is
    /// discarded (malformed/oversized metadata never reaches the UI).
    Rejected(LegacyAdViolation),
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
///
/// Since BORU-DIR-19 the store is a **bounded cache** (PDF Task 7.1): the
/// receive path ([`receive`](Self::receive)) enforces metadata bounds,
/// clamps absurd TTLs, rate-limits per author, deduplicates identical
/// broadcasts, and caps the entry count — a peer cannot allocate unbounded
/// memory or cause constant re-rendering through the legacy directory
/// gossip path.
#[derive(Debug)]
pub struct DirectoryStore {
    /// Active advertisements keyed by (topic, author).
    /// Each entry holds the ad + received timestamp for eviction.
    ads: HashMap<(TopicId, PublicKey), (RoomAdvertisement, Instant)>,
    /// Per-author sliding-window rate limiter for remote advertisements
    /// (reuses the control-plane [`ControlPlaneRateLimiter`] pattern,
    /// keyed on the authenticated advertisement author).
    rate_limiter: ControlPlaneRateLimiter,
    /// Maximum number of stored entries (bounded memory).
    max_entries: usize,
}

/// How long an advertisement stays active after it was (re)ceived
/// (BORU-DIR-08).  Uses the advertisement's own TTL field
/// (`expires_after_secs`), clamped to at least 1 second.  Advertisements
/// that predate the TTL field decode with
/// [`DEFAULT_ADVERT_TTL_SECS`](crate::chat_core::DEFAULT_ADVERT_TTL_SECS).
fn ad_lifetime(ad: &RoomAdvertisement) -> Duration {
    Duration::from_secs(u64::from(ad.expires_after_secs.max(1)))
}

/// Digest of the **user-visible** legacy advertisement metadata
/// (BORU-DIR-19 dedup identity). Dynamic liveness fields
/// (`member_count`, `last_activity`) are excluded so a periodic refresh —
/// which only bumps those hints — is a pure liveness refresh
/// ([`LegacyAdmitOutcome::Duplicate`]) and cannot churn the UI.
fn legacy_metadata_digest(ad: &RoomAdvertisement) -> [u8; 32] {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    ad.topic.as_bytes().hash(&mut hasher);
    ad.room_name.hash(&mut hasher);
    ad.description.hash(&mut hasher);
    ad.ticket.hash(&mut hasher);
    let hash = hasher.finish();
    // Fold the u64 hash into a 32-byte digest (collision-resistant enough
    // for a dedup identity; the (topic, author) key already bounds it).
    let mut digest = [0u8; 32];
    digest[..8].copy_from_slice(&hash.to_le_bytes());
    digest[8..16].copy_from_slice(&(!hash).to_le_bytes());
    digest[16..24].copy_from_slice(&hash.to_le_bytes());
    digest[24..32].copy_from_slice(&(!hash).to_le_bytes());
    digest
}

impl DirectoryStore {
    /// Create a new empty store with the default bounds.
    pub fn new() -> Self {
        Self::with_limits(MAX_DIRECTORY_STORE_ENTRIES)
    }

    /// Create a new empty store with an explicit entry cap (tests use small
    /// caps to exercise the eviction path cheaply).
    pub fn with_limits(max_entries: usize) -> Self {
        Self {
            ads: HashMap::new(),
            rate_limiter: ControlPlaneRateLimiter::with_limits(
                LEGACY_AD_RATE_LIMIT_MAX,
                LEGACY_AD_RATE_LIMIT_WINDOW,
                MAX_DIRECTORY_STORE_ENTRIES,
            ),
            max_entries: max_entries.max(1),
        }
    }

    /// Insert or update an advertisement for a room by a specific author.
    ///
    /// If an advertisement already exists for the same (topic, author) pair,
    /// it is replaced with the new value and the received timestamp is reset
    /// to the current time — a **refresh** (BORU-DIR-08).  The entry's
    /// lifetime restarts from `expires_after_secs` (see
    /// [`evict_expired`](Self::evict_expired)).
    ///
    /// This is the **local/trusted** publish path (the app mirrors its own
    /// broadcasts into the store). Remote advertisements go through
    /// [`receive`](Self::receive), which applies the abuse controls.
    pub fn upsert(&mut self, ad: RoomAdvertisement, author: PublicKey) {
        self.insert_bounded(ad, author, Instant::now());
    }

    /// Admit a remote advertisement through the receive-boundary abuse
    /// controls (BORU-DIR-19, PDF Task 7.1):
    ///
    /// 1. **Bounds** — room name / description / ticket length and TTL range
    ///    are checked against the protocol limits; malformed or oversized
    ///    advertisements are discarded ([`LegacyAdmitOutcome::Rejected`]).
    /// 2. **TTL clamp** — an absurd `expires_after_secs` is clamped into the
    ///    protocol range so a stale room can never linger forever.
    /// 3. **Per-author rate limit** — a peer cannot flood the directory with
    ///    more than [`LEGACY_AD_RATE_LIMIT_MAX`] advertisements per
    ///    [`LEGACY_AD_RATE_LIMIT_WINDOW`].
    /// 4. **Deduplication** — an advertisement identical in its user-visible
    ///    metadata (name/description/ticket) to the cached one is a pure
    ///    liveness refresh ([`LegacyAdmitOutcome::Duplicate`]); it updates
    ///    the received timestamp but produces **no UI event**, so repeated
    ///    identical broadcasts cannot cause constant re-rendering.
    /// 5. **Bounded cache** — when at capacity the store evicts expired
    ///    entries first, then the least-recently-received entry.
    ///
    /// `now` is explicit so tests can drive TTL expiry deterministically.
    pub fn receive(
        &mut self,
        ad: RoomAdvertisement,
        author: PublicKey,
        now: Instant,
    ) -> LegacyAdmitOutcome {
        // 1. Bounds — discard malformed/oversized metadata.
        if let Err(violation) = legacy_advertisement_bounds_check(&ad) {
            return LegacyAdmitOutcome::Rejected(violation);
        }
        // 2. Clamp absurd TTLs to the protocol-defined range.
        let mut ad = ad;
        clamp_legacy_ttl(&mut ad);
        // 3. Per-author rate limit.
        if !self.rate_limiter.admit(&author) {
            return LegacyAdmitOutcome::RateLimited;
        }
        // 4. Deduplicate identical user-visible metadata.
        let key = (ad.topic, author);
        if let Some((existing, _)) = self.ads.get(&key) {
            if legacy_metadata_digest(existing) == legacy_metadata_digest(&ad) {
                if let Some((_, received)) = self.ads.get_mut(&key) {
                    *received = now;
                }
                return LegacyAdmitOutcome::Duplicate;
            }
        }
        // 5. Bounded insert (evicts when full).
        let is_new = self.insert_bounded(ad, author, now);
        if is_new {
            LegacyAdmitOutcome::Added
        } else {
            LegacyAdmitOutcome::Refreshed
        }
    }

    /// Insert `(ad, author)` under the entry cap. Returns `true` when the
    /// `(topic, author)` key was not previously stored (a genuinely new
    /// listing).
    fn insert_bounded(&mut self, ad: RoomAdvertisement, author: PublicKey, now: Instant) -> bool {
        let key = (ad.topic, author);
        let is_new = !self.ads.contains_key(&key);
        if is_new {
            self.evict_for_capacity(now);
        }
        self.ads.insert(key, (ad, now));
        is_new
    }

    /// Evict one entry to make room when at capacity: expired entries first,
    /// then the least-recently-received entry (bounded memory).
    fn evict_for_capacity(&mut self, now: Instant) {
        if self.ads.len() < self.max_entries {
            return;
        }
        // Prefer evicting an expired entry.
        let expired: Vec<(TopicId, PublicKey)> = self
            .ads
            .iter()
            .filter(|(_, (ad, received))| now.duration_since(*received) >= ad_lifetime(ad))
            .map(|(key, _)| *key)
            .collect();
        if !expired.is_empty() {
            for key in expired {
                self.ads.remove(&key);
            }
            return;
        }
        // Otherwise evict the least-recently-received entry.
        let oldest = self
            .ads
            .iter()
            .min_by_key(|(_, (_, received))| *received)
            .map(|(key, _)| *key);
        if let Some(key) = oldest {
            self.ads.remove(&key);
        }
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

    /// Apply a verified room withdrawal (BORU-DIR-09, PDF Task 3.3).
    ///
    /// Directory clients call this when a withdrawal **verifies**: it
    /// removes the matching advertisement — the one `author` published for
    /// `topic` — immediately, instead of waiting for the advertisement TTL.
    ///
    /// # Authority rule
    ///
    /// A withdrawal is keyed by `(topic, author)` exactly like the
    /// advertisement itself, so it can only ever remove the advertisement
    /// the **verified signer** published. A spoofed or misattributed
    /// withdrawal (wrong key, replayed for a different room) removes
    /// nothing here and can never remove an unrelated author's listing.
    ///
    /// TTL expiry remains the safety net: an advertisement whose withdrawal
    /// is missed is still evicted by [`evict_expired`](Self::evict_expired)
    /// once its `expires_after_secs` elapses without a refresh.
    ///
    /// Returns `true` when an advertisement was actually removed — callers
    /// use this to refresh UI state (e.g. bump the directory sidebar
    /// revision) only when something changed.
    pub fn withdraw(&mut self, topic: TopicId, author: PublicKey) -> bool {
        self.remove(topic, author)
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
        // Derive from a secret-key seed so every id maps to a distinct,
        // always-valid key (ed25519 `SecretKey::from_bytes` is infallible
        // and accepts any 32 bytes). The previous "first valid candidate >=
        // id" scan could collide: some 32-byte patterns are not valid ed25519
        // points (e.g. [0x2b; 32]), so `make_public_key(43)` and
        // `make_public_key(44)` could return the SAME key.
        iroh::SecretKey::from_bytes(&[id; 32]).public()
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

    // ── BORU-DIR-09 (PDF Task 3.3): withdrawal / tombstone ────────────

    /// A verified withdrawal removes the matching advertisement immediately
    /// — intentional unlisting must be faster than waiting for the TTL
    /// (PDF Task 3.3 acceptance criterion).
    #[test]
    fn directory_store_withdrawal_removes_matching_ad() {
        let mut store = DirectoryStore::new();
        let topic = make_topic(1);
        let author = make_public_key(42);

        store.upsert(make_ad("room", topic), author);
        assert!(
            store.contains(topic, author),
            "ad present before withdrawal"
        );

        assert!(
            store.withdraw(topic, author),
            "withdrawal of the matching (topic, author) removes the ad"
        );
        assert!(!store.contains(topic, author), "ad removed immediately");
        assert!(store.list_active().is_empty());
        // A second withdrawal for the same pair is a no-op (idempotent).
        assert!(!store.withdraw(topic, author));
    }

    /// A spoofed / misattributed withdrawal cannot remove an unrelated
    /// advertisement: it is keyed by (topic, author) exactly like the ad,
    /// so it can only ever remove what the verified signer published
    /// (PDF Task 3.3: "Spoofed withdrawals cannot remove unrelated rooms").
    #[test]
    fn directory_store_withdrawal_cannot_remove_unrelated_rooms() {
        let mut store = DirectoryStore::new();
        let topic = make_topic(1);
        let other_topic = make_topic(2);
        let owner = make_public_key(42);
        let stranger = make_public_key(43);

        store.upsert(make_ad("owner-room", topic), owner);
        store.upsert(make_ad("stranger-endorsement", topic), stranger);

        // The owner's withdrawal removes only the owner's own ad — the
        // stranger's independent endorsement stays.
        assert!(store.withdraw(topic, owner));
        assert!(!store.contains(topic, owner));
        assert!(
            store.contains(topic, stranger),
            "other author's ad untouched"
        );

        // A withdrawal for a room the signer never advertised removes
        // nothing.
        assert!(!store.withdraw(topic, make_public_key(44)));
        assert!(!store.withdraw(other_topic, owner));
    }

    /// TTL remains the final cleanup mechanism: an advertisement whose
    /// withdrawal is missed (never arrives / is dropped) still leaves the
    /// directory when its `expires_after_secs` elapses without a refresh
    /// (PDF Task 3.3 step 5).
    #[test]
    fn directory_store_missed_withdrawal_still_expires_via_ttl() {
        let mut store = DirectoryStore::new();
        let topic = make_topic(1);
        let author = make_public_key(42);
        let mut ad = make_ad("no-withdrawal-room", topic);
        ad.expires_after_secs = 1; // 1-second TTL for the test

        store.upsert(ad, author);
        assert_eq!(store.list_active().len(), 1, "fresh ad is active");

        // No withdrawal arrives — the ad is still live until its TTL.
        std::thread::sleep(Duration::from_millis(1_200));
        let evicted = store.evict_expired();
        assert_eq!(evicted, vec![(topic, author)], "TTL eviction still applies");
        assert!(store.list_active().is_empty());
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

    // ── BORU-DIR-19 (PDF Task 7.1): spam + resource limits ────────────

    /// Malformed/oversized metadata is discarded at the receive boundary:
    /// oversized room names, descriptions, and tickets never reach the
    /// store (PDF Task 7.1 acceptance criterion).
    #[test]
    fn legacy_receive_rejects_oversized_metadata() {
        let mut store = DirectoryStore::new();
        let author = make_public_key(42);

        // Oversized room name.
        let mut ad = make_ad("ok", make_topic(1));
        ad.room_name = "x".repeat(LEGACY_MAX_ROOM_NAME_LEN + 1);
        assert_eq!(
            store.receive(ad, author, Instant::now()),
            LegacyAdmitOutcome::Rejected(LegacyAdViolation::RoomNameTooLong {
                len: LEGACY_MAX_ROOM_NAME_LEN + 1,
                max: LEGACY_MAX_ROOM_NAME_LEN,
            }),
            "oversized room name discarded"
        );

        // Oversized description.
        let mut ad = make_ad("ok", make_topic(1));
        ad.description = "y".repeat(LEGACY_MAX_DESCRIPTION_LEN + 1);
        assert!(
            matches!(
                store.receive(ad, author, Instant::now()),
                LegacyAdmitOutcome::Rejected(LegacyAdViolation::DescriptionTooLong { .. })
            ),
            "oversized description discarded"
        );

        // Oversized ticket.
        let mut ad = make_ad("ok", make_topic(1));
        ad.ticket = "t".repeat(LEGACY_MAX_TICKET_LEN + 1);
        assert!(
            matches!(
                store.receive(ad, author, Instant::now()),
                LegacyAdmitOutcome::Rejected(LegacyAdViolation::TicketTooLong { .. })
            ),
            "oversized ticket discarded"
        );

        // Absurd TTL (would keep the room live for ~136 years) is clamped
        // to the protocol maximum, not stored forever.
        let mut ad = make_ad("ok", make_topic(1));
        ad.expires_after_secs = u32::MAX;
        assert_eq!(
            store.receive(ad.clone(), author, Instant::now()),
            LegacyAdmitOutcome::Added,
            "absurd TTL is clamped, not dropped"
        );
        let stored = store.list_active();
        assert_eq!(stored.len(), 1, "clamped ad is stored");
        assert_eq!(
            stored[0].0.expires_after_secs, DEFAULT_MAX_ADVERT_TTL_SECS,
            "stored TTL is bounded by the protocol maximum"
        );

        assert!(
            store.list_active().len() <= 1,
            "every rejected advertisement left the store untouched (only the clamped ad is stored)"
        );
    }

    /// A peer cannot allocate unbounded memory with room advertisements:
    /// the entry count is capped, and identical re-broadcasts never grow
    /// the store.
    #[test]
    fn legacy_store_caps_entries_and_evicts_lru() {
        let mut store = DirectoryStore::with_limits(4);
        let author = make_public_key(42);
        let now = Instant::now();

        for i in 0..4u8 {
            let mut ad = make_ad(&format!("room-{i}"), make_topic(i));
            ad.expires_after_secs = 300;
            assert_eq!(
                store.receive(ad, author, now + Duration::from_secs(u64::from(i))),
                LegacyAdmitOutcome::Added
            );
        }
        assert_eq!(store.len(), 4, "at capacity");

        // A fifth distinct room evicts the least-recently-received entry
        // (room-0 was received first).
        let fifth = make_ad("room-4", make_topic(4));
        assert_eq!(
            store.receive(fifth, author, now + Duration::from_secs(4)),
            LegacyAdmitOutcome::Added
        );
        assert_eq!(store.len(), 4, "entry count stays capped");
        assert!(
            !store.contains(make_topic(0), author),
            "least-recently-received entry evicted"
        );
        assert!(store.contains(make_topic(4), author));
    }

    /// Repeated identical advertisements do not cause constant re-rendering:
    /// the same user-visible metadata re-broadcast by the same author is a
    /// `Duplicate` — one entry, no UI event. Dynamic liveness hints
    /// (member_count / last_activity) do not defeat the dedup.
    #[test]
    fn legacy_receive_dedupes_identical_advertisements() {
        let mut store = DirectoryStore::new();
        let topic = make_topic(1);
        let author = make_public_key(42);
        let now = Instant::now();

        let ad = make_ad("lounge", topic);
        assert_eq!(
            store.receive(ad.clone(), author, now),
            LegacyAdmitOutcome::Added,
            "first sighting is new"
        );
        assert_eq!(store.len(), 1);

        // Identical re-broadcast (even with a bumped member count / activity
        // hint): pure liveness refresh, no UI event.
        let mut refresh = ad.clone();
        refresh.member_count = 99;
        refresh.last_activity = 9_999;
        assert_eq!(
            store.receive(refresh, author, now + Duration::from_secs(1)),
            LegacyAdmitOutcome::Duplicate,
            "identical metadata re-broadcast is deduped"
        );
        assert_eq!(store.len(), 1, "no second card from repeated gossip");

        // A real metadata change (description edited) is a refresh, not a
        // duplicate — the UI may update the card.
        let mut changed = ad.clone();
        changed.description = "edited".to_string();
        assert_eq!(
            store.receive(changed, author, now + Duration::from_secs(2)),
            LegacyAdmitOutcome::Refreshed,
            "metadata change is a refresh"
        );
        assert_eq!(store.len(), 1, "still one entry per (topic, author)");
    }

    /// Advertisements are rate-limited per peer/authority: a flood from one
    /// author is dropped after the per-author budget, while other authors
    /// are unaffected (PDF Task 7.1 step 2).
    #[test]
    fn legacy_receive_rate_limits_per_author() {
        let mut store = DirectoryStore::new();
        let author_a = make_public_key(42);
        let author_b = make_public_key(43);
        let now = Instant::now();

        // Author A exhausts the per-author budget with distinct rooms.
        for i in 0..LEGACY_AD_RATE_LIMIT_MAX {
            let ad = make_ad(&format!("flood-{i}"), make_topic(i as u8));
            assert_eq!(
                store.receive(ad, author_a, now + Duration::from_secs(u64::from(i))),
                LegacyAdmitOutcome::Added,
                "within-budget advertisements admitted"
            );
        }
        let overflow = make_ad("overflow", make_topic(0xFE));
        assert_eq!(
            store.receive(overflow, author_a, now + Duration::from_secs(60)),
            LegacyAdmitOutcome::RateLimited,
            "author over budget is rate-limited"
        );

        // A different author is independent of A's budget.
        let fresh = make_ad("fresh", make_topic(0xFD));
        assert_eq!(
            store.receive(fresh, author_b, now + Duration::from_secs(61)),
            LegacyAdmitOutcome::Added,
            "a different author is not rate-limited by A's flood"
        );
    }
}
