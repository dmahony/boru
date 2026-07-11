//! Exact replica of iced_chat message flow:
//! Subscribes, spawns forward_gossip_events, sends via sender.broadcast,
//! receives via NetEvent channel -> handle_net_event -> ChatCallbacks.

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
    friends::{FriendId, FriendsStore},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
};
use n0_error::Result;
use n0_future::{task, time::sleep};
use rand::{RngExt, SeedableRng};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
#[derive(Debug)]
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
    sender: Option<GossipSender>,
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
    fn set_pending_file(&mut self, name: String, ticket: String) {
        self.pending_file = Some((name, ticket));
    }
    fn set_pending_image(&mut self, name: String, hash: MessageHash, from: PublicKey) {
        self.pending_image = Some((name, hash, from));
    }
    fn has_message(&self, hash: &MessageHash) -> bool {
        self.entries
            .iter()
            .any(|e| e.message_hash.as_ref() == Some(hash))
    }
    fn edit_message(&mut self, hash: &MessageHash, new_text: String) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.message_hash.as_ref() == Some(hash))
        {
            entry.body = new_text;
            entry.edited = true;
        }
    }
    fn delete_message(&mut self, hash: &MessageHash) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.message_hash.as_ref() == Some(hash))
        {
            entry.body = "[message deleted]".to_string();
        }
    }
    fn add_reaction(&mut self, hash: &MessageHash, emoji: String) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.message_hash.as_ref() == Some(hash))
        {
            entry.reactions.push(emoji);
        }
    }
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
) -> Result<(Router, Endpoint, SecretKey, Gossip, PublicKey)> {
    let ep = Endpoint::builder(presets::N0)
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

#[tokio::test]
async fn test_iced_chat_exact_flow() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(42);

    let (router_a, ep_a, sk_a, gossip_a, pk_a) = spawn_peer(&mut rng).await?;
    let (router_b, ep_b, sk_b, gossip_b, pk_b) = spawn_peer(&mut rng).await?;

    println!("Peer A: {}", pk_a.fmt_short());
    println!("Peer B: {}", pk_b.fmt_short());

    let topic = TopicId::from_bytes(rng.random());

    // Peer A: subscribe like CreateNewRoom/OpenRoom
    println!("\nA: subscribing (no bootstrap, like OpenRoom)...");
    let sub_a = gossip_a.subscribe(topic, vec![]).await?;
    let (sender_a, receiver_a) = sub_a.split();

    let (net_tx_a, net_rx_a) = tokio::sync::mpsc::unbounded_channel();
    let net_rx_a = Arc::new(tokio::sync::Mutex::new(net_rx_a));

    task::spawn(forward_gossip_events(receiver_a, net_tx_a));

    // Broadcast AboutMe
    let about_me = SignedMessage::sign_and_encode(
        &sk_a,
        &Message::AboutMe {
            name: "Alice".into(),
            profile_image_ticket: None,
        },
    )
    .unwrap();
    sender_a.broadcast(about_me).await?;

    // Peer B: join like JoinFromTicket
    sleep(Duration::from_millis(100)).await;
    println!("B: subscribing (with A as bootstrap, like JoinFromTicket)...");
    let sub_b = gossip_b.subscribe(topic, vec![pk_a]).await?;
    let (sender_b, receiver_b) = sub_b.split();

    let (net_tx_b, net_rx_b) = tokio::sync::mpsc::unbounded_channel();
    let net_rx_b = Arc::new(tokio::sync::Mutex::new(net_rx_b));

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

    // --- Sim chat state (like iced_chat's IcedChat) ---
    let mut sim_a = SimChat {
        local_public: pk_a,
        entries: vec![],
        names: HashMap::new(),
        friends: FriendsStore::empty_at("/tmp/iced-test-a"),
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
        friends: FriendsStore::empty_at("/tmp/iced-test-b"),
        pending_file: None,
        pending_image: None,
        neighbors: std::collections::HashSet::new(),
        received_messages: vec![],
        sender: Some(sender_b.clone()),
    };

    // Wait for gossip to connect
    let drain_net =
        |rx: &Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<NetEvent>>>,
         _label: &str,
         sim: &mut SimChat| {
            let mut count = 0;
            loop {
                let item = rx.try_lock().unwrap().try_recv();
                match item {
                    Ok(event) => {
                        count += 1;
                        let _ = handle_net_event(event, sim);
                    }
                    Err(_) => break,
                }
            }
            count
        };

    for i in 0..60 {
        sleep(Duration::from_millis(200)).await;
        drain_net(&net_rx_a, "A", &mut sim_a);
        drain_net(&net_rx_b, "B", &mut sim_b);
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

    assert!(!sim_a.neighbors.is_empty(), "A should have neighbors");
    assert!(!sim_b.neighbors.is_empty(), "B should have neighbors");

    // Drain stale
    drain_net(&net_rx_a, "A", &mut sim_a);
    drain_net(&net_rx_b, "B", &mut sim_b);

    // --- A sends a text message (exact iced_chat SendPressed path) ---
    println!("\n--- A broadcasts message ---");
    let text_a = "hello from Alice!".to_string();
    let encoded_a =
        SignedMessage::sign_and_encode(&sk_a, &Message::Message { text: text_a }).unwrap();
    sender_a.broadcast(encoded_a).await?;

    sleep(Duration::from_secs(3)).await;
    drain_net(&net_rx_a, "A", &mut sim_a);
    drain_net(&net_rx_b, "B", &mut sim_b);

    println!("A received: {:?}", sim_a.received_messages);
    println!("B received: {:?}", sim_b.received_messages);

    let b_got_alice = sim_b
        .received_messages
        .iter()
        .any(|m| m.contains("hello from Alice"));
    assert!(b_got_alice, "B must receive Alice's text!");

    // --- B sends back ---
    println!("\n--- B broadcasts message ---");
    let text_b = "hey Alice, Bob here!".to_string();
    let encoded_b =
        SignedMessage::sign_and_encode(&sk_b, &Message::Message { text: text_b }).unwrap();
    sender_b.broadcast(encoded_b).await?;

    sleep(Duration::from_secs(3)).await;
    drain_net(&net_rx_a, "A", &mut sim_a);
    drain_net(&net_rx_b, "B", &mut sim_b);

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
    drop(ep_a);
    drop(router_b);
    drop(ep_b);

    println!("\n✓ ICED_CHAT EXACT FLOW: TWO-WAY COMMUNICATION VERIFIED");
    Ok(())
}
