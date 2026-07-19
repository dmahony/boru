//! Rate-limit enforcement for catalogue protocol connections.
//!
//! Two independent limiters:
//!
//! 1. **Concurrency limiter** (`CatalogueConcurrencyLimiter`) — caps the number
//!    of catalogue connections being served simultaneously.  When the limit is
//!    reached, new connections receive a [`CatalogResponse::Error`] with
//!    [`CatalogErrorCode::Busy`].
//!
//! 2. **Per-peer rate limiter** (`PeerCatalogueRateLimiter`) — limits the number
//!    of requests from a single peer over a sliding time window.  When a peer
//!    exceeds the limit, new requests receive
//!    [`CatalogErrorCode::RateLimited`].
//!
//! Both limiters use `Mutex` (not `tokio::sync::Mutex`) because their
//! critical sections are synchronous and never hold a lock across an `.await`
//! point.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::catalogue_protocol::{CatalogErrorCode, CatalogResponse, CatalogWireResponse};
use crate::protocol_version::{write_frame, CATALOGUE_RETRIEVAL_V1};

// ── Compile-time rate-limit constants ──────────────────────────────────────

/// Maximum concurrent catalogue connections being served.
///
/// When this limit is reached, new connections receive a `Busy` response.
pub const MAX_CONCURRENT_CATALOGUE_CONNECTIONS: usize = 16;

/// Maximum catalogue requests per peer over a sliding time window.
pub const MAX_CATALOGUE_REQUESTS_PER_PEER: u32 = 32;

/// Duration of the sliding rate-limit window for per-peer accounting.
pub const CATALOGUE_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(10);

// ── Configuration ──────────────────────────────────────────────────────────

/// Tuning parameters for catalogue rate limits.
///
/// Every value has a sensible default — callers that don't need custom tuning
/// can use [`CatalogueRateConfig::default()`].
#[derive(Debug, Clone)]
pub struct CatalogueRateConfig {
    /// Maximum number of catalogue connections being served concurrently.
    ///
    /// When this limit is reached, new connections receive a `Busy` response.
    /// Default: 16.
    pub max_concurrent_connections: usize,

    /// Maximum number of catalogue requests per peer in a sliding window.
    /// Default: 10.
    pub max_requests_per_peer: u32,

    /// Duration of the sliding rate-limit window for per-peer accounting.
    /// Default: 10 seconds.
    pub rate_limit_window: Duration,
}

impl Default for CatalogueRateConfig {
    fn default() -> Self {
        Self {
            max_concurrent_connections: 16,
            max_requests_per_peer: 10,
            rate_limit_window: Duration::from_secs(10),
        }
    }
}

// ── Concurrency Limiter ────────────────────────────────────────────────────

/// Bounds the number of catalogue connections being served simultaneously.
///
/// Acquires a [`tokio::sync::Semaphore`] permit on connection accept and
/// holds it for the lifetime of the serving task.  When the semaphore is
/// exhausted, `try_acquire` returns `None` and the caller should respond
/// with `Busy`.
#[derive(Debug)]
pub struct CatalogueConcurrencyLimiter {
    semaphore: Arc<Semaphore>,
}

impl CatalogueConcurrencyLimiter {
    /// Create a new concurrency limiter with the given maximum.
    pub fn new(max: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max.max(1))),
        }
    }

    /// Try to acquire a permit that keeps one concurrent slot occupied.
    ///
    /// Returns `Some(permit)` when a slot is available, `None` when the
    /// concurrency limit has been reached.
    pub fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        self.semaphore.clone().try_acquire_owned().ok()
    }
}

// ── Per-Peer Rate Limiter ──────────────────────────────────────────────────

/// Per-peer sliding-window rate limiter.
///
/// Each peer is tracked independently.  A fixed number of requests are
/// allowed within a sliding time window; excess requests are rejected.
///
/// The implementation uses a `VecDeque<Instant>` per peer, purging expired
/// entries on each check.
#[derive(Debug)]
pub struct PeerCatalogueRateLimiter {
    windows: Mutex<HashMap<String, VecDeque<Instant>>>,
    max_requests: u32,
    window_duration: Duration,
}

impl PeerCatalogueRateLimiter {
    /// Create a new per-peer rate limiter.
    pub fn new(max_requests: u32, window_duration: Duration) -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            max_requests: max_requests.max(1),
            window_duration,
        }
    }

    /// Check whether `peer` is within the rate limit and record this request.
    ///
    /// Returns `true` when the request is allowed, `false` when the peer
    /// has exceeded the rate limit (and should receive `RateLimited`).
    pub fn check_and_record(&self, peer: &str) -> bool {
        let mut windows = self.windows.lock().expect("peer rate limiter poisoned");
        let now = Instant::now();
        let window_start = now - self.window_duration;

        let entries = windows.entry(peer.to_owned()).or_default();

        // ── Purge expired entries ──────────────────────────────────
        // VecDeque is ordered by insertion time, so we pop from the front
        // while entries are older than the window start.
        loop {
            match entries.front() {
                Some(&t) if t < window_start => {
                    entries.pop_front();
                }
                _ => break,
            }
        }

        // ── Check limit ────────────────────────────────────────────
        if entries.len() as u32 >= self.max_requests {
            return false; // Rate limited — do NOT record this request.
        }

        entries.push_back(now);
        true
    }

    /// Reset rate-limit state for `peer`.
    ///
    /// Used in tests to clear state between scenarios without creating a new
    /// limiter.
    pub fn reset_peer(&self, peer: &str) {
        let mut windows = self.windows.lock().expect("peer rate limiter poisoned");
        windows.remove(peer);
    }
}

// ── Response helpers ───────────────────────────────────────────────────────

/// Write a `Busy` error response on `send` and finish the stream.
///
/// This is used when the concurrency limit prevents a catalogue connection
/// from being served.  It writes the response synchronously through the
/// async frame helpers.
pub async fn write_busy_response(
    send: &mut iroh::endpoint::SendStream,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let response = CatalogResponse::error(CatalogErrorCode::Busy, "server busy");
    let wire_resp = CatalogWireResponse::new(response);
    let bytes = postcard::to_stdvec(&wire_resp)?;
    write_frame(send, CATALOGUE_RETRIEVAL_V1, &bytes).await?;
    Ok(())
}

/// Write a `RateLimited` error response on `send` and finish the stream.
pub async fn write_rate_limited_response(
    send: &mut iroh::endpoint::SendStream,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let response = CatalogResponse::error(CatalogErrorCode::RateLimited, "too many requests");
    let wire_resp = CatalogWireResponse::new(response);
    let bytes = postcard::to_stdvec(&wire_resp)?;
    write_frame(send, CATALOGUE_RETRIEVAL_V1, &bytes).await?;
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // ── Concurrency limiter ──────────────────────────────────────────

    #[test]
    fn test_concurrency_limiter_acquire_release() {
        let limiter = CatalogueConcurrencyLimiter::new(2);

        let p1 = limiter.try_acquire();
        assert!(p1.is_some(), "first acquire should succeed");

        let p2 = limiter.try_acquire();
        assert!(p2.is_some(), "second acquire should succeed");

        // Third acquire should fail (limit = 2).
        let p3 = limiter.try_acquire();
        assert!(p3.is_none(), "third acquire should fail");

        // Release one slot.
        drop(p1);

        // Now we can acquire again.
        let p4 = limiter.try_acquire();
        assert!(p4.is_some(), "acquire after release should succeed");
    }

    #[test]
    fn test_concurrency_limiter_zero_max() {
        // Even when configured with 0, the limiter ensures at least 1.
        let limiter = CatalogueConcurrencyLimiter::new(0);
        assert!(limiter.try_acquire().is_some(), "minimum 1 concurrent slot");
    }

    // ── Per-peer rate limiter ────────────────────────────────────────

    #[test]
    fn test_per_peer_rate_limiter_allows_within_limit() {
        let limiter = PeerCatalogueRateLimiter::new(3, Duration::from_secs(60));
        let peer = "peer1";

        assert!(limiter.check_and_record(peer), "request 1 allowed");
        assert!(limiter.check_and_record(peer), "request 2 allowed");
        assert!(limiter.check_and_record(peer), "request 3 allowed");
    }

    #[test]
    fn test_per_peer_rate_limiter_rejects_excess() {
        let limiter = PeerCatalogueRateLimiter::new(3, Duration::from_secs(60));
        let peer = "peer1";

        assert!(limiter.check_and_record(peer), "request 1 allowed");
        assert!(limiter.check_and_record(peer), "request 2 allowed");
        assert!(limiter.check_and_record(peer), "request 3 allowed");
        assert!(!limiter.check_and_record(peer), "request 4 rejected");
    }

    #[test]
    fn test_per_peer_rate_limiter_different_peers_independent() {
        let limiter = PeerCatalogueRateLimiter::new(2, Duration::from_secs(60));

        // peer1 exhausts its budget.
        assert!(limiter.check_and_record("peer1"), "peer1 request 1");
        assert!(limiter.check_and_record("peer1"), "peer1 request 2");
        assert!(
            !limiter.check_and_record("peer1"),
            "peer1 request 3 rejected"
        );

        // peer2 unaffected.
        assert!(limiter.check_and_record("peer2"), "peer2 request 1");
        assert!(limiter.check_and_record("peer2"), "peer2 request 2");
        assert!(
            !limiter.check_and_record("peer2"),
            "peer2 request 3 rejected"
        );
    }

    #[test]
    fn test_per_peer_rate_limiter_window_expiry() {
        // Use a very short window so old entries expire.
        let limiter = PeerCatalogueRateLimiter::new(2, Duration::from_millis(20));
        let peer = "peer1";

        assert!(limiter.check_and_record(peer), "request 1 allowed");
        assert!(limiter.check_and_record(peer), "request 2 allowed");
        assert!(!limiter.check_and_record(peer), "request 3 rejected");

        // Wait for the window to expire.
        std::thread::sleep(Duration::from_millis(30));

        // Now requests should be allowed again (old window expired).
        assert!(
            limiter.check_and_record(peer),
            "request after window expiry allowed"
        );
    }

    #[test]
    fn test_per_peer_rate_limiter_reset() {
        let limiter = PeerCatalogueRateLimiter::new(2, Duration::from_secs(60));
        let peer = "peer1";

        assert!(limiter.check_and_record(peer), "request 1 allowed");
        assert!(limiter.check_and_record(peer), "request 2 allowed");
        assert!(!limiter.check_and_record(peer), "request 3 rejected");

        limiter.reset_peer(peer);

        // After reset, requests are allowed again.
        assert!(
            limiter.check_and_record(peer),
            "request after reset allowed"
        );
    }

    /// Ensures the rate limiter works with very large time windows (no overflow).
    #[test]
    fn test_per_peer_rate_limiter_large_window() {
        let limiter = PeerCatalogueRateLimiter::new(1, Duration::from_secs(365 * 24 * 3600)); // 1 year
        let peer = "peer1";

        assert!(limiter.check_and_record(peer), "first request allowed");
        assert!(!limiter.check_and_record(peer), "second request rejected");
    }

    /// The rate limiter accepts the minimum allowed max_requests (1).
    #[test]
    fn test_per_peer_rate_limiter_min_requests() {
        let limiter = PeerCatalogueRateLimiter::new(0, Duration::from_secs(60));
        // Even 0 is clamped to 1.
        assert!(limiter.check_and_record("p"), "min 1 request allowed");
    }

    // ── Response helpers ─────────────────────────────────────────────
    //
    // The write helpers are async and require a QUIC transport layer,
    // so they are not tested in isolation.  They are exercised by the
    // integration tests in test_remote_catalogue_integration.rs.
}
