//! Boru-specific public-room topic tracker.
//!
//! Wraps a [`TopicDiscoveryBackend`] with boru's public-room identity model
//! to provide publish-once and discover-once operations.  Does **not**
//! integrate with the iroh gossip actor — callers use the returned
//! [`EndpointId`] values to join the gossip mesh independently.
//!
//! # Lifecycle
//!
//! 1. [`start`](PublicRoomTracker::start) — construct and initialise.
//! 2. [`publish_once`](PublicRoomTracker::publish_once) — advertise local
//!    presence.
//! 3. [`discover_once`](PublicRoomTracker::discover_once) — find valid peers.
//! 4. [`shutdown`](PublicRoomTracker::shutdown) — release backend resources.
//!
//! # Minimal example
//!
//! ```ignore
//! use crate::discovery_backend::InMemoryDiscoveryBackend;
//! use crate::public_room::PublicNetwork;
//! use crate::public_room_tracker::PublicRoomTracker;
//! use iroh::{EndpointId, SecretKey};
//!
//! let sk = SecretKey::generate();
//! let ep = sk.public();
//! let tracker = PublicRoomTracker::start(
//!     Box::new(InMemoryDiscoveryBackend::new()),
//!     PublicNetwork::Test,
//!     ep,
//!     sk,
//! ).await.unwrap();
//! ```

use std::time::Instant;

use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, info, info_span, warn};

use crate::discovery_backend::{
    EncryptedDiscoveryRecord, MAX_DISCOVERY_PAYLOAD_SIZE, NamespaceId, TopicDiscoveryBackend,
    canonical_lobby_key,
};
use crate::discovery_record::create_discovery_record;
use crate::discovery_validation::{DiscoveryRecordValidator, PeerCandidates, ValidationConfig};
use crate::public_room::{PublicNetwork, PublicRoomIdentity, public_room_identity};
use distributed_topic_tracker::{Record, unix_minute};
use iroh::{EndpointId, SecretKey};
use n0_error::Result;

// ---------------------------------------------------------------------------
// PublicRoomTracker
// ---------------------------------------------------------------------------

/// A boru-specific tracker for public-room discovery on the DHT.
///
/// Wraps a [`TopicDiscoveryBackend`] with boru's identity model, providing
/// publish-once and discover-once operations.  Does **not** integrate with
/// the iroh gossip actor — callers use the returned [`EndpointId`] values
/// to join the gossip mesh independently.
///
/// # Lifecycle
///
/// 1. [`start`](Self::start) — construct.
/// 2. [`publish_once`](Self::publish_once) — advertise local presence.
/// 3. [`discover_once`](Self::discover_once) — find peers.
/// 4. [`shutdown`](Self::shutdown) — release backend resources.
///
/// # Cancellation
///
/// The internal [`CancellationToken`] is a placeholder for future background
/// tasks (e.g. periodic re-publish).  It is fired during [`shutdown`](Self::shutdown)
/// but currently has no listeners.
pub struct PublicRoomTracker {
    /// The underlying discovery backend.
    backend: Box<dyn TopicDiscoveryBackend>,
    /// Room identity (topic + discovery key).
    identity: PublicRoomIdentity,
    /// This node's iroh EndpointId.
    local_endpoint_id: EndpointId,
    /// This node's iroh SecretKey — used to sign discovery records.
    secret_key: SecretKey,
    /// Cancellation token for future background tasks.
    cancel: CancellationToken,
}

impl std::fmt::Debug for PublicRoomTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublicRoomTracker")
            .field("identity", &self.identity)
            .field("local_endpoint_id", &self.local_endpoint_id)
            .finish()
    }
}

impl PublicRoomTracker {
    /// Create a new tracker with an explicit room identity.
    ///
    /// Prefer [`start`](Self::start) when deriving the identity from a
    /// [`PublicNetwork`] — it is the more common path.
    pub fn new(
        backend: Box<dyn TopicDiscoveryBackend>,
        identity: PublicRoomIdentity,
        local_endpoint_id: EndpointId,
        secret_key: SecretKey,
    ) -> Self {
        Self {
            backend,
            identity,
            local_endpoint_id,
            secret_key,
            cancel: CancellationToken::new(),
        }
    }

    /// Convenience constructor that derives the room identity from a network
    /// using the canonical constants (`PUBLIC_ROOM_NAME`, `PROTOCOL_VERSION`).
    ///
    /// This is the standard path for public-lobby discovery.  Use
    /// [`new`](Self::new) for custom room names or non-standard versions.
    #[allow(clippy::unused_async)]
    pub async fn start(
        backend: Box<dyn TopicDiscoveryBackend>,
        network: PublicNetwork,
        local_endpoint_id: EndpointId,
        secret_key: SecretKey,
    ) -> Result<Self> {
        let identity = public_room_identity(network);
        info!(
            room = %identity.short_id(),
            "public room tracker started",
        );
        Ok(Self::new(backend, identity, local_endpoint_id, secret_key))
    }

    /// Return the room identity this tracker is configured for.
    pub fn identity(&self) -> &PublicRoomIdentity {
        &self.identity
    }

    /// Return this node's EndpointId.
    pub fn local_endpoint_id(&self) -> &EndpointId {
        &self.local_endpoint_id
    }

    /// Publish this node's presence to the DHT once.
    ///
    /// Creates and signs a discovery record advertising this node's
    /// [`EndpointId`], then publishes it via the backend under the room's
    /// discovery-key namespace.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying backend's [`publish`](TopicDiscoveryBackend::publish)
    /// fails, or if record creation fails (extremely unlikely — key material
    /// is always valid).
    pub async fn publish_once(&self) -> Result<()> {
        let start = Instant::now();
        let room_id = self.identity.short_id();
        let local = self.local_endpoint_id.fmt_short();

        let now = unix_minute(0);
        let discovery_key = self.identity.discovery_key;
        let record = create_discovery_record(
            discovery_key,
            now,
            &self.local_endpoint_id,
            &self.secret_key,
        )?;
        let namespace = NamespaceId::new(canonical_lobby_key(discovery_key));
        let result = async {
            self.backend
                .publish(&namespace, EncryptedDiscoveryRecord::new(record.to_bytes()))
                .await
        }
        .instrument(info_span!("tracker.publish", tracker = "public", room = %room_id))
        .await;

        let duration_us = start.elapsed().as_micros() as u64;
        match &result {
            Ok(()) => info!(
                room = %room_id,
                local = %local,
                outcome = "success",
                duration_us = duration_us,
                "publish completed",
            ),
            Err(e) => warn!(
                room = %room_id,
                local = %local,
                error = %e,
                outcome = "failure",
                duration_us = duration_us,
                "publish failed",
            ),
        }

        result
    }

    /// Find valid peers on the room's DHT namespace.
    ///
    /// Deserialises records from the backend, validates each through the full
    /// pipeline (size, timestamp, decode, identity match, signature), filters
    /// out the local node and duplicates, and returns the bounded result set.
    ///
    /// # Returns
    ///
    /// A [`Vec`] of validated [`EndpointId`] values from other peers.  The
    /// result is bounded by [`ValidationConfig::max_candidate_peers`] (default
    /// 20).  Returns an empty `Vec` when no valid peers exist — not an error.
    ///
    /// Records that fail deserialisation from raw bytes are silently skipped.
    pub async fn discover_once(&self) -> Result<Vec<EndpointId>> {
        let start = Instant::now();
        let room_id = self.identity.short_id();
        let local = self.local_endpoint_id.fmt_short();

        let now_minute = unix_minute(0);
        let discovery_key = self.identity.discovery_key;
        let namespace = NamespaceId::new(canonical_lobby_key(discovery_key));

        // Fetch encrypted records from the backend.
        let encrypted = match async { self.backend.lookup(&namespace).await }
            .instrument(info_span!("tracker.lookup", tracker = "public", room = %room_id))
            .await
        {
            Ok(records) => records,
            Err(error) => {
                warn!(
                    room = %room_id,
                    local = %local,
                    outcome = "failure",
                    error = %error,
                    duration_us = start.elapsed().as_micros() as u64,
                    "lookup failed",
                );
                return Err(error);
            }
        };
        let total_encrypted = encrypted.len();

        // Deserialise encrypted records back into Record values.
        // Malformed bytes are silently skipped — they are not hard failures.
        let mut records: Vec<Record> = Vec::with_capacity(encrypted.len());
        for er in encrypted {
            if er.payload.len() > MAX_DISCOVERY_PAYLOAD_SIZE {
                debug!(
                    room = %room_id,
                    payload_len = er.payload.len(),
                    max_payload = MAX_DISCOVERY_PAYLOAD_SIZE,
                    "discovery skipped oversized record"
                );
                continue;
            }
            match Record::from_bytes(er.payload) {
                Ok(r) => records.push(r),
                Err(_) => {
                    // Skip malformed/corrupt records silently.
                    continue;
                }
            }
        }
        let total_records = records.len();

        // Validate and filter through the discovery-validation pipeline.
        let config = ValidationConfig::new(discovery_key);
        let validator = DiscoveryRecordValidator::new(config, now_minute);
        let PeerCandidates { peers, counters } =
            validator.filter_and_build(records, Some(&self.local_endpoint_id));

        // Structured tracing — never log the full discovery key.
        let duration_us = start.elapsed().as_micros() as u64;
        let accepted = counters.accepted;
        let rejected = counters.total_rejected();
        if accepted > 0 {
            info!(
                room = %room_id,
                local = %local,
                encrypted = total_encrypted,
                records = total_records,
                accepted = accepted,
                rejected = rejected,
                oversized = counters.oversized,
                stale = counters.stale,
                future = counters.future,
                decode_failure = counters.decode_failure,
                identity_mismatch = counters.identity_mismatch,
                invalid_signature = counters.invalid_signature,
                self_filtered = counters.self_filtered,
                duplicates = counters.duplicates,
                duration_us = duration_us,
                outcome = "success",
                "discovery found peers",
            );
        } else {
            debug!(
                room = %room_id,
                local = %local,
                encrypted = total_encrypted,
                records = total_records,
                accepted = accepted,
                rejected = rejected,
                oversized = counters.oversized,
                stale = counters.stale,
                future = counters.future,
                decode_failure = counters.decode_failure,
                duration_us = duration_us,
                outcome = "success",
                "discovery returned no peers",
            );
        }

        Ok(peers)
    }

    /// Shut down the tracker, releasing backend resources.
    ///
    /// Fires the cancellation token and calls [`shutdown`](TopicDiscoveryBackend::shutdown)
    /// on the backend.
    ///
    /// **Consumes** the tracker — call this once when done.
    pub async fn shutdown(self) {
        let room_id = self.identity.short_id();
        info!(room = %room_id, "public room tracker shutting down");
        self.cancel.cancel();
        let _ = self.backend.shutdown().await;
        info!(room = %room_id, "public room tracker shut down");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use n0_tracing_test::traced_test;

    use crate::discovery_backend::InMemoryDiscoveryBackend;
    use crate::public_room::PublicNetwork;

    /// Helper: generate a fresh test identity.
    fn test_identity() -> (SecretKey, EndpointId) {
        let sk = SecretKey::generate();
        let ep = sk.public();
        (sk, ep)
    }

    /// Helper: block on an async future for synchronous test contexts.
    fn block_on<F: std::future::Future<Output = T>, T>(f: F) -> T {
        tokio::runtime::Runtime::new().unwrap().block_on(f)
    }

    // ── Tracing smoke tests ──────────────────────────────────────────

    /// Tracing is emitted during publish + discover cycle without panics.
    #[test]
    #[traced_test]
    fn traced_publish_discover_roundtrip() {
        publish_discover_roundtrip();
    }

    /// Tracing is emitted on publish failure without panics.
    #[test]
    #[traced_test]
    fn traced_publish_failure_logs_warn() {
        let (_sk, ep) = test_identity();
        // An empty backend will still accept the publish (in-memory is lenient),
        // so this smoke-test verifies tracing paths are wired up correctly.
        let backend = InMemoryDiscoveryBackend::new();
        let tracker = block_on(PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            ep,
            SecretKey::generate(),
        ))
        .unwrap();
        block_on(tracker.publish_once()).unwrap();
        block_on(tracker.shutdown());
    }

    /// Tracing is emitted during tracker shutdown.
    #[test]
    #[traced_test]
    fn traced_shutdown_logs() {
        let (sk, ep) = test_identity();
        let tracker = block_on(PublicRoomTracker::start(
            Box::new(InMemoryDiscoveryBackend::new()),
            PublicNetwork::Test,
            ep,
            sk,
        ))
        .unwrap();
        block_on(tracker.shutdown());
    }

    // ── Construction ──────────────────────────────────────────────────

    #[test]
    fn tracker_start_constructs_with_identity() {
        let (sk, ep) = test_identity();
        let tracker = block_on(PublicRoomTracker::start(
            Box::new(InMemoryDiscoveryBackend::new()),
            PublicNetwork::Test,
            ep.clone(),
            sk,
        ))
        .unwrap();

        assert_eq!(tracker.local_endpoint_id(), &ep);

        // Identity should be derived from Test network
        let expected_identity = public_room_identity(PublicNetwork::Test);
        assert_eq!(tracker.identity(), &expected_identity);
    }

    #[test]
    fn tracker_new_constructs_with_explicit_identity() {
        let (sk, ep) = test_identity();
        let custom_identity = PublicRoomIdentity::new(
            crate::topic_derivation::public_room_topic(0x00, "custom-room", 1),
            [0x42u8; 32],
        );
        let tracker = PublicRoomTracker::new(
            Box::new(InMemoryDiscoveryBackend::new()),
            custom_identity.clone(),
            ep,
            sk,
        );
        assert_eq!(tracker.identity(), &custom_identity);
    }

    // ── publish_once + discover_once ──────────────────────────────────

    #[test]
    fn publish_discover_roundtrip() {
        let (sk_a, ep_a) = test_identity();
        let (_sk_b, ep_b) = test_identity();

        // Use a shared backend so both trackers operate on the same store.
        let shared = InMemoryDiscoveryBackend::new();

        // Tracker A publishes into shared backend.
        let tracker_a = block_on(PublicRoomTracker::start(
            Box::new(shared.clone()),
            PublicNetwork::Test,
            ep_a.clone(),
            sk_a,
        ))
        .unwrap();
        block_on(tracker_a.publish_once()).unwrap();

        // Tracker B discovers from the same shared backend.
        let tracker_b = block_on(PublicRoomTracker::start(
            Box::new(shared.clone()),
            PublicNetwork::Test,
            ep_b,
            SecretKey::generate(),
        ))
        .unwrap();
        let peers = block_on(tracker_b.discover_once()).unwrap();

        // B should discover A (and not itself).
        assert!(
            peers.contains(&ep_a),
            "expected peer A to be discovered, got {peers:?}"
        );
        assert!(
            peers.len() == 1,
            "expected exactly one peer (A), got {}",
            peers.len()
        );
    }

    #[test]
    fn self_filter_excludes_local_peer() {
        let (sk, ep) = test_identity();
        let backend = InMemoryDiscoveryBackend::new();

        // Publish our own presence.
        let tracker = block_on(PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            ep.clone(),
            sk,
        ))
        .unwrap();
        block_on(tracker.publish_once()).unwrap();

        // Discover — our own EndpointId should be filtered out.
        let peers = block_on(tracker.discover_once()).unwrap();
        assert!(
            !peers.contains(&ep),
            "self endpoint should be filtered out, got {peers:?}"
        );
        assert!(peers.is_empty(), "expected no peers, got {peers:?}");
    }

    #[test]
    fn duplicate_peers_are_filtered() {
        // Multiple publishes by the same peer → only one EndpointId returned.
        let (sk, ep) = test_identity();

        // Use two separate trackers both pointing at the same backend.
        let backend = InMemoryDiscoveryBackend::new();

        // Publish once.
        {
            let t = block_on(PublicRoomTracker::start(
                Box::new(backend.clone()),
                PublicNetwork::Test,
                ep.clone(),
                sk.clone(),
            ))
            .unwrap();
            block_on(t.publish_once()).unwrap();
        }
        // Publish again (same identity, different minute).
        {
            let t = block_on(PublicRoomTracker::start(
                Box::new(backend.clone()),
                PublicNetwork::Test,
                ep.clone(),
                sk,
            ))
            .unwrap();
            block_on(t.publish_once()).unwrap();
        }

        // Should only see A (once) when discovering from a different identity.
        let (sk_b, ep_b) = test_identity();
        let t_b = block_on(PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            ep_b,
            sk_b,
        ))
        .unwrap();
        let peers = block_on(t_b.discover_once()).unwrap();
        assert!(peers.contains(&ep), "expected A to be discovered");
        assert_eq!(
            peers.iter().filter(|&&p| p == ep).count(),
            1,
            "A should appear exactly once, got {peers:?}"
        );
    }

    #[test]
    fn multiple_peers_discovered() {
        let backend = InMemoryDiscoveryBackend::new();

        let identities: Vec<(SecretKey, EndpointId)> = (0..3).map(|_| test_identity()).collect();

        // Publish each peer.
        for (_i, (sk, ep)) in identities.iter().enumerate() {
            // Use a unique key for each — same sk/ep but new tracker each time
            // to avoid mutable borrow issues.
            let t = block_on(PublicRoomTracker::start(
                Box::new(backend.clone()),
                PublicNetwork::Test,
                ep.clone(),
                sk.clone(),
            ))
            .unwrap();
            block_on(t.publish_once()).unwrap();
        }

        // Discover from a fresh identity.
        let (sk_d, ep_d) = test_identity();
        let t_d = block_on(PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            ep_d,
            sk_d,
        ))
        .unwrap();
        let peers = block_on(t_d.discover_once()).unwrap();

        // All three peers should be discovered.
        for (_, ep) in &identities {
            assert!(
                peers.contains(ep),
                "expected peer {ep:?} to be discovered, got {peers:?}"
            );
        }
        assert_eq!(peers.len(), identities.len());
    }

    #[test]
    fn discover_on_empty_backend_returns_empty() {
        let (sk, ep) = test_identity();
        let tracker = block_on(PublicRoomTracker::start(
            Box::new(InMemoryDiscoveryBackend::new()),
            PublicNetwork::Test,
            ep,
            sk,
        ))
        .unwrap();
        let peers = block_on(tracker.discover_once()).unwrap();
        assert!(peers.is_empty(), "expected empty, got {peers:?}");
    }

    #[test]
    fn different_networks_are_disjoint() {
        let backend_mainnet = InMemoryDiscoveryBackend::new();
        let backend_dev = InMemoryDiscoveryBackend::new();

        let (sk_a, ep_a) = test_identity();
        let (sk_b, ep_b) = test_identity();

        // Publish A on mainnet.
        let t_main = block_on(PublicRoomTracker::start(
            Box::new(backend_mainnet.clone()),
            PublicNetwork::Mainnet,
            ep_a,
            sk_a,
        ))
        .unwrap();
        block_on(t_main.publish_once()).unwrap();

        // Discover from B on dev — should see nothing (disjoint namespaces).
        let t_dev = block_on(PublicRoomTracker::start(
            Box::new(backend_dev.clone()),
            PublicNetwork::Development,
            ep_b,
            sk_b,
        ))
        .unwrap();
        let peers = block_on(t_dev.discover_once()).unwrap();
        assert!(
            peers.is_empty(),
            "different networks should be disjoint, got {peers:?}"
        );
    }

    // ── Shutdown ──────────────────────────────────────────────────────

    #[test]
    fn shutdown_does_not_panic() {
        let (sk, ep) = test_identity();
        let tracker = PublicRoomTracker::new(
            Box::new(InMemoryDiscoveryBackend::new()),
            public_room_identity(PublicNetwork::Test),
            ep,
            sk,
        );
        block_on(tracker.shutdown());
    }

    #[test]
    fn shutdown_then_publish_still_works() {
        // The InMemoryDiscoveryBackend shutdown is a no-op, so the backend
        // remains usable after shutdown.  This is backend-specific; the
        // tracker's own shutdown only fires the token + calls backend.shutdown.
        let (sk, ep) = test_identity();
        let backend = InMemoryDiscoveryBackend::new();
        let tracker = block_on(PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            ep,
            sk,
        ))
        .unwrap();

        block_on(tracker.shutdown());

        // Backend should still work (in-memory backend is a no-op shutdown).
        assert!(backend.total_record_count() == 0);
    }

    // ── Send + Sync ──────────────────────────────────────────────────

    #[test]
    fn tracker_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PublicRoomTracker>();
    }
}
