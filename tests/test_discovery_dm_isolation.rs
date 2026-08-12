#![cfg(feature = "net")]

//! # Direct-message isolation test — DMs only on the direct topic
//!
//! BORU-DISC-23 (PDF task 20): after A and B create a friendship (both join
//! the deterministic pair topic [`direct_topic`]) and the internal discovery
//! topic is live as **networking infrastructure**, private direct messages
//! flow in BOTH directions using ONLY the expected direct topic — never the
//! discovery topic — and the discovery topic carries only discovery
//! payloads, never chat.
//!
//! ## What the test proves
//!
//! 1. **Friendship / direct chat established** — A and B compute the
//!    deterministic pair topic [`direct_topic`]`(pk_a, pk_b)` (unchanged by
//!    this refactor; the PDF hard rule) and both subscribe to it over a real
//!    in-process gossip mesh (the `OpenFriendChat` → `BackgroundSubscribe`
//!    pattern). A and B also join the internal discovery topic
//!    ([`discovery_topic`], [`TopicKind::Discovery`]) via
//!    [`DiscoveryService::join`] — the startup infrastructure path.
//! 2. **A→B and B→A both use ONLY the expected direct topic** — raw spy
//!    subscriptions on BOTH topics on BOTH nodes capture every payload with
//!    the topic it arrived on ([`WireSample`]). The spy samples prove A's DM
//!    reached B on the direct topic and B's DM reached A on the direct
//!    topic — and every direct-topic sample verifies as a chat
//!    [`SignedMessage`] (never a discovery payload).
//! 3. **No direct payload on discovery** — the discovery-topic spies see
//!    every discovery `Hello` / `Presence` that crossed the mesh (each
//!    decodes as a [`DiscoveryMessage`]) and NONE verify as a chat
//!    [`SignedMessage`]: no DM was ever routed through discovery (the hard
//!    rule).
//! 4. **No discovery payload on the direct topic** — the direct-topic spies
//!    never observe a discovery message, and vice versa the discovery spies
//!    never observe a chat message.
//! 5. **Domain separation** — the direct topic and the discovery topic are
//!    distinct, classify as [`TopicKind::Conversation`] vs
//!    [`TopicKind::Discovery`], and differ from the public lobby.
//!
//! The test CAPTURES the topic IDs used for each direction ([`WireSample`]
//! records the topic per payload) and asserts the isolation guarantee on
//! those captured samples.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use boru_core::{
    api::{Event as GossipEvent, GossipTopic},
    chat_core::{Message, SignedMessage},
    contact::direct_topic,
    discovery_message::DiscoveryMessage,
    discovery_service::{AnnounceOutcome, DiscoveryService, PeerSource},
    discovery_topic::{discovery_topic, is_discovery_topic, topic_kind, TopicKind},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    public_room::PublicNetwork,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint, PublicKey,
    RelayMode, SecretKey,
};
use n0_error::{bail_any, Result};
use n0_future::StreamExt;
use rand::{RngExt, SeedableRng};
use tokio::task::JoinHandle;

/// How long a two-node mesh may take to form (dial + topic joins + gossip
/// handshakes). Generous for CI, but every poll loop exits as soon as its
/// condition is satisfied.
const MESH_TIMEOUT: Duration = Duration::from_secs(20);
/// Poll interval while waiting for the mesh / spies.
const POLL_TICK: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Spawn a fresh in-process node: real iroh endpoint (no relay, loopback)
/// with the shared in-memory address book, plus a gossip actor and protocol
/// router. Mirrors the deterministic harness node setup.
async fn spawn_node(
    rng: &mut impl rand::Rng,
    memory: MemoryLookup,
) -> Result<(Router, Endpoint, SecretKey, Gossip)> {
    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(SecretKey::from_bytes(&rng.random()))
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

/// Deterministic test identity from a single seed byte.
fn test_key(byte: u8) -> PublicKey {
    let mut seed = [0u8; 32];
    seed[0] = byte;
    SecretKey::from_bytes(&seed).public()
}

/// A payload captured by a wire spy: the gossip topic it was received on and
/// the raw payload bytes. Recording the topic **per sample** is what lets the
/// test CAPTURE the topic IDs used for each direction and prove the
/// isolation guarantee.
#[derive(Debug, Clone)]
struct WireSample {
    topic: TopicId,
    content: Vec<u8>,
}

/// Spawn a raw spy subscription on `topic`: it captures every payload that
/// crossed the mesh on that topic (in addition to any service's own
/// subscription) so the test can prove which topic carried which payload.
async fn spawn_spy(
    gossip: &Gossip,
    topic: TopicId,
    collected: Arc<Mutex<Vec<WireSample>>>,
) -> Result<JoinHandle<()>> {
    let mut spy = gossip.subscribe(topic, Vec::new()).await?;
    Ok(tokio::spawn(async move {
        while let Some(Ok(event)) = spy.next().await {
            if let GossipEvent::Received(message) = event {
                collected
                    .lock()
                    .expect("spy lock poisoned")
                    .push(WireSample {
                        topic,
                        content: message.content.to_vec(),
                    });
            }
        }
    }))
}

/// A node under test: its network half is kept alive for the whole test.
struct DmNode {
    _router: Router,
    _endpoint: Endpoint,
    _gossip: Gossip,
    sk: SecretKey,
}

/// A two-node direct-message isolation harness: A and B are friends on the
/// deterministic pair topic AND both run the internal discovery topic as
/// networking infrastructure, over one real loopback gossip mesh.
struct DmIsolationHarness {
    a: DmNode,
    b: DmNode,
    /// The internal discovery gossip topic (infrastructure only).
    discovery: TopicId,
    /// The deterministic pair topic for A↔B (the direct conversation).
    direct: TopicId,
    pk_a: PublicKey,
    pk_b: PublicKey,
    service_a: DiscoveryService,
    service_b: DiscoveryService,
    /// The friendship/direct-chat subscriptions (OpenFriendChat →
    /// BackgroundSubscribe mirror).
    sub_direct_a: GossipTopic,
    sub_direct_b: GossipTopic,
    /// Captured wire samples, one spy per node per topic.
    spy_disc_a: Arc<Mutex<Vec<WireSample>>>,
    spy_disc_b: Arc<Mutex<Vec<WireSample>>>,
    spy_direct_a: Arc<Mutex<Vec<WireSample>>>,
    spy_direct_b: Arc<Mutex<Vec<WireSample>>>,
    _spy_disc_a: JoinHandle<()>,
    _spy_disc_b: JoinHandle<()>,
    _spy_direct_a: JoinHandle<()>,
    _spy_direct_b: JoinHandle<()>,
}

impl DmIsolationHarness {
    /// Start A and B: both join the internal discovery topic via
    /// [`DiscoveryService::join`] (B bootstraps to A), then both subscribe to
    /// the deterministic pair topic `direct_topic(pk_a, pk_b)` — the
    /// friendship / direct chat. Raw spies subscribe before anything else so
    /// no payload is missed.
    async fn spawn(rng: &mut impl rand::Rng, network: PublicNetwork) -> Result<Self> {
        let discovery = discovery_topic(network);

        // Shared in-memory address book: both endpoints can dial each other
        // by endpoint id (the deterministic two-node pattern).
        let memory = MemoryLookup::new();
        let (router_a, ep_a, sk_a, gossip_a) = spawn_node(rng, memory.clone()).await?;
        let (router_b, ep_b, sk_b, gossip_b) = spawn_node(rng, memory.clone()).await?;
        memory.add_endpoint_info(ep_a.addr());
        memory.add_endpoint_info(ep_b.addr());

        let pk_a = sk_a.public();
        let pk_b = sk_b.public();
        // The deterministic pair topic — do NOT change the derivation (PDF
        // hard rule). Same topic from either side (order-independent).
        let direct = direct_topic(&pk_a, &pk_b);
        assert_eq!(direct, direct_topic(&pk_b, &pk_a), "direct topic is order-independent");

        // Raw spies subscribe before the services / chat subs so nothing is
        // missed — one spy per node per topic.
        let spy_disc_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
        let spy_disc_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
        let spy_direct_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
        let spy_direct_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
        let spy_task_disc_a = spawn_spy(&gossip_a, discovery, spy_disc_a.clone()).await?;
        let spy_task_disc_b = spawn_spy(&gossip_b, discovery, spy_disc_b.clone()).await?;
        let spy_task_direct_a = spawn_spy(&gossip_a, direct, spy_direct_a.clone()).await?;
        let spy_task_direct_b = spawn_spy(&gossip_b, direct, spy_direct_b.clone()).await?;

        // Discovery networking infrastructure joins first (startup path from
        // `examples/iced_chat/main.rs`); B bootstraps to A.
        let service_a = DiscoveryService::join(&gossip_a, discovery, Vec::new(), pk_a)
            .await
            .expect("A joins the internal discovery topic")
            .with_announce_min_interval(Duration::ZERO);
        let service_b = DiscoveryService::join(&gossip_b, discovery, vec![ep_a.id()], pk_b)
            .await
            .expect("B joins the internal discovery topic")
            .with_announce_min_interval(Duration::ZERO);

        // The friendship / direct chat: both sides subscribe to the
        // deterministic pair topic. Each side bootstraps to the other (the
        // OpenFriendChat → BackgroundSubscribe pattern with the friend's
        // endpoint), so both swarms complete their join handshake and
        // broadcasts are not lost to the empty-mesh trap.
        let sub_direct_a = gossip_a.subscribe(direct, vec![ep_b.id()]).await?;
        let sub_direct_b = gossip_b.subscribe(direct, vec![ep_a.id()]).await?;

        Ok(Self {
            a: DmNode {
                _router: router_a,
                _endpoint: ep_a,
                _gossip: gossip_a,
                sk: sk_a,
            },
            b: DmNode {
                _router: router_b,
                _endpoint: ep_b,
                _gossip: gossip_b,
                sk: sk_b,
            },
            discovery,
            direct,
            pk_a,
            pk_b,
            service_a,
            service_b,
            sub_direct_a,
            sub_direct_b,
            spy_disc_a,
            spy_disc_b,
            spy_direct_a,
            spy_direct_b,
            _spy_disc_a: spy_task_disc_a,
            _spy_disc_b: spy_task_disc_b,
            _spy_direct_a: spy_task_direct_a,
            _spy_direct_b: spy_task_direct_b,
        })
    }

    /// Stop the spies and shut both discovery services down cleanly.
    async fn shutdown(self) {
        self._spy_disc_a.abort();
        self._spy_disc_b.abort();
        self._spy_direct_a.abort();
        self._spy_direct_b.abort();
        self.service_a.shutdown().await;
        self.service_b.shutdown().await;
    }
}

/// Wait until `service`'s peer registry contains `peer`.
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

/// Wait until `service`'s registry entry for `peer` reports `source`.
async fn wait_for_source(
    service: &DiscoveryService,
    peer: PublicKey,
    source: PeerSource,
    what: &str,
) -> Result<()> {
    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        if let Some((_, entry)) = service.known_peers().iter().find(|(id, _)| *id == peer) {
            if entry.source == source {
                return Ok(());
            }
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    bail_any!("timed out waiting for {what} to report source {source:?}")
}

/// Wait until a gossip topic subscription is joined — i.e. its stream has
/// processed at least one `NeighborUp` and the swarm edge exists — so a
/// broadcast is not lost to the empty-mesh trap. Polling the stream is
/// REQUIRED: `is_joined()` only reflects `NeighborUp` events that have been
/// drained from the subscription, and this helper drains them via
/// [`GossipTopic::joined`].
async fn wait_for_joined(sub: &mut GossipTopic, what: &str) -> Result<()> {
    match tokio::time::timeout(MESH_TIMEOUT, sub.joined()).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error.into()),
        Err(_) => bail_any!("timed out waiting for {what} to join"),
    }
}

/// Wait until the given spy has captured a chat [`SignedMessage`] whose text
/// equals `expected_text` (proves that direction's DM actually arrived).
async fn wait_for_dm(
    spy: &Arc<Mutex<Vec<WireSample>>>,
    expected_text: &str,
    what: &str,
) -> Result<()> {
    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        let samples = spy.lock().expect("spy lock poisoned").clone();
        for sample in &samples {
            if let Ok((_, Message::Message { text }, _)) =
                SignedMessage::verify_and_decode(&sample.content)
            {
                if text == expected_text {
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    let count = spy.lock().expect("spy lock poisoned").len();
    bail_any!("timed out waiting for {what}: spy captured {count} samples")
}

// ---------------------------------------------------------------------------
// Payload classification helpers
// ---------------------------------------------------------------------------

/// Assert every captured sample on a direct-topic spy is a chat
/// [`SignedMessage`] (never a discovery payload), and that the expected DM
/// text for this direction is present — i.e. this direction used ONLY the
/// direct topic.
fn assert_dm_direction(samples: &[WireSample], expected_topic: &TopicId, direction: &str, text: &str) {
    assert!(
        !samples.is_empty(),
        "{direction}: direct spy must have captured the DM"
    );
    let mut saw_expected = false;
    for sample in samples {
        assert_eq!(
            &sample.topic, expected_topic,
            "{direction}: DM payload arrived on the wrong topic: {sample:?}"
        );
        match SignedMessage::verify_and_decode(&sample.content) {
            Ok((from, Message::Message { text: got }, _)) => {
                assert!(
                    !from.as_bytes().iter().all(|&b| b == 0),
                    "{direction}: DM must be signed by a real key"
                );
                if got == text {
                    saw_expected = true;
                }
            }
            Ok((_, other, _)) => {
                panic!("{direction}: direct topic carried a non-DM chat payload: {other:?}")
            }
            Err(error) => {
                panic!("{direction}: direct topic carried a non-chat payload: {error}")
            }
        }
        assert!(
            postcard::from_bytes::<DiscoveryMessage>(&sample.content).is_err(),
            "{direction}: a discovery message leaked onto the direct topic: {sample:?}"
        );
    }
    assert!(
        saw_expected,
        "{direction}: expected DM text {text:?} never arrived on the direct topic"
    );
}

/// Assert every captured sample on a discovery-topic spy decodes as a
/// [`DiscoveryMessage`] and NONE verifies as a chat [`SignedMessage`] — no
/// direct payload was ever routed through discovery.
fn assert_discovery_only(samples: &[WireSample], expected_topic: &TopicId, who: &str) {
    assert!(
        !samples.is_empty(),
        "{who}: spy must have observed the discovery exchange on the topic"
    );
    for sample in samples {
        assert_eq!(
            &sample.topic, expected_topic,
            "{who}: discovery payload arrived on the wrong topic: {sample:?}"
        );
        let is_discovery = postcard::from_bytes::<DiscoveryMessage>(&sample.content).is_ok();
        // BORU-CP-04: control-plane presence envelopes (magic "BC") are the
        // second legitimate wire format on the discovery topic.
        let is_control = sample
            .content
            .starts_with(&boru_core::control_plane::message::CONTROL_PLANE_MAGIC)
            && matches!(
                boru_core::control_plane::message::ControlEnvelope::decode(&sample.content),
                Ok(boru_core::control_plane::message::ControlPlaneDecode::Message(_))
            );
        assert!(
            is_discovery || is_control,
            "{who}: discovery topic carried a non-discovery payload"
        );
        assert!(
            SignedMessage::verify_and_decode(&sample.content).is_err(),
            "{who}: discovery topic carried a chat payload (SignedMessage)"
        );
    }
}

// =========================================================================
// 1. A→B and B→A use ONLY the direct topic — never discovery
// =========================================================================

/// A and B are friends (both subscribed to the deterministic pair topic) and
/// both run the internal discovery topic as infrastructure. A sends a DM to B
/// and B sends a DM to A over a real loopback gossip mesh. The captured wire
/// samples prove: each direction used ONLY `direct_topic(pk_a, pk_b)`; the
/// discovery topic carried NO chat payload (only discovery messages); and the
/// direct topic carried NO discovery payload.
#[tokio::test]
async fn direct_messages_use_only_direct_topic_both_directions() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xD15C23A1); // BORU-DISC-23
    let mut harness = DmIsolationHarness::spawn(&mut rng, PublicNetwork::Test).await?;

    // ── Friendship mesh forms: discovery registry + direct-topic joins ──
    wait_for_peer(&harness.service_a, harness.pk_b, "A to learn B").await?;
    wait_for_peer(&harness.service_b, harness.pk_a, "B to learn A").await?;
    wait_for_joined(&mut harness.sub_direct_a, "A direct-topic subscription").await?;
    wait_for_joined(&mut harness.sub_direct_b, "B direct-topic subscription").await?;

    // Drive discovery presence traffic (the infrastructure side), so both
    // discovery spies have live samples to classify.
    assert_eq!(
        harness.service_a.announce_presence().await?,
        AnnounceOutcome::Announced,
        "A's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &harness.service_b,
        harness.pk_a,
        PeerSource::Presence,
        "B's registry for A",
    )
    .await?;
    assert_eq!(
        harness.service_b.announce_presence().await?,
        AnnounceOutcome::Announced,
        "B's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &harness.service_a,
        harness.pk_b,
        PeerSource::Presence,
        "A's registry for B",
    )
    .await?;

    // ── A → B: a private direct message on the direct topic ────────────
    let text_ab = "hello from A — direct only";
    let dm_ab = SignedMessage::sign_and_encode(
        &harness.a.sk,
        &Message::Message {
            text: text_ab.into(),
        },
    )
    .expect("A signs the direct message");
    harness.sub_direct_a.broadcast(dm_ab).await?;
    wait_for_dm(
        &harness.spy_direct_b,
        text_ab,
        "B to receive A's direct message",
    )
    .await?;

    // ── B → A: the reverse direction on the direct topic ───────────────
    let text_ba = "hello from B — direct only";
    let dm_ba = SignedMessage::sign_and_encode(
        &harness.b.sk,
        &Message::Message {
            text: text_ba.into(),
        },
    )
    .expect("B signs the direct message");
    harness.sub_direct_b.broadcast(dm_ba).await?;
    wait_for_dm(
        &harness.spy_direct_a,
        text_ba,
        "A to receive B's direct message",
    )
    .await?;

    // ── Capture the wire samples (topic ID per payload) ────────────────
    let disc_a = harness.spy_disc_a.lock().expect("spy lock poisoned").clone();
    let disc_b = harness.spy_disc_b.lock().expect("spy lock poisoned").clone();
    let direct_a = harness
        .spy_direct_a
        .lock()
        .expect("spy lock poisoned")
        .clone();
    let direct_b = harness
        .spy_direct_b
        .lock()
        .expect("spy lock poisoned")
        .clone();

    // ── Captured topic IDs — recorded as evidence in the test output ───
    println!("captured direct topic id:    {}", harness.direct);
    println!("captured discovery topic id: {}", harness.discovery);
    println!(
        "wire samples: discovery_A={} discovery_B={} direct_A={} direct_B={}",
        disc_a.len(),
        disc_b.len(),
        direct_a.len(),
        direct_b.len()
    );

    // ── Domain separation (the topics are distinct classes) ────────────
    assert_ne!(
        harness.direct, harness.discovery,
        "direct topic and discovery topic must be different TopicIds"
    );
    assert_eq!(
        topic_kind(harness.direct),
        TopicKind::Conversation,
        "direct topic is a conversation topic"
    );
    assert_eq!(
        topic_kind(harness.discovery),
        TopicKind::Discovery,
        "discovery topic is networking infrastructure"
    );
    assert!(is_discovery_topic(harness.discovery));

    // ── (a) Both directions use ONLY the expected direct topic ─────────
    assert_dm_direction(
        &direct_b,
        &harness.direct,
        "A→B",
        text_ab,
    );
    assert_dm_direction(
        &direct_a,
        &harness.direct,
        "B→A",
        text_ba,
    );

    // ── (b) NO direct payload ever crossed the discovery topic ─────────
    assert_discovery_only(&disc_a, &harness.discovery, "A discovery spy");
    assert_discovery_only(&disc_b, &harness.discovery, "B discovery spy");

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 2. Domain separation guards (no mesh needed)
// =========================================================================

/// The deterministic direct-topic derivation (untouched by this refactor) is
/// order-independent and domain-separated from the discovery topic and the
/// public lobby on every network.
#[test]
fn direct_and_discovery_topics_are_domain_separated() {
    let a = test_key(0xAB);
    let b = test_key(0xBC);
    let direct_ab = direct_topic(&a, &b);
    let direct_ba = direct_topic(&b, &a);
    assert_eq!(
        direct_ab, direct_ba,
        "direct topic must be order-independent (both sides derive the same)"
    );

    for network in [
        PublicNetwork::Mainnet,
        PublicNetwork::Development,
        PublicNetwork::Test,
    ] {
        let disc = discovery_topic(network);
        assert_ne!(
            direct_ab, disc,
            "{network:?}: direct topic must differ from the discovery topic"
        );
        assert_eq!(
            topic_kind(direct_ab),
            TopicKind::Conversation,
            "{network:?}: direct topic is a conversation"
        );
        assert_eq!(
            topic_kind(disc),
            TopicKind::Discovery,
            "{network:?}: discovery topic is infrastructure"
        );

        // Domain separation from the public lobby (a conversation, not
        // discovery, and not the direct topic).
        let lobby = boru_core::topic_derivation::public_room_topic(
            network.network_byte(),
            "public-lobby",
            1,
        );
        assert_ne!(
            direct_ab, lobby,
            "{network:?}: direct topic must differ from the public lobby"
        );
        assert_ne!(
            disc, lobby,
            "{network:?}: discovery topic must differ from the public lobby"
        );
    }
}
