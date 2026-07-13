//! Debug test: rapidly send 3 images and verify ALL entries appear.
//!
//! Compile with: cargo test --features gui --test test_multi_image_burst -- --nocapture

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use boru_chat::chat_callbacks::ChatCallbacks;
use boru_chat::chat_core::{
    download_candidates, forward_gossip_events, handle_net_event, ChatEntry, Message, MessageHash,
    NetEvent, SignedMessage,
};
use boru_chat::friends::FriendId;
use boru_chat::net::{Gossip, GOSSIP_ALPN};
use boru_chat::proto::TopicId;
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, PublicKey,
    RelayMode, SecretKey,
};
use iroh_blobs::{store::mem::MemStore, BlobsProtocol};
use n0_error::Result;
use n0_future::{task, time::sleep};
use rand::{RngExt, SeedableRng};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;

#[expect(dead_code)]
struct BurstPeer {
    local_public: PublicKey,
    entries: Vec<ChatEntry>,
    names: HashMap<PublicKey, String>,
    neighbors: std::collections::HashSet<PublicKey>,
    pending_file: Option<(String, String)>,
    pending_image: std::collections::VecDeque<(String, MessageHash, PublicKey)>,
    blob_store: MemStore,
    endpoint: iroh::Endpoint,
    log: Vec<String>,
}

impl ChatCallbacks for BurstPeer {
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
        self.log.push(format!("[sys] {text}"));
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
        self.log.push(format!("[{label}] {text}"));
        self.entries.push(ChatEntry::remote(label, text));
    }
    fn set_pending_file(&mut self, name: String, ticket: String) {
        self.pending_file = Some((name, ticket));
    }
    fn set_pending_image(&mut self, name: String, hash: MessageHash, from: PublicKey) {
        self.log.push(format!(
            "[set_pending_image] name={name} from={} queue_len={}",
            from.fmt_short(),
            self.pending_image.len() + 1
        ));
        self.pending_image.push_back((name, hash, from));
    }
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
) -> Result<(
    Router,
    iroh::Endpoint,
    SecretKey,
    Gossip,
    PublicKey,
    MemStore,
)> {
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
    let blob_store = MemStore::new();
    let blobs_protocol = BlobsProtocol::new(&blob_store, None);
    let router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .accept(iroh_blobs::ALPN, blobs_protocol.clone())
        .spawn();
    Ok((
        router,
        ep.clone(),
        ep.secret_key().clone(),
        gossip,
        pk,
        blob_store,
    ))
}

async fn drain_events(
    rx: &Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<NetEvent>>>,
    peer: &mut BurstPeer,
) -> usize {
    let mut count = 0;
    loop {
        let event = {
            let mut guard = rx.lock().await;
            guard.try_recv().ok()
        };
        match event {
            Some(event) => {
                count += 1;
                let _ = handle_net_event(event, peer);
            }
            None => break,
        }
    }
    count
}

fn count_image_entries(entries: &[ChatEntry]) -> usize {
    entries
        .iter()
        .filter(|e| e.body.starts_with("[Image"))
        .count()
}

#[tokio::test]
async fn test_three_remote_image_burst() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(99999);

    let (router_a, ep_a, sk_a, gossip_a, pk_a, bs_a) = spawn_peer(&mut rng).await?;
    let (router_b, ep_b, _sk_b, gossip_b, pk_b, bs_b) = spawn_peer(&mut rng).await?;

    println!("Peer A (sender):   {}", pk_a.fmt_short());
    println!("Peer B (receiver): {}", pk_b.fmt_short());

    let topic = TopicId::from_bytes(rng.random());

    // Subscribe A
    let sub_a = gossip_a.subscribe(topic, vec![]).await?;
    let (sender_a, receiver_a) = sub_a.split();
    let (ntx_a, _nrx_a) = tokio::sync::mpsc::unbounded_channel();
    task::spawn(forward_gossip_events(receiver_a, ntx_a));
    let about_a = SignedMessage::sign_and_encode(
        &sk_a,
        &Message::AboutMe {
            name: "Alice".into(),
            profile_image_ticket: None,
        },
    )
    .unwrap();
    sender_a.broadcast(about_a).await?;

    // Subscribe B
    sleep(Duration::from_millis(100)).await;
    let memlook = MemoryLookup::new();
    if let Ok(addr_lookup) = ep_b.address_lookup() {
        addr_lookup.add(memlook.clone());
    }
    memlook.set_endpoint_info(ep_a.addr());
    let sub_b = gossip_b.subscribe(topic, vec![pk_a]).await?;
    let (_sender_b, receiver_b) = sub_b.split();
    let (ntx_b, nrx_b) = tokio::sync::mpsc::unbounded_channel();
    let nrx_b = Arc::new(Mutex::new(nrx_b));
    task::spawn(forward_gossip_events(receiver_b, ntx_b));
    let about_b = SignedMessage::sign_and_encode(
        &_sk_b,
        &Message::AboutMe {
            name: "Bob".into(),
            profile_image_ticket: None,
        },
    )
    .unwrap();
    _sender_b.broadcast(about_b).await?;

    // Wait for connection
    let mut peer_b = BurstPeer {
        local_public: pk_b,
        entries: Vec::new(),
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        pending_file: None,
        pending_image: std::collections::VecDeque::new(),
        blob_store: bs_b.clone(),
        endpoint: ep_b.clone(),
        log: Vec::new(),
    };

    let mut connected = false;
    for i in 0..60 {
        sleep(Duration::from_millis(200)).await;
        drain_events(&nrx_b, &mut peer_b).await;
        if !peer_b.neighbors.is_empty() {
            println!("  Connected at tick {i}");
            connected = true;
            break;
        }
    }
    assert!(connected, "B should connect to A");
    drain_events(&nrx_b, &mut peer_b).await;

    // Broadcast ALL 3 images immediately (simulating rapid-fire sends)
    println!("\n=== Broadcasting 3 ImageShare messages in rapid succession ===");
    let images = [
        ("burst1.png", b"1111111111111111" as &[u8]),
        ("burst2.png", b"2222222222222222"),
        ("burst3.png", b"3333333333333333"),
    ];

    for (i, (name, data)) in images.iter().enumerate() {
        let tag_info = bs_a.blobs().add_bytes(data.to_vec()).await.unwrap();
        let hash: MessageHash = *tag_info.hash.as_bytes();
        let msg = Message::ImageShare {
            name: name.to_string(),
            hash,
        };
        let encoded = SignedMessage::sign_and_encode(&sk_a, &msg).unwrap();
        sender_a.broadcast(encoded).await?;
        println!("  [{i}] Broadcasted: {name}");
    }

    // Wait and drain
    sleep(Duration::from_secs(5)).await;
    let ev_count = drain_events(&nrx_b, &mut peer_b).await;

    println!("\n=== AFTER DRAIN ===");
    println!("Events processed: {ev_count}");
    println!("Total entries:    {}", peer_b.entries.len());
    println!("Image entries:    {}", count_image_entries(&peer_b.entries));
    println!("Pending queue:    {}", peer_b.pending_image.len());

    for (i, e) in peer_b.entries.iter().enumerate() {
        let tag = if e.body.starts_with("[Image") {
            "IMG"
        } else if e.body.starts_with("[sys]") || e.body.starts_with("System") {
            "SYS"
        } else {
            "MSG"
        };
        println!("  [{i}] {tag}: {:?}", &e.body[..e.body.len().min(70)]);
    }
    for (i, (n, h, s)) in peer_b.pending_image.iter().enumerate() {
        println!(
            "  [pending {i}] {n} hash={} from={}",
            hex::encode(h),
            s.fmt_short()
        );
    }

    // CRITICAL ASSERTIONS
    assert!(
        peer_b.pending_image.len() >= 3,
        "B should have ALL 3 images queued (got {})",
        peer_b.pending_image.len()
    );

    // Now drain the queue
    println!("\n=== Downloading all queued images ===");
    let mut downloaded = 0;
    while let Some((name, hash, sender_pk)) = peer_b.pending_image.pop_front() {
        let blob_hash: iroh_blobs::Hash = hash.into();
        let candidates = download_candidates(sender_pk, &peer_b.neighbors);
        println!("  Downloading {name}...");
        match bs_b
            .downloader(&peer_b.endpoint)
            .download(blob_hash, candidates)
            .await
        {
            Ok(()) => {
                let mut reader = bs_b.blobs().reader(blob_hash);
                let mut buf = Vec::new();
                let _ = reader.read_to_end(&mut buf).await;
                peer_b.entries.push(ChatEntry::remote(
                    sender_pk.fmt_short().to_string(),
                    format!("[Image: {name}]"),
                ));
                downloaded += 1;
            }
            Err(e) => println!("  ✗ {name} failed: {e}"),
        }
    }

    println!("\n=== FINAL ===");
    println!("Downloaded successfully:  {downloaded}");
    println!(
        "Image entries in log:     {}",
        count_image_entries(&peer_b.entries)
    );
    println!("Total entries in log:     {}", peer_b.entries.len());

    assert_eq!(downloaded, 3, "Should have downloaded all 3 images");
    assert_eq!(
        count_image_entries(&peer_b.entries),
        3,
        "Should have 3 image entries in log"
    );

    println!("\n✓✓ THREE-IMAGE BURST SUCCESSFUL ✓✓");

    drop(sender_a);
    drop(_sender_b);
    drop(router_a);
    drop(router_b);
    Ok(())
}
