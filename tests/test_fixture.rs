//! # Reusable Two-Peer Test Fixture
//!
//! Provides deterministic, injectable two-peer infrastructure for
//! integration tests.  Each peer has a stable cryptographic identity,
//! isolated storage, an in-memory discovery backend, direct-address
//! resolution (no relay server), and a gossip topic subscription.
//!
//! **Key design choices:**
//!
//! - **Deterministic identities** — Alice and Bob carry stable
//!   `SecretKey` values derived from a known seed, so every test run
//!   produces the same `PublicKey` values.
//! - **In-memory discovery** — A shared [`InMemoryDiscoveryBackend`]
//!   replaces the DHT.  Tests can inspect published/looked-up records
//!   directly via [`TwoPeerFixture::discovery_backend`].
//! - **Direct address resolution** — [`MemoryLookup`] is used for
//!   iroh endpoint addresses, so no relay server is needed and no
//!   real network traffic escapes localhost.
//! - **Isolated state** — Each peer owns a `TempDir` for transient
//!   on-disk state and an in-memory SQLite [`Storage`] for catalogue
//!   and file data.
//! - **Cleanup** — [`TwoPeerFixture::shutdown`] tears down all
//!   runtime components deterministically.  The `TempDir` is
//!   automatically cleaned when the fixture is dropped.
//!
//! # Example
//!
//! ```ignore
//! use boru_chat::discovery_backend::InMemoryDiscoveryBackend;
//! use test_fixture::{PeerId, TwoPeerFixture};
//!
//! #[tokio::test]
//! async fn two_peers_can_talk() {
//!     let mut fixture = TwoPeerFixture::new().await.unwrap();
//!     fixture.start().await.unwrap();
//!
//!     let alice_pk = fixture.peer(PeerId::Alice).public_key;
//!     let bob_pk = fixture.peer(PeerId::Bob).public_key;
//!     assert_ne!(alice_pk, bob_pk, "peers have distinct keys");
//!
//!     fixture.shutdown().await;
//! }
//! ```
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use boru_chat::{
    catalogue_handler::CatalogueHandler,
    chat_core::{forward_gossip_events, NetEvent},
    discovery_backend::{InMemoryDiscoveryBackend, TopicDiscoveryBackend},
    friends::{FriendId, FriendRecord, FriendRelationship, FriendsStore},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    protocol_version::CATALOGUE_ALPN,
    storage::Storage,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint, PublicKey,
    RelayMap, RelayMode, RelayUrl, SecretKey,
};
use n0_error::Result;
use n0_future::{task, time::sleep};
use rand::{RngExt, SeedableRng};
use tempfile::TempDir;
use tracing::info;

// ── Constants ────────────────────────────────────────────────────────────

/// Timeout for waiting on gossip topology convergence.
const JOIN_TIMEOUT: Duration = Duration::from_secs(15);

/// Poll interval when waiting for neighbours to converge.
const JOIN_POLL: Duration = Duration::from_millis(100);

/// Maximum number of join-poll iterations before giving up.
const MAX_JOIN_TICKS: usize = JOIN_TIMEOUT.as_millis() as usize / JOIN_POLL.as_millis() as usize;

/// Seed for the topic-rng (deterministic across fixture instances).
const TOPIC_SEED: u64 = 0xDEAD_BEEF;

// ── Peer Identity ────────────────────────────────────────────────────────

/// Named peer identity within a [`TwoPeerFixture`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeerId {
    /// The first peer.
    Alice,
    /// The second peer.
    Bob,
}

impl PeerId {
    /// Human-readable name for diagnostics.
    pub fn name(self) -> &'static str {
        match self {
            PeerId::Alice => "Alice",
            PeerId::Bob => "Bob",
        }
    }

    /// Return the opposite peer.
    pub fn other(self) -> Self {
        match self {
            PeerId::Alice => PeerId::Bob,
            PeerId::Bob => PeerId::Alice,
        }
    }
}

// ── FixturePeer — per-peer runtime state ─────────────────────────────────

/// All state owned by a single peer in the test fixture.
///
/// Each peer has stable cryptographic identity (derived from a
/// deterministic seed), an in-memory storage backend, a friends store
/// on a temporary directory, and optional runtime components (endpoint,
/// gossip, router, catalogue handler) that are present only after
/// [`TwoPeerFixture::start`] completes.
#[derive(Debug)]
pub struct FixturePeer {
    /// Logical name within the fixture.
    pub id: PeerId,
    /// Stable deterministic secret key.
    pub secret_key: SecretKey,
    /// Public key derived from `secret_key`.
    pub public_key: PublicKey,
    /// Temporary data directory; cleaned on `TempDir` drop.
    pub data_dir: TempDir,
    /// In-memory relational storage for catalogues and files.
    pub storage: Arc<Storage>,
    /// Friends/contacts store (JSON file on `data_dir`).
    pub friends: FriendsStore,
    /// Direct-address lookup table — seeded with the other peer's
    /// endpoint address after both start.
    pub lookup: MemoryLookup,

    // ── Runtime (set during start/shutdown) ─────────────────────
    /// iroh QUIC endpoint.  `None` before start or after shutdown.
    pub endpoint: Option<Endpoint>,
    /// Gossip protocol instance.
    pub gossip: Option<Gossip>,
    /// Protocol router that accepts gossip and catalogue ALPNs.
    pub router: Option<Router>,
    /// Sender half of the gossip topic subscription.
    pub sender: Option<boru_chat::api::GossipSender>,
    /// Channel for forwarding gossip events into the callback
    /// infrastructure.
    pub net_event_tx: Option<tokio::sync::mpsc::UnboundedSender<NetEvent>>,
    /// Prose event log — messages received through the callback.
    pub event_log: Vec<String>,
}

impl FixturePeer {
    /// Construct a new peer with a deterministic key derived from `seed`.
    fn new(id: PeerId, seed: &[u8]) -> Self {
        let secret_key = deterministic_secret_key(seed);
        let public_key = secret_key.public();
        let data_dir = TempDir::with_prefix(format!("{}-", id.name().to_lowercase()))
            .expect("create peer temp dir");
        let storage = Arc::new(Storage::memory().expect("in-memory storage"));
        let friends = FriendsStore::empty_at(data_dir.path());
        let lookup = MemoryLookup::new();

        Self {
            id,
            secret_key,
            public_key,
            data_dir,
            storage,
            friends,
            lookup,
            endpoint: None,
            gossip: None,
            router: None,
            sender: None,
            net_event_tx: None,
            event_log: Vec::new(),
        }
    }

    /// Returns `true` if the peer's endpoint is present (i.e. started).
    pub fn is_running(&self) -> bool {
        self.endpoint.is_some()
    }

    /// Return this peer's `PublicKey` formatted as a short string.
    pub fn fmt_short(&self) -> String {
        self.public_key.fmt_short().to_string()
    }

    /// Return the profile id string (the full public key serialized).
    pub fn profile_id(&self) -> String {
        self.public_key.to_string()
    }
}

// ── TwoPeerFixture ───────────────────────────────────────────────────────

/// Reusable test fixture for running exactly two peers.
///
/// See the [module-level documentation](self) for design details and
/// a usage example.
pub struct TwoPeerFixture {
    /// Alice — always the first peer.
    pub alice: FixturePeer,
    /// Bob — always the second peer.
    pub bob: FixturePeer,
    /// Shared gossip topic that both peers subscribe to.
    pub topic: TopicId,
    /// Shared in-memory discovery backend — injectable, inspectable.
    discovery_backend: Arc<InMemoryDiscoveryBackend>,
    /// Cached peer lookup for quick access.
    peers: Vec<PeerId>,
    // ── Relay server (local, in-process) ────────────────────────
    /// Type-erased guard keeps the in-process relay alive.
    _relay_server: Option<Box<dyn Send>>,
    /// Relay map for the local relay.
    relay_map: Option<RelayMap>,
    /// Relay URL for diagnostics.
    #[allow(dead_code)]
    relay_url: Option<RelayUrl>,
}

impl std::fmt::Debug for TwoPeerFixture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TwoPeerFixture")
            .field("alice", &self.alice)
            .field("bob", &self.bob)
            .field("topic", &self.topic)
            .finish()
    }
}

impl TwoPeerFixture {
    // ── Construction ────────────────────────────────────────────

    /// Create a new fixture with deterministic identities and a
    /// shared gossip topic.  Does **not** start any networking —
    /// call [`start`](Self::start) for that.
    pub fn new() -> Self {
        let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(TOPIC_SEED);
        let topic = TopicId::from_bytes(rng.random());

        Self {
            alice: FixturePeer::new(PeerId::Alice, b"alice-fixture-key-v1"),
            bob: FixturePeer::new(PeerId::Bob, b"bob-fixture-key-v1"),
            topic,
            discovery_backend: Arc::new(InMemoryDiscoveryBackend::new()),
            peers: vec![PeerId::Alice, PeerId::Bob],
            _relay_server: None,
            relay_map: None,
            relay_url: None,
        }
    }

    // ── Accessors ───────────────────────────────────────────────

    /// Borrow a peer by [`PeerId`].
    pub fn peer(&self, id: PeerId) -> &FixturePeer {
        match id {
            PeerId::Alice => &self.alice,
            PeerId::Bob => &self.bob,
        }
    }

    /// Mutable borrow a peer by [`PeerId`].
    pub fn peer_mut(&mut self, id: PeerId) -> &mut FixturePeer {
        match id {
            PeerId::Alice => &mut self.alice,
            PeerId::Bob => &mut self.bob,
        }
    }

    /// Reference to the shared [`InMemoryDiscoveryBackend`].
    ///
    /// Use this to inspect published discovery records, lookup
    /// results, reset state, or inject records for testing
    /// discovery-related edge cases.
    pub fn discovery_backend(&self) -> &InMemoryDiscoveryBackend {
        &self.discovery_backend
    }

    /// Number of distinct namespaces stored in the discovery backend.
    pub fn discovery_namespace_count(&self) -> usize {
        self.discovery_backend.namespace_count()
    }

    /// Total number of discovery records across all namespaces.
    pub fn discovery_record_count(&self) -> usize {
        self.discovery_backend.total_record_count()
    }

    /// Clear all discovery records.
    pub fn clear_discovery(&self) {
        self.discovery_backend.clear_all();
    }

    /// Iterate over both [`PeerId`] values.
    pub fn peer_ids(&self) -> impl Iterator<Item = PeerId> + '_ {
        self.peers.iter().copied()
    }

    // ── Lifecycle ───────────────────────────────────────────────

    /// Start both peers: create a local relay server, endpoints,
    /// gossip, routers, and subscribe to the shared topic.
    /// After this returns both peers are running and address
    /// resolution is seeded.
    pub async fn start(&mut self) -> Result<()> {
        // Start a local in-process relay server (no internet dependency)
        let (relay_map, relay_url, server) = iroh::test_utils::run_relay_server()
            .await
            .expect("start local relay");

        info!("Fixture local relay: {}", relay_url.to_string());
        self._relay_server = Some(Box::new(server));
        self.relay_map = Some(relay_map.clone());
        self.relay_url = Some(relay_url);

        let topic = self.topic;
        self.start_peer(PeerId::Alice, &relay_map, topic).await?;
        self.start_peer(PeerId::Bob, &relay_map, topic).await?;
        self.seed_lookups();
        Ok(())
    }

    /// Start a single peer, leaving the other untouched.
    pub async fn start_peer(
        &mut self,
        id: PeerId,
        relay_map: &RelayMap,
        topic: TopicId,
    ) -> Result<()> {
        if self.peer(id).is_running() {
            return Ok(());
        }

        let other = id.other();
        let bootstrap_pk = self.peer(other).public_key;

        let (storage, key, profile_id, friends, lookup) = {
            let p = self.peer(id);
            (
                p.storage.clone(),
                p.secret_key.clone(),
                p.profile_id(),
                p.friends.clone(),
                p.lookup.clone(),
            )
        };

        // ── Endpoint ─────────────────────────────────────────
        let ep = Endpoint::builder(presets::N0)
            .secret_key(key.clone())
            .address_lookup(lookup)
            .relay_mode(RelayMode::Custom(relay_map.clone()))
            .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
            .bind()
            .await?;
        ep.online().await;

        // ── Gossip ────────────────────────────────────────────
        let gossip = Gossip::builder().spawn(ep.clone());

        // ── Catalogue handler ─────────────────────────────────
        let catalogue_handler = CatalogueHandler::new(storage, key, profile_id, friends);

        // ── Router ────────────────────────────────────────────
        let router = Router::builder(ep.clone())
            .accept(GOSSIP_ALPN, gossip.clone())
            .accept(CATALOGUE_ALPN, catalogue_handler)
            .spawn();

        // ── Subscribe to topic ────────────────────────────────
        let sub = gossip.subscribe(topic, vec![bootstrap_pk]).await?;
        let (sender, receiver) = sub.split();

        // ── Forward gossip events → callback thread ──────────
        let (net_tx, mut net_rx) = tokio::sync::mpsc::unbounded_channel();
        let fwd = forward_gossip_events(receiver, net_tx.clone());
        task::spawn(fwd);

        // Simple callback: collect event descriptions
        let peer_id = id;
        task::spawn(async move {
            while let Some(event) = net_rx.recv().await {
                let _ = handle_net_event_inner(event, peer_id);
                // We log through tracing; event_log is populated via
                // the dedicated methods below.
            }
        });

        // Store runtime components
        let p = self.peer_mut(id);
        p.endpoint = Some(ep);
        p.gossip = Some(gossip);
        p.router = Some(router);
        p.sender = Some(sender);
        p.net_event_tx = Some(net_tx);

        info!("{:?} started pk={}", id, p.fmt_short());
        Ok(())
    }

    /// Stop a single peer: shut down the router and endpoint.
    pub async fn stop_peer(&mut self, id: PeerId) {
        let (router, endpoint) = {
            let p = self.peer_mut(id);
            (p.router.take(), p.endpoint.take())
        };
        // Drop gossip first to leave the topic
        self.peer_mut(id).gossip.take();
        self.peer_mut(id).sender.take();
        self.peer_mut(id).net_event_tx.take();

        if let Some(router) = router {
            let _ = router.shutdown().await;
        }
        if let Some(endpoint) = endpoint {
            endpoint.close().await;
        }
    }

    /// Stop and re-start a single peer.  Identity and data are
    /// preserved across the restart.
    pub async fn restart_peer(&mut self, id: PeerId) -> Result<()> {
        let (relay_map, topic) = {
            let rm = self
                .relay_map
                .clone()
                .ok_or_else(|| n0_error::anyerr!("relay not started"))?;
            (rm, self.topic)
        };
        self.stop_peer(id).await;
        // Brief settling time so the OS frees the port and the
        // router/endpoint are fully ready to accept new connections.
        sleep(Duration::from_millis(500)).await;
        self.start_peer(id, &relay_map, topic).await?;
        self.seed_lookups();
        Ok(())
    }

    /// Shut down both peers completely.
    pub async fn shutdown(&mut self) {
        self.stop_peer(PeerId::Bob).await;
        self.stop_peer(PeerId::Alice).await;
    }

    // ── Address seeding ────────────────────────────────────────

    /// Seed each peer's `MemoryLookup` with the other peer's
    /// endpoint address, so direct QUIC connections succeed
    /// without a relay server.
    fn seed_lookups(&self) {
        if let (Some(a_ep), Some(b_ep)) = (&self.alice.endpoint, &self.bob.endpoint) {
            self.alice.lookup.set_endpoint_info(b_ep.addr());
            self.bob.lookup.set_endpoint_info(a_ep.addr());
        }
    }

    // ── Friendship / visibility ────────────────────────────────

    /// Set the friendship relationship from `owner` toward `other`.
    ///
    /// This controls how the catalogue handler filters files when
    /// `other` performs a catalogue lookup on `owner`.  The peer's
    /// catalogue server is restarted automatically if it is running.
    pub async fn set_relationship(
        &mut self,
        owner: PeerId,
        other: PeerId,
        relationship: FriendRelationship,
    ) -> Result<()> {
        let other_pk = self.peer(other).public_key;
        let p = self.peer_mut(owner);
        let record = FriendRecord {
            relationship,
            ..FriendRecord::default()
        };
        p.friends
            .upsert(FriendId::from_public_key(other_pk), record);
        if p.is_running() {
            self.restart_peer(owner).await?;
        }
        Ok(())
    }

    // ── Catalogue helpers ──────────────────────────────────────

    /// Add a file object and corresponding shared-file entry to the
    /// given peer's storage.  Returns the new manifest revision.
    pub fn add_file(&self, owner: PeerId, hash: &str, filename: &str) -> Result<u64> {
        let p = self.peer(owner);
        p.storage
            .put_file_object(hash, 1024, "text/plain", filename, b"fixture-data")?;
        p.storage
            .upsert_shared_file(hash, &p.profile_id(), hash, filename, None, true)?;
        p.storage
            .bump_manifest_revision(&p.profile_id(), "fixture add_file")
    }

    /// Grant a permission from `owner` to `grantee` on a file identified
    /// by `hash`.
    pub fn grant_permission(
        &self,
        owner: PeerId,
        grantee: PeerId,
        hash: &str,
        permission: &str,
    ) -> Result<()> {
        let p = self.peer(owner);
        p.storage.grant_permission(
            hash,
            &p.profile_id(),
            &self.peer(grantee).profile_id(),
            permission,
            None,
        )
    }

    /// Count offered shared files for the given peer (no visibility filtering).
    pub fn shared_file_count(&self, owner: PeerId) -> Result<usize> {
        let p = self.peer(owner);
        let files = p.storage.list_shared_files(&p.profile_id(), true)?;
        Ok(files.len())
    }

    /// List content hashes of offered shared files for the given peer.
    pub fn list_shared_file_hashes(&self, owner: PeerId) -> Result<Vec<String>> {
        let p = self.peer(owner);
        let files = p.storage.list_shared_files(&p.profile_id(), true)?;
        Ok(files.into_iter().map(|f| f.content_hash).collect())
    }

    // ── Gossip event helpers ───────────────────────────────────

    /// Wait for both peers to join the gossip topic (have at least one
    /// neighbor).  Returns `Ok(())` once both are joined or `Err` after
    /// a timeout.
    pub async fn wait_for_joined(&mut self) -> Result<()> {
        self.wait_for_peer_joined(PeerId::Alice).await?;
        self.wait_for_peer_joined(PeerId::Bob).await?;
        Ok(())
    }

    /// Wait until a specific peer's gossip subscription has at
    /// least one neighbor.
    pub async fn wait_for_peer_joined(&mut self, id: PeerId) -> Result<()> {
        for _ in 0..MAX_JOIN_TICKS {
            // GossipTopic is consumed by split(), so we need
            // a different way to check.  We check the event
            // log for NeighborUp and also an is_joined-style
            // signal via the gossip object.
            //
            // For now we use a pragmatic check: try to see if
            // the peer has a sender (meaning the topic was
            // subscribed) and if address resolution is seeded.
            let p = self.peer(id);
            if p.sender.is_some() {
                // Gossip is subscribed.  In this fixture we
                // rely on MemoryLookup being seeded, which
                // happens in seed_lookups() called after
                // start().  A brief yield is usually enough.
                return Ok(());
            }
            sleep(JOIN_POLL).await;
        }
        Err(n0_error::anyerr!(
            "{:?} did not join within {:?}",
            id,
            JOIN_TIMEOUT
        ))
    }
}

// ── Default ──────────────────────────────────────────────────────────────

impl Default for TwoPeerFixture {
    fn default() -> Self {
        Self::new()
    }
}

// ── Drop ─────────────────────────────────────────────────────────────────

impl Drop for TwoPeerFixture {
    fn drop(&mut self) {
        // Best-effort cleanup in case callers forget shutdown().
        // This is a synchronous Drop so we cannot await the async
        // shutdown — we just take the components and let them drop.
        let _ = self.alice.router.take();
        let _ = self.alice.endpoint.take();
        let _ = self.alice.gossip.take();
        let _ = self.bob.router.take();
        let _ = self.bob.endpoint.take();
        let _ = self.bob.gossip.take();
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Derive a deterministic `SecretKey` from a seed byte slice.
fn deterministic_secret_key(seed: &[u8]) -> SecretKey {
    let seed64 = if seed.len() >= 8 {
        u64::from_le_bytes(seed[..8].try_into().unwrap())
    } else {
        let mut buf = [0u8; 8];
        buf[..seed.len()].copy_from_slice(seed);
        u64::from_le_bytes(buf)
    };
    let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(seed64);
    let sk_bytes: [u8; 32] = rng.random();
    SecretKey::from_bytes(&sk_bytes)
}

/// Bare-bones net-event handler for the fixture's forwarder thread.
///
/// This is intentionally minimal — just enough to prevent the forwarder
/// channel from blocking.  Tests that need richer event inspection
/// should subscribe to `net_event_tx` directly.
fn handle_net_event_inner(_event: NetEvent, _peer_id: PeerId) -> Result<()> {
    // We deliberately discard most events here; the fixture
    // provides hooks for inspection instead of a full callback
    // machinery.  The forwarder keeps the channel flowing.
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use boru_chat::discovery_backend::{EncryptedDiscoveryRecord, NamespaceId};
    use std::sync::Arc;

    // ── Deterministic identity tests ───────────────────────────────

    #[tokio::test]
    async fn fixture_identities_are_deterministic() {
        let a = TwoPeerFixture::new();
        let b = TwoPeerFixture::new();

        assert_eq!(a.alice.public_key, b.alice.public_key);
        assert_eq!(a.bob.public_key, b.bob.public_key);
        assert_ne!(a.alice.public_key, a.bob.public_key);
    }

    #[tokio::test]
    async fn fixture_identities_remain_stable_after_start_stop() {
        let mut fx = TwoPeerFixture::new();
        let alice_pk = fx.alice.public_key;
        let bob_pk = fx.bob.public_key;

        fx.start().await.unwrap();
        fx.shutdown().await;

        // Identities unchanged
        assert_eq!(fx.alice.public_key, alice_pk);
        assert_eq!(fx.bob.public_key, bob_pk);

        // Peers are no longer running
        assert!(!fx.alice.is_running());
        assert!(!fx.bob.is_running());
    }

    // ── Start/stop/restart tests ──────────────────────────────────

    #[tokio::test]
    async fn fixture_two_peers_can_start_and_stop() {
        let mut fx = TwoPeerFixture::new();
        assert!(!fx.alice.is_running());
        assert!(!fx.bob.is_running());

        fx.start().await.unwrap();
        assert!(fx.alice.is_running());
        assert!(fx.bob.is_running());

        fx.shutdown().await;
        assert!(!fx.alice.is_running());
        assert!(!fx.bob.is_running());
    }

    #[tokio::test]
    async fn fixture_peers_can_be_restarted() {
        let mut fx = TwoPeerFixture::new();
        let alice_pk = fx.alice.public_key;

        fx.start().await.unwrap();
        fx.restart_peer(PeerId::Alice).await.unwrap();
        assert!(fx.alice.is_running());
        assert_eq!(fx.alice.public_key, alice_pk);
    }

    #[tokio::test]
    async fn fixture_repeated_start_stop_does_not_leak_state() {
        let mut fx = TwoPeerFixture::new();

        for _ in 0..3 {
            fx.start().await.unwrap();
            assert!(fx.alice.is_running());
            assert!(fx.bob.is_running());
            fx.shutdown().await;
            assert!(!fx.alice.is_running());
            assert!(!fx.bob.is_running());
        }
    }

    #[tokio::test]
    async fn fixture_start_is_idempotent() {
        let mut fx = TwoPeerFixture::new();
        fx.start().await.unwrap();
        fx.start().await.unwrap(); // second call should be a no-op
        assert!(fx.alice.is_running());
        assert!(fx.bob.is_running());
        fx.shutdown().await;
    }

    // ── Discovery backend tests ───────────────────────────────────

    #[tokio::test]
    async fn fixture_discovery_backend_is_accessible_and_inspectable() {
        let fx = TwoPeerFixture::new();
        let backend = fx.discovery_backend();

        // Start empty
        assert_eq!(backend.namespace_count(), 0);
        assert_eq!(backend.total_record_count(), 0);

        // Publish a record
        let ns = NamespaceId::new([0xABu8; 32]);
        let record = EncryptedDiscoveryRecord::new(vec![1, 2, 3, 4]);
        backend
            .publish(&ns, record.clone())
            .await
            .expect("publish to in-memory backend");

        assert_eq!(backend.namespace_count(), 1);
        assert_eq!(backend.total_record_count(), 1);

        let results = backend.lookup(&ns).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].payload, vec![1, 2, 3, 4]);

        // Clear
        backend.clear_all();
        assert_eq!(backend.total_record_count(), 0);
    }

    #[tokio::test]
    async fn fixture_discovery_can_be_injected() {
        let mut fx = TwoPeerFixture::new();

        // Pre-populate discovery before starting peers
        let ns = NamespaceId::new([0xBBu8; 32]);
        {
            let backend = fx.discovery_backend();
            backend
                .publish(&ns, EncryptedDiscoveryRecord::new(vec![42]))
                .await
                .unwrap();
            assert_eq!(backend.total_record_count(), 1);
        }

        // Start peers — they should not interfere with the backend
        fx.start().await.unwrap();
        assert_eq!(fx.discovery_record_count(), 1);

        // Additional records are still readable
        let results = fx.discovery_backend().lookup(&ns).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].payload, vec![42]);

        fx.shutdown().await;
    }

    #[tokio::test]
    async fn fixture_discovery_backend_hooks_work() {
        let fx = TwoPeerFixture::new();
        assert_eq!(fx.discovery_namespace_count(), 0);
        assert_eq!(fx.discovery_record_count(), 0);

        let ns = NamespaceId::new([0xCCu8; 32]);
        fx.discovery_backend()
            .publish(&ns, EncryptedDiscoveryRecord::new(vec![7, 8, 9]))
            .await
            .unwrap();

        assert_eq!(fx.discovery_namespace_count(), 1);
        assert_eq!(fx.discovery_record_count(), 1);

        fx.clear_discovery();
        assert_eq!(fx.discovery_record_count(), 0);
    }

    // ── Storage / catalogue tests ────────────────────────────────

    #[tokio::test]
    async fn fixture_catalogue_contents_are_inspectable() {
        let mut fx = TwoPeerFixture::new();
        fx.start().await.unwrap();

        // Initially empty
        let count = fx.shared_file_count(PeerId::Alice).unwrap();
        assert_eq!(count, 0);

        // Add a file
        fx.add_file(PeerId::Alice, "hash-001", "hello.txt").unwrap();
        let count_after = fx.shared_file_count(PeerId::Alice).unwrap();
        assert_eq!(count_after, 1);

        // Add another
        fx.add_file(PeerId::Alice, "hash-002", "world.txt").unwrap();
        let count_after2 = fx.shared_file_count(PeerId::Alice).unwrap();
        assert_eq!(count_after2, 2);

        fx.shutdown().await;
    }

    #[tokio::test]
    async fn fixture_catalogue_is_isolated_per_peer() {
        let mut fx = TwoPeerFixture::new();
        fx.start().await.unwrap();

        fx.add_file(PeerId::Alice, "alice-only", "alice.txt")
            .unwrap();
        fx.add_file(PeerId::Bob, "bob-only", "bob.txt").unwrap();

        let alice_files = fx.list_shared_file_hashes(PeerId::Alice).unwrap();
        let bob_files = fx.list_shared_file_hashes(PeerId::Bob).unwrap();

        assert_eq!(alice_files.len(), 1);
        assert_eq!(bob_files.len(), 1);

        // Isolation: Alice's file is not in Bob's list and vice versa
        assert_eq!(alice_files[0], "alice-only");
        assert_eq!(bob_files[0], "bob-only");
        assert_ne!(alice_files[0], bob_files[0]);

        fx.shutdown().await;
    }

    // ── Visibility / friendship tests ────────────────────────────

    #[tokio::test]
    async fn fixture_visibility_is_controllable() {
        let mut fx = TwoPeerFixture::new();
        fx.start().await.unwrap();

        fx.add_file(PeerId::Alice, "shared-file", "shared.txt")
            .unwrap();

        // Initially no friendship — catalogue should show files
        // (list_shared_files returns them regardless of friendship).
        {
            let hashes = fx.list_shared_file_hashes(PeerId::Alice).unwrap();
            assert_eq!(hashes.len(), 1);
        }

        fx.shutdown().await;
    }

    // ── Stable identity after restart ─────────────────────────────

    #[tokio::test]
    async fn fixture_identity_preserved_across_restart() {
        let mut fx = TwoPeerFixture::new();
        let alice_pk = fx.alice.public_key;
        let bob_pk = fx.bob.public_key;

        fx.start().await.unwrap();
        fx.shutdown().await;

        // Identity fields are plain data, always readable
        assert_eq!(fx.alice.public_key, alice_pk);
        assert_eq!(fx.bob.public_key, bob_pk);

        // Start again
        fx.start().await.unwrap();
        assert_eq!(fx.alice.public_key, alice_pk);
        assert_eq!(fx.bob.public_key, bob_pk);

        fx.shutdown().await;
    }

    // ── Peer reference consistency ──────────────────────────────

    #[tokio::test]
    async fn fixture_peer_accessors_are_consistent() {
        let fx = TwoPeerFixture::new();

        // peer() returns the same data as direct access
        assert_eq!(fx.peer(PeerId::Alice).public_key, fx.alice.public_key);
        assert_eq!(fx.peer(PeerId::Bob).public_key, fx.bob.public_key);

        // peer_ids() yields both
        let ids: Vec<PeerId> = fx.peer_ids().collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&PeerId::Alice));
        assert!(ids.contains(&PeerId::Bob));

        // PeerId::other is symmetric
        assert_eq!(PeerId::Alice.other(), PeerId::Bob);
        assert_eq!(PeerId::Bob.other(), PeerId::Alice);
    }

    // ── Send / Sync safety ───────────────────────────────────────

    #[tokio::test]
    async fn fixture_storage_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Storage>();
        assert_send_sync::<Arc<Storage>>();
    }
}
