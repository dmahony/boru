#![cfg(feature = "net")]

//! # Group isolation test — group payloads stay on the group topic
//!
//! BORU-DISC-24 (PDF task 21): with the internal discovery topic live as
//! **networking infrastructure**, A and B explicitly join a group (a normal
//! conversation topic) and exchange messages. The captured wire samples
//! prove:
//!
//! 1. **Group membership is explicit, not discovery-granted** — A and B both
//!    subscribe to the group topic (the user-facing group join), and the
//!    group swarm's membership stays EXACTLY {A, B} — the discovery
//!    traffic (Hello / Presence) running concurrently on the internal
//!    discovery topic never changes group membership (BORU-DISC-11
//!    semantics: discovery does NOT grant membership).
//! 2. **Group payloads travel ONLY on the group topic** — A→B and B→A
//!    group messages arrive on the group topic (`WireSample` records the
//!    topic per payload), never on the discovery topic.
//! 3. **Discovery traffic stays on the discovery topic** — the discovery
//!    spies observe every Hello / Presence that crossed the mesh (each
//!    decodes as a [`DiscoveryMessage`]) and NONE verifies as a chat
//!    [`SignedMessage`]: no group payload was ever routed through discovery.
//! 4. **No discovery payload on the group topic** — the group spies only
//!    ever see chat [`SignedMessage`]s, never a [`DiscoveryMessage`].
//! 5. **Domain separation** — the group topic and the discovery topic are
//!    distinct, classify as [`TopicKind::Conversation`] vs
//!    [`TopicKind::Discovery`], and differ from the public lobby.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use boru_core::{
    api::{Event as GossipEvent, GossipTopic},
    chat_core::{Message, SignedMessage},
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

/// How long the two-node mesh may take to form (dial + topic joins + gossip
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

/// A payload captured by a wire spy: the gossip topic it was received on and
/// the raw payload bytes. Recording the topic **per sample** is what lets the
/// test CAPTURE the topic IDs used for group messages and prove the
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
struct GroupNode {
    _router: Router,
    _endpoint: Endpoint,
    _gossip: Gossip,
    sk: SecretKey,
}

/// A two-node group isolation harness: A and B are both members of a group
/// (explicit user-facing subscription to the group topic) AND both run the
/// internal discovery topic as networking infrastructure, over one real
/// loopback gossip mesh.
struct GroupIsolationHarness {
    a: GroupNode,
    b: GroupNode,
    /// The internal discovery gossip topic (infrastructure only).
    discovery: TopicId,
    /// The group topic A and B explicitly joined (a conversation).
    group: TopicId,
    pk_a: PublicKey,
    pk_b: PublicKey,
    service_a: DiscoveryService,
    service_b: DiscoveryService,
    /// The explicit group subscriptions (the group membership).
    sub_group_a: GossipTopic,
    sub_group_b: GossipTopic,
    /// Captured wire samples, one spy per node per topic.
    spy_disc_a: Arc<Mutex<Vec<WireSample>>>,
    spy_disc_b: Arc<Mutex<Vec<WireSample>>>,
    spy_group_a: Arc<Mutex<Vec<WireSample>>>,
    spy_group_b: Arc<Mutex<Vec<WireSample>>>,
    _spy_disc_a: JoinHandle<()>,
    _spy_disc_b: JoinHandle<()>,
    _spy_group_a: JoinHandle<()>,
    _spy_group_b: JoinHandle<()>,
}

impl GroupIsolationHarness {
    /// Start A and B: both join the internal discovery topic via
    /// [`DiscoveryService::join`] (B bootstraps to A), then A creates a group
    /// and both A and B explicitly subscribe to the group topic (each
    /// bootstrapping to the other so the swarm completes its join handshake).
    /// Raw spies subscribe before anything else so no payload is missed.
    async fn spawn(rng: &mut impl rand::Rng, network: PublicNetwork) -> Result<Self> {
        let discovery = discovery_topic(network);
        // The group topic: in the app a group is created with a fresh random
        // topic (`TopicId::from_bytes(rand::random())` in app/groups.rs) and
        // shared with members via the invitation flow. Mirror that here —
        // seeded by the test rng so the captured topic is reproducible.
        let group = TopicId::from_bytes(rng.random());

        // Shared in-memory address book: both endpoints can dial each other
        // by endpoint id (the deterministic two-node pattern).
        let memory = MemoryLookup::new();
        let (router_a, ep_a, sk_a, gossip_a) = spawn_node(rng, memory.clone()).await?;
        let (router_b, ep_b, sk_b, gossip_b) = spawn_node(rng, memory.clone()).await?;
        memory.add_endpoint_info(ep_a.addr());
        memory.add_endpoint_info(ep_b.addr());

        let pk_a = sk_a.public();
        let pk_b = sk_b.public();

        // Raw spies subscribe before the services / group subs so nothing is
        // missed — one spy per node per topic.
        let spy_disc_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
        let spy_disc_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
        let spy_group_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
        let spy_group_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
        let spy_task_disc_a = spawn_spy(&gossip_a, discovery, spy_disc_a.clone()).await?;
        let spy_task_disc_b = spawn_spy(&gossip_b, discovery, spy_disc_b.clone()).await?;
        let spy_task_group_a = spawn_spy(&gossip_a, group, spy_group_a.clone()).await?;
        let spy_task_group_b = spawn_spy(&gossip_b, group, spy_group_b.clone()).await?;

        // Discovery networking infrastructure joins first (startup path from
        // `src/bin/boru/main.rs`); B bootstraps to A.
        let service_a =
            DiscoveryService::join(&gossip_a, discovery, Vec::new(), pk_a, sk_a.clone())
                .await
                .expect("A joins the internal discovery topic")
                .with_announce_min_interval(Duration::ZERO);
        let service_b =
            DiscoveryService::join(&gossip_b, discovery, vec![ep_a.id()], pk_b, sk_b.clone())
                .await
                .expect("B joins the internal discovery topic")
                .with_announce_min_interval(Duration::ZERO);

        // The group: both members subscribe to the group topic. Each side
        // bootstraps to the other so both swarms complete their join
        // handshake and broadcasts are not lost to the empty-mesh trap.
        let sub_group_a = gossip_a.subscribe(group, vec![ep_b.id()]).await?;
        let sub_group_b = gossip_b.subscribe(group, vec![ep_a.id()]).await?;

        Ok(Self {
            a: GroupNode {
                _router: router_a,
                _endpoint: ep_a,
                _gossip: gossip_a,
                sk: sk_a,
            },
            b: GroupNode {
                _router: router_b,
                _endpoint: ep_b,
                _gossip: gossip_b,
                sk: sk_b,
            },
            discovery,
            group,
            pk_a,
            pk_b,
            service_a,
            service_b,
            sub_group_a,
            sub_group_b,
            spy_disc_a,
            spy_disc_b,
            spy_group_a,
            spy_group_b,
            _spy_disc_a: spy_task_disc_a,
            _spy_disc_b: spy_task_disc_b,
            _spy_group_a: spy_task_group_a,
            _spy_group_b: spy_task_group_b,
        })
    }

    /// Stop the spies and shut both discovery services down cleanly.
    async fn shutdown(self) {
        self._spy_disc_a.abort();
        self._spy_disc_b.abort();
        self._spy_group_a.abort();
        self._spy_group_b.abort();
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

/// Snapshot the group membership (current swarm neighbors) of a group
/// subscription. `EndpointId` is a type alias for `PublicKey`, so the
/// membership is directly comparable to the member public keys.
fn group_members(sub: &GossipTopic) -> Vec<PublicKey> {
    sub.neighbors().collect()
}

/// Wait until the given spy has captured a chat [`SignedMessage`] whose text
/// equals `expected_text` (proves that direction's group message actually
/// arrived).
async fn wait_for_group_msg(
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

/// Assert every captured sample on a group-topic spy is a chat
/// [`SignedMessage`] (never a discovery payload), and that the expected group
/// message text for this direction is present — i.e. this direction used ONLY
/// the group topic.
fn assert_group_direction(
    samples: &[WireSample],
    expected_topic: &TopicId,
    direction: &str,
    text: &str,
) {
    assert!(
        !samples.is_empty(),
        "{direction}: group spy must have captured the group message"
    );
    let mut saw_expected = false;
    for sample in samples {
        assert_eq!(
            &sample.topic, expected_topic,
            "{direction}: group payload arrived on the wrong topic: {sample:?}"
        );
        match SignedMessage::verify_and_decode(&sample.content) {
            Ok((from, Message::Message { text: got }, _)) => {
                assert!(
                    !from.as_bytes().iter().all(|&b| b == 0),
                    "{direction}: group message must be signed by a real key"
                );
                if got == text {
                    saw_expected = true;
                }
            }
            Ok((_, other, _)) => {
                panic!("{direction}: group topic carried a non-message chat payload: {other:?}")
            }
            Err(error) => {
                panic!("{direction}: group topic carried a non-chat payload: {error}")
            }
        }
        assert!(
            postcard::from_bytes::<DiscoveryMessage>(&sample.content).is_err(),
            "{direction}: a discovery message leaked onto the group topic: {sample:?}"
        );
    }
    assert!(
        saw_expected,
        "{direction}: expected group message text {text:?} never arrived on the group topic"
    );
}

/// Assert every captured sample on a discovery-topic spy decodes as a
/// [`DiscoveryMessage`] and NONE verifies as a chat [`SignedMessage`] — no
/// group payload was ever routed through discovery.
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
// 1. Group payloads stay on the group topic while discovery runs
// =========================================================================

/// A and B are both members of a group (explicit user-facing subscription to
/// the group topic) and both run the internal discovery topic as
/// infrastructure. A and B exchange group messages over a real loopback
/// gossip mesh while discovery presence traffic continues concurrently. The
/// captured wire samples prove: group messages used ONLY the group topic;
/// the group swarm membership stayed EXACTLY {A, B} (discovery does NOT
/// grant membership); the discovery topic carried NO chat payload; and the
/// group topic carried NO discovery payload.
#[tokio::test]
async fn group_messages_stay_on_group_topic_while_discovery_runs() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xD15C24A1); // BORU-DISC-24
    let mut harness = GroupIsolationHarness::spawn(&mut rng, PublicNetwork::Test).await?;

    // ── Mesh forms: discovery registry + group topic joins ────────────
    wait_for_peer(&harness.service_a, harness.pk_b, "A to learn B").await?;
    wait_for_peer(&harness.service_b, harness.pk_a, "B to learn A").await?;
    wait_for_joined(&mut harness.sub_group_a, "A group subscription").await?;
    wait_for_joined(&mut harness.sub_group_b, "B group subscription").await?;

    // The group swarm membership is exactly the two explicit members — no
    // discovery-granted membership.
    assert_eq!(
        group_members(&harness.sub_group_a),
        vec![harness.pk_b],
        "A's group membership must be exactly B before discovery traffic"
    );
    assert_eq!(
        group_members(&harness.sub_group_b),
        vec![harness.pk_a],
        "B's group membership must be exactly A before discovery traffic"
    );

    // ── Discovery presence traffic continues concurrently ─────────────
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

    // Discovery traffic must NOT change group membership (BORU-DISC-11:
    // discovery does not grant membership).
    assert_eq!(
        group_members(&harness.sub_group_a),
        vec![harness.pk_b],
        "A's group membership must be unchanged by discovery presence traffic"
    );
    assert_eq!(
        group_members(&harness.sub_group_b),
        vec![harness.pk_a],
        "B's group membership must be unchanged by discovery presence traffic"
    );

    // ── A → B: a group message on the group topic ─────────────────────
    let text_ab = "hello group from A — group topic only";
    let group_msg_ab = SignedMessage::sign_and_encode(
        &harness.a.sk,
        &Message::Message {
            text: text_ab.into(),
        },
    )
    .expect("A signs the group message");
    harness.sub_group_a.broadcast(group_msg_ab).await?;
    wait_for_group_msg(
        &harness.spy_group_b,
        text_ab,
        "B to receive A's group message",
    )
    .await?;

    // ── B → A: the reverse direction on the group topic ───────────────
    let text_ba = "hello group from B — group topic only";
    let group_msg_ba = SignedMessage::sign_and_encode(
        &harness.b.sk,
        &Message::Message {
            text: text_ba.into(),
        },
    )
    .expect("B signs the group message");
    harness.sub_group_b.broadcast(group_msg_ba).await?;
    wait_for_group_msg(
        &harness.spy_group_a,
        text_ba,
        "A to receive B's group message",
    )
    .await?;

    // ── Capture the wire samples (topic ID per payload) ────────────────
    let disc_a = harness
        .spy_disc_a
        .lock()
        .expect("spy lock poisoned")
        .clone();
    let disc_b = harness
        .spy_disc_b
        .lock()
        .expect("spy lock poisoned")
        .clone();
    let group_a = harness
        .spy_group_a
        .lock()
        .expect("spy lock poisoned")
        .clone();
    let group_b = harness
        .spy_group_b
        .lock()
        .expect("spy lock poisoned")
        .clone();

    // ── Captured topic IDs — recorded as evidence in the test output ───
    println!("captured group topic id:     {}", harness.group);
    println!("captured discovery topic id: {}", harness.discovery);
    println!(
        "wire samples: discovery_A={} discovery_B={} group_A={} group_B={}",
        disc_a.len(),
        disc_b.len(),
        group_a.len(),
        group_b.len()
    );

    // ── Domain separation (the topics are distinct classes) ────────────
    assert_ne!(
        harness.group, harness.discovery,
        "group topic and discovery topic must be different TopicIds"
    );
    assert_eq!(
        topic_kind(harness.group),
        TopicKind::Conversation,
        "group topic is a conversation topic"
    );
    assert_eq!(
        topic_kind(harness.discovery),
        TopicKind::Discovery,
        "discovery topic is networking infrastructure"
    );
    assert!(is_discovery_topic(harness.discovery));

    // ── (a) Both directions use ONLY the expected group topic ──────────
    assert_group_direction(&group_b, &harness.group, "A→B", text_ab);
    assert_group_direction(&group_a, &harness.group, "B→A", text_ba);

    // ── (b) NO group payload ever crossed the discovery topic ──────────
    assert_discovery_only(&disc_a, &harness.discovery, "A discovery spy");
    assert_discovery_only(&disc_b, &harness.discovery, "B discovery spy");

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 2. Domain separation guards (no mesh needed)
// =========================================================================

/// A group topic (a random topic, as the app creates groups) is a
/// Conversation-kind topic, domain-separated from the discovery topic and
/// the public lobby on every network.
#[test]
fn group_and_discovery_topics_are_domain_separated() {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xD15C24A2);

    for network in [
        PublicNetwork::Mainnet,
        PublicNetwork::Development,
        PublicNetwork::Test,
    ] {
        let disc = discovery_topic(network);
        // A group topic, created exactly like the app does
        // (TopicId::from_bytes(rand::random()) in app/groups.rs).
        let group = TopicId::from_bytes(rng.random());
        assert_ne!(
            group, disc,
            "{network:?}: group topic must differ from the discovery topic"
        );
        assert_eq!(
            topic_kind(group),
            TopicKind::Conversation,
            "{network:?}: group topic is a conversation"
        );
        assert_eq!(
            topic_kind(disc),
            TopicKind::Discovery,
            "{network:?}: discovery topic is infrastructure"
        );

        // Domain separation from the public lobby (a conversation, not
        // discovery, and not the group topic).
        let lobby = boru_core::topic_derivation::public_room_topic(
            network.network_byte(),
            "public-lobby",
            1,
        );
        assert_ne!(
            group, lobby,
            "{network:?}: group topic must differ from the public lobby"
        );
        assert_ne!(
            disc, lobby,
            "{network:?}: discovery topic must differ from the public lobby"
        );
    }
}
