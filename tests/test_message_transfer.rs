//! Minimal reproduction: two peers using the exact same pattern as iced_chat.
//! Subscribes via `gossip.subscribe()`, spawns `forward_gossip_events`,
//! sends broadcast via `sender.broadcast()`, receives via NetEvent channel
//! processed through `handle_net_event`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, PublicKey,
    RelayMode, SecretKey,
};
use iroh_gossip::chat_callbacks::ChatCallbacks;
use iroh_gossip::chat_core::{
    forward_gossip_events, handle_net_event, ChatEntry, Message, MessageHash, NetEvent,
    SignedMessage,
};
use iroh_gossip::friends::FriendId;
use iroh_gossip::net::{Gossip, GOSSIP_ALPN};
use iroh_gossip::proto::TopicId;
use n0_error::Result;
use n0_future::{task, time::sleep};
use rand::{RngExt, SeedableRng};
use tokio::sync::Mutex;

struct TestChat {
    local_public: PublicKey,
    entries: Vec<ChatEntry>,
    names: HashMap<PublicKey, String>,
    neighbors: std::collections::HashSet<PublicKey>,
    received_messages: Vec<String>,
}

impl ChatCallbacks for TestChat {
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
    sim: &mut TestChat,
) -> usize {
    let mut count = 0;
    while let Ok(event) = rx.try_lock().unwrap().try_recv() {
        count += 1;
        let _ = handle_net_event(event, sim);
    }
    count
}

#[tokio::test]
async fn test_two_peers_transfer_messages_iced_style() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(42);

    let (router_a, _ep_a, sk_a, gossip_a, pk_a) = spawn_peer(&mut rng).await?;
    let (router_b, _ep_b, sk_b, gossip_b, pk_b) = spawn_peer(&mut rng).await?;

    println!("A: {}", pk_a.fmt_short());
    println!("B: {}", pk_b.fmt_short());

    let topic = TopicId::from_bytes(rng.random());
    println!("Topic: {topic}");

    // ── Peer A: subscribe (like iced_chat CreateNewRoom/OpenRoom) ──
    println!("\n--- A: subscribing (empty bootstrap, no join wait) ---");
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
        },
    )
    .unwrap();
    sender_a.broadcast(about_me).await?;

    // ── Peer B: subscribe with A as bootstrap (like iced_chat JoinFromTicket) ──
    sleep(Duration::from_millis(500)).await;
    println!("\n--- B: subscribing (with A as bootstrap) ---");
    let sub_b = gossip_b.subscribe(topic, vec![pk_a]).await?;
    let (sender_b, receiver_b) = sub_b.split();
    let (net_tx_b, net_rx_b) = tokio::sync::mpsc::unbounded_channel();
    let net_rx_b = Arc::new(Mutex::new(net_rx_b));
    task::spawn(forward_gossip_events(receiver_b, net_tx_b));

    let about_me_b =
        SignedMessage::sign_and_encode(&sk_b, &Message::AboutMe { name: "Bob".into() }).unwrap();
    sender_b.broadcast(about_me_b).await?;

    // ── Wait for both to connect ──
    let mut sim_a = TestChat {
        local_public: pk_a,
        entries: vec![],
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        received_messages: vec![],
    };
    let mut sim_b = TestChat {
        local_public: pk_b,
        entries: vec![],
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        received_messages: vec![],
    };

    for i in 0..60 {
        sleep(Duration::from_millis(200)).await;
        drain_net(&net_rx_a, &mut sim_a);
        drain_net(&net_rx_b, &mut sim_b);
        if !sim_a.neighbors.is_empty() && !sim_b.neighbors.is_empty() {
            println!("  Both connected at tick {i}");
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

    assert!(
        !sim_a.neighbors.is_empty(),
        "A has no neighbors after waiting"
    );
    assert!(
        !sim_b.neighbors.is_empty(),
        "B has no neighbors after waiting"
    );
    println!("✓ Both peers connected");

    // Drain stale events
    drain_net(&net_rx_a, &mut sim_a);
    drain_net(&net_rx_b, &mut sim_b);

    // Clear accumulated messages from AboutMe processing
    sim_a.received_messages.clear();
    sim_b.received_messages.clear();
    sim_a.entries.clear();
    sim_b.entries.clear();

    // ── A sends a text message (exact iced_chat SendPressed path) ──
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
    assert!(b_got_alice, "B must receive Alice's text message!");

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
    assert!(a_got_bob, "A must receive Bob's message!");

    // Cleanup
    drop(sender_a);
    drop(sender_b);
    drop(router_a);
    drop(router_b);

    println!("\n✓ TWO-WAY MESSAGE TRANSFER VERIFIED");
    Ok(())
}
