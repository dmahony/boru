//! Single-use download-descriptor nonce / replay protection (authorization).
//!
//! Owns [`NonceStore`], the in-memory store that marks descriptor nonces
//! consumed so a replayed descriptor cannot be accepted twice.

use std::collections::HashMap;
use std::sync::Mutex;

/// Outcome of checking a nonce against the [`NonceStore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonceCheck {
    /// The nonce is new — no replay detected.
    Accepted,
    /// The nonce was already consumed — replay attempt.
    Replayed,
}

/// In-memory store for tracking used download-descriptor nonces.
///
/// Each nonce is stored with an expiry timestamp (`expires_at_ms`).  A
/// nonce that appears in the store is considered *consumed* and will not
/// be accepted again, even if the descriptor's TTL has not yet elapsed.
///
/// # Replay-prevention policy
///
/// - Descriptors are **single-use**: a nonce is marked consumed upon
///   first presentation to the download protocol.
/// - Replayed descriptors (same nonce) are rejected with
///   [`DescriptorVerification::NonceReused`](crate::file_access_protocol::DescriptorVerification::NonceReused).
/// - Expired entries are cleaned up lazily on every [`check`](Self::check) /
///   [`check_and_mark`](Self::check_and_mark) call, keeping the store
///   bounded to at most one TTL window's worth of entries.
///
/// # Concurrency
///
/// `NonceStore` is `Send + Sync`, suitable for sharing via `Arc` between
/// the issuance handler and the download transfer handler.
#[derive(Debug)]
pub struct NonceStore {
    /// Map from nonce bytes to the descriptor's `expires_at_ms`.
    seen: Mutex<HashMap<[u8; 32], u64>>,
}

impl NonceStore {
    /// Create a new empty nonce store.
    pub fn new() -> Self {
        Self {
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Check whether `nonce` has already been consumed.
    ///
    /// Returns [`NonceCheck::Accepted`] if the nonce is new (or its
    /// prior entry has expired).  Returns [`NonceCheck::Replayed`] if
    /// the nonce is already in the store and its expiry has not passed.
    ///
    /// A side-effect-free check — the nonce is NOT marked.
    pub fn check(&self, nonce: &[u8; 32], now_ms: u64) -> NonceCheck {
        let mut map = self.seen.lock().expect("NonceStore lock poisoned");
        self.evict_expired(&mut map, now_ms);

        if map.contains_key(nonce) {
            NonceCheck::Replayed
        } else {
            NonceCheck::Accepted
        }
    }

    /// Atomically check and mark a nonce as consumed.
    ///
    /// If the nonce is new (or its prior entry has expired), it is
    /// inserted with the given `expires_at_ms` and `Accepted` is
    /// returned.  If it is already tracked and unexpired, `Replayed`
    /// is returned and the map is unchanged.
    pub fn check_and_mark(&self, nonce: [u8; 32], expires_at_ms: u64, now_ms: u64) -> NonceCheck {
        let mut map = self.seen.lock().expect("NonceStore lock poisoned");
        self.evict_expired(&mut map, now_ms);

        if map.contains_key(&nonce) {
            return NonceCheck::Replayed;
        }

        map.insert(nonce, expires_at_ms);
        NonceCheck::Accepted
    }

    /// Remove all nonces whose expiry has passed.
    fn evict_expired(&self, map: &mut HashMap<[u8; 32], u64>, now_ms: u64) {
        map.retain(|_, expires_at| *expires_at > now_ms);
    }

    /// Return the number of tracked (unexpired) nonces.
    ///
    /// Useful for testing and metrics.
    pub fn len(&self) -> usize {
        self.seen.lock().expect("NonceStore lock poisoned").len()
    }

    /// Return true if the store holds no unexpired nonces.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for NonceStore {
    fn default() -> Self {
        Self::new()
    }
}
