//! Private-room DHT tracker — thin wrapper around [`PublicRoomTracker`] for
//! private rooms that use a shared [`DiscoverySecret`].
//!
//! Where `PublicRoomTracker` derives its room identity from a public
//! [`PublicNetwork`] using canonical constants, `PrivateRoomTracker` derives
//! the discovery key from a caller-supplied gossip topic and a secret, so
//! only peers sharing the secret can discover each other.
//!
//! # Lifecycle
//!
//! 1. [`new`](PrivateRoomTracker::new) — construct with a topic + discovery
//!    secret.
//! 2. [`publish_once`](PrivateRoomTracker::publish_once) — advertise local
//!    presence on the private room's DHT namespace.
//! 3. [`discover_once`](PrivateRoomTracker::discover_once) — find peers that
//!    share the same discovery secret.
//! 4. [`shutdown`](PrivateRoomTracker::shutdown) — release backend resources.
//!
//! # Example
//!
//! ```ignore
//! use crate::discovery_backend::InMemoryDiscoveryBackend;
//! use crate::discovery_secret::DiscoverySecret;
//! use crate::private_room_tracker::PrivateRoomTracker;
//! use iroh::{EndpointId, SecretKey};
//!
//! let sk = SecretKey::generate();
//! let ep = sk.public();
//! let topic = crate::proto::TopicId::from_bytes(rand::random());
//! let secret = DiscoverySecret::generate();
//!
//! let tracker = PrivateRoomTracker::new(
//!     Box::new(InMemoryDiscoveryBackend::new()),
//!     topic,
//!     secret,
//!     ep,
//!     sk,
//! );
//! ```

use iroh::{EndpointId, SecretKey};
use n0_error::Result;

use crate::discovery_backend::TopicDiscoveryBackend;
use crate::discovery_secret::DiscoverySecret;
use crate::proto::TopicId;
use crate::public_room::PublicRoomIdentity;
use crate::public_room_tracker::PublicRoomTracker;

// ---------------------------------------------------------------------------
// Domain separator
// ---------------------------------------------------------------------------

/// Domain separator for deriving a private-room discovery key from a gossip
/// topic and a [`DiscoverySecret`].
///
/// Deliberately different from [`PUBLIC_ROOM_DOMAIN_SEPARATOR`] and
/// [`DISCOVERY_KEY_DOMAIN_SEPARATOR`] to ensure domain separation between
/// public and private discovery namespaces.
const PRIVATE_DISCOVERY_KEY_DOMAIN_SEPARATOR: &[u8] = b"boru-chat private-room discovery-key v1";

// ---------------------------------------------------------------------------
// PrivateRoomTracker
// ---------------------------------------------------------------------------

/// A thin wrapper around [`PublicRoomTracker`] that derives the room identity
/// from a caller-supplied gossip topic and a shared [`DiscoverySecret`].
///
/// Peers that know the same `(topic, discovery_secret)` pair derive the same
/// DHT namespace and can discover each other.  Peers with different secrets
/// (or different topics) are isolated — they operate on disjoint namespaces.
///
/// All discovery operations delegate to the inner [`PublicRoomTracker`];
/// this type exists purely to own the private-room identity derivation so
/// callers don't have to construct a [`PublicRoomIdentity`] by hand.
///
/// The type is `Send + Sync` when the inner tracker is (it always is).
#[derive(Debug)]
pub struct PrivateRoomTracker {
    inner: PublicRoomTracker,
    /// The gossip topic for this private room (kept for identity queries).
    topic: TopicId,
}

impl PrivateRoomTracker {
    /// Create a new private-room tracker for the given topic and discovery
    /// secret.
    ///
    /// The room identity is derived deterministically from:
    ///
    /// ```text
    /// discovery_key = BLAKE3(
    ///     PRIVATE_DISCOVERY_KEY_DOMAIN_SEPARATOR ||
    ///     topic_bytes ||
    ///     discovery_secret_bytes
    /// )
    /// topic = caller-supplied TopicId  (passed through unchanged)
    /// ```
    ///
    /// # Parameters
    ///
    /// * `backend` — the DHT-like discovery backend (in-memory for tests,
    ///   `MainlineDhtBackend` for production).
    /// * `topic` — the gossip mesh topic for this private room.
    /// * `discovery_secret` — the shared secret that controls access to the
    ///   room's discovery namespace.
    /// * `local_endpoint_id` — this node's iroh EndpointId.
    /// * `secret_key` — this node's iroh SecretKey for signing records.
    pub fn new(
        backend: Box<dyn TopicDiscoveryBackend>,
        topic: TopicId,
        discovery_secret: DiscoverySecret,
        local_endpoint_id: EndpointId,
        secret_key: SecretKey,
    ) -> Self {
        let discovery_key = derive_private_discovery_key(&topic, &discovery_secret);
        let identity = PublicRoomIdentity::new(topic, discovery_key);
        let inner = PublicRoomTracker::new(backend, identity, local_endpoint_id, secret_key);
        Self { inner, topic }
    }

    /// Return the room identity used by the inner tracker.
    pub fn identity(&self) -> &PublicRoomIdentity {
        self.inner.identity()
    }

    /// Return the gossip topic for this private room.
    pub fn topic(&self) -> &TopicId {
        &self.topic
    }

    /// Return this node's EndpointId.
    pub fn local_endpoint_id(&self) -> &EndpointId {
        self.inner.local_endpoint_id()
    }

    /// Publish this node's presence to the DHT once.
    ///
    /// Delegates to the inner [`PublicRoomTracker::publish_once`].
    pub async fn publish_once(&self) -> Result<()> {
        self.inner.publish_once().await
    }

    /// Find valid peers on the room's DHT namespace.
    ///
    /// Delegates to the inner [`PublicRoomTracker::discover_once`].
    pub async fn discover_once(&self) -> Result<Vec<EndpointId>> {
        self.inner.discover_once().await
    }

    /// Shut down the tracker, releasing backend resources.
    ///
    /// Delegates to the inner [`PublicRoomTracker::shutdown`].
    ///
    /// **Consumes** the tracker — call this once when done.
    pub async fn shutdown(self) {
        self.inner.shutdown().await
    }
}

// ---------------------------------------------------------------------------
// Discovery key derivation
// ---------------------------------------------------------------------------

/// Derive a 32-byte discovery key from a private-room gossip topic and a
/// shared [`DiscoverySecret`].
///
/// # Derivation
///
/// ```text
/// discovery_key = BLAKE3(
///     PRIVATE_DISCOVERY_KEY_DOMAIN_SEPARATOR ||
///     topic.as_bytes() ||
///     discovery_secret.as_bytes()
/// )
/// ```
///
/// # Properties
///
/// * **Deterministic** — same (topic, secret) always produces the same key.
/// * **Domain-separated** — the prefix differs from all public-room
///   derivation constants.
/// * **Secret-dependent** — different secrets on the same topic produce
///   different keys.
/// * **Topic-dependent** — the same secret on different topics produces
///   different keys.
fn derive_private_discovery_key(topic: &TopicId, secret: &DiscoverySecret) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(PRIVATE_DISCOVERY_KEY_DOMAIN_SEPARATOR);
    hasher.update(topic.as_bytes());
    hasher.update(secret.as_bytes());
    let hash = hasher.finalize();
    *hash.as_bytes()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::discovery_backend::InMemoryDiscoveryBackend;

    /// Helper: generate a fresh test identity (SecretKey + EndpointId).
    fn test_identity() -> (SecretKey, EndpointId) {
        let sk = SecretKey::generate();
        let ep = sk.public();
        (sk, ep)
    }

    /// Helper: generate a random TopicId for test private rooms.
    fn test_topic() -> TopicId {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("OS entropy failed");
        TopicId::from(bytes)
    }

    /// Helper: block on an async future for synchronous test contexts.
    fn block_on<F: std::future::Future<Output = T>, T>(f: F) -> T {
        tokio::runtime::Runtime::new().unwrap().block_on(f)
    }

    // ── Construction ──────────────────────────────────────────────────

    /// Construction with explicit parameters preserves the topic and
    /// produces a valid identity.
    #[test]
    fn constructor_creates_tracker() {
        let topic = test_topic();
        let secret = DiscoverySecret::from_bytes([0x42u8; 32]);
        let (sk, ep) = test_identity();

        let tracker = PrivateRoomTracker::new(
            Box::new(InMemoryDiscoveryBackend::new()),
            topic,
            secret,
            ep.clone(),
            sk,
        );

        assert_eq!(tracker.topic(), &topic);
        assert_eq!(tracker.local_endpoint_id(), &ep);
        // The identity's topic should match what we passed in.
        assert_eq!(tracker.identity().topic, topic);
    }

    /// Two trackers with the same (topic, secret) produce identical
    /// identities (same discovery key).
    #[test]
    fn same_topic_and_secret_produce_same_identity() {
        let topic = test_topic();
        let secret = DiscoverySecret::from_bytes([0x42u8; 32]);
        let (sk_a, ep_a) = test_identity();
        let (_sk_b, _ep_b) = test_identity();

        let tracker_a = PrivateRoomTracker::new(
            Box::new(InMemoryDiscoveryBackend::new()),
            topic,
            secret,
            ep_a,
            sk_a,
        );
        // Same params again.
        let tracker_b = PrivateRoomTracker::new(
            Box::new(InMemoryDiscoveryBackend::new()),
            topic,
            secret,
            _ep_b,
            _sk_b,
        );

        assert_eq!(tracker_a.identity(), tracker_b.identity());
    }

    // ── Two peers on same discovery secret discover each other ─────────

    /// Two peers that share the same (topic, secret) can discover each
    /// other via the in-memory backend.
    #[test]
    fn same_secret_discovers_peers() {
        let topic = test_topic();
        let secret = DiscoverySecret::generate();
        let backend = InMemoryDiscoveryBackend::new();

        let (sk_a, ep_a) = test_identity();
        let (_sk_b, ep_b) = test_identity();

        // Peer A publishes.
        let tracker_a =
            PrivateRoomTracker::new(Box::new(backend.clone()), topic, secret, ep_a.clone(), sk_a);
        block_on(tracker_a.publish_once()).unwrap();

        // Peer B discovers.
        let tracker_b = PrivateRoomTracker::new(
            Box::new(backend.clone()),
            topic,
            secret,
            ep_b,
            SecretKey::generate(),
        );
        let peers = block_on(tracker_b.discover_once()).unwrap();

        assert!(
            peers.contains(&ep_a),
            "expected peer A to be discovered, got {peers:?}"
        );
        assert_eq!(
            peers.len(),
            1,
            "expected exactly one peer, got {}",
            peers.len()
        );
    }

    // ── Two peers on different discovery secrets are isolated ──────────

    /// Peers with different secrets on the same topic are isolated — they
    /// cannot discover each other.
    #[test]
    fn different_secrets_are_isolated() {
        let topic = test_topic();
        let secret_a = DiscoverySecret::from_bytes([1u8; 32]);
        let secret_b = DiscoverySecret::from_bytes([2u8; 32]);

        let backend_a = InMemoryDiscoveryBackend::new();
        let backend_b = InMemoryDiscoveryBackend::new();

        let (sk_a, ep_a) = test_identity();
        let (_sk_b, ep_b) = test_identity();

        // Peer A publishes with secret A.
        let tracker_a =
            PrivateRoomTracker::new(Box::new(backend_a.clone()), topic, secret_a, ep_a, sk_a);
        block_on(tracker_a.publish_once()).unwrap();

        // Peer B tries to discover with secret B — should see nothing.
        let tracker_b =
            PrivateRoomTracker::new(Box::new(backend_b.clone()), topic, secret_b, ep_b, _sk_b);
        let peers = block_on(tracker_b.discover_once()).unwrap();

        assert!(
            peers.is_empty(),
            "peers with different secrets should be isolated, got {peers:?}"
        );
    }

    // ── Self-filter works ─────────────────────────────────────────────

    /// A peer does not discover its own EndpointId.
    #[test]
    fn self_filter_excludes_local_peer() {
        let topic = test_topic();
        let secret = DiscoverySecret::generate();
        let backend = InMemoryDiscoveryBackend::new();
        let (sk, ep) = test_identity();

        let tracker =
            PrivateRoomTracker::new(Box::new(backend.clone()), topic, secret, ep.clone(), sk);
        block_on(tracker.publish_once()).unwrap();

        let peers = block_on(tracker.discover_once()).unwrap();
        assert!(
            !peers.contains(&ep),
            "self endpoint should be filtered out, got {peers:?}"
        );
        assert!(peers.is_empty(), "expected no peers, got {peers:?}");
    }

    // ── Duplicate filter works ────────────────────────────────────────

    /// Multiple publishes by the same peer produce only one EndpointId.
    #[test]
    fn duplicate_peers_are_filtered() {
        let topic = test_topic();
        let secret = DiscoverySecret::generate();
        let backend = InMemoryDiscoveryBackend::new();
        let (sk, ep) = test_identity();

        // Publish twice.
        {
            let t = PrivateRoomTracker::new(
                Box::new(backend.clone()),
                topic,
                secret,
                ep.clone(),
                sk.clone(),
            );
            block_on(t.publish_once()).unwrap();
        }
        {
            let t =
                PrivateRoomTracker::new(Box::new(backend.clone()), topic, secret, ep.clone(), sk);
            block_on(t.publish_once()).unwrap();
        }

        // Discover from a different peer.
        let (sk_b, ep_b) = test_identity();
        let t_b = PrivateRoomTracker::new(Box::new(backend.clone()), topic, secret, ep_b, sk_b);
        let peers = block_on(t_b.discover_once()).unwrap();

        assert!(
            peers.contains(&ep),
            "expected A to be discovered, got {peers:?}"
        );
        assert_eq!(
            peers.iter().filter(|&&p| p == ep).count(),
            1,
            "A should appear exactly once, got {peers:?}"
        );
    }

    // ── Empty discovery returns empty ─────────────────────────────────

    /// Discovering on an empty backend returns an empty list.
    #[test]
    fn discover_empty_backend_returns_empty() {
        let topic = test_topic();
        let secret = DiscoverySecret::generate();
        let (sk, ep) = test_identity();

        let tracker = PrivateRoomTracker::new(
            Box::new(InMemoryDiscoveryBackend::new()),
            topic,
            secret,
            ep,
            sk,
        );
        let peers = block_on(tracker.discover_once()).unwrap();
        assert!(peers.is_empty(), "expected empty, got {peers:?}");
    }

    // ── Shutdown prevents further operations ──────────────────────────

    /// Shutdown consumes the tracker without panicking.  The underlying
    /// backend remains usable (in-memory shutdown is a no-op).
    #[test]
    fn shutdown_does_not_panic() {
        let topic = test_topic();
        let secret = DiscoverySecret::generate();
        let (sk, ep) = test_identity();

        let tracker = PrivateRoomTracker::new(
            Box::new(InMemoryDiscoveryBackend::new()),
            topic,
            secret,
            ep,
            sk,
        );
        block_on(tracker.shutdown());
    }

    // ── Send + Sync ──────────────────────────────────────────────────

    /// The type satisfies Send + Sync (compile-time check).
    #[test]
    fn tracker_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PrivateRoomTracker>();
    }

    // ── Discovery key derivation ──────────────────────────────────────

    /// Same (topic, secret) produces the same key.
    #[test]
    fn derive_key_is_deterministic() {
        let topic = test_topic();
        let secret = DiscoverySecret::from_bytes([0xab; 32]);
        let a = derive_private_discovery_key(&topic, &secret);
        let b = derive_private_discovery_key(&topic, &secret);
        assert_eq!(a, b);
    }

    /// Different secrets produce different keys (same topic).
    #[test]
    fn derive_key_differs_by_secret() {
        let topic = test_topic();
        let secret_a = DiscoverySecret::from_bytes([1u8; 32]);
        let secret_b = DiscoverySecret::from_bytes([2u8; 32]);
        let a = derive_private_discovery_key(&topic, &secret_a);
        let b = derive_private_discovery_key(&topic, &secret_b);
        assert_ne!(a, b);
    }

    /// Different topics produce different keys (same secret).
    #[test]
    fn derive_key_differs_by_topic() {
        let secret = DiscoverySecret::from_bytes([0x42u8; 32]);
        let topic_a = {
            let mut b = [0u8; 32];
            b[0] = 1;
            TopicId::from(b)
        };
        let topic_b = {
            let mut b = [0u8; 32];
            b[0] = 2;
            TopicId::from(b)
        };
        let a = derive_private_discovery_key(&topic_a, &secret);
        let b = derive_private_discovery_key(&topic_b, &secret);
        assert_ne!(a, b);
    }

    /// Output is non-zero (avalanche sanity).
    #[test]
    fn derive_key_is_nonzero() {
        let topic = test_topic();
        let secret = DiscoverySecret::generate();
        let key = derive_private_discovery_key(&topic, &secret);
        assert!(key.iter().any(|&b| b != 0));
    }
}
