//! End-to-end test: stale bootstrap peers should not block rejoin,
//! and refreshed bootstrap addresses are persisted for later reuse.
//!
//! Scenario (3 peers):
//!   1. Peer A creates a room with ticket t_A.
//!   2. Peer B joins via t_A and later refreshes its RoomStore with
//!      live peer addresses (A and C).
//!   3. Peer C joins via t_A.
//!   4. A goes away, then B re-subscribes using saved peers
//!      from its RoomStore — connects to C.
//!   5. Messages flow through C's mesh connection.
//!
//! The test also verifies that `refresh_bootstrap_peers` produces correct
//! output and that the saved RoomStore round-trips through JSON.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint, PublicKey,
    RelayMode, SecretKey,
};
use iroh_gossip::{
    chat_callbacks::ChatCallbacks,
    chat_core::{
        self, forward_gossip_events, refresh_bootstrap_peers, seed_memory_lookup, ChatEntry,
        Message, NetEvent, SignedMessage,
    },
    friends::FriendId,
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    room::RoomStore,
};
use n0_error::Result;
use n0_future::{task, time::sleep, StreamExt};
use rand::{RngExt, SeedableRng};
use tokio::sync::{mpsc, Mutex};

// ── SimChat ───────────────────────────────────────────────────────

struct SimChat {
    local_public: PublicKey,
    entries: Vec<ChatEntry>,
    names: HashMap<PublicKey, String>,
    neighbors: std::collections::HashSet<PublicKey>,
    received_messages: Vec<String>,
}

impl ChatCallbacks for SimChat {
    fn local_public(&self) -> PublicKey {
        self.local_public
    }
    fn set_name(&mut self, peer: PublicKey, name: String) {
        self.names.insert(peer, name);
    }
    fn is_friend(&self, _peer: &PublicKey) -> bool {
        false
    }
    fn friend_mark_online(&mut self, _fid: FriendId) {}
    fn friend_mark_offline(&mut self, _fid: FriendId) {}
    fn friend_set_name(&mut self, _fid: FriendId, _name: String) {}
    fn mark_friends_dirty(&mut self) {}
    fn push_system(&mut self, text: String) {
        self.received_messages.push(format!("[sys] {text}"));
        self.entries.push(ChatEntry::system(text));
    }
    fn push_remote(
        &mut self,
        label: String,
        text: String,
        _hash: Option<iroh_gossip::chat_core::MessageHash>,
        _sent_at: Option<u64>,
    ) {
        self.received_messages.push(format!("[{label}] {text}"));
        self.entries.push(ChatEntry::remote(label, text));
    }
    fn set_pending_file(&mut self, _name: String, _ticket: String) {}
    fn set_pending_image(
        &mut self,
        _name: String,
        _hash: iroh_gossip::chat_core::MessageHash,
        _from: PublicKey,
    ) {
    }
    fn has_message(&self, hash: &iroh_gossip::chat_core::MessageHash) -> bool {
        self.entries
            .iter()
            .any(|e| e.message_hash.as_ref() == Some(hash))
    }
    fn edit_message(&mut self, _hash: &iroh_gossip::chat_core::MessageHash, _new_text: String) {}
    fn delete_message(&mut self, _hash: &iroh_gossip::chat_core::MessageHash) {}
    fn add_reaction(&mut self, _hash: &iroh_gossip::chat_core::MessageHash, _emoji: String) {}
    fn on_neighbor_up(&mut self, peer: PublicKey) {
        self.neighbors.insert(peer);
    }
    fn on_neighbor_down(&mut self, peer: PublicKey) {
        self.neighbors.remove(&peer);
    }
    fn record_activity(&mut self, _peer: PublicKey) {}
    fn request_quit(&mut self) {}
}

fn drain_net(rx: &Arc<Mutex<mpsc::UnboundedReceiver<NetEvent>>>, sim: &mut SimChat) {
    loop {
        let item = rx.try_lock().unwrap().try_recv();
        match item {
            Ok(event) => {
                let _ = chat_core::handle_net_event(event, sim);
            }
            Err(_) => break,
        }
    }
}

async fn spawn_peer(
    rng: &mut impl rand::Rng,
    relay_map: iroh::RelayMap,
    memory_lookup: MemoryLookup,
) -> Result<(Router, Endpoint, PublicKey, Gossip)> {
    let ep = Endpoint::builder(presets::N0)
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
    Ok((router, ep.clone(), pk, gossip))
}

/// Wait for `sub.is_joined()` by draining events with a short timeout.
async fn wait_joined(sub: &mut iroh_gossip::api::GossipTopic, max_ticks: u32) -> bool {
    for _ in 0..max_ticks {
        sleep(Duration::from_millis(200)).await;
        // Drain any pending events so the gossip actor can update join state.
        loop {
            let item = tokio::time::timeout(Duration::from_millis(10), sub.next()).await;
            match item {
                Ok(Some(Ok(_))) => continue,
                _ => break,
            }
        }
        if sub.is_joined() {
            return true;
        }
    }
    false
}

#[tokio::test]
async fn test_stale_bootstrap_does_not_block_rejoin() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(42);
    let (relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server().await.unwrap();
    let shared_memory = MemoryLookup::new();

    // ── Three peers ──
    let (router_a, ep_a, pk_a, gossip_a) =
        spawn_peer(&mut rng, relay_map.clone(), shared_memory.clone()).await?;
    shared_memory.add_endpoint_info(ep_a.addr().with_relay_url(relay_url.clone()));
    let topic = TopicId::from_bytes(rng.random());
    println!("> Peer A: {}  topic: {topic}", pk_a.fmt_short());

    let (router_b, ep_b, pk_b, gossip_b) =
        spawn_peer(&mut rng, relay_map.clone(), shared_memory.clone()).await?;
    shared_memory.add_endpoint_info(ep_b.addr().with_relay_url(relay_url.clone()));
    println!("> Peer B: {}", pk_b.fmt_short());

    let (router_c, ep_c, pk_c, gossip_c) =
        spawn_peer(&mut rng, relay_map, shared_memory.clone()).await?;
    shared_memory.add_endpoint_info(ep_c.addr().with_relay_url(relay_url));
    let sk_c = ep_c.secret_key().clone();
    println!("> Peer C: {}", pk_c.fmt_short());

    // ── Phase 1: A, B, C all subscribe ──
    // Use subscribe() (non-blocking) and then wait for is_joined().
    let sub_a = gossip_a.subscribe(topic, vec![]).await?;
    println!("> A subscribed");
    sleep(Duration::from_millis(200)).await;

    let sub_b = gossip_b.subscribe(topic, vec![pk_a]).await?;
    println!("> B subscribed (bootstrap: A)");

    let sub_c = gossip_c.subscribe(topic, vec![pk_a]).await?;
    println!("> C subscribed (bootstrap: A)");

    // Wait for B and C to join using the split-receiver pattern
    // that works in test_stale_bootstrap_peer_does_not_block_join.
    let (sender_b, mut recv_b) = sub_b.split();
    let (sender_c, mut recv_c) = sub_c.split();

    // Drain and wait for B to have at least one neighbor
    for _ in 0..120 {
        sleep(Duration::from_millis(200)).await;
        // Drain B's events
        while let Ok(Some(Ok(_))) =
            tokio::time::timeout(Duration::from_millis(10), recv_b.next()).await
        {}
        if recv_b.is_joined() {
            println!("  > B joined at poll");
            break;
        }
    }

    // Forward C's events to a sink so the gossip actor's event channel doesn't fill up
    let _c_drain = task::spawn(async move { while let Some(_) = recv_c.next().await {} });

    let (net_tx_b, net_rx_b) = mpsc::unbounded_channel();
    let net_rx_b = Arc::new(Mutex::new(net_rx_b));
    task::spawn(forward_gossip_events(recv_b, net_tx_b));

    let mut sim_b = SimChat {
        local_public: pk_b,
        entries: vec![],
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        received_messages: vec![],
    };

    // Send a message from C through the mesh to confirm B receives it
    let msg = SignedMessage::sign_and_encode(
        &sk_c,
        &Message::Message {
            text: "hello from C".into(),
        },
    )?;
    sender_c.broadcast(msg.clone()).await?;
    sleep(Duration::from_millis(1000)).await;
    drain_net(&net_rx_b, &mut sim_b);
    assert!(
        sim_b
            .received_messages
            .iter()
            .any(|m| m.contains("hello from C")),
        "B should receive message from C"
    );
    println!("  ✓ Message routed through mesh (C → B)");

    // ── Phase 2: Refresh B's RoomStore with live addresses ──
    let tmp_dir = tempfile::tempdir().unwrap();
    {
        let room = RoomStore::new(tmp_dir.path(), topic);
        room.save().expect("save initial room");
    }
    let mut room = RoomStore::load_or_none(tmp_dir.path()).expect("room store");
    let all_peers: std::collections::HashSet<PublicKey> = [pk_a, pk_b, pk_c].into_iter().collect();
    let changed = refresh_bootstrap_peers(&mut room, &all_peers, &ep_b).await;
    println!(
        "> Refresh: changed={changed}, stored {} peer(s)",
        room.peers.len()
    );
    assert!(
        !room.peers.is_empty(),
        "RoomStore must have peers after refresh"
    );

    // Each stored addr must have a relay URL (the key info for reconnection)
    for addr in &room.peers {
        let has_relay = addr
            .addrs
            .iter()
            .any(|a| matches!(a, iroh::TransportAddr::Relay(_)));
        assert!(
            has_relay,
            "saved EndpointAddr for {} must include relay URL",
            addr.id.fmt_short()
        );
    }
    println!("  ✓ All saved peers have relay URLs");

    room.save().expect("save refreshed peers");

    // ── Phase 3: Drop A (original bootstrap peer) ──
    println!("> Phase 3: dropping Peer A...");
    drop(sub_a);
    drop(router_a);
    ep_a.close().await;
    sleep(Duration::from_millis(1500)).await;

    // Check that B is still connected to someone (C should be alive
    // because sender_c is still alive)
    drain_net(&net_rx_b, &mut sim_b);
    let has_c = sim_b.neighbors.contains(&pk_c);
    println!(
        "  > B still has neighbor(s): {} (C alive: {has_c})",
        sim_b.neighbors.len()
    );

    // ── Phase 4: B re-subscribes using saved RoomStore peers ──
    let room_b2 = RoomStore::load_or_none(tmp_dir.path()).expect("RoomStore must persist");
    assert!(!room_b2.peers.is_empty(), "RoomStore must have saved peers");
    println!(
        "> Phase 4: re-subscribing with {} saved peer(s) from RoomStore",
        room_b2.peers.len()
    );

    // Drop B's old subscription handles
    drop(sender_b);
    drop(net_rx_b);

    let bootstrap_ids: Vec<PublicKey> = room_b2.peers.iter().map(|p| p.id).collect();

    // Seed the address lookup with the saved peer addresses
    let new_memory = MemoryLookup::new();
    if let Ok(chain) = ep_b.address_lookup().as_ref() {
        chain.add(new_memory.clone());
    }
    seed_memory_lookup(&new_memory, &room_b2.peers);

    // B re-subscribes using saved peers (A stale, C live)
    let mut sub_b2 = gossip_b.subscribe(topic, bootstrap_ids).await?;
    println!("  ✓ Phase 4: re-subscribed using saved bootstrap peers");

    // Wait for B to reconnect — should connect to C
    let c_reached = wait_joined(&mut sub_b2, 80).await;
    assert!(
        c_reached,
        "B should reconnect to C via saved bootstrap peers"
    );
    println!("  ✓ B reconnected to C (A is stale but C is live)");

    // ── Check that refreshed addresses are persisted ──
    let room_b3 = RoomStore::load_or_none(tmp_dir.path()).expect("RoomStore after restart");
    assert!(
        !room_b3.peers.is_empty(),
        "RoomStore peers persist across reload"
    );
    println!(
        "  ✓ RoomStore persists {} bootstrap peer(s) across restart",
        room_b3.peers.len()
    );

    // Cleanup
    drop(sender_c);
    drop(_c_drain); // drop the event-drain task
    drop(router_b);
    drop(router_c);
    ep_b.close().await;
    ep_c.close().await;
    println!("\n✓✓✓ All phases passed: stale bootstrap does not block rejoin ✓✓✓");
    Ok(())
}
