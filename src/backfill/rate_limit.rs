//! Per-peer rate-limiting state for the server side of the backfill protocol.
//!
//! Tracks in-flight requests per remote [`PublicKey`] so a single peer cannot
//! pile on unbounded concurrent backfill work.  The state is plain (no I/O);
//! the [`MAX_ACTIVE_PEERS`](crate::backfill::MAX_ACTIVE_PEERS) cap bounds the
//! map size.

use std::collections::HashMap;
use std::time::Instant;

use iroh::PublicKey;

use crate::backfill::MAX_ACTIVE_PEERS;

// ── Per-peer rate-limiting state (server side) ─────────────────────────────────

/// Tracks in-flight backfill requests per remote peer.
#[derive(Debug, Default)]
pub(crate) struct BackfillRateLimit {
    active: HashMap<PublicKey, Instant>,
}

impl BackfillRateLimit {
    /// Try to register an incoming request.
    /// Returns `true` if accepted, `false` if a request from this peer is already in flight
    /// or the active set has reached [`MAX_ACTIVE_PEERS`].
    pub(crate) fn try_accept(&mut self, peer: PublicKey) -> bool {
        if self.active.contains_key(&peer) {
            return false;
        }
        if self.active.len() >= MAX_ACTIVE_PEERS {
            return false;
        }
        self.active.insert(peer, Instant::now());
        true
    }

    /// Remove a peer from the active set (call after request completes).
    pub(crate) fn release(&mut self, peer: &PublicKey) {
        self.active.remove(peer);
    }

    /// Prune stale entries (requests that hung without cleanup).
    /// Returns the number of active entries remaining after pruning.
    pub(crate) fn prune_stale(&mut self, max_age: std::time::Duration) -> usize {
        let now = Instant::now();
        self.active
            .retain(|_, started| now.duration_since(*started) < max_age);
        self.active.len()
    }
}
