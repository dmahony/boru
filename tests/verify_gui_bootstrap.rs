//! Ad-hoc verification: GUI bootstrap peer plumbing.
//!
//! Tests that bootstrap peers from a Ticket are preserved and passed into
//! gossip::subscribe() — exactly the code path the GUI join-from-ticket
//! flow uses after the fix.
//!
//! Scenario:
//!   1. Peer A creates a ticket via Ticket { topic, peers: [...] }
//!      (same as IcedChat::room_ticket())
//!   2. Peer B parses that ticket and subscribes with the bootstrap
//!      peers extracted from it — the corrected GUI flow.
//!   3. Both exchange messages over a local relay.
//!   4. Fail if peers are lost or messages don't arrive.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use boru_chat::{
    chat_callbacks::ChatCallbacks,
    chat_core::{forward_gossip_events, ChatEntry, Message, MessageHash, NetEvent, SignedMessage},
    friends::{FriendId, FriendsStore},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, tls::CaTlsConfig,
    Endpoint, PublicKey, RelayMode, SecretKey,
};
use n0_error::Result;
use n0_future::{task, time::sleep};
use rand::{RngExt, SeedableRng};
use tokio::sync::{mpsc, Mutex};

// ── SimChat as used by test_iced_chat_flow.rs ──────────────

#[expect(dead_code)]
struct SimChat {
    local_public: PublicKey,
    entries: Vec<ChatEntry>,
    names: HashMap<PublicKey, String>,
    friends: FriendsStore,
    pending_file: Option<(String, String)>,
    pending_image: Option<(String, MessageHash, PublicKey)>,
    neighbors: std::collections::HashSet<PublicKey>,
    received_messages: Vec<String>,
    sender: Option<boru_chat::api::GossipSender>,
}

impl ChatCallbacks for SimChat {
    fn local_public(&self) -> PublicKey {
        self.local_public
    }
    fn set_name(&mut self, peer: PublicKey, name: String) -> Option<String> {
        self.names.insert(peer, name)
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
        _peer: PublicKey,
        label: String,
        text: String,
        _hash: Option<MessageHash>,
        _sent_at: Option<u64>,
    ) {
        self.received_messages.push(format!("[{label}] {text}"));
        self.entries.push(ChatEntry::remote(label, text));
    }
    fn set_pending_file(&mut self, _name: String, _ticket: String) {}
    fn set_pending_image(&mut self, _name: String, _hash: MessageHash, _from: PublicKey) {}
    fn has_message(&self, hash: &MessageHash) -> bool {
        self.entries
            .iter()
            .any(|e| e.message_hash.as_ref() == Some(hash))
    }
    fn edit_message(&mut self, _hash: &MessageHash, _new_text: String) {}
    fn delete_message(&mut self, _hash: &MessageHash) {}
    fn add_reaction(&mut self, _hash: &MessageHash, _emoji: String) {}
    fn on_neighbor_up(&mut self, peer: PublicKey) {
        self.neighbors.insert(peer);
    }
    fn on_neighbor_down(&mut self, peer: PublicKey) {
        self.neighbors.remove(&peer);
    }
    fn record_activity(&mut self, _peer: PublicKey) {}
    fn request_quit(&mut self) {}
}

// ── Helpers ────────────────────────────────────────────────

async fn spawn_peer(
    rng: &mut impl rand::Rng,
    relay_map: iroh::RelayMap,
    memory: MemoryLookup,
) -> Result<(Router, Endpoint, PublicKey, Gossip)> {
    let ep = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::from_bytes(&rng.random()))
        .address_lookup(MemoryLookup::new())
        .relay_mode(RelayMode::Custom(relay_map))
        .alpns(vec![GOSSIP_ALPN.to_vec()])
        // The in-process relay uses a test certificate. This matches the
        // other local relay integration tests and keeps the test isolated
        // from the host trust store.
        .ca_tls_config(CaTlsConfig::insecure_skip_verify())
        .bind()
        .await?;
    // Register the shared lookup through the endpoint's lookup manager. This
    // is the same composition used by the room and mesh E2E tests and keeps
    // the bootstrap address visible to the endpoint after bind().
    ep.address_lookup()?.add(memory);
    ep.online().await;
    let pk = ep.secret_key().public();
    let gossip = Gossip::builder().spawn(ep.clone());
    let router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();
    Ok((router, ep.clone(), pk, gossip))
}

fn drain_net(rx: &Arc<Mutex<mpsc::UnboundedReceiver<NetEvent>>>, sim: &mut SimChat) {
    loop {
        let item = rx.try_lock().unwrap().try_recv();
        match item {
            Ok(event) => {
                let _ = boru_chat::chat_core::handle_net_event(event, sim);
            }
            Err(_) => break,
        }
    }
}

#[tokio::test]
async fn test_gui_bootstrap_plumbing() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(42);
    let (relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server().await.unwrap();
    let memory = MemoryLookup::new();

    // ── Peer A (ticket creator) ──
    let (router_a, ep_a, pk_a, gossip_a) =
        spawn_peer(&mut rng, relay_map.clone(), memory.clone()).await?;
    memory.add_endpoint_info(ep_a.addr().with_relay_url(relay_url.clone()));
    let sk_a = ep_a.secret_key().clone();

    let topic = TopicId::from_bytes(rng.random());
    println!("> Topic: {topic}");
    println!("> Peer A (creator): {}", pk_a.fmt_short());

    // A creates a ticket — exactly as IcedChat::room_ticket() does
    let ticket = boru_chat::chat_core::Ticket {
        topic,
        peers: vec![ep_a.addr().with_relay_url(relay_url.clone())],
        discovery_secret: None,
    };
    let ticket_str = ticket.to_string();
    println!("> Ticket: {ticket_str}");

    // ── Simulate GUI join-from-ticket: parse ticket, extract topic AND peers ──
    // This is THE FIX: the GUI now preserves ticket.peers instead of discarding them.
    let parsed_ticket = boru_chat::chat_core::Ticket::from_str(&ticket_str).unwrap();
    let extracted_peers = parsed_ticket.peers;
    let extracted_peer_ids: Vec<_> = extracted_peers.iter().map(|a| a.id).collect();

    // Verify peers were extracted (this is what the fix ensures)
    assert!(
        !extracted_peer_ids.is_empty(),
        "CRITICAL: bootstrap peers were lost from ticket! This is the old bug."
    );
    println!(
        "> Extracted {} bootstrap peer(s) from ticket",
        extracted_peer_ids.len()
    );

    // ── Peer B (joiner) — subscribes with bootstrap peers from ticket ──
    let (router_b, ep_b, pk_b, gossip_b) =
        spawn_peer(&mut rng, relay_map.clone(), memory.clone()).await?;
    memory.add_endpoint_info(ep_b.addr().with_relay_url(relay_url));
    let sk_b = ep_b.secret_key().clone();

    println!("> Peer B (joiner): {}", pk_b.fmt_short());

    // Use plain subscriptions so the forwarding tasks below observe the
    // NeighborUp events. subscribe_and_join consumes those events internally,
    // which would make the GUI-state neighbor assertion below falsely fail.
    // B uses the bootstrap peers from the parsed ticket — THE FIX IN ACTION.
    let (topic_a, topic_b) = tokio::try_join!(
        gossip_a.subscribe(topic, vec![]),
        gossip_b.subscribe(topic, extracted_peer_ids),
    )?;
    let (sender_a, rx_a) = topic_a.split();
    let (sender_b, rx_b) = topic_b.split();

    // ── Bridge gossip events to NetEvent channels ──
    let (net_tx_a, net_rx_a) = mpsc::unbounded_channel();
    let net_rx_a = Arc::new(Mutex::new(net_rx_a));
    task::spawn(forward_gossip_events(rx_a, net_tx_a));

    let (net_tx_b, net_rx_b) = mpsc::unbounded_channel();
    let net_rx_b = Arc::new(Mutex::new(net_rx_b));
    task::spawn(forward_gossip_events(rx_b, net_tx_b));

    // ── Create SimChat instances (simulating GUI state) ──
    let tmp_dir = tempfile::tempdir().unwrap();
    let mut sim_a = SimChat {
        local_public: pk_a,
        entries: vec![],
        names: HashMap::new(),
        friends: FriendsStore::empty_at(tmp_dir.path().join("a")),
        pending_file: None,
        pending_image: None,
        neighbors: std::collections::HashSet::new(),
        received_messages: vec![],
        sender: Some(sender_a.clone()),
    };
    let mut sim_b = SimChat {
        local_public: pk_b,
        entries: vec![],
        names: HashMap::new(),
        friends: FriendsStore::empty_at(tmp_dir.path().join("b")),
        pending_file: None,
        pending_image: None,
        neighbors: std::collections::HashSet::new(),
        received_messages: vec![],
        sender: Some(sender_b.clone()),
    };

    // ── Wait for gossip neighbors to connect ──
    println!("\n> Waiting for neighbors to connect...");
    let max_ticks = 80u32;
    for i in 0..max_ticks {
        sleep(Duration::from_millis(250)).await;
        drain_net(&net_rx_a, &mut sim_a);
        drain_net(&net_rx_b, &mut sim_b);
        if !sim_a.neighbors.is_empty() && !sim_b.neighbors.is_empty() {
            println!("  ✓ Both connected at tick {i}");
            break;
        }
        if i % 10 == 9 {
            println!(
                "  tick {}: A neighbors={}, B neighbors={}",
                i,
                sim_a.neighbors.len(),
                sim_b.neighbors.len()
            );
        }
    }

    let a_has_neighbors = !sim_a.neighbors.is_empty();
    let b_has_neighbors = !sim_b.neighbors.is_empty();
    assert!(
        a_has_neighbors,
        "A should have neighbors (via bootstrap from ticket)"
    );
    assert!(
        b_has_neighbors,
        "B should have neighbors (via bootstrap from ticket)"
    );
    println!("  ✓ Both peers have gossip neighbors");

    // ── Message exchange: A → B ──
    println!("\n> A broadcasts message to B...");
    drain_net(&net_rx_a, &mut sim_a);
    drain_net(&net_rx_b, &mut sim_b);

    let msg_a = SignedMessage::sign_and_encode(
        &sk_a,
        &Message::Message {
            text: "hello from Alice via bootstrap ticket!".into(),
        },
    )
    .unwrap();
    sender_a.broadcast(msg_a).await?;

    for _ in 0..30 {
        sleep(Duration::from_millis(200)).await;
        drain_net(&net_rx_b, &mut sim_b);
        if sim_b
            .received_messages
            .iter()
            .any(|m| m.contains("hello from Alice"))
        {
            break;
        }
    }

    let b_got_message = sim_b
        .received_messages
        .iter()
        .any(|m| m.contains("hello from Alice"));
    assert!(
        b_got_message,
        "B must receive message from A via bootstrap-peered connection"
    );
    println!("  ✓ B received message from A");

    // ── Message exchange: B → A ──
    let msg_b = SignedMessage::sign_and_encode(
        &sk_b,
        &Message::Message {
            text: "hey Alice, Bob here via bootstrap!".into(),
        },
    )
    .unwrap();
    sender_b.broadcast(msg_b).await?;

    for _ in 0..30 {
        sleep(Duration::from_millis(200)).await;
        drain_net(&net_rx_a, &mut sim_a);
        if sim_a
            .received_messages
            .iter()
            .any(|m| m.contains("hey Alice"))
        {
            break;
        }
    }

    let a_got_message = sim_a
        .received_messages
        .iter()
        .any(|m| m.contains("hey Alice"));
    assert!(
        a_got_message,
        "A must receive message from B via bootstrap-peered connection"
    );
    println!("  ✓ A received message from B");

    // ── Cleanup ──
    drop(sender_a);
    drop(sender_b);
    drop(router_a);
    drop(ep_a);
    drop(router_b);
    drop(ep_b);

    println!("\n✓✓✓ GUI BOOTSTRAP PLUMBING VERIFIED: ticket peers reach gossip.subscribe() ✓✓✓");
    Ok(())
}
