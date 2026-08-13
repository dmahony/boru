#![cfg(feature = "net")]

//! # BORU-DIR-23 (PDF Phase 8): Required test matrix for the public room
//! directory
//!
//! The PDF's *"Required test matrix"* (Phase 8, Diagnostics and tests)
//! lists 16 scenarios that must hold for the public room directory. This
//! file is the dedicated integration test suite for those 16 scenarios —
//! two real Boru nodes (an advertiser/owner `A` and a viewer `B`) joined
//! to the internal discovery gossip topic over a loopback mesh, with the
//! advertisements/withdrawals crossing the real control-plane receive
//! gate into the bounded [`RoomDirectory`] cache.
//!
//! ## Scenario → test map
//!
//! | # | Scenario | Test |
//! |---|----------|------|
//! | 1 | Create discoverable room — other client sees it without joining | [`create_discoverable_room_visible_without_join`] |
//! | 2 | Create unlisted room — not visible via directory discovery | [`create_unlisted_room_not_visible`] |
//! | 3 | Create private room — no directory advertisement emitted | [`create_private_room_no_advertisement`] |
//! | 4 | Join advertised room — normal join path; room appears in conversations | [`join_advertised_room_normal_join_path`] |
//! | 5 | Open directory only — no topic subscription / membership change | [`open_directory_only_no_subscription_or_membership`] |
//! | 6 | Advertiser restarts — advertisement returns after discovery startup | [`advertiser_restart_republication_returns_room`] |
//! | 7 | Advertiser disappears — room becomes stale and expires after TTL | [`advertiser_disappears_room_expires_after_ttl`] |
//! | 8 | Room becomes unlisted — withdrawal removes it quickly, TTL as fallback | [`room_becomes_unlisted_withdrawal_removes_quickly`] |
//! | 9 | Room metadata changes — card updates, no duplicate entry | [`room_metadata_change_updates_without_duplicate`] |
//! | 10 | Duplicate advertisements — one entry, no UI churn | [`duplicate_advertisements_one_entry_no_churn`] |
//! | 11 | Malformed advertisement — rejected safely, no chat/UI corruption | [`malformed_advertisement_rejected_safely`] |
//! | 12 | Oversized advertisement — rejected before large allocation/rendering | [`oversized_advertisement_rejected_before_rendering`] |
//! | 13 | Unsupported room protocol — marked incompatible; Join blocked/explained | [`unsupported_room_protocol_marked_incompatible`] |
//! | 14 | Already joined room — directory shows Open instead of Join | [`already_joined_room_shows_open`] |
//! | 15 | Hidden room — does not reappear on refresh until unhidden | [`hidden_room_stays_hidden_until_unhidden`] |
//! | 16 | Spoofed withdrawal — cannot remove a room unless authority validates | [`spoofed_withdrawal_cannot_remove_room`] |
//!
//! The suite also exercises the production TTL wiring added by BORU-DIR-23:
//! [`DiscoveryService::with_directory_sweep_interval`] controls the
//! room-directory expiry sweep that evicts stale advertisements, so
//! scenarios 6/7's "expires after TTL" behaviour is covered by a unit test
//! in `src/discovery_service.rs` (`directory_expiry_sweep_evicts_expired_entries`)
//! and proven end-to-end here with deterministic fake-time eviction.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use boru_core::{
    control_plane::advertisement::{AdvertisementAuth, PublicRoomAdvertisement, RoomVisibility},
    control_plane::message::ControlEnvelope,
    conversations::ConversationStore,
    discovery_service::{AnnounceOutcome, ControlEvent, DiscoveryService},
    discovery_topic::discovery_topic,
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    public_room::PublicNetwork,
    room_directory::{LocalJoinState, LocalRoomFacts, RoomAction, RoomCompatibility},
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint,
    PublicKey, RelayMode, SecretKey,
};
use bytes::Bytes;
use n0_error::{bail_any, Result};
use n0_future::StreamExt;
use rand::{RngExt, SeedableRng};
use tempfile::TempDir;
use tokio::task::JoinHandle;

/// How long a two-node mesh may take to form and deliver an advertisement
/// (dial + topic join + gossip delivery). Generous for CI; every poll loop
/// exits as soon as its condition is satisfied.
const MESH_TIMEOUT: Duration = Duration::from_secs(20);
/// Poll interval while waiting for directory / registry state.
const POLL_TICK: Duration = Duration::from_millis(100);
/// Quiet window used to assert that *no* further event arrives (dedup,
/// rejection paths). Short — just long enough for a stray delivery to
/// surface, short enough to keep the suite fast.
const QUIET_WINDOW: Duration = Duration::from_millis(500);
/// The advertisement TTL used by most tests. The control-plane receive
/// gate clamps to a 60s minimum, which is far longer than any test waits —
/// entries are removed deterministically via
/// [`RoomDirectory::evict_expired_at`](boru_core::room_directory::RoomDirectory::evict_expired_at)
/// (the same fake-time pattern the app's periodic sweep uses in
/// production).
const ADVERT_TTL_SECS: u32 = 60;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A real in-process Boru node: endpoint + gossip + discovery service +
/// a fresh conversation store (the user-facing surface that must stay
/// untouched by discovery) + a raw gossip sender on the discovery topic
/// used to broadcast pre-built control envelopes directly over the mesh
/// (bypassing the announce throttle, which is a test-only concern — the
/// receive side is the code under test).
struct Node {
    _router: Router,
    _endpoint: Endpoint,
    _gossip: Gossip,
    secret: SecretKey,
    service: DiscoveryService,
    store: ConversationStore,
    _dir: TempDir,
    /// A second gossip subscription on the discovery topic, kept purely as
    /// a broadcast handle for raw control envelopes.
    _sender_keepalive: GossipSenderKeepalive,
}

/// Owns the split halves of the extra subscription so the sender stays
/// alive for the node's lifetime (dropping the receiver would not drop the
/// sender, but keeping both explicit is clearer).
struct GossipSenderKeepalive {
    _sender: boru_core::api::GossipSender,
    _receiver_task: JoinHandle<()>,
}

/// A two-node directory harness: A = advertiser/owner, B = viewer.
struct DirectoryHarness {
    a: Node,
    b: Node,
    topic: TopicId,
    pk_a: PublicKey,
    pk_b: PublicKey,
    memory: MemoryLookup,
    rng: rand::rngs::ChaCha12Rng,
}

/// Spawn a fresh in-process endpoint: no relay, loopback, shared in-memory
/// address book (the deterministic two-node pattern).
async fn spawn_node(
    rng: &mut impl rand::Rng,
    memory: MemoryLookup,
) -> Result<(Router, Endpoint, SecretKey, Gossip)> {
    let secret = SecretKey::from_bytes(&rng.random::<[u8; 32]>());
    spawn_node_with_secret(memory, secret).await
}

/// [`spawn_node`] with an explicit identity — used by the advertiser-restart
/// scenario, where the restarted node must keep the SAME secret key (same
/// room authority) as the original advertiser.
async fn spawn_node_with_secret(
    memory: MemoryLookup,
    secret: SecretKey,
) -> Result<(Router, Endpoint, SecretKey, Gossip)> {
    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(secret)
        .address_lookup(memory)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    let gossip = Gossip::builder().spawn(ep.clone());
    let router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();
    Ok((router, ep.clone(), ep.secret_key().clone(), gossip))
}

impl Node {
    /// Build a node over an already-spawned endpoint: fresh conversation
    /// store, discovery service joined to `topic`, and an extra raw
    /// broadcast sender on the topic.
    async fn new(
        router: Router,
        endpoint: Endpoint,
        secret: SecretKey,
        gossip: Gossip,
        topic: TopicId,
        bootstrap: Vec<PublicKey>,
    ) -> Result<Self> {
        let dir = TempDir::new().expect("temp dir for conversation store");
        let store = ConversationStore::empty_at(dir.path());
        let pk = secret.public();
        let service = DiscoveryService::join(&gossip, topic, bootstrap, pk)
            .await
            .expect("join discovery topic")
            .with_announce_min_interval(Duration::ZERO)
            .with_control_announce_min_interval(Duration::ZERO)
            .with_advert_min_interval(Duration::ZERO);
        // Extra subscription on the same topic, kept for raw broadcasting.
        let extra = gossip.subscribe(topic, Vec::new()).await?;
        let (sender, mut receiver) = extra.split();
        let receiver_task = tokio::spawn(async move {
            while receiver.next().await.is_some() {}
        });
        Ok(Self {
            _router: router,
            _endpoint: endpoint,
            _gossip: gossip,
            secret,
            service,
            store,
            _dir: dir,
            _sender_keepalive: GossipSenderKeepalive {
                _sender: sender,
                _receiver_task: receiver_task,
            },
        })
    }

    /// Broadcast a raw byte payload on the discovery gossip topic.
    async fn broadcast_bytes(&self, bytes: Vec<u8>) {
        let sender = match &self._sender_keepalive {
            GossipSenderKeepalive { _sender, .. } => _sender,
        };
        sender
            .broadcast(Bytes::from(bytes))
            .await
            .expect("raw broadcast on discovery topic");
    }

    /// Broadcast a pre-built control envelope on the discovery topic.
    async fn broadcast_envelope(&self, envelope: ControlEnvelope) {
        self.broadcast_bytes(envelope.encode()).await;
    }

    /// The node's directory cache handle.
    fn directory(&self) -> Arc<Mutex<boru_core::room_directory::RoomDirectory>> {
        self.service.room_directory()
    }

    async fn shutdown(self) {
        let GossipSenderKeepalive {
            _receiver_task, ..
        } = self._sender_keepalive;
        _receiver_task.abort();
        self.service.shutdown().await;
        // Graceful full teardown (same pattern as test_discovery_restart):
        // stopping the gossip actor sends Disconnect to peers and closes the
        // QUIC connection cleanly, so a peer that stays online processes
        // this node's departure and drops it as a neighbour. Without this, a
        // restarted advertiser's fresh join would be ignored because the
        // surviving peer still holds the old edge (the restart scenario).
        let _ = self._gossip.shutdown().await;
        drop((self._router, self._endpoint, self._dir, self.store));
    }
}

impl DirectoryHarness {
    /// Start A and B on the internal discovery topic. A subscribes with no
    /// bootstrap (B dials in); B bootstraps to A. Both exchange the join
    /// hello so the mesh edge forms before any advertisement is sent.
    async fn spawn() -> Result<Self> {
        let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xD1AB1E23);
        let topic = discovery_topic(PublicNetwork::Test);

        let memory = MemoryLookup::new();
        let (router_a, ep_a, sk_a, gossip_a) = spawn_node(&mut rng, memory.clone()).await?;
        let (router_b, ep_b, sk_b, gossip_b) = spawn_node(&mut rng, memory.clone()).await?;
        memory.add_endpoint_info(ep_a.addr());
        memory.add_endpoint_info(ep_b.addr());

        let pk_a = sk_a.public();
        let pk_b = sk_b.public();

        let a = Node::new(router_a, ep_a.clone(), sk_a, gossip_a, topic, Vec::new()).await?;
        let b = Node::new(
            router_b,
            ep_b.clone(),
            sk_b,
            gossip_b,
            topic,
            vec![ep_a.id()],
        )
        .await?;

        // Ensure the mesh edge exists before the tests start announcing.
        wait_for_peer(&a.service, pk_b, "A to learn B").await?;
        wait_for_peer(&b.service, pk_a, "B to learn A").await?;

        Ok(Self {
            a,
            b,
            topic,
            pk_a,
            pk_b,
            memory,
            rng,
        })
    }

    /// Spawn an extra node C (an attacker / second advertiser) joined to
    /// the same topic, bootstrapping to B. Used by the spoofing scenarios.
    async fn spawn_attacker(&mut self) -> Result<Node> {
        let (router_c, ep_c, sk_c, gossip_c) = spawn_node(&mut self.rng, self.memory.clone()).await?;
        self.memory.add_endpoint_info(ep_c.addr());
        let pk_c = sk_c.public();
        let node = Node::new(router_c, ep_c.clone(), sk_c, gossip_c, self.topic, vec![self.pk_b])
            .await?;
        // Let C join the mesh fully before the test proceeds: every edge of
        // the triangle (A↔C, C↔B) must be settled, or an advertisement
        // broadcast right after C joins can be dropped while the gossip
        // actor is still churning neighbour sets.
        wait_for_peer(&self.b.service, pk_c, "B to learn C").await?;
        wait_for_peer(&self.a.service, pk_c, "A to learn C").await?;
        wait_for_peer(&node.service, self.pk_a, "C to learn A").await?;
        wait_for_peer(&node.service, self.pk_b, "C to learn B").await?;
        Ok(node)
    }

    async fn shutdown(self) {
        self.a.shutdown().await;
        self.b.shutdown().await;
    }
}

// ---------------------------------------------------------------------------
// Advert / envelope builders
// ---------------------------------------------------------------------------

/// A minimal signed, discoverable advertisement for `room_id` owned by
/// `owner`. The caller is responsible for the owner holding the secret key.
fn signed_advert(
    owner: &SecretKey,
    room_id: TopicId,
    room_name: &str,
    visibility: RoomVisibility,
) -> PublicRoomAdvertisement {
    let mut advert = PublicRoomAdvertisement::minimal(room_id, room_name.to_string(), *owner.public().as_bytes());
    advert.visibility = visibility;
    advert.expires_after_secs = ADVERT_TTL_SECS;
    advert.sign(owner);
    advert
}

/// Current unix time in seconds — used for raw-broadcast timestamps and
/// for sequences that must exceed the discovery service's own
/// wall-clock-seeded sequence counter.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_secs()
}

/// A raw-broadcast control sequence guaranteed NEWER than anything the
/// discovery service has broadcast for the same identity: the service
/// seeds its sequence at `now_secs` and increments by one per announce, so
/// `now_secs() + 1_000_000 + offset` is always ahead of it.
fn raw_seq(offset: u64) -> u64 {
    now_secs() + 1_000_000 + offset
}

/// A control-plane PUBLIC_ROOM_ADVERTISEMENT envelope from `sender`'s node
/// carrying `advert` (which must be signed by `sender`'s key).
fn advert_envelope(sender: &SecretKey, sequence: u64, advert: PublicRoomAdvertisement) -> ControlEnvelope {
    ControlEnvelope::public_room_advertisement(sender.public(), sequence, now_secs(), advert)
}

/// A signed PUBLIC_ROOM_WITHDRAWAL envelope from `sender` for `room_id`,
/// claiming `claimed_owner` as the room's owner (normally the sender's own
/// key; spoofing tests pass a different owner to impersonate).
fn withdrawal_envelope(
    sender: &SecretKey,
    sequence: u64,
    room_id: TopicId,
    claimed_owner: [u8; 32],
) -> ControlEnvelope {
    let mut withdrawal =
        boru_core::control_plane::advertisement::PublicRoomWithdrawal::minimal(room_id, claimed_owner);
    withdrawal.timestamp_secs = now_secs();
    withdrawal.sign(sender);
    ControlEnvelope::public_room_withdrawal(sender.public(), sequence, now_secs(), withdrawal)
}

/// A room id derived deterministically from a byte (distinct topic bytes so
/// tests never collide).
fn room_id(byte: u8) -> TopicId {
    TopicId::from_bytes([byte; 32])
}

// ---------------------------------------------------------------------------
// Wait helpers
// ---------------------------------------------------------------------------

async fn wait_for_peer(service: &DiscoveryService, peer: PublicKey, what: &str) -> Result<()> {
    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        if service.known_peers().iter().any(|(id, _)| *id == peer) {
            return Ok(());
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    let known: Vec<String> = service
        .known_peers()
        .iter()
        .map(|(id, _)| id.fmt_short().to_string())
        .collect();
    bail_any!("timed out waiting for {what}: registry has {known:?}")
}

async fn wait_for_directory_entry(
    service: &DiscoveryService,
    id: TopicId,
    what: &str,
) -> Result<()> {
    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        let dir = service.room_directory();
        if dir.lock().unwrap().contains(&id) {
            return Ok(());
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    bail_any!("timed out waiting for {what} to appear in the directory")
}

async fn wait_for_directory_empty(
    service: &DiscoveryService,
    id: TopicId,
    what: &str,
) -> Result<()> {
    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        let dir = service.room_directory();
        if !dir.lock().unwrap().contains(&id) {
            return Ok(());
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    bail_any!("timed out waiting for {what} to leave the directory")
}

/// Assert that no `ControlEvent::RoomAdvertisement` arrives on `events`
/// within a quiet window (dedup / rejection paths must not churn the UI).
async fn assert_no_room_advertisement_event(
    events: &mut tokio::sync::broadcast::Receiver<ControlEvent>,
) {
    let deadline = Instant::now() + QUIET_WINDOW;
    while Instant::now() < deadline {
        match tokio::time::timeout(POLL_TICK, events.recv()).await {
            Ok(Ok(ControlEvent::RoomAdvertisement(_))) => {
                panic!("unexpected RoomAdvertisement event (dedup/rejection must not churn the UI)")
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break,
            Err(_) => {}
        }
    }
}

// =========================================================================
// 1. Create discoverable room — other client sees it without joining
// =========================================================================

/// A discovers a room: A announces a signed discoverable advertisement over
/// the real discovery mesh; B's directory cache gains the room. B has NOT
/// joined it (no conversation record, entry offers Join).
#[tokio::test]
async fn create_discoverable_room_visible_without_join() -> Result<()> {
    let harness = DirectoryHarness::spawn().await?;
    let id = room_id(0x01);

    let advert = signed_advert(
        &harness.a.secret,
        id,
        "Open Lounge",
        RoomVisibility::PublicDiscoverable,
    );
    assert_eq!(
        harness
            .a
            .service
            .announce_room_advertisement(advert.clone())
            .await?,
        AnnounceOutcome::Announced
    );

    // The other client sees it in Discover Rooms — without joining.
    wait_for_directory_entry(&harness.b.service, id, "B to see the discoverable room").await?;
    {
        let dir = harness.b.directory();
        let guard = dir.lock().unwrap();

        let entry = guard.get(&id).expect("room cached");
        assert_eq!(entry.advert.room_name, "Open Lounge");
        assert_eq!(
            entry.advert.visibility,
            RoomVisibility::PublicDiscoverable
        );
        assert_eq!(
            entry.auth,
            AdvertisementAuth::Verified {
                publisher: harness.pk_a
            },
            "an owner-signed advertisement is verified"
        );
        assert_eq!(
            entry.local_join_state,
            LocalJoinState::NotJoined,
            "discovering a room never joins it"
        );
        assert_eq!(entry.offered_action(), RoomAction::Join);
    }
    // And no conversation/membership record was created by discovery.
    assert!(
        harness.b.store.find(&id).is_none(),
        "discovery must never persist a room as a conversation"
    );

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 2. Create unlisted room — not visible via directory discovery
// =========================================================================

/// A tries to advertise a PublicUnlisted room: the emit-site guard refuses
/// it (NotDiscoverable, nothing broadcast) and B never sees it.
#[tokio::test]
async fn create_unlisted_room_not_visible() -> Result<()> {
    let harness = DirectoryHarness::spawn().await?;
    let id = room_id(0x02);

    let advert = signed_advert(&harness.a.secret, id, "Secret", RoomVisibility::PublicUnlisted);
    assert_eq!(
        harness
            .a
            .service
            .announce_room_advertisement(advert)
            .await?,
        AnnounceOutcome::NotDiscoverable,
        "unlisted rooms must never be advertised"
    );

    // Give a stray (incorrect) broadcast a chance to arrive, then verify B
    // never saw it.
    tokio::time::sleep(QUIET_WINDOW).await;
    assert!(
        !harness.b.directory().lock().unwrap().contains(&id),
        "unlisted room must not appear via directory discovery"
    );
    assert!(harness.b.directory().lock().unwrap().is_empty());

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 3. Create private room — no directory advertisement emitted
// =========================================================================

/// A tries to advertise a Private room: refused with NotDiscoverable, no
/// advertisement reaches B.
#[tokio::test]
async fn create_private_room_no_advertisement() -> Result<()> {
    let harness = DirectoryHarness::spawn().await?;
    let id = room_id(0x03);

    let advert = signed_advert(&harness.a.secret, id, "Private Club", RoomVisibility::Private);
    assert_eq!(
        harness
            .a
            .service
            .announce_room_advertisement(advert)
            .await?,
        AnnounceOutcome::NotDiscoverable,
        "private rooms must never emit a directory advertisement"
    );

    tokio::time::sleep(QUIET_WINDOW).await;
    assert!(
        !harness.b.directory().lock().unwrap().contains(&id),
        "private room must never appear in the directory"
    );
    assert!(harness.b.directory().lock().unwrap().is_empty());

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 4. Join advertised room — normal join path; room appears in conversations
// =========================================================================

/// B sees an advertised room (Join offered), then explicitly joins it:
/// the join subscribes to the room topic through the normal public-room
/// path and creates the local conversation record exactly once from the
/// advertised metadata (the app's `ensure_directory_joined_record`
/// semantics). After the join the directory derives Joined → Open.
#[tokio::test]
async fn join_advertised_room_normal_join_path() -> Result<()> {
    let mut harness = DirectoryHarness::spawn().await?;
    let id = room_id(0x04);

    let advert = signed_advert(
        &harness.a.secret,
        id,
        "Joinable Room",
        RoomVisibility::PublicDiscoverable,
    );
    harness
        .a
        .service
        .announce_room_advertisement(advert.clone())
        .await?;
    wait_for_directory_entry(&harness.b.service, id, "B to discover the room").await?;

    // Before the join: entry offers Join, no conversation record.
    {
        let dir = harness.b.directory();
        let guard = dir.lock().unwrap();

        let entry = guard.get(&id).unwrap();
        assert_eq!(entry.offered_action(), RoomAction::Join);
    }
    assert!(harness.b.store.find(&id).is_none());

    // ── Explicit join (normal path) ───────────────────────────────────
    // The join target is compatible (the advertised protocol matches), so
    // joining proceeds: subscribe to the room topic via the normal
    // public-room join path (gossip.subscribe; `subscribe_and_join` would
    // block here because the advertiser A is not subscribed to the room
    // topic, so there is no peer to confirm the join with), then create the
    // conversation record exactly once from the advertised metadata.
    let room_sub = harness
        .b
        ._gossip
        .subscribe(id, vec![harness.a._endpoint.id()])
        .await
        .expect("normal public-room join path subscribes to the room topic");
    let (_room_sender, _room_rx) = room_sub.split();

    // Replicate the app's `ensure_directory_joined_record`: create the
    // record from advertised metadata (name/visibility/description/tags),
    // never from any peer/privilege field (BORU-DIR-16/18).
    let mut entry = boru_core::conversations::ConversationEntry::new(
        id,
        "",
        advert.room_name.clone(),
    );
    entry.visibility = advert.visibility;
    entry.description = advert.short_description.clone();
    entry.tags = advert.tags.clone();
    harness.b.store.upsert(entry.clone());

    // The real local room database is the source of truth for Joined: feed
    // the facts back so the directory derives Joined → Open.
    harness
        .b
        .directory()
        .lock()
        .unwrap()
        .sync_local_states(LocalRoomFacts {
            joined: std::collections::BTreeSet::from([id]),
            pending: std::collections::BTreeSet::new(),
            hidden: std::collections::BTreeSet::new(),
        });

    // The room now appears in conversations, exactly once.
    assert!(
        harness.b.store.find(&id).is_some(),
        "joined room appears in conversations"
    );
    assert_eq!(
        harness.b.store.iter().filter(|e| e.topic == id).count(),
        1,
        "exactly one conversation record"
    );
    // Re-join / re-open is a no-op (no duplicate record).
    harness.b.store.upsert(entry);
    assert_eq!(
        harness.b.store.iter().filter(|e| e.topic == id).count(),
        1,
        "re-open never duplicates the record"
    );

    // The directory now shows Open instead of Join.
    {
        let dir = harness.b.directory();
        let guard = dir.lock().unwrap();

        let entry = guard.get(&id).unwrap();
        assert_eq!(entry.local_join_state, LocalJoinState::Joined);
        assert_eq!(entry.offered_action(), RoomAction::Open);
    }

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 5. Open directory only — no room topic subscription / membership change
// =========================================================================

/// B opens/reads the directory (the advertisement arrives and is cached):
/// no conversation record is created, the room's local state stays
/// NotJoined (no membership change), and the directory is a pure browse
/// surface — the room's chat topic was never subscribed.
#[tokio::test]
async fn open_directory_only_no_subscription_or_membership() -> Result<()> {
    let harness = DirectoryHarness::spawn().await?;
    let id = room_id(0x05);

    let advert = signed_advert(
        &harness.a.secret,
        id,
        "Browse Only",
        RoomVisibility::PublicDiscoverable,
    );
    harness
        .a
        .service
        .announce_room_advertisement(advert)
        .await?;
    wait_for_directory_entry(&harness.b.service, id, "B's directory to contain the room").await?;

    // B merely opened the directory: no membership change...
    {
        let dir = harness.b.directory();
        let guard = dir.lock().unwrap();

        let entry = guard.get(&id).unwrap();
        assert_eq!(
            entry.local_join_state,
            LocalJoinState::NotJoined,
            "opening the directory never changes membership"
        );
        assert_eq!(entry.offered_action(), RoomAction::Join);
    }
    // ...no conversation record (directory is separate from membership)...
    assert!(
        harness.b.store.find(&id).is_none(),
        "opening the directory never persists a conversation"
    );
    assert!(harness.b.store.is_empty());
    // ...and the discovery service's only subscription is the discovery
    // topic (it structurally cannot subscribe to room topics — the receive
    // path only ever writes to the bounded cache and emits typed events).
    assert_eq!(
        harness.b.service.topic(),
        harness.topic,
        "the discovery service subscribes only to the discovery topic"
    );

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 6. Advertiser restarts — advertisement returns after discovery startup
// =========================================================================

/// A announces a room, goes away (its entry expires at B), then "restarts"
/// with the SAME room authority (new endpoint, same secret key) and
/// re-publishes after discovery startup — B sees the room again.
#[tokio::test]
async fn advertiser_restart_republication_returns_room() -> Result<()> {
    let harness = DirectoryHarness::spawn().await?;
    let id = room_id(0x06);

    let advert = signed_advert(
        &harness.a.secret,
        id,
        "Persistent Room",
        RoomVisibility::PublicDiscoverable,
    );
    harness
        .a
        .service
        .announce_room_advertisement(advert.clone())
        .await?;
    wait_for_directory_entry(&harness.b.service, id, "B to see the room").await?;

    // A disappears (restart). Its advertisement goes stale at B and expires
    // after the TTL — the deterministic fake-time eviction the app's
    // periodic sweep performs (BORU-DIR-23).
    let a_secret = harness.a.secret.clone();
    harness.a.shutdown().await;
    {
        let dir_handle = harness.b.directory();

        let mut dir = dir_handle.lock().unwrap();
        let evicted = dir.evict_expired_at(Instant::now() + Duration::from_secs(61));
        assert_eq!(evicted, vec![id], "expired after TTL");
        assert!(!dir.contains(&id));
    }

    // A restarts: a fresh node with the SAME room authority (same secret
    // key → same owner_peer_id), joining the discovery topic and
    // republishing after discovery startup (PDF Task 3.1). The sequence
    // counter of the fresh service starts at wall-clock seconds, so the
    // re-announcement is strictly newer than A's pre-restart sequence and
    // is accepted by B's receive gate.
    let (router_a2, ep_a2, sk_a2, gossip_a2) =
        spawn_node_with_secret(harness.memory.clone(), a_secret).await?;
    // A restart gives the endpoint a fresh transient address; the shared
    // address book must REPLACE the stale pre-restart entry for the same
    // identity (add_endpoint_info would keep the dead address alongside).
    harness.memory.set_endpoint_info(ep_a2.addr());
    let a2 = Node::new(
        router_a2,
        ep_a2.clone(),
        sk_a2.clone(),
        gossip_a2,
        harness.topic,
        vec![harness.pk_b],
    )
    .await?;
    // Let the restarted node's gossip mesh with B form before the
    // re-announcement: A2 must have received B's control hello/presence
    // (the mesh edge is bidirectional once frames flow), otherwise a
    // broadcast sent before the edge exists is silently dropped and the
    // room would stay stale at B.
    wait_for_peer(&a2.service, harness.pk_b, "A2 to learn B after restart").await?;
    wait_for_peer(&harness.b.service, sk_a2.public(), "B to learn A2 after restart").await?;
    let advert2 = signed_advert(
        &sk_a2,
        id,
        "Persistent Room",
        RoomVisibility::PublicDiscoverable,
    );
    assert_eq!(
        a2.service.announce_room_advertisement(advert2.clone()).await?,
        AnnounceOutcome::Announced,
        "the restarted advertiser republishes after discovery startup"
    );
    // The production app re-announces on its periodic ~60s tick, so a
    // broadcast lost while the gossip mesh settles is retried: re-announce
    // once after the mesh has had time to form.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        a2.service.announce_room_advertisement(advert2).await?,
        AnnounceOutcome::Announced,
        "the periodic refresh re-announces the room"
    );

    // The room advertisement returns to B's directory.
    wait_for_directory_entry(&harness.b.service, id, "B to see the room after A restarts").await?;
    {
        let dir = harness.b.directory();
        let guard = dir.lock().unwrap();

        let entry = guard.get(&id).unwrap();
        assert_eq!(entry.advert.room_name, "Persistent Room");
        assert_eq!(entry.auth, AdvertisementAuth::Verified { publisher: sk_a2.public() });
    }

    a2.shutdown().await;
    harness.b.shutdown().await;
    Ok(())
}

// =========================================================================
// 7. Advertiser disappears — room becomes stale and expires after TTL
// =========================================================================

/// A announces a room and then disappears (shuts down). B's directory
/// keeps the room while its TTL is live, then the expiry sweep (the
/// deterministic fake-time equivalent of the production periodic sweep)
/// removes it: stale rooms cannot remain permanently live.
#[tokio::test]
async fn advertiser_disappears_room_expires_after_ttl() -> Result<()> {
    let harness = DirectoryHarness::spawn().await?;
    let id = room_id(0x07);

    let advert = signed_advert(
        &harness.a.secret,
        id,
        "Doomed Room",
        RoomVisibility::PublicDiscoverable,
    );
    harness
        .a
        .service
        .announce_room_advertisement(advert)
        .await?;
    wait_for_directory_entry(&harness.b.service, id, "B to see the room").await?;

    // Still live before the TTL elapses.
    {
        let dir = harness.b.directory();
        assert!(dir.lock().unwrap().contains(&id));
    }

    // A disappears; no refresh arrives.
    harness.a.shutdown().await;

    // The room becomes stale and expires after the TTL (eviction is the
    // same call the service's periodic directory-expiry sweep makes).
    {
        let dir_handle = harness.b.directory();

        let mut dir = dir_handle.lock().unwrap();
        let evicted = dir.evict_expired_at(Instant::now() + Duration::from_secs(61));
        assert_eq!(evicted, vec![id]);
        assert!(!dir.contains(&id), "expired room leaves the active directory");
        assert!(
            dir.snapshot().iter().all(|e| e.advert.room_id != id),
            "expired room no longer appears in the browse surface"
        );
    }

    harness.b.shutdown().await;
    Ok(())
}

// =========================================================================
// 8. Room becomes unlisted — withdrawal removes it quickly, TTL as fallback
// =========================================================================

/// A unlists a room: the owner sends a signed withdrawal; B removes the
/// advertisement immediately (well within the TTL — no waiting for
/// expiry). The TTL fallback is covered by scenario 7.
#[tokio::test]
async fn room_becomes_unlisted_withdrawal_removes_quickly() -> Result<()> {
    let harness = DirectoryHarness::spawn().await?;
    let id = room_id(0x08);

    let advert = signed_advert(
        &harness.a.secret,
        id,
        "Going Private",
        RoomVisibility::PublicDiscoverable,
    );
    harness
        .a
        .service
        .announce_room_advertisement(advert)
        .await?;
    wait_for_directory_entry(&harness.b.service, id, "B to see the room").await?;

    // The owner switches the room to unlisted and sends a withdrawal.
    harness
        .a
        .broadcast_envelope(withdrawal_envelope(
            &harness.a.secret,
            raw_seq(2),
            id,
            *harness.pk_a.as_bytes(),
        ))
        .await;

    // B removes the matching advertisement quickly (well under the TTL).
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !harness.b.directory().lock().unwrap().contains(&id) {
            break;
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    assert!(
        !harness.b.directory().lock().unwrap().contains(&id),
        "withdrawal removes the room quickly, TTL is only the fallback"
    );

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 9. Room metadata changes — card updates without a duplicate entry
// =========================================================================

/// A re-publishes the SAME room with changed metadata (new name, new
/// sequence): B's directory updates the single card to the new metadata —
/// never a second entry.
#[tokio::test]
async fn room_metadata_change_updates_without_duplicate() -> Result<()> {
    let harness = DirectoryHarness::spawn().await?;
    let id = room_id(0x09);

    let advert1 = signed_advert(
        &harness.a.secret,
        id,
        "Old Name",
        RoomVisibility::PublicDiscoverable,
    );
    harness
        .a
        .service
        .announce_room_advertisement(advert1)
        .await?;
    wait_for_directory_entry(&harness.b.service, id, "B to see the room").await?;
    assert_eq!(
        harness.b.directory().lock().unwrap().get(&id).unwrap().advert.room_name,
        "Old Name"
    );
    assert_eq!(harness.b.directory().lock().unwrap().len(), 1);

    // Same room_id, changed metadata, newer envelope sequence.
    let advert2 = signed_advert(
        &harness.a.secret,
        id,
        "New Name",
        RoomVisibility::PublicDiscoverable,
    );
    harness
        .a
        .broadcast_envelope(advert_envelope(&harness.a.secret, raw_seq(5), advert2))
        .await;

    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        let dir_handle = harness.b.directory();

        let dir = dir_handle.lock().unwrap();
        if dir.get(&id).is_some_and(|e| e.advert.room_name == "New Name") {
            break;
        }
        drop(dir);
        tokio::time::sleep(POLL_TICK).await;
    }

    let dir_handle = harness.b.directory();

    let dir = dir_handle.lock().unwrap();
    assert_eq!(dir.len(), 1, "metadata change never creates a duplicate entry");
    assert_eq!(
        dir.get(&id).unwrap().advert.room_name,
        "New Name",
        "directory card updates to the new metadata"
    );

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 10. Duplicate advertisements — one entry, no UI churn
// =========================================================================

/// A broadcasts the same advertisement twice (an exact re-broadcast — the
/// periodic refresh with a NEW envelope sequence but byte-identical content
/// is scenario 9's metadata update; here the SAME envelope bytes arrive
/// again, as gossip re-delivery or a naive repeat would): B keeps ONE entry
/// and no second `RoomAdvertisement` UI event fires (repeated gossip must
/// not churn the UI).
#[tokio::test]
async fn duplicate_advertisements_one_entry_no_churn() -> Result<()> {
    let harness = DirectoryHarness::spawn().await?;
    let id = room_id(0x0A);
    let mut events = harness.b.service.control_events();

    let advert = signed_advert(
        &harness.a.secret,
        id,
        "Stable Room",
        RoomVisibility::PublicDiscoverable,
    );
    let envelope = advert_envelope(&harness.a.secret, raw_seq(9), advert.clone());
    harness.a.broadcast_envelope(envelope.clone()).await;
    wait_for_directory_entry(&harness.b.service, id, "B to see the room").await?;

    // Consume the one legit event.
    loop {
        match tokio::time::timeout(POLL_TICK, events.recv()).await {
            Ok(Ok(ControlEvent::RoomAdvertisement(_))) => break,
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break,
            Err(_) => {}
        }
    }
    assert_eq!(harness.b.directory().lock().unwrap().len(), 1);

    // Identical advertisement re-broadcast: the receive gate's (sender,
    // sequence) dedup rejects the second copy before the directory (and
    // the gossip actor's content-hash dedup may drop it even earlier).
    harness.a.broadcast_envelope(envelope).await;

    // One entry; no second card.
    tokio::time::sleep(POLL_TICK).await;
    let dir_handle = harness.b.directory();

    let dir = dir_handle.lock().unwrap();
    assert_eq!(dir.len(), 1, "duplicate advertisement must not create a second entry");
    assert!(dir.contains(&id));
    drop(dir);

    // No UI churn: no second RoomAdvertisement event.
    assert_no_room_advertisement_event(&mut events).await;

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 11. Malformed advertisement — rejected safely, no chat/UI corruption
// =========================================================================

/// A raw, malformed control-plane frame (PUBLIC_ROOM_ADVERTISEMENT header
/// with garbage payload) is dropped by B's receive gate: no directory
/// entry, no event, no panic — and a subsequent valid advertisement is
/// still accepted (no corruption).
#[tokio::test]
async fn malformed_advertisement_rejected_safely() -> Result<()> {
    let harness = DirectoryHarness::spawn().await?;
    let malformed_id = room_id(0xBB);
    let mut events = harness.b.service.control_events();

    // Magic "BC" + version + garbage header/payload section: a control-plane
    // frame that cannot decode as a PUBLIC_ROOM_ADVERTISEMENT envelope is
    // dropped at the receive gate.
    let mut bytes = boru_core::control_plane::message::CONTROL_PLANE_MAGIC.to_vec();
    bytes.push(boru_core::control_plane::message::CONTROL_PLANE_PROTOCOL_VERSION);
    bytes.extend_from_slice(&[0xFF; 64]); // garbage header + payload

    harness.a.broadcast_bytes(bytes).await;

    // Rejected safely: no directory entry, no event, no panic.
    tokio::time::sleep(QUIET_WINDOW).await;
    assert!(
        !harness.b.directory().lock().unwrap().contains(&malformed_id),
        "malformed advertisement must not enter the directory"
    );
    assert!(harness.b.directory().lock().unwrap().is_empty());
    assert_no_room_advertisement_event(&mut events).await;

    // B still accepts a valid advertisement afterwards (no corruption).
    let good_id = room_id(0x11);
    let advert = signed_advert(
        &harness.a.secret,
        good_id,
        "Still Works",
        RoomVisibility::PublicDiscoverable,
    );
    harness
        .a
        .service
        .announce_room_advertisement(advert)
        .await?;
    wait_for_directory_entry(&harness.b.service, good_id, "B to accept the valid advertisement").await?;

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 12. Oversized advertisement — rejected before large allocation/rendering
// =========================================================================

/// A broadcasts an advertisement whose encoded size exceeds the protocol
/// bound (`max_encoded_len`): B's receive gate rejects it before any
/// allocation into the directory / UI rendering. A subsequent valid
/// advertisement is still accepted.
#[tokio::test]
async fn oversized_advertisement_rejected_before_rendering() -> Result<()> {
    let harness = DirectoryHarness::spawn().await?;
    let oversized_id = room_id(0xCC);
    let mut events = harness.b.service.control_events();

    // A room name far beyond `DEFAULT_MAX_ROOM_NAME_LEN` (64) — the
    // receive gate's minimal-advertisement policy (`AdvertisementBounds`)
    // rejects it before the directory (or any UI) ever sees it. Kept well
    // under `MAX_CONTROL_PAYLOAD_LEN` (4096) so the envelope itself still
    // encodes — the *protocol* bound is what must reject it.
    let mut advert = PublicRoomAdvertisement::minimal(
        oversized_id,
        "x".repeat(100),
        *harness.pk_a.as_bytes(),
    );
    advert.sign(&harness.a.secret);
    harness
        .a
        .broadcast_envelope(advert_envelope(&harness.a.secret, raw_seq(1), advert))
        .await;

    // Rejected before large allocation / UI rendering.
    tokio::time::sleep(QUIET_WINDOW).await;
    assert!(
        !harness.b.directory().lock().unwrap().contains(&oversized_id),
        "oversized advertisement must be rejected before entering the directory"
    );
    assert!(harness.b.directory().lock().unwrap().is_empty());
    assert_no_room_advertisement_event(&mut events).await;

    // B still accepts a valid advertisement afterwards.
    let good_id = room_id(0x12);
    let advert = signed_advert(
        &harness.a.secret,
        good_id,
        "Right Sized",
        RoomVisibility::PublicDiscoverable,
    );
    harness
        .a
        .service
        .announce_room_advertisement(advert)
        .await?;
    wait_for_directory_entry(&harness.b.service, good_id, "B to accept the valid advertisement").await?;

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 13. Unsupported room protocol — marked incompatible; Join blocked/explained
// =========================================================================

/// A room advertising a newer-than-adjacent chat protocol is marked
/// incompatible in B's directory: `compatibility == Unsupported`,
/// `local_join_state == Incompatible`, and the UI action is
/// `Incompatible` (Join is blocked/explained — the app-level join gate that
/// surfaces the explanation is unit-tested in `app.rs`).
#[tokio::test]
async fn unsupported_room_protocol_marked_incompatible() -> Result<()> {
    let harness = DirectoryHarness::spawn().await?;
    let id = room_id(0x13);

    let mut advert = signed_advert(
        &harness.a.secret,
        id,
        "Future Room",
        RoomVisibility::PublicDiscoverable,
    );
    advert.room_protocol_version = boru_core::public_room::PROTOCOL_VERSION + 2;
    advert.sign(&harness.a.secret); // re-sign after the field change
    harness
        .a
        .service
        .announce_room_advertisement(advert)
        .await?;
    wait_for_directory_entry(&harness.b.service, id, "B to see the incompatible room").await?;

    let dir_handle = harness.b.directory();

    let dir = dir_handle.lock().unwrap();
    let entry = dir.get(&id).unwrap();
    assert_eq!(
        entry.compatibility,
        RoomCompatibility::Unsupported,
        "a protocol more than one version newer is Unsupported"
    );
    assert_eq!(
        entry.local_join_state,
        LocalJoinState::Incompatible,
        "incompatible rooms are never offered as joinable"
    );
    assert_eq!(
        entry.offered_action(),
        RoomAction::Incompatible,
        "Join is blocked/explained for incompatible rooms"
    );
    // And it was never auto-joined / never created a conversation.
    assert!(harness.b.store.find(&id).is_none());
    drop(dir);

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 14. Already joined room — directory shows Open instead of Join
// =========================================================================

/// Once B has joined a room (the real local room database reports it), the
/// directory derives `Joined` and offers **Open** — never Join.
#[tokio::test]
async fn already_joined_room_shows_open() -> Result<()> {
    let harness = DirectoryHarness::spawn().await?;
    let id = room_id(0x14);

    let advert = signed_advert(
        &harness.a.secret,
        id,
        "My Room",
        RoomVisibility::PublicDiscoverable,
    );
    harness
        .a
        .service
        .announce_room_advertisement(advert)
        .await?;
    wait_for_directory_entry(&harness.b.service, id, "B to see the room").await?;
    assert_eq!(
        harness.b.directory().lock().unwrap().get(&id).unwrap().offered_action(),
        RoomAction::Join,
        "before joining, the room offers Join"
    );

    // The user has joined the room (real local room database source of
    // truth).
    harness
        .b
        .directory()
        .lock()
        .unwrap()
        .sync_local_states(LocalRoomFacts {
            joined: std::collections::BTreeSet::from([id]),
            pending: std::collections::BTreeSet::new(),
            hidden: std::collections::BTreeSet::new(),
        });

    let dir_handle = harness.b.directory();

    let guard = dir_handle.lock().unwrap();
    let entry = guard.get(&id).unwrap();
    assert_eq!(
        entry.local_join_state,
        LocalJoinState::Joined,
        "joined state derives from the real room database"
    );
    assert_eq!(
        entry.offered_action(),
        RoomAction::Open,
        "the directory offers Open instead of Join for an already-joined room"
    );

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 15. Hidden room — does not reappear on refresh until unhidden
// =========================================================================

/// B hides a room: it disappears from the browse surface; a re-advertised
/// refresh (same room, new sequence) does NOT bring it back; unhiding
/// restores it.
#[tokio::test]
async fn hidden_room_stays_hidden_until_unhidden() -> Result<()> {
    let harness = DirectoryHarness::spawn().await?;
    let id = room_id(0x15);

    let advert = signed_advert(
        &harness.a.secret,
        id,
        "Hide Me",
        RoomVisibility::PublicDiscoverable,
    );
    harness
        .a
        .service
        .announce_room_advertisement(advert.clone())
        .await?;
    wait_for_directory_entry(&harness.b.service, id, "B to see the room").await?;
    assert!(harness.b.directory().lock().unwrap().snapshot().iter().any(|e| e.advert.room_id == id));

    // B hides the room (persisted preference fed into the cache).
    harness
        .b
        .directory()
        .lock()
        .unwrap()
        .sync_local_states(LocalRoomFacts {
            joined: std::collections::BTreeSet::new(),
            pending: std::collections::BTreeSet::new(),
            hidden: std::collections::BTreeSet::from([id]),
        });

    {
        let dir_handle = harness.b.directory();

        let dir = dir_handle.lock().unwrap();
        assert!(
            dir.snapshot().iter().all(|e| e.advert.room_id != id),
            "hidden room disappears from the browse surface"
        );
        assert_eq!(
            dir.get(&id).unwrap().local_join_state,
            LocalJoinState::Blocked,
            "hidden preference derives Blocked"
        );
    }

    // A refreshes the advertisement (new sequence, same content): the room
    // does NOT reappear while the hide preference persists.
    harness
        .a
        .broadcast_envelope(advert_envelope(&harness.a.secret, raw_seq(7), advert.clone()))
        .await;
    tokio::time::sleep(POLL_TICK * 3).await;
    {
        let dir_handle = harness.b.directory();

        let dir = dir_handle.lock().unwrap();
        assert!(
            dir.snapshot().iter().all(|e| e.advert.room_id != id),
            "hidden room must not reappear on refresh"
        );
        assert!(dir.contains(&id), "the cache still holds the entry (hidden, not deleted)");
    }

    // B unhides: the room reappears in the browse surface.
    harness
        .b
        .directory()
        .lock()
        .unwrap()
        .sync_local_states(LocalRoomFacts {
            joined: std::collections::BTreeSet::new(),
            pending: std::collections::BTreeSet::new(),
            hidden: std::collections::BTreeSet::new(),
        });
    let dir_handle = harness.b.directory();

    let dir = dir_handle.lock().unwrap();
    assert!(
        dir.snapshot().iter().any(|e| e.advert.room_id == id),
        "unhiding restores the room to the browse surface"
    );
    assert_eq!(dir.get(&id).unwrap().offered_action(), RoomAction::Join);
    drop(dir);

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 16. Spoofed withdrawal — cannot remove a room unless authority validates
// =========================================================================

/// A third node C (NOT the room's authority) cannot remove A's room: a
/// withdrawal signed by C (either claiming C as owner, or impersonating A)
/// is rejected — the entry survives. Only the authority's own withdrawal
/// removes it.
#[tokio::test]
async fn spoofed_withdrawal_cannot_remove_room() -> Result<()> {
    let mut harness = DirectoryHarness::spawn().await?;
    let id = room_id(0x16);
    let attacker = harness.spawn_attacker().await?;

    let advert = signed_advert(
        &harness.a.secret,
        id,
        "Guarded Room",
        RoomVisibility::PublicDiscoverable,
    );
    harness
        .a
        .service
        .announce_room_advertisement(advert)
        .await?;
    wait_for_directory_entry(&harness.b.service, id, "B to see the room").await?;

    // Attack 1: C signs a withdrawal claiming ITSELF as the room's owner
    // (the realistic spoof — an attacker withdrawing a room they do not
    // own). The receive gate verifies the signature (valid for C) but the
    // directory's authority guard refuses to remove an entry owned by A.
    attacker
        .broadcast_envelope(withdrawal_envelope(
            &attacker.secret,
            raw_seq(1),
            id,
            *attacker.secret.public().as_bytes(),
        ))
        .await;
    tokio::time::sleep(POLL_TICK * 3).await;
    assert!(
        harness.b.directory().lock().unwrap().contains(&id),
        "a non-authority withdrawal must not remove the room"
    );

    // Attack 2: C forges a withdrawal claiming to BE A (owner = A). The
    // signature is C's, so verification against the sender fails the
    // authority check outright.
    attacker
        .broadcast_envelope(withdrawal_envelope(
            &attacker.secret,
            raw_seq(2),
            id,
            *harness.pk_a.as_bytes(),
        ))
        .await;
    tokio::time::sleep(POLL_TICK * 3).await;
    assert!(
        harness.b.directory().lock().unwrap().contains(&id),
        "a forged-authority withdrawal must not remove the room"
    );

    // The real authority's withdrawal DOES remove it.
    harness
        .a
        .broadcast_envelope(withdrawal_envelope(
            &harness.a.secret,
            raw_seq(3),
            id,
            *harness.pk_a.as_bytes(),
        ))
        .await;
    wait_for_directory_empty(&harness.b.service, id, "the authority's withdrawal to remove the room").await?;

    attacker.shutdown().await;
    harness.shutdown().await;
    Ok(())
}
