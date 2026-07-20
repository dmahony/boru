//! Integration test: bootstrap reliability improvements.
//!
//! Tests that:
//!   1. `collect_bootstrap_peers` correctly deduplicates and merges
//!      address sources from multiple sources (ticket + room store).
//!   2. RoomStore round-trips peer addresses for later reuse.
//!   3. After the original bootstrap peer goes offline, a newcomer
//!      can still join the room via a relay-mediated connection
//!      when bootstrap peers are stale (same as test_no_bootstrap).
//!   4. The bootstrap-refresh path (set_peers → load → use) works.
//!
//! Tests 1–2 are pure unit tests.  Tests 3–4 are integration tests
//! that verify end-to-end behaviour.

use std::time::Duration;

use boru_chat::chat_core::{collect_bootstrap_peers, seed_memory_lookup};
use boru_chat::net::{Gossip, GOSSIP_ALPN};
use boru_chat::proto::TopicId;
use boru_chat::room::RoomStore;
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, EndpointAddr,
    PublicKey, RelayMode, RelayUrl, SecretKey,
};
use n0_error::Result;
use n0_future::{time::sleep, StreamExt};
use rand::{RngExt, SeedableRng};

// ── Test 1: collect_bootstrap_peers deduplication and merging ──────────

#[test]
fn test_collect_bootstrap_peers_dedup() {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(1);
    let sk1 = SecretKey::from_bytes(&rng.random());
    let sk2 = SecretKey::from_bytes(&rng.random());
    let pk1 = sk1.public();
    let pk2 = sk2.public();

    let addr1 = EndpointAddr::new(pk1)
        .with_relay_url("https://relay-1.example.com".parse::<RelayUrl>().unwrap());
    let addr2 = EndpointAddr::new(pk2)
        .with_relay_url("https://relay-2.example.com".parse::<RelayUrl>().unwrap());
    // Duplicate of addr1 with a different relay (should be deduped by id)
    let addr1_dup = EndpointAddr::new(pk1)
        .with_relay_url("https://relay-3.example.com".parse::<RelayUrl>().unwrap());

    let ticket_peers = [addr1.clone(), addr2.clone()];
    let room_peers = [addr1_dup.clone()]; // same pk1, different relay

    let (peer_ids, all_addrs) = collect_bootstrap_peers([&ticket_peers[..], &room_peers[..]]);

    // Should have 2 unique peer IDs (pk1, pk2)
    assert_eq!(peer_ids.len(), 2, "should have 2 unique peer IDs");
    assert!(peer_ids.contains(&pk1), "pk1 should be in peer_ids");
    assert!(peer_ids.contains(&pk2), "pk2 should be in peer_ids");

    // Should have 2 unique addresses (first addr1 wins, plus addr2)
    assert_eq!(all_addrs.len(), 2, "should have 2 unique addresses");
    // The first addr1 entry (with relay-1) should be preserved
    let addr1_found = all_addrs.iter().find(|a| a.id == pk1).unwrap();
    let relay_urls_1: Vec<_> = addr1_found.relay_urls().collect();
    assert!(
        relay_urls_1
            .iter()
            .any(|u| u.to_string().contains("relay-1")),
        "first source's relay URL should be preserved"
    );

    // Empty sources
    let (ids, addrs) = collect_bootstrap_peers([&[] as &[EndpointAddr]]);
    assert!(ids.is_empty(), "empty sources → empty peer_ids");
    assert!(addrs.is_empty(), "empty sources → empty addrs");
}

// ── Test 2: seed_memory_lookup populates the address lookup ───────────

#[test]
fn test_seed_memory_lookup_populates() {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(2);
    let sk = SecretKey::from_bytes(&rng.random());
    let pk = sk.public();

    let addr = EndpointAddr::new(pk).with_relay_url(
        "https://relay-test.example.com"
            .parse::<RelayUrl>()
            .unwrap(),
    );

    let lookup = MemoryLookup::new();
    seed_memory_lookup(&lookup, std::slice::from_ref(&addr));

    let resolved = lookup.get_endpoint_info(pk);
    assert!(
        resolved.is_some(),
        "seed_memory_lookup should add the address"
    );
    let resolved_addr = resolved.unwrap().into_endpoint_addr();
    assert_eq!(resolved_addr.id, pk, "resolved ID should match");
}

// ── Test 3: RoomStore peers round-trip ────────────────────────────────

#[test]
fn test_room_store_peers_roundtrip() -> Result<()> {
    let dir = tempfile::tempdir().map_err(|e| n0_error::anyerr!("{e}"))?;
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(3);

    let topic = TopicId::from_bytes(rng.random());
    let sk = SecretKey::from_bytes(&rng.random());
    let pk = sk.public();
    let addr = EndpointAddr::new(pk);

    // Create store with a topic
    let mut store = RoomStore::new(dir.path(), topic);
    assert!(store.peers.is_empty(), "new store has empty peers");

    // Set peers and persist
    store.set_peers(vec![addr.clone()])?;

    // Reload from disk
    let loaded = RoomStore::load(dir.path())?.expect("should load saved room store");
    assert_eq!(loaded.topic, topic, "topic should survive roundtrip");
    assert_eq!(loaded.peers.len(), 1, "should have 1 peer");
    assert_eq!(loaded.peers[0].id, pk, "peer ID should survive roundtrip");

    // Clear peers
    let mut reloaded = loaded;
    reloaded.clear_peers()?;
    let cleared = RoomStore::load(dir.path())?.expect("should load cleared store");
    assert!(cleared.peers.is_empty(), "cleared store has empty peers");

    // Load a v1-format file (no peers field) — the store migrates
    // it to the current schema version with empty peers.
    let v1_path = dir.path().join("room.json");
    let topic_bytes: Vec<u8> = topic.as_bytes().to_vec();
    let v1_json = serde_json::json!({
        "schema_version": 1,
        "topic": topic_bytes
    });
    std::fs::write(&v1_path, v1_json.to_string()).map_err(|e| n0_error::anyerr!("{e}"))?;
    let migrated =
        RoomStore::load(dir.path())?.expect("v1 format should be migrated to current schema");
    assert_eq!(
        migrated.schema_version, 3,
        "v1 file should be migrated to v3"
    );
    assert_eq!(migrated.topic, topic, "topic should survive v1 migration");
    assert!(
        migrated.peers.is_empty(),
        "v1 file has no peers, should default to empty"
    );
    assert!(
        migrated.discovery_secret.is_none(),
        "v1 file has no discovery_secret"
    );

    Ok(())
}

// ── Test 4: Stale bootstrap scenario —
//    can a newcomer still join when the original bootstrap peer is gone?
//    This uses the relay to mediate the connection, same pattern as
//    test_no_bootstrap.

async fn spawn_peer(
    rng: &mut impl rand::Rng,
    relay_map: iroh::RelayMap,
    memory_lookup: MemoryLookup,
) -> Result<(Router, iroh::Endpoint, SecretKey, Gossip, PublicKey)> {
    let ep = iroh::Endpoint::builder(presets::N0)
        .secret_key(SecretKey::from_bytes(&rng.random()))
        .address_lookup(memory_lookup)
        .relay_mode(RelayMode::Custom(relay_map))
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .bind()
        .await?;
    ep.online().await;
    let pk = ep.secret_key().public();
    let gossip = Gossip::builder().spawn(ep.clone());
    let router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();
    Ok((router, ep.clone(), ep.secret_key().clone(), gossip, pk))
}

/// Spawn a peer using a shared MemoryLookup (for cross-peer address discovery).
async fn spawn_peer_with_lookup(
    rng: &mut impl rand::Rng,
    lookup: MemoryLookup,
    relay_map: iroh::RelayMap,
) -> Result<(Router, iroh::Endpoint, SecretKey, Gossip, PublicKey)> {
    let ep = iroh::Endpoint::builder(presets::N0)
        .secret_key(SecretKey::from_bytes(&rng.random()))
        .address_lookup(lookup)
        .relay_mode(RelayMode::Custom(relay_map))
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .bind()
        .await?;
    ep.online().await;
    let pk = ep.secret_key().public();
    let gossip = Gossip::builder().spawn(ep.clone());
    let router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();
    Ok((router, ep.clone(), ep.secret_key().clone(), gossip, pk))
}

#[tokio::test]
async fn test_stale_bootstrap_peer_does_not_block_join() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(42);
    let (relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server().await.unwrap();
    let shared_memory = MemoryLookup::new();

    // ── Phase 1: A and B connect ─────────────────────────────────────
    let (router_a, ep_a, _sk_a, gossip_a, pk_a) =
        spawn_peer(&mut rng, relay_map.clone(), shared_memory.clone()).await?;
    shared_memory.add_endpoint_info(ep_a.addr().with_relay_url(relay_url.clone()));
    let (router_b, ep_b, _sk_b, gossip_b, pk_b) =
        spawn_peer(&mut rng, relay_map.clone(), shared_memory.clone()).await?;
    shared_memory.add_endpoint_info(ep_b.addr().with_relay_url(relay_url.clone()));

    println!("A: {}\nB: {}", pk_a.fmt_short(), pk_b.fmt_short());

    let topic = TopicId::from_bytes(rng.random());
    println!("Topic: {topic}");

    // A subscribes (no bootstrap — acts as the bootstrap peer)
    let sub_a = gossip_a.subscribe(topic, vec![]).await?;
    let (sender_a, _recv_a) = sub_a.split();
    // Don't forward events from A — we don't need them

    // B subscribes with A as bootstrap (like JoinFromTicket)
    sleep(Duration::from_millis(200)).await;
    let sub_b = gossip_b.subscribe(topic, vec![pk_a]).await?;
    let (sender_b, mut recv_b) = sub_b.split();

    // Wait for B to connect to A
    for i in 0..60 {
        sleep(Duration::from_millis(200)).await;
        // Drain events for B
        while let Ok(Some(Ok(_))) =
            tokio::time::timeout(Duration::from_millis(10), recv_b.next()).await
        {}
        if recv_b.is_joined() {
            println!("  B connected at tick {i}");
            break;
        }
        if i % 10 == 9 {
            println!("  tick {i}: B joined={}", recv_b.is_joined());
        }
    }
    assert!(recv_b.is_joined(), "B should connect to A");

    // A sends a message that B should receive
    let msg = b"hello from A";
    sender_a.broadcast(msg[..].into()).await?;

    sleep(Duration::from_millis(1000)).await;
    let mut got_msg = false;
    while let Ok(Some(Ok(ev))) =
        tokio::time::timeout(Duration::from_millis(100), recv_b.next()).await
    {
        if matches!(ev, boru_chat::api::Event::Received(_)) {
            got_msg = true;
            break;
        }
    }
    assert!(got_msg, "B should receive message from A while connected");

    // ── Phase 2: A goes offline completely ────────────────────────────
    println!("\n--- A going offline ---");
    drop(sender_a);
    drop(router_a);
    drop(gossip_a);
    drop(ep_a);
    // Give the relay time to notice A is gone
    sleep(Duration::from_secs(2)).await;

    // ── Phase 3: C tries to join with A's stale address ──────────────
    println!("\n--- C joining (A is offline, but B is still online) ---");
    // Create a shared MemoryLookup seeded with B's address so C can
    // resolve B through the address lookup even though the only
    // bootstrap hint it has (A) is stale.  In a real app, the room
    // ticket would contain EndpointAddr entries for all known peers,
    // which get seeded into the address lookup before subscribing.
    let shared_lookup = MemoryLookup::new();
    shared_lookup.set_endpoint_info(ep_b.addr());
    let (router_c, _ep_c, _sk_c, gossip_c, _pk_c) =
        spawn_peer_with_lookup(&mut rng, shared_lookup, relay_map).await?;

    // C subscribes using A and B as bootstrap hints.  A is stale but B
    // is alive, which tests that the bootstrap mechanism tolerates
    // stale entries and connects through any live peer.  In a real app
    // the room ticket carries all known peer IDs, so a newcomer has
    // multiple bootstrap candidates.
    let sub_c = gossip_c.subscribe(topic, vec![pk_a, pk_b]).await?;
    let (_sender_c, mut recv_c) = sub_c.split();

    // C should eventually join — B is still on the topic and the relay
    // can help C find B.
    let mut c_joined = false;
    for i in 0..120 {
        sleep(Duration::from_millis(200)).await;
        while let Ok(Some(Ok(_))) =
            tokio::time::timeout(Duration::from_millis(10), recv_c.next()).await
        {}
        if recv_c.is_joined() {
            println!("  C connected at tick {i}");
            c_joined = true;
            break;
        }
        if i % 20 == 19 {
            println!("  tick {i}: C joined={}", recv_c.is_joined());
        }
    }
    assert!(
        c_joined,
        "C should eventually connect even with stale bootstrap"
    );

    // ── Phase 4: C communicates with B ────────────────────────────────
    // Send a message from B to C
    let msg_b = b"hello from B, A is gone!";
    sender_b.broadcast(msg_b[..].into()).await?;

    sleep(Duration::from_millis(1500)).await;
    let mut c_got_b = false;
    while let Ok(Some(Ok(ev))) =
        tokio::time::timeout(Duration::from_millis(100), recv_c.next()).await
    {
        if matches!(ev, boru_chat::api::Event::Received(_)) {
            c_got_b = true;
            break;
        }
    }
    assert!(c_got_b, "C should receive message from B after connecting");

    // Cleanup
    drop(sender_b);
    drop(recv_b);
    drop(router_b);
    drop(router_c);

    println!("\n✓ TEST PASSED — stale bootstrap peer does not block join");
    Ok(())
}

// ── Test 5: RoomStore peers are loaded and used by collect_bootstrap_peers ──

#[test]
fn test_room_store_peers_flow_into_collect_bootstrap_peers() {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(5);
    let sk_a = SecretKey::from_bytes(&rng.random());
    let sk_b = SecretKey::from_bytes(&rng.random());
    let pk_a = sk_a.public();
    let pk_b = sk_b.public();

    // Simulate ticket with peer A only
    let ticket_peers = [EndpointAddr::new(pk_a)];
    // Simulate RoomStore with peer B
    let room_peers = [EndpointAddr::new(pk_b)];

    let (peer_ids, all_addrs) = collect_bootstrap_peers([&ticket_peers[..], &room_peers[..]]);

    // Should have both peers from both sources
    assert_eq!(peer_ids.len(), 2, "should merge ticket + room store peers");
    assert_eq!(all_addrs.len(), 2, "should have both addresses");
    assert!(peer_ids.contains(&pk_a), "ticket peer should be present");
    assert!(
        peer_ids.contains(&pk_b),
        "room store peer should be present"
    );

    // If both sources have the same peer — dedup
    let duplicate = [EndpointAddr::new(pk_a), EndpointAddr::new(pk_a)];
    let (ids, addrs) = collect_bootstrap_peers([&duplicate[..]]);
    assert_eq!(ids.len(), 1, "duplicate peer IDs should be deduped");
    assert_eq!(addrs.len(), 1, "duplicate addresses should be deduped");
}
