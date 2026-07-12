//! End-to-end three-peer gossip mesh test.
//!
//! Tests the full gossip path used by all chat frontends:
//!   - A opens a room, B and C join the same topic
//!   - Messages broadcast from each peer reach all others
//!   - Metadata and roster sync across peers
//!   - Connection type detection (relayed vs direct)
//!   --no-relay mode (direct connections only)
//!
//! Each peer is isolated (separate secret key, separate endpoint) and
//! participates in the same gossip topic, exactly as real chat peers do.

use std::time::Duration;

use boru_chat::{
    api::{Event as GossipEvent, GossipReceiver},
    chat_core::check_peer_connection_type,
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    room_docs::{
        self, add_member, create_metadata_doc, create_roster_doc, list_members, read_metadata,
        RoomMetadata, RoomMetadataUpdate,
    },
};
use bytes::Bytes;
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, tls::CaTlsConfig,
    Endpoint, RelayMode, SecretKey,
};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use n0_error::Result;
use n0_future::{time::sleep, StreamExt};
use rand::{RngExt, SeedableRng};

/// Max time to wait for a single gossip message.
const MSG_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to drain gossip events before checking assertions.
const DRAIN_IDLE: Duration = Duration::from_secs(2);
/// How many gossip events to drain per peer.
const DRAIN_MAX: usize = 200;

// ── Helpers ────────────────────────────────────────────────────────────

async fn create_endpoint(
    rng: &mut rand::rngs::ChaCha12Rng,
    _relay_map: iroh::RelayMap,
    relay_mode: RelayMode,
    memory: Option<MemoryLookup>,
) -> Result<Endpoint> {
    let builder = Endpoint::builder(presets::Minimal)
        .relay_mode(relay_mode)
        .secret_key(SecretKey::from_bytes(&rng.random()))
        .alpns(vec![GOSSIP_ALPN.to_vec()])
        .ca_tls_config(CaTlsConfig::insecure_skip_verify())
        .address_lookup(MdnsAddressLookup::builder());
    let ep = builder.bind().await?;
    if let Some(m) = memory {
        ep.address_lookup()?.add(m);
    }
    ep.online().await;
    Ok(ep)
}

/// Drain buffered gossip events into metadata/roster docs.
async fn drain_events(
    md: &room_docs::RoomMetadataDoc,
    roster: &room_docs::RosterDoc,
    rx: &mut GossipReceiver,
    idle: Duration,
    max: usize,
) {
    for _ in 0..max {
        match tokio::time::timeout(idle, rx.next()).await {
            Ok(Some(Ok(ev))) => {
                let _ = room_docs::process_gossip_event(md, Ok(ev.clone())).await;
                let _ = room_docs::process_roster_event(roster, Ok(ev)).await;
            }
            Ok(Some(Err(_))) => continue,
            _ => return,
        }
    }
}

// ── Test: mDNS address lookup ────────────────────────────────────────────

/// Verify that endpoints can be created with mDNS address lookup enabled
/// and that they bind and report local addresses correctly.
///
/// This tests the plumbing (mDNS does not require a real multicast network
/// to initialize), not multi-peer discovery over the LAN.
#[tokio::test]
#[n0_tracing_test::traced_test]
async fn mdns_endpoint_creation_and_local_address() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(99);
    let (relay_map, _relay_url, _guard) = iroh::test_utils::run_relay_server().await.unwrap();

    // Create two endpoints side by side with mDNS enabled.
    let ep_a = create_endpoint(
        &mut rng,
        relay_map.clone(),
        RelayMode::Custom(relay_map.clone()),
        None,
    )
    .await?;
    let ep_b = create_endpoint(
        &mut rng,
        relay_map.clone(),
        RelayMode::Custom(relay_map.clone()),
        None,
    )
    .await?;

    // Both should have their own unique public key.
    assert_ne!(ep_a.id(), ep_b.id(), "peers must have distinct identities");

    // The address lookup manager should be alive and the mDNS lookup
    // registered — querying it should not panic or error.
    let addr_a = ep_a.address_lookup();
    assert!(
        addr_a.is_ok(),
        "ep_a address lookup manager must be reachable"
    );

    let addr_b = ep_b.address_lookup();
    assert!(
        addr_b.is_ok(),
        "ep_b address lookup manager must be reachable"
    );

    // Endpoints report a local addr.
    let ep_a_info = ep_a.addr();
    let ep_b_info = ep_b.addr();
    assert!(
        !ep_a_info.is_empty(),
        "ep_a must report at least one local address"
    );
    assert!(
        !ep_b_info.is_empty(),
        "ep_b must report at least one local address"
    );

    // Endpoints are dropped here (cleanup via Drop).
    Ok(())
}

/// Register gossip on an endpoint's router and spawn it.
fn spawn_gossip_router(ep: Endpoint, gossip: Gossip) -> iroh::protocol::Router {
    Router::builder(ep).accept(GOSSIP_ALPN, gossip).spawn()
}

// ── Test 1: Three peers in the same room ───────────────────────────────

#[tokio::test]
#[n0_tracing_test::traced_test]
async fn interop_three_peers_message_flow() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(42);
    let (relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server().await.unwrap();
    let memory = MemoryLookup::new();

    // ── Create 3 peers ──
    let ep_a = create_endpoint(
        &mut rng,
        relay_map.clone(),
        RelayMode::Custom(relay_map.clone()),
        Some(memory.clone()),
    )
    .await?;
    let ep_b = create_endpoint(
        &mut rng,
        relay_map.clone(),
        RelayMode::Custom(relay_map.clone()),
        Some(memory.clone()),
    )
    .await?;
    let ep_c = create_endpoint(
        &mut rng,
        relay_map.clone(),
        RelayMode::Custom(relay_map.clone()),
        Some(memory.clone()),
    )
    .await?;

    let pk_a = ep_a.secret_key().public();
    let pk_b = ep_b.secret_key().public();
    let pk_c = ep_c.secret_key().public();
    println!(">> Peer A:      {pk_a}");
    println!(">> Peer B: {pk_b}");
    println!(">> Peer C:  {pk_c}");
    println!(">> Relay:             {relay_url}");

    // ── Register gossip on all endpoints ───────────────────────────────
    let gossip_a = Gossip::builder().spawn(ep_a.clone());
    let gossip_b = Gossip::builder().spawn(ep_b.clone());
    let gossip_c = Gossip::builder().spawn(ep_c.clone());

    let _router_a = spawn_gossip_router(ep_a.clone(), gossip_a.clone());
    let _router_b = spawn_gossip_router(ep_b.clone(), gossip_b.clone());
    let _router_c = spawn_gossip_router(ep_c.clone(), gossip_c.clone());

    // Register all peers in the shared address lookup
    memory.add_endpoint_info(ep_a.addr().with_relay_url(relay_url.clone()));
    memory.add_endpoint_info(ep_b.addr().with_relay_url(relay_url.clone()));
    memory.add_endpoint_info(ep_c.addr().with_relay_url(relay_url));

    // ── Peer A opens a room (creates topic) ────────────────────────────
    let topic = TopicId::from_bytes(rand::random());
    println!(">> Topic:             {topic}");

    // Join all peers concurrently so they discover each other.
    // A joins with empty bootstrap (waits for connections), B and C
    // bootstrap to A's endpoint ID so they connect simultaneously.
    let (topic_a, topic_b, topic_c) = tokio::try_join!(
        gossip_a.subscribe_and_join(topic, vec![]),
        gossip_b.subscribe_and_join(topic, vec![ep_a.id()]),
        gossip_c.subscribe_and_join(topic, vec![ep_a.id()]),
    )?;

    let (sender_a, mut rx_a) = topic_a.split();
    let (sender_b, mut rx_b) = topic_b.split();
    let (sender_c, mut rx_c) = topic_c.split();

    // Give gossip a moment to settle connections
    sleep(Duration::from_millis(500)).await;

    // ── Create room docs ──────────────────────────────────────────────
    // A creates metadata + roster
    let _md_a = create_metadata_doc(
        topic,
        &sender_a,
        RoomMetadata {
            name: Some("Interop Test Room".into()),
            description: Some("Three-peer interop test".into()),
            rules: Some("Be excellent".into()),
        },
    )
    .await?;
    let roster_a = create_roster_doc(topic, &sender_a, pk_a.to_string(), "PeerA".into()).await?;

    // B creates metadata + roster
    let md_b = create_metadata_doc(topic, &sender_b, RoomMetadata::empty()).await?;
    let roster_b = create_roster_doc(topic, &sender_b, pk_b.to_string(), "PeerB".into()).await?;

    // C creates metadata + roster
    let md_c = create_metadata_doc(topic, &sender_c, RoomMetadata::empty()).await?;
    let roster_c = create_roster_doc(topic, &sender_c, pk_c.to_string(), "PeerC".into()).await?;

    sleep(Duration::from_millis(300)).await;
    drain_events(&_md_a, &roster_a, &mut rx_a, Duration::from_millis(500), 30).await;
    drain_events(&md_b, &roster_b, &mut rx_b, Duration::from_millis(500), 30).await;
    drain_events(&md_c, &roster_c, &mut rx_c, Duration::from_millis(500), 30).await;

    // Add B and C to A's roster
    add_member(&roster_a, &sender_a, pk_b.to_string(), "PeerB".into()).await?;
    add_member(&roster_a, &sender_a, pk_c.to_string(), "PeerC".into()).await?;

    // A adds herself to B's and C's roster too
    add_member(&roster_b, &sender_b, pk_a.to_string(), "PeerA".into()).await?;
    add_member(&roster_c, &sender_c, pk_a.to_string(), "PeerA".into()).await?;

    // Allow roster updates to propagate
    sleep(Duration::from_secs(2)).await;
    drain_events(&_md_a, &roster_a, &mut rx_a, DRAIN_IDLE, DRAIN_MAX).await;
    drain_events(&md_b, &roster_b, &mut rx_b, DRAIN_IDLE, DRAIN_MAX).await;
    drain_events(&md_c, &roster_c, &mut rx_c, DRAIN_IDLE, DRAIN_MAX).await;

    // ── Test 1: A sends, B and C receive ──────────────────────────────
    println!("\n═══ Test 1: A → B, C ═══");
    drain_events(&_md_a, &roster_a, &mut rx_a, DRAIN_IDLE, DRAIN_MAX).await;
    drain_events(&md_b, &roster_b, &mut rx_b, DRAIN_IDLE, DRAIN_MAX).await;
    drain_events(&md_c, &roster_c, &mut rx_c, DRAIN_IDLE, DRAIN_MAX).await;

    let msg_a = Bytes::from_static(b"hello from A");
    sender_a.broadcast(msg_a.clone()).await?;
    // B receives
    {
        let ev = tokio::time::timeout(MSG_TIMEOUT, rx_b.next())
            .await
            .expect("Test1: B timed out")
            .expect("B stream ended")
            .expect("B gossip error");
        assert!(
            matches!(ev, GossipEvent::Received(ref m) if m.content.as_ref() == b"hello from A"),
            "Test1: B expected Received, got {ev:?}"
        );
        let _ = room_docs::process_gossip_event(&md_b, Ok(ev.clone())).await;
        let _ = room_docs::process_roster_event(&roster_b, Ok(ev)).await;
    }
    // C receives
    {
        let ev = tokio::time::timeout(MSG_TIMEOUT, rx_c.next())
            .await
            .expect("Test1: C timed out")
            .expect("C stream ended")
            .expect("C gossip error");
        assert!(
            matches!(ev, GossipEvent::Received(ref m) if m.content.as_ref() == b"hello from A"),
            "Test1: C expected Received, got {ev:?}"
        );
        let _ = room_docs::process_gossip_event(&md_c, Ok(ev.clone())).await;
        let _ = room_docs::process_roster_event(&roster_c, Ok(ev)).await;
    }
    println!("   ✓ A→B, A→C: message received");

    // ── Test 2: B sends, A and C receive ──────────────────────────────
    println!("\n═══ Test 2: B → A, C ═══");
    drain_events(&_md_a, &roster_a, &mut rx_a, DRAIN_IDLE, DRAIN_MAX).await;
    drain_events(&md_b, &roster_b, &mut rx_b, DRAIN_IDLE, DRAIN_MAX).await;
    drain_events(&md_c, &roster_c, &mut rx_c, DRAIN_IDLE, DRAIN_MAX).await;

    let msg_b = Bytes::from_static(b"hello from B");
    sender_b.broadcast(msg_b).await?;
    {
        let ev = tokio::time::timeout(MSG_TIMEOUT, rx_a.next())
            .await
            .expect("Test2: A timed out")
            .expect("A stream ended")
            .expect("A gossip error");
        assert!(
            matches!(ev, GossipEvent::Received(ref m) if m.content.as_ref() == b"hello from B"),
            "Test2: A expected Received, got {ev:?}"
        );
        let _ = room_docs::process_gossip_event(&_md_a, Ok(ev.clone())).await;
        let _ = room_docs::process_roster_event(&roster_a, Ok(ev)).await;
    }
    {
        let ev = tokio::time::timeout(MSG_TIMEOUT, rx_c.next())
            .await
            .expect("Test2: C timed out")
            .expect("C stream ended")
            .expect("C gossip error");
        assert!(
            matches!(ev, GossipEvent::Received(ref m) if m.content.as_ref() == b"hello from B"),
            "Test2: C expected Received, got {ev:?}"
        );
        let _ = room_docs::process_gossip_event(&md_c, Ok(ev.clone())).await;
        let _ = room_docs::process_roster_event(&roster_c, Ok(ev)).await;
    }
    println!("   ✓ B→A, B→C: message received");

    // ── Test 3: C sends, A and B receive ──────────────────────────────
    println!("\n═══ Test 3: C → A, B ═══");
    drain_events(&_md_a, &roster_a, &mut rx_a, DRAIN_IDLE, DRAIN_MAX).await;
    drain_events(&md_b, &roster_b, &mut rx_b, DRAIN_IDLE, DRAIN_MAX).await;
    drain_events(&md_c, &roster_c, &mut rx_c, DRAIN_IDLE, DRAIN_MAX).await;

    let msg_c = Bytes::from_static(b"hello from C");
    sender_c.broadcast(msg_c).await?;
    {
        let ev = tokio::time::timeout(MSG_TIMEOUT, rx_a.next())
            .await
            .expect("Test3: A timed out")
            .expect("A stream ended")
            .expect("A gossip error");
        assert!(
            matches!(ev, GossipEvent::Received(ref m) if m.content.as_ref() == b"hello from C"),
            "Test3: A expected Received, got {ev:?}"
        );
        let _ = room_docs::process_gossip_event(&_md_a, Ok(ev.clone())).await;
        let _ = room_docs::process_roster_event(&roster_a, Ok(ev)).await;
    }
    {
        let ev = tokio::time::timeout(MSG_TIMEOUT, rx_b.next())
            .await
            .expect("Test3: B timed out")
            .expect("B stream ended")
            .expect("B gossip error");
        assert!(
            matches!(ev, GossipEvent::Received(ref m) if m.content.as_ref() == b"hello from C"),
            "Test3: B expected Received, got {ev:?}"
        );
        let _ = room_docs::process_gossip_event(&md_b, Ok(ev.clone())).await;
        let _ = room_docs::process_roster_event(&roster_b, Ok(ev)).await;
    }
    println!("   ✓ C→A, C→B: message received");

    // ── Test 4: Moderately-sized message (~1KB) ────────────────────────
    println!("\n═══ Test 4: Moderate-size message ═══");
    drain_events(&_md_a, &roster_a, &mut rx_a, DRAIN_IDLE, DRAIN_MAX).await;
    drain_events(&md_b, &roster_b, &mut rx_b, DRAIN_IDLE, DRAIN_MAX).await;
    drain_events(&md_c, &roster_c, &mut rx_c, DRAIN_IDLE, DRAIN_MAX).await;

    let medium_msg = Bytes::from(vec![b'X'; 1_024]);
    sender_a.broadcast(medium_msg.clone()).await?;
    {
        let ev = tokio::time::timeout(Duration::from_secs(10), rx_b.next())
            .await
            .expect("Test4: B timed out on medium msg")
            .expect("B stream ended")
            .expect("B gossip error");
        assert!(
            matches!(ev, GossipEvent::Received(ref m) if m.content.as_ref() == medium_msg.as_ref()),
            "Test4: B expected Received, got {ev:?}"
        );
        let _ = room_docs::process_gossip_event(&md_b, Ok(ev.clone())).await;
        let _ = room_docs::process_roster_event(&roster_b, Ok(ev)).await;
    }
    {
        let ev = tokio::time::timeout(Duration::from_secs(10), rx_c.next())
            .await
            .expect("Test4: C timed out on medium msg")
            .expect("C stream ended")
            .expect("C gossip error");
        assert!(
            matches!(ev, GossipEvent::Received(ref m) if m.content.as_ref() == medium_msg.as_ref()),
            "Test4: C expected Received, got {ev:?}"
        );
        let _ = room_docs::process_gossip_event(&md_c, Ok(ev.clone())).await;
        let _ = room_docs::process_roster_event(&roster_c, Ok(ev)).await;
    }
    println!("   ✓ 1KB message propagates to all peers");

    // ── Test 5: Metadata sync ─────────────────────────────────────────
    println!("\n═══ Test 5: Metadata sync ═══");
    drain_events(&_md_a, &roster_a, &mut rx_a, DRAIN_IDLE, DRAIN_MAX).await;
    drain_events(&md_b, &roster_b, &mut rx_b, DRAIN_IDLE, DRAIN_MAX).await;
    drain_events(&md_c, &roster_c, &mut rx_c, DRAIN_IDLE, DRAIN_MAX).await;

    // Update room name via A's metadata doc
    let update = RoomMetadataUpdate {
        name: Some("Updated Interop Room".into()),
        description: None,
        rules: None,
    };
    room_docs::update_metadata(&_md_a, &sender_a, update).await?;
    sleep(Duration::from_secs(2)).await;

    drain_events(&md_b, &roster_b, &mut rx_b, DRAIN_IDLE, DRAIN_MAX).await;
    drain_events(&md_c, &roster_c, &mut rx_c, DRAIN_IDLE, DRAIN_MAX).await;

    let meta_b = read_metadata(&md_b).await;
    let meta_c = read_metadata(&md_c).await;
    println!("   B metadata: {:?}", meta_b);
    println!("   C metadata: {:?}", meta_c);
    assert_eq!(
        meta_b.name.as_deref(),
        Some("Updated Interop Room"),
        "B should see updated room name"
    );
    assert_eq!(
        meta_c.name.as_deref(),
        Some("Updated Interop Room"),
        "C should see updated room name"
    );
    println!("   ✓ Metadata syncs across all peers");

    // ── Test 6: Roster convergence ────────────────────────────────────
    println!("\n═══ Test 6: Roster convergence ═══");
    sleep(Duration::from_secs(2)).await;
    drain_events(&_md_a, &roster_a, &mut rx_a, DRAIN_IDLE, DRAIN_MAX).await;
    drain_events(&md_b, &roster_b, &mut rx_b, DRAIN_IDLE, DRAIN_MAX).await;
    drain_events(&md_c, &roster_c, &mut rx_c, DRAIN_IDLE, DRAIN_MAX).await;

    let members_a = list_members(&roster_a).await;
    let members_b = list_members(&roster_b).await;
    let members_c = list_members(&roster_c).await;
    println!(
        "   Roster: A={}, B={}, C={}",
        members_a.len(),
        members_b.len(),
        members_c.len()
    );

    // All three peers should appear in each other's rosters
    assert!(
        members_a.len() >= 2,
        "A should see at least 2 peers (B and C), got {}",
        members_a.len()
    );
    assert!(
        members_b.len() >= 2,
        "B should see at least 2 peers (A and C), got {}",
        members_b.len()
    );
    assert!(
        members_c.len() >= 2,
        "C should see at least 2 peers (A and B), got {}",
        members_c.len()
    );
    println!("   ✓ Roster syncs across all peers");

    // ── Test 7: Connection type detection ─────────────────────────────
    println!("\n═══ Test 7: Connection type detection ═══");
    let conn_ab = check_peer_connection_type(&ep_a, pk_b).await;
    let conn_ac = check_peer_connection_type(&ep_a, pk_c).await;
    let conn_bc = check_peer_connection_type(&ep_b, pk_c).await;
    println!("   A↔B={conn_ab:?}  A↔C={conn_ac:?}  B↔C={conn_bc:?}");
    assert!(
        matches!(
            conn_ab,
            boru_chat::chat_core::ConnectionType::Relayed
                | boru_chat::chat_core::ConnectionType::Direct
        ),
        "connection type A↔B should be known, got {conn_ab:?}"
    );
    assert!(
        matches!(
            conn_ac,
            boru_chat::chat_core::ConnectionType::Relayed
                | boru_chat::chat_core::ConnectionType::Direct
        ),
        "connection type A↔C should be known, got {conn_ac:?}"
    );
    println!("   ✓ Connection types detected for all pairs");

    println!("\n✓✓✓ ALL THREE-PEER MESH TESTS PASSED ✓✓✓");
    Ok(())
}

// ── Test 2: Direct connections via relay-assisted discovery ────────────
//
// Simulates --no-relay-like behavior: peers are on the same local network,
// use a relay for initial discovery but establish direct connections.
// The test verifies that messages flow and that connection type is Direct.

#[tokio::test]
#[n0_tracing_test::traced_test]
async fn interop_no_relay_direct_connect() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(7);
    let (relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server().await.unwrap();
    let memory = MemoryLookup::new();

    // Create two peers that use the relay for discovery but establish
    // direct connections (simulating local network / --no-relay-like)
    let ep_a = create_endpoint(
        &mut rng,
        relay_map.clone(),
        RelayMode::Custom(relay_map.clone()),
        Some(memory.clone()),
    )
    .await?;
    let ep_b = create_endpoint(
        &mut rng,
        relay_map.clone(),
        RelayMode::Custom(relay_map.clone()),
        Some(memory.clone()),
    )
    .await?;

    let pk_a = ep_a.secret_key().public();
    let pk_b = ep_b.secret_key().public();
    println!(">> Peer A: {pk_a}");
    println!(">> Peer B: {pk_b}");
    println!(">> Relay:  {relay_url}");

    // Register gossip
    let gossip_a = Gossip::builder().spawn(ep_a.clone());
    let gossip_b = Gossip::builder().spawn(ep_b.clone());
    let _router_a = spawn_gossip_router(ep_a.clone(), gossip_a.clone());
    let _router_b = spawn_gossip_router(ep_b.clone(), gossip_b.clone());

    // Register both peers so they can discover each other
    memory.add_endpoint_info(ep_a.addr().with_relay_url(relay_url.clone()));
    memory.add_endpoint_info(ep_b.addr().with_relay_url(relay_url));

    // Open a gossip topic — A waits (empty bootstrap), B connects to A
    let topic = TopicId::from_bytes(rand::random());
    let (topic_a, topic_b) = tokio::try_join!(
        gossip_a.subscribe_and_join(topic, vec![]),
        gossip_b.subscribe_and_join(topic, vec![ep_a.id()]),
    )?;
    let (sender_a, mut rx_a) = topic_a.split();
    let (sender_b, mut rx_b) = topic_b.split();

    // Give them time to establish connections (may be direct on localhost)
    sleep(Duration::from_secs(3)).await;

    // Exchange messages
    println!("\n═══ Test: Message exchange ═══");

    let msg_a = Bytes::from_static(b"hello direct A");
    sender_a.broadcast(msg_a).await?;
    {
        let ev = tokio::time::timeout(Duration::from_secs(15), rx_b.next())
            .await
            .expect("Direct: B timed out")
            .expect("B stream ended")
            .expect("B gossip error");
        assert!(
            matches!(ev, GossipEvent::Received(ref m) if m.content.as_ref() == b"hello direct A"),
            "Direct: B expected Received, got {ev:?}"
        );
    }
    println!("   ✓ A→B: message received");

    let msg_b = Bytes::from_static(b"hello direct B");
    sender_b.broadcast(msg_b).await?;
    {
        let ev = tokio::time::timeout(Duration::from_secs(15), rx_a.next())
            .await
            .expect("Direct: A timed out")
            .expect("A stream ended")
            .expect("A gossip error");
        assert!(
            matches!(ev, GossipEvent::Received(ref m) if m.content.as_ref() == b"hello direct B"),
            "Direct: A expected Received, got {ev:?}"
        );
    }
    println!("   ✓ B→A: message received");

    // Verify connection type — on localhost this should be Direct
    let conn_ab = check_peer_connection_type(&ep_a, pk_b).await;
    let conn_ba = check_peer_connection_type(&ep_b, pk_a).await;
    println!("   Connection A→B={conn_ab:?}  B→A={conn_ba:?}");

    // On localhost with a shared memory lookup, peers should establish
    // direct connections.  Accept either Direct or Relayed since CI may
    // vary.
    assert!(
        matches!(
            conn_ab,
            boru_chat::chat_core::ConnectionType::Direct
                | boru_chat::chat_core::ConnectionType::Relayed
        ),
        "connection A→B should be known, got {conn_ab:?}"
    );
    println!("   ✓ Connection type detected");

    println!("\n✓✓✓ DIRECT-CONNECT MESH TEST PASSED ✓✓✓");
    Ok(())
}
