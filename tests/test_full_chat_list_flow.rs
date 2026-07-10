//! Full simulation of iced_chat chat-list flow:
//! 1. A creates a new room via New Chat (CreateNewRoom path)
//! 2. A gets a ticket (printed on screen)
//! 3. B joins via that ticket (JoinFromTicket path)
//! 4. A sends a message — verify B receives it
//! 5. B sends a message — verify A receives it

use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint, PublicKey,
    RelayMode, SecretKey,
};
use iroh_gossip::{
    api::GossipSender,
    chat_callbacks::ChatCallbacks,
    chat_core::{
        forward_gossip_events, handle_net_event, ChatEntry, Message, MessageHash, NetEvent,
        SignedMessage,
    },
    friends::FriendId,
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
};
use n0_error::Result;
use n0_future::{task, time::sleep};
use rand::{RngExt, SeedableRng};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

struct SimChat {
    local_public: PublicKey,
    entries: Vec<ChatEntry>,
    names: HashMap<PublicKey, String>,
    neighbors: std::collections::HashSet<PublicKey>,
    received_messages: Vec<String>,
    sender: Option<GossipSender>,
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

#[tokio::test]
async fn test_full_chat_list_flow() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(42);

    // Spawn two peers
    let ep_a = Endpoint::builder(presets::N0)
        .secret_key(SecretKey::from_bytes(&rng.random()))
        .address_lookup(MemoryLookup::new())
        .relay_mode(RelayMode::Default)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    ep_a.online().await;
    let sk_a = ep_a.secret_key().clone();
    let pk_a = sk_a.public();
    let gossip_a = Gossip::builder().spawn(ep_a.clone());
    let router_a = Router::builder(ep_a.clone())
        .accept(GOSSIP_ALPN, gossip_a.clone())
        .spawn();
    let addr_a = ep_a.addr();
    println!("A: {}", pk_a.fmt_short());

    let ep_b = Endpoint::builder(presets::N0)
        .secret_key(SecretKey::from_bytes(&rng.random()))
        .address_lookup(MemoryLookup::new())
        .relay_mode(RelayMode::Default)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    ep_b.online().await;
    let sk_b = ep_b.secret_key().clone();
    let pk_b = sk_b.public();
    let gossip_b = Gossip::builder().spawn(ep_b.clone());
    let router_b = Router::builder(ep_b.clone())
        .accept(GOSSIP_ALPN, gossip_b.clone())
        .spawn();
    println!("B: {}", pk_b.fmt_short());

    // STEP 1: A creates a new room (like NewChat button in chat list)
    let topic = TopicId::from_bytes(rng.random());
    println!("\n[Step 1] A: CreateNewRoom (random topic)");

    // Create ticket exactly like iced_chat does in room_ticket()
    let ticket = iroh_gossip::chat_core::Ticket {
        topic,
        peers: vec![addr_a],
    };
    let ticket_str = ticket.to_string();
    println!("  Ticket: {ticket_str}");

    // Subscribe like CreateNewRoom does
    let sub_a = gossip_a.subscribe(topic, vec![]).await?;
    let (sender_a, receiver_a) = sub_a.split();
    let (net_tx_a, net_rx_a) = tokio::sync::mpsc::unbounded_channel();
    let net_rx_a = Arc::new(tokio::sync::Mutex::new(net_rx_a));
    task::spawn(forward_gossip_events(receiver_a, net_tx_a));

    // Broadcast AboutMe like CreateNewRoom does
    let am = SignedMessage::sign_and_encode(
        &sk_a,
        &Message::AboutMe {
            name: "Alice".into(),
        },
    )
    .unwrap();
    sender_a.broadcast(am).await?;

    println!("  A subscribed and broadcast AboutMe");

    // STEP 2: B joins via the ticket (like JoinFromTicket in chat list)
    println!("\n[Step 2] B: JoinFromTicket");
    let parsed_ticket: iroh_gossip::chat_core::Ticket = ticket_str.parse().unwrap();
    println!(
        "  Parsed topic={}, peers={}",
        parsed_ticket.topic,
        parsed_ticket
            .peers
            .iter()
            .map(|p| p.id.fmt_short().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    assert_eq!(parsed_ticket.topic, topic, "Parsed topic must match");

    // Set up memory lookup exactly like JoinFromTicket does
    let memory_lookup = MemoryLookup::new();
    if let Ok(addr_lookup) = ep_b.address_lookup() {
        addr_lookup.add(memory_lookup.clone());
    }
    for peer in &parsed_ticket.peers {
        memory_lookup.set_endpoint_info(peer.clone());
    }
    let bootstrap_peers: Vec<_> = parsed_ticket.peers.iter().map(|p| p.id).collect();

    let sub_b = gossip_b.subscribe(topic, bootstrap_peers).await?;
    let (sender_b, receiver_b) = sub_b.split();
    let (net_tx_b, net_rx_b) = tokio::sync::mpsc::unbounded_channel();
    let net_rx_b = Arc::new(tokio::sync::Mutex::new(net_rx_b));
    task::spawn(forward_gossip_events(receiver_b, net_tx_b));

    let am_b =
        SignedMessage::sign_and_encode(&sk_b, &Message::AboutMe { name: "Bob".into() }).unwrap();
    sender_b.broadcast(am_b).await?;
    println!("  B subscribed and broadcast AboutMe");

    // Create sim chat state
    let mut sim_a = SimChat {
        local_public: pk_a,
        entries: vec![],
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        received_messages: vec![],
        sender: Some(sender_a.clone()),
    };
    let mut sim_b = SimChat {
        local_public: pk_b,
        entries: vec![],
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        received_messages: vec![],
        sender: Some(sender_b.clone()),
    };

    let drain = |rx: &Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<NetEvent>>>,
                 sim: &mut SimChat| {
        loop {
            match rx.try_lock().unwrap().try_recv() {
                Ok(event) => {
                    let _ = handle_net_event(event, sim);
                }
                Err(_) => break,
            }
        }
    };

    // Wait for gossip to connect
    for i in 0..60 {
        sleep(Duration::from_millis(200)).await;
        drain(&net_rx_a, &mut sim_a);
        drain(&net_rx_b, &mut sim_b);
        if sim_a.neighbors.len() > 0 && sim_b.neighbors.len() > 0 {
            println!("  Both connected at tick {i}");
            break;
        }
        if i % 10 == 9 {
            println!(
                "  tick {}: A neighbors={} B neighbors={}",
                i,
                sim_a.neighbors.len(),
                sim_b.neighbors.len()
            );
        }
    }
    assert!(sim_a.neighbors.len() > 0, "A must connect");
    assert!(sim_b.neighbors.len() > 0, "B must connect");

    drain(&net_rx_a, &mut sim_a);
    drain(&net_rx_b, &mut sim_b);

    // STEP 3: A sends a message — just like SendPressed in iced_chat
    println!("\n[Step 3] A sends message");
    let msg_a = "Hello Bob, this is Alice!".to_string();
    let enc_a = SignedMessage::sign_and_encode(&sk_a, &Message::Message { text: msg_a }).unwrap();
    sender_a.broadcast(enc_a).await?;
    println!("  A broadcast done");

    sleep(Duration::from_secs(3)).await;
    drain(&net_rx_a, &mut sim_a);
    drain(&net_rx_b, &mut sim_b);

    println!("  B received: {:?}", sim_b.received_messages);
    assert!(
        sim_b
            .received_messages
            .iter()
            .any(|m| m.contains("Hello Bob")),
        "B must receive A's message"
    );

    // STEP 4: B sends a message back
    println!("\n[Step 4] B sends message");
    let msg_b = "Hi Alice, Bob here!".to_string();
    let enc_b = SignedMessage::sign_and_encode(&sk_b, &Message::Message { text: msg_b }).unwrap();
    sender_b.broadcast(enc_b).await?;

    sleep(Duration::from_secs(3)).await;
    drain(&net_rx_a, &mut sim_a);

    println!("  A received: {:?}", sim_a.received_messages);
    assert!(
        sim_a
            .received_messages
            .iter()
            .any(|m| m.contains("Hi Alice")),
        "A must receive B's message"
    );

    // Print full exchange for debugging
    println!("\n=== Full message exchange ===");
    println!("A: {:?}", sim_a.received_messages);
    println!("B: {:?}", sim_b.received_messages);

    drop(sender_a);
    drop(sender_b);
    drop(router_a);
    drop(ep_a);
    drop(router_b);
    drop(ep_b);

    println!("\n✓ FULL CHAT LIST FLOW PASSED — two-way message transfer works");
    Ok(())
}
