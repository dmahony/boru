//! In-memory sender-side registry for direct file offers.
//!
//! A registry entry is deliberately local-only: the filesystem path is kept in
//! this module and is never part of a wire message. Receivers identify an
//! offer by its opaque [`FileOfferId`](crate::chat_core::protocol::FileOfferId)
//! and the authenticated sender identity.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use iroh::PublicKey;

use crate::chat_core::protocol::FileOfferId;

/// Default lifetime of a sender-side offer before it is eligible for pruning.
///
/// Offers are deliberately process-local. Twenty-four hours is long enough for
/// normal direct-chat use while bounding abandoned source paths in memory.
pub const DEFAULT_FILE_OFFER_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Local information needed to authorize and serve one direct file offer.
///
/// [`path`](Self::path) is never serialized or sent to a peer. The wire
/// message contains only the opaque ID, safe display name, and size.
#[derive(Debug, Clone)]
pub struct FileOffer {
    /// Opaque identifier announced to the recipient.
    pub id: FileOfferId,
    /// Peer authorized to request this offer.
    pub authorized_peer: PublicKey,
    /// Local filesystem path. This field must remain process-local.
    pub path: PathBuf,
    /// Safe basename shown to the recipient.
    pub display_name: String,
    /// File size in bytes at offer creation time.
    pub size: u64,
    /// Monotonic creation timestamp used for expiry.
    pub created_at: Instant,
    /// Filesystem modification time at offer creation time.
    pub modified_at: SystemTime,
}

impl FileOffer {
    /// Construct a file offer using the current time as its creation time.
    pub fn new(
        id: FileOfferId,
        authorized_peer: PublicKey,
        path: impl Into<PathBuf>,
        display_name: String,
        size: u64,
        modified_at: SystemTime,
    ) -> Self {
        Self {
            id,
            authorized_peer,
            path: path.into(),
            display_name,
            size,
            created_at: Instant::now(),
            modified_at,
        }
    }

    /// Return the local path for serving the offer.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// In-memory mapping from opaque offer IDs to local file information.
#[derive(Debug)]
pub struct FileOfferRegistry {
    offers: HashMap<FileOfferId, FileOffer>,
    ttl: Duration,
}

impl Default for FileOfferRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl FileOfferRegistry {
    /// Create an empty registry using [`DEFAULT_FILE_OFFER_TTL`].
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_FILE_OFFER_TTL)
    }

    /// Create an empty registry with a caller-selected expiry lifetime.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            offers: HashMap::new(),
            ttl,
        }
    }

    /// Insert an offer, returning the previous entry for the same ID if any.
    pub fn register(&mut self, offer: FileOffer) -> Option<FileOffer> {
        self.offers.insert(offer.id, offer)
    }

    /// Look up an offer by its opaque ID.
    pub fn get(&self, id: &FileOfferId) -> Option<&FileOffer> {
        self.offers.get(id)
    }

    /// Return whether an offer has exceeded its configured lifetime.
    pub fn is_expired(&self, offer: &FileOffer) -> bool {
        offer.created_at.elapsed() > self.ttl
    }

    /// Remove and return an offer by its opaque ID.
    pub fn remove(&mut self, id: &FileOfferId) -> Option<FileOffer> {
        self.offers.remove(id)
    }

    /// Remove offers older than the configured TTL and return the count removed.
    pub fn prune_expired(&mut self) -> usize {
        let ttl = self.ttl;
        let now = Instant::now();
        let before = self.offers.len();
        self.offers
            .retain(|_, offer| now.duration_since(offer.created_at) <= ttl);
        before - self.offers.len()
    }

    /// Remove expired offers and offers whose local source is no longer a file.
    /// The transfer handler still validates the source at request time.
    pub fn prune_stale(&mut self) -> usize {
        let ttl = self.ttl;
        let now = Instant::now();
        let before = self.offers.len();
        self.offers.retain(|_, offer| {
            now.duration_since(offer.created_at) <= ttl && offer.path().is_file()
        });
        before - self.offers.len()
    }

    /// Remove every offer when the owning application shuts down.
    pub fn clear(&mut self) {
        self.offers.clear();
    }

    /// Return the number of currently registered offers.
    pub fn len(&self) -> usize {
        self.offers.len()
    }

    /// Whether the registry contains no offers.
    pub fn is_empty(&self) -> bool {
        self.offers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(id: FileOfferId, peer: PublicKey, name: &str) -> FileOffer {
        FileOffer::new(
            id,
            peer,
            format!("/local-only/{name}"),
            name.to_owned(),
            42,
            SystemTime::UNIX_EPOCH,
        )
    }

    #[test]
    fn register_and_get_round_trip() {
        let peer = iroh::SecretKey::generate().public();
        let id = FileOfferId::generate();
        let expected = offer(id, peer, "report.pdf");
        let mut registry = FileOfferRegistry::new();
        assert!(registry.register(expected.clone()).is_none());

        let actual = registry.get(&id).expect("registered offer");
        assert_eq!(actual.id, expected.id);
        assert_eq!(actual.authorized_peer, expected.authorized_peer);
        assert_eq!(actual.path, expected.path);
        assert_eq!(actual.display_name, expected.display_name);
        assert_eq!(actual.size, expected.size);
        assert_eq!(actual.modified_at, expected.modified_at);
    }

    #[test]
    fn remove_returns_registered_offer() {
        let peer = iroh::SecretKey::generate().public();
        let id = FileOfferId::generate();
        let mut registry = FileOfferRegistry::new();
        registry.register(offer(id, peer, "photo.png"));

        assert!(registry.remove(&id).is_some());
        assert!(registry.get(&id).is_none());
        assert!(registry.remove(&id).is_none());
    }

    #[test]
    fn prune_expired_removes_old_and_keeps_fresh_offers() {
        let peer = iroh::SecretKey::generate().public();
        let old_id = FileOfferId::generate();
        let fresh_id = FileOfferId::generate();
        let mut registry = FileOfferRegistry::with_ttl(Duration::from_secs(60));
        let mut old = offer(old_id, peer, "old.bin");
        old.created_at = Instant::now() - Duration::from_secs(61);
        registry.register(old);
        registry.register(offer(fresh_id, peer, "fresh.bin"));

        assert_eq!(registry.prune_expired(), 1);
        assert!(registry.get(&old_id).is_none());
        assert!(registry.get(&fresh_id).is_some());
    }

    #[test]
    fn distinct_ids_allow_same_display_name() {
        let peer = iroh::SecretKey::generate().public();
        let first_id = FileOfferId::generate();
        let second_id = FileOfferId::generate();
        let mut registry = FileOfferRegistry::new();
        registry.register(offer(first_id, peer, "same-name.txt"));
        registry.register(offer(second_id, peer, "same-name.txt"));

        assert_eq!(registry.len(), 2);
        assert_eq!(
            registry.get(&first_id).unwrap().display_name,
            "same-name.txt"
        );
        assert_eq!(
            registry.get(&second_id).unwrap().display_name,
            "same-name.txt"
        );
    }

    #[test]
    fn prune_stale_removes_missing_sources_and_keeps_existing_sources() {
        let peer = iroh::SecretKey::generate().public();
        let temp = tempfile::tempdir().unwrap();
        let existing_path = temp.path().join("existing.bin");
        std::fs::write(&existing_path, b"content").unwrap();
        let existing_id = FileOfferId::generate();
        let missing_id = FileOfferId::generate();
        let mut registry = FileOfferRegistry::new();
        registry.register(FileOffer::new(
            existing_id,
            peer,
            existing_path,
            "existing.bin".to_owned(),
            7,
            SystemTime::now(),
        ));
        registry.register(offer(missing_id, peer, "missing.bin"));

        assert_eq!(registry.prune_stale(), 1);
        assert!(registry.get(&existing_id).is_some());
        assert!(registry.get(&missing_id).is_none());
    }

    #[test]
    fn clear_removes_all_offers_and_lookup_does_not_consume_them() {
        let peer = iroh::SecretKey::generate().public();
        let id = FileOfferId::generate();
        let mut registry = FileOfferRegistry::new();
        registry.register(offer(id, peer, "repeat.bin"));
        assert!(registry.get(&id).is_some());
        assert!(registry.get(&id).is_some());
        registry.clear();
        assert!(registry.is_empty());
    }
}
