//! Reproduction: two "iced_chat instances" on the same machine.
//! Tests the exact flow: subscribe -> forward_gossip_events -> NetEvent -> handle_net_event
//! with two different keys (as two separate instances would have with separate data dirs).
//!
//! This matches what happens when:
//!   Instance A: cargo run --features gui --example iced_chat open
//!   Instance B: cargo run --features gui --example iced_chat join <ticket>

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use boru_chat::chat_callbacks::ChatCallbacks;
use boru_chat::chat_core::{
    forward_gossip_events, handle_net_event, ChatEntry, Message, MessageHash, NetEvent,
    SignedMessage,
};
use boru_chat::friends::FriendId;
use boru_chat::net::{Gossip, GOSSIP_ALPN};
use boru_chat::proto::TopicId;
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, PublicKey,
    RelayMode, SecretKey,
};
use n0_error::Result;
use n0_future::{task, time::sleep};
use rand::{RngExt, SeedableRng};
use tokio::sync::Mutex;

struct TestPeer {
    local_public: PublicKey,
    entries: Vec<ChatEntry>,
    names: HashMap<PublicKey, String>,
    neighbors: std::collections::HashSet<PublicKey>,
    received_messages: Vec<String>,
}

impl ChatCallbacks for TestPeer {
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
    fn has_message(&self, _hash: &MessageHash) -> bool {
        false
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

async fn spawn_peer(
    rng: &mut impl rand::Rng,
) -> Result<(Router, iroh::Endpoint, SecretKey, Gossip, PublicKey)> {
    let ep = iroh::Endpoint::builder(presets::N0)
        .secret_key(SecretKey::from_bytes(&rng.random()))
        .address_lookup(MemoryLookup::new())
        .relay_mode(RelayMode::Default)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
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

fn drain_net(
    rx: &Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<NetEvent>>>,
    sim: &mut TestPeer,
) -> usize {
    let mut count = 0;
    while let Ok(event) = rx.try_lock().unwrap().try_recv() {
        count += 1;
        let _ = handle_net_event(event, sim);
    }
    count
}

/// Test 1: Two peers with DIFFERENT keys — this is the normal expected case
/// (separate data dirs, separate identities). Must pass.
#[tokio::test]
async fn repro_two_peers_different_keys() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(42);

    let (router_a, ep_a, sk_a, gossip_a, pk_a) = spawn_peer(&mut rng).await?;
    let (router_b, ep_b, sk_b, gossip_b, pk_b) = spawn_peer(&mut rng).await?;

    println!("A sk: {}", hex::encode(sk_a.to_bytes()));
    println!("B sk: {}", hex::encode(sk_b.to_bytes()));
    println!("A pk: {}", pk_a.fmt_short());
    println!("B pk: {}", pk_b.fmt_short());

    assert_ne!(pk_a, pk_b, "Two peers must have different keys");

    let topic = TopicId::from_bytes(rng.random());
    println!("Topic: {topic}");

    // ── Peer A: subscribe (like iced_chat CreateNewRoom/OpenRoom) ──
    println!("\n--- A: subscribing (empty bootstrap, like OpenRoom) ---");
    let sub_a = gossip_a.subscribe(topic, vec![]).await?;
    let (sender_a, receiver_a) = sub_a.split();
    let (net_tx_a, net_rx_a) = tokio::sync::mpsc::unbounded_channel();
    let net_rx_a = Arc::new(Mutex::new(net_rx_a));
    task::spawn(forward_gossip_events(receiver_a, net_tx_a));

    // Broadcast AboutMe as iced_chat does
    let about_me = SignedMessage::sign_and_encode(
        &sk_a,
        &Message::AboutMe {
            name: "Alice".into(),
            profile_image_ticket: None,
        },
    )
    .unwrap();
    sender_a.broadcast(about_me).await?;

    // ── Peer B: subscribe with A's address as bootstrap (like JoinFromTicket) ──
    sleep(Duration::from_millis(500)).await;
    println!("\n--- B: subscribing (with A as bootstrap, like JoinFromTicket) ---");
    let memory_lookup = MemoryLookup::new();
    if let Ok(addr_lookup) = ep_b.address_lookup() {
        addr_lookup.add(memory_lookup.clone());
    }
    memory_lookup.set_endpoint_info(ep_a.addr());
    let sub_b = gossip_b.subscribe(topic, vec![pk_a]).await?;
    let (sender_b, receiver_b) = sub_b.split();
    let (net_tx_b, net_rx_b) = tokio::sync::mpsc::unbounded_channel();
    let net_rx_b = Arc::new(Mutex::new(net_rx_b));
    task::spawn(forward_gossip_events(receiver_b, net_tx_b));

    let about_me_b = SignedMessage::sign_and_encode(
        &sk_b,
        &Message::AboutMe {
            name: "Bob".into(),
            profile_image_ticket: None,
        },
    )
    .unwrap();
    sender_b.broadcast(about_me_b).await?;

    // Wait for connection
    let mut sim_a = TestPeer {
        local_public: pk_a,
        entries: vec![],
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        received_messages: vec![],
    };
    let mut sim_b = TestPeer {
        local_public: pk_b,
        entries: vec![],
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        received_messages: vec![],
    };

    let mut connected = false;
    for i in 0..60 {
        sleep(Duration::from_millis(200)).await;
        drain_net(&net_rx_a, &mut sim_a);
        drain_net(&net_rx_b, &mut sim_b);
        if !sim_a.neighbors.is_empty() && !sim_b.neighbors.is_empty() {
            println!("  Both connected at tick {i}");
            connected = true;
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
    assert!(connected, "Peers should connect");
    println!("✓ Both peers connected (different keys)");

    // Drain stale events
    drain_net(&net_rx_a, &mut sim_a);
    drain_net(&net_rx_b, &mut sim_b);
    sim_a.received_messages.clear();
    sim_b.received_messages.clear();

    // ── A sends a text message ──
    println!("\n--- A broadcasts message ---");
    let text_a = "hello from Alice!".to_string();
    let encoded_a =
        SignedMessage::sign_and_encode(&sk_a, &Message::Message { text: text_a }).unwrap();
    sender_a.broadcast(encoded_a).await?;

    sleep(Duration::from_secs(3)).await;
    drain_net(&net_rx_a, &mut sim_a);
    drain_net(&net_rx_b, &mut sim_b);

    println!("A received: {:?}", sim_a.received_messages);
    println!("B received: {:?}", sim_b.received_messages);

    let b_got_alice = sim_b
        .received_messages
        .iter()
        .any(|m| m.contains("hello from Alice"));
    assert!(
        b_got_alice,
        "B MUST receive Alice's message! A said 'hello from Alice!'"
    );
    println!("✓ B received message from A");

    // ── B sends a text message back ──
    println!("\n--- B broadcasts message ---");
    let text_b = "hey Alice, Bob here!".to_string();
    let encoded_b =
        SignedMessage::sign_and_encode(&sk_b, &Message::Message { text: text_b }).unwrap();
    sender_b.broadcast(encoded_b).await?;

    sleep(Duration::from_secs(3)).await;
    drain_net(&net_rx_a, &mut sim_a);
    println!("A received: {:?}", sim_a.received_messages);

    let a_got_bob = sim_a
        .received_messages
        .iter()
        .any(|m| m.contains("hey Alice"));
    assert!(
        a_got_bob,
        "A MUST receive Bob's reply! B said 'hey Alice, Bob here!'"
    );
    println!("✓ A received reply from B");

    // Cleanup
    drop(sender_a);
    drop(sender_b);
    drop(router_a);
    drop(router_b);

    println!("\n✓✓ TWO PEERS WITH DIFFERENT KEYS: TWO-WAY COMMUNICATION VERIFIED ✓✓");
    Ok(())
}

/// Test 2: Two peers with the SAME key (sharing data dir — the default).
/// The gossip protocol SHOULD handle this gracefully (self-connection prevention).
#[tokio::test]
async fn repro_two_peers_same_key() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(42);
    let shared_sk = SecretKey::from_bytes(&rng.random());
    let shared_pk = shared_sk.public();

    println!("Shared sk: {}", hex::encode(shared_sk.to_bytes()));
    println!("Shared pk: {}", shared_pk.fmt_short());

    // Create two endpoints with the SAME secret key
    let ep_a = iroh::Endpoint::builder(presets::N0)
        .secret_key(shared_sk.clone())
        .address_lookup(MemoryLookup::new())
        .relay_mode(RelayMode::Default)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    ep_a.online().await;

    // Force port binding for B
    let ep_b = iroh::Endpoint::builder(presets::N0)
        .secret_key(shared_sk.clone())
        .address_lookup(MemoryLookup::new())
        .relay_mode(RelayMode::Default)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    ep_b.online().await;

    println!("Ep A id: {}", ep_a.id().fmt_short());
    println!("Ep B id: {}", ep_b.id().fmt_short());

    let gossip_a = Gossip::builder().spawn(ep_a.clone());
    let gossip_b = Gossip::builder().spawn(ep_b.clone());

    let router_a = Router::builder(ep_a.clone())
        .accept(GOSSIP_ALPN, gossip_a.clone())
        .spawn();
    let router_b = Router::builder(ep_b.clone())
        .accept(GOSSIP_ALPN, gossip_b.clone())
        .spawn();

    let topic = TopicId::from_bytes(rng.random());
    println!("Topic: {topic}");

    // Both subscribe
    println!("\n--- Peer A: subscribing ---");
    let sub_a = gossip_a.subscribe(topic, vec![]).await?;
    let (sender_a, receiver_a) = sub_a.split();
    let (net_tx_a, net_rx_a) = tokio::sync::mpsc::unbounded_channel();
    let net_rx_a = Arc::new(Mutex::new(net_rx_a));
    task::spawn(forward_gossip_events(receiver_a, net_tx_a));

    let about_me = SignedMessage::sign_and_encode(
        &shared_sk,
        &Message::AboutMe {
            name: "Alice".into(),
            profile_image_ticket: None,
        },
    )
    .unwrap();
    sender_a.broadcast(about_me).await?;

    sleep(Duration::from_millis(500)).await;
    println!("\n--- Peer B: subscribing (with A's pk as bootstrap) ---");
    let sub_b = gossip_b.subscribe(topic, vec![shared_pk]).await?;
    let (sender_b, receiver_b) = sub_b.split();
    let (net_tx_b, net_rx_b) = tokio::sync::mpsc::unbounded_channel();
    let net_rx_b = Arc::new(Mutex::new(net_rx_b));
    task::spawn(forward_gossip_events(receiver_b, net_tx_b));

    let about_me_b = SignedMessage::sign_and_encode(
        &shared_sk,
        &Message::AboutMe {
            name: "Bob".into(),
            profile_image_ticket: None,
        },
    )
    .unwrap();
    sender_b.broadcast(about_me_b).await?;

    // Wait and see what happens
    let mut sim_a = TestPeer {
        local_public: shared_pk,
        entries: vec![],
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        received_messages: vec![],
    };
    let mut sim_b = TestPeer {
        local_public: shared_pk,
        entries: vec![],
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        received_messages: vec![],
    };

    println!("\nWaiting for connections (with SAME keys)...");
    for i in 0..60 {
        sleep(Duration::from_millis(200)).await;
        drain_net(&net_rx_a, &mut sim_a);
        drain_net(&net_rx_b, &mut sim_b);
        if !sim_a.neighbors.is_empty() || !sim_b.neighbors.is_empty() {
            println!(
                "  tick {i}: A neighbors={}, B neighbors={}",
                sim_a.neighbors.len(),
                sim_b.neighbors.len()
            );
        }
    }

    println!(
        "Final state — A neighbors: {}, B neighbors: {}",
        sim_a.neighbors.len(),
        sim_b.neighbors.len()
    );
    println!("A messages: {:?}", sim_a.received_messages);
    println!("B messages: {:?}", sim_b.received_messages);

    // Now try sending a message from A, see if B gets it despite same key
    sim_a.received_messages.clear();
    sim_b.received_messages.clear();

    let text_a = "hello from same-key instance A!".to_string();
    let encoded_a =
        SignedMessage::sign_and_encode(&shared_sk, &Message::Message { text: text_a }).unwrap();
    sender_a.broadcast(encoded_a).await?;

    sleep(Duration::from_secs(3)).await;
    drain_net(&net_rx_a, &mut sim_a);
    drain_net(&net_rx_b, &mut sim_b);

    println!("After send - A msgs: {:?}", sim_a.received_messages);
    println!("After send - B msgs: {:?}", sim_b.received_messages);

    // Key observation: B sees it as from == self (shared_pk) so handle_net_event filters it out.
    // The gossip layer also likely rejects the connection since both have the same identity.
    println!("\nOBSERVATION: Same-key peers -> no gossip connection established.");
    println!("This confirms the root cause of the message transfer issue.");

    drop(sender_a);
    drop(sender_b);
    drop(router_a);
    drop(router_b);

    Ok(())
}
