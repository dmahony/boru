#![cfg(feature = "net")]
#![allow(deprecated)]

//! Two-instance normal-chat regression test.
//!
//! This deliberately exercises the application-facing path:
//! GossipTopic -> forward_gossip_events -> NetEvent -> handle_net_event ->
//! ChatCallbacks, with durable receiver history and GUI-facing state.  It is
//! not a diagnostic-probe test.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use boru_core::chat_callbacks::ChatCallbacks;
use boru_core::chat_core::{
    forward_gossip_events, handle_net_event, message_hash, ChatEntry, Message, MessageHash,
    NetEvent, SignedMessage,
};
use boru_core::chat_history::{ChatHistoryStore, HistoryEntry};
use boru_core::friends::FriendId;
use boru_core::net::{Gossip, GOSSIP_ALPN};
use boru_core::proto::TopicId;
use distributed_topic_tracker::{Dht, DhtConfig};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint, PublicKey,
    RelayMode, SecretKey,
};
use n0_error::Result;
use n0_future::{task, time::sleep};
use rand::{RngExt, SeedableRng};
use tempfile::TempDir;
use tokio::sync::Mutex;

#[derive(Debug, Default)]
struct GuiState {
    active_room: Option<TopicId>,
    total_entry_count: usize,
    neighbor_count: usize,
}

struct TestInstance {
    local_public: PublicKey,
    data_dir: PathBuf,
    entries: Vec<ChatEntry>,
    names: HashMap<PublicKey, String>,
    neighbors: HashSet<PublicKey>,
    received_messages: Vec<String>,
    received_hashes: Vec<MessageHash>,
    history: ChatHistoryStore,
    gui: GuiState,
}

impl TestInstance {
    fn new(local_public: PublicKey, data_dir: PathBuf, topic: TopicId) -> Self {
        Self {
            local_public,
            history: ChatHistoryStore::load_or_default(&data_dir),
            data_dir,
            entries: Vec::new(),
            names: HashMap::new(),
            neighbors: HashSet::new(),
            received_messages: Vec::new(),
            received_hashes: Vec::new(),
            gui: GuiState {
                active_room: Some(topic),
                ..GuiState::default()
            },
        }
    }

    fn sync_gui(&mut self) {
        self.gui.neighbor_count = self.neighbors.len();
        self.gui.total_entry_count = self.entries.len();
    }

    fn persist_remote(&mut self, peer: PublicKey, text: &str, hash: Option<MessageHash>) {
        let hash = hash.expect("normal text NetEvent must carry a message hash");
        self.received_hashes.push(hash);
        self.received_messages.push(text.to_owned());
        self.history.push_with_id(HistoryEntry::new(
            self.gui.active_room.expect("room must be active"),
            peer.to_string(),
            text.as_bytes().to_vec(),
            "text",
            text,
        ));
        // `save()` is a deprecated no-op (chat history is SQLite-only).  Write
        // the legacy JSON fixture directly so the restart-reload assertion
        // below still exercises the migration/read path.
        std::fs::write(
            self.history.file_path(),
            serde_json::to_vec(&self.history).expect("serialize history"),
        )
        .expect("receiver history must persist");
    }
}

impl ChatCallbacks for TestInstance {
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
        self.entries.push(ChatEntry::system(text));
        self.sync_gui();
    }
    fn push_remote(
        &mut self,
        peer: PublicKey,
        label: String,
        text: String,
        hash: Option<MessageHash>,
        _sent_at: Option<u64>,
    ) {
        self.persist_remote(peer, &text, hash);
        self.entries.push(ChatEntry::remote(label, text));
        self.sync_gui();
    }
    fn set_pending_file(
        &mut self,
        _name: String,
        _ticket: String,
        _size: u64,
        _thumbnail: Option<[u8; 32]>,
        _sender_label: Option<String>,
    ) {
    }
    fn set_pending_image(&mut self, _name: String, _hash: MessageHash, _from: PublicKey) {}
    fn has_message(&self, _hash: &MessageHash) -> bool {
        false
    }
    fn edit_message(&mut self, _hash: &MessageHash, _new_text: String) {}
    fn delete_message(&mut self, _hash: &MessageHash) {}
    fn add_reaction(&mut self, _hash: &MessageHash, _emoji: String) {}
    fn on_neighbor_up(&mut self, peer: PublicKey) {
        self.neighbors.insert(peer);
        self.sync_gui();
    }
    fn on_neighbor_down(&mut self, peer: PublicKey) {
        self.neighbors.remove(&peer);
        self.sync_gui();
    }
    fn record_activity(&mut self, _peer: PublicKey) {}
    fn request_quit(&mut self) {}
}

struct Peer {
    _router: Router,
    endpoint: Endpoint,
    memory_lookup: MemoryLookup,
    gossip: Gossip,
    secret_key: SecretKey,
    public_key: PublicKey,
    _dht: Dht,
}

async fn spawn_peer(secret_key: SecretKey) -> Result<Peer> {
    // DHT is intentionally constructed for each instance, matching the GUI
    // startup path.  The gossip transport remains relay-capable and uses no
    // direct address injection other than the explicit bootstrap record.
    let dht = Dht::new(&DhtConfig::default());
    let memory_lookup = MemoryLookup::new();
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret_key.clone())
        .address_lookup(memory_lookup.clone())
        .relay_mode(RelayMode::Default)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    endpoint.online().await;
    let gossip = Gossip::builder().spawn(endpoint.clone());
    let router = Router::builder(endpoint.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();
    let public_key = secret_key.public();
    Ok(Peer {
        _router: router,
        endpoint,
        memory_lookup,
        gossip,
        secret_key,
        public_key,
        _dht: dht,
    })
}

fn add_bootstrap_address(local: &Peer, remote: &Peer) {
    local
        .memory_lookup
        .set_endpoint_info(remote.endpoint.addr());
}

struct RoomHandle {
    sender: boru_core::api::GossipSender,
    receiver: Arc<Mutex<tokio::sync::mpsc::Receiver<NetEvent>>>,
}

async fn open_room(peer: &Peer, topic: TopicId, bootstrap: Vec<PublicKey>) -> Result<RoomHandle> {
    let sub = peer.gossip.subscribe(topic, bootstrap).await?;
    let (sender, gossip_receiver) = sub.split();
    let (net_tx, net_rx) = tokio::sync::mpsc::channel(128);
    let receiver = Arc::new(Mutex::new(net_rx));
    task::spawn(forward_gossip_events(gossip_receiver, net_tx));
    Ok(RoomHandle { sender, receiver })
}

fn drain_net(
    rx: &Arc<Mutex<tokio::sync::mpsc::Receiver<NetEvent>>>,
    instance: &mut TestInstance,
) -> usize {
    let mut count = 0;
    while let Ok(event) = rx.try_lock().unwrap().try_recv() {
        count += 1;
        if let NetEvent::Message { from, message, .. } = &event {
            if *from != instance.local_public {
                if let Message::Message { text } = message {
                    eprintln!(
                        "normal-chat receive room={} peer={} hash={}",
                        instance.gui.active_room.expect("room"),
                        from.fmt_short(),
                        hex::encode(message_hash(message))
                    );
                    assert!(
                        !text.is_empty(),
                        "empty normal-chat text is not a valid receipt"
                    );
                }
            }
        }
        handle_net_event(event, instance).expect("NetEvent must be accepted");
    }
    count
}

async fn wait_ready(a: &RoomHandle, b: &RoomHandle, ia: &mut TestInstance, ib: &mut TestInstance) {
    for _ in 0..80 {
        sleep(Duration::from_millis(100)).await;
        drain_net(&a.receiver, ia);
        drain_net(&b.receiver, ib);
        if ia.neighbors.contains(&ib.local_public) && ib.neighbors.contains(&ia.local_public) {
            return;
        }
    }
    panic!(
        "NeighborUp readiness failed: A neighbors={} B neighbors={} room={}",
        ia.neighbors.len(),
        ib.neighbors.len(),
        ia.gui.active_room.expect("room")
    );
}

async fn send_and_verify(
    sender: &RoomHandle,
    receiver: &RoomHandle,
    from: &SecretKey,
    text: &str,
    topic: TopicId,
    receiving: &mut TestInstance,
    sending: &mut TestInstance,
) {
    assert!(
        !sending.neighbors.is_empty(),
        "inactive sender must fail this test"
    );
    let message = Message::Message {
        text: text.to_owned(),
    };
    let expected_hash = message_hash(&message);
    let encoded = SignedMessage::sign_and_encode(from, &message).expect("sign normal chat message");
    sender
        .sender
        .broadcast(encoded)
        .await
        .expect("normal chat broadcast");
    for _ in 0..50 {
        sleep(Duration::from_millis(100)).await;
        drain_net(&sender.receiver, sending);
        drain_net(&receiver.receiver, receiving);
        if receiving.received_hashes.contains(&expected_hash) {
            break;
        }
    }
    assert!(
        receiving.received_hashes.contains(&expected_hash),
        "missing remote NetEvent::Message for text hash={} room={}",
        hex::encode(expected_hash),
        topic
    );
    assert!(receiving.received_messages.iter().any(|m| m == text));
    let reloaded = ChatHistoryStore::load(&receiving.data_dir)
        .expect("history reload")
        .expect("receiver history file");
    assert!(reloaded
        .entries()
        .iter()
        .any(|entry| entry.text_preview == text));
    assert_eq!(receiving.gui.active_room, Some(topic));
    assert!(
        receiving.gui.total_entry_count > 0,
        "GUI state did not receive the message"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_instance_dht_normal_chat_survives_reopen_and_restart() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xD17C_2026);
    let dir_a = TempDir::with_prefix("boru-dht-chat-a-")?;
    let dir_b = TempDir::with_prefix("boru-dht-chat-b-")?;
    let sk_a = SecretKey::from_bytes(&rng.random());
    let sk_b = SecretKey::from_bytes(&rng.random());
    let topic = TopicId::from_bytes(rng.random());

    let peer_a = spawn_peer(sk_a.clone()).await?;
    let peer_b = spawn_peer(sk_b.clone()).await?;
    assert_ne!(peer_a.public_key, peer_b.public_key);
    eprintln!(
        "normal-chat setup room={} peers={}↔{} dht=enabled relay=default",
        topic,
        peer_a.public_key.fmt_short(),
        peer_b.public_key.fmt_short()
    );

    add_bootstrap_address(&peer_a, &peer_b);
    add_bootstrap_address(&peer_b, &peer_a);
    let room_a = open_room(&peer_a, topic, vec![]).await?;
    let room_b = open_room(&peer_b, topic, vec![peer_a.public_key]).await?;
    let mut instance_a = TestInstance::new(peer_a.public_key, dir_a.path().to_path_buf(), topic);
    let mut instance_b = TestInstance::new(peer_b.public_key, dir_b.path().to_path_buf(), topic);
    wait_ready(&room_a, &room_b, &mut instance_a, &mut instance_b).await;

    send_and_verify(
        &room_a,
        &room_b,
        &peer_a.secret_key,
        "dht normal message A1",
        topic,
        &mut instance_b,
        &mut instance_a,
    )
    .await;
    send_and_verify(
        &room_b,
        &room_a,
        &peer_b.secret_key,
        "dht normal message B1",
        topic,
        &mut instance_a,
        &mut instance_b,
    )
    .await;

    // Reopen the same room with the persisted topic and fresh application
    // event channels; this catches stale sender/forwarder state.
    drop(room_a);
    drop(room_b);
    add_bootstrap_address(&peer_a, &peer_b);
    add_bootstrap_address(&peer_b, &peer_a);
    let room_a = open_room(&peer_a, topic, vec![]).await?;
    let room_b = open_room(&peer_b, topic, vec![peer_a.public_key]).await?;
    instance_a.gui.active_room = Some(topic);
    instance_b.gui.active_room = Some(topic);
    wait_ready(&room_a, &room_b, &mut instance_a, &mut instance_b).await;
    send_and_verify(
        &room_a,
        &room_b,
        &peer_a.secret_key,
        "dht normal message A2 after reopen",
        topic,
        &mut instance_b,
        &mut instance_a,
    )
    .await;

    // Restart B with the same identity and its distinct persisted directory.
    eprintln!("normal-chat lifecycle: restarting B");
    drop(room_b);
    peer_b.endpoint.close().await;
    drop(peer_b);
    let peer_b = spawn_peer(sk_b).await?;
    drop(room_a);
    add_bootstrap_address(&peer_a, &peer_b);
    add_bootstrap_address(&peer_b, &peer_a);
    let room_a = open_room(&peer_a, topic, vec![peer_b.public_key]).await?;
    let room_b = open_room(&peer_b, topic, vec![peer_a.public_key]).await?;
    instance_b = TestInstance::new(peer_b.public_key, dir_b.path().to_path_buf(), topic);
    assert!(instance_b
        .history
        .entries()
        .iter()
        .any(|entry| entry.text_preview == "dht normal message A1"));
    wait_ready(&room_a, &room_b, &mut instance_a, &mut instance_b).await;
    send_and_verify(
        &room_b,
        &room_a,
        &peer_b.secret_key,
        "dht normal message B2 after restart",
        topic,
        &mut instance_a,
        &mut instance_b,
    )
    .await;

    assert!(instance_a.gui.neighbor_count > 0 && instance_b.gui.neighbor_count > 0);
    drop(room_a);
    drop(room_b);
    peer_a.endpoint.close().await;
    peer_b.endpoint.close().await;
    drop(peer_a);
    drop(peer_b);
    Ok(())
}
