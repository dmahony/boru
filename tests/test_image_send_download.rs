//! End-to-end test: send an ImageShare between two peers and verify the
//! receiver auto-downloads and reads back the blob.
//!
//! Mirrors the exact flow used by the Iced GUI:
//!   1. Sender adds file to blob_store, broadcasts ImageShare{name, hash}
//!   2. Receiver gets NetEvent::Message(ImageShare) -> set_pending_image
//!   3. Downloader fetches blob from sender
//!   4. Verify the downloaded bytes match the original

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, PublicKey,
    RelayMode, SecretKey,
};
use iroh_blobs::{store::mem::MemStore, BlobsProtocol};
use iroh_gossip::chat_callbacks::ChatCallbacks;
use iroh_gossip::chat_core::{
    download_candidates, forward_gossip_events, handle_net_event, ChatEntry, Message, MessageHash,
    NetEvent, SignedMessage,
};
use iroh_gossip::friends::FriendId;
use iroh_gossip::net::{Gossip, GOSSIP_ALPN};
use iroh_gossip::proto::TopicId;
use n0_error::Result;
use n0_future::{task, time::sleep};
use rand::{RngExt, SeedableRng};
use tokio::sync::Mutex;

// ── Test peer that tracks pending_image like IcedChat ─────────

struct ImageTestPeer {
    local_public: PublicKey,
    entries: Vec<ChatEntry>,
    names: HashMap<PublicKey, String>,
    neighbors: std::collections::HashSet<PublicKey>,
    received_messages: Vec<String>,
    pending_file: Option<(String, String)>,
    pending_image: Option<(String, MessageHash, PublicKey)>,
    /// The blob store, so we can trigger the download ourselves.
    blob_store: MemStore,
    endpoint: iroh::Endpoint,
}

impl ChatCallbacks for ImageTestPeer {
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
    fn push_remote(&mut self, label: String, text: String, _hash: Option<MessageHash>) {
        self.received_messages.push(format!("[{label}] {text}"));
        self.entries.push(ChatEntry::remote(label, text));
    }
    fn set_pending_file(&mut self, name: String, ticket: String) {
        self.pending_file = Some((name, ticket));
    }
    fn set_pending_image(&mut self, name: String, hash: MessageHash, from: PublicKey) {
        self.pending_image = Some((name, hash, from));
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

async fn spawn_peer_with_blobs(
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
    Ok((router, ep.clone(), ep.secret_key().clone(), gossip, pk, blob_store))
}

fn drain_net(
    rx: &Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<NetEvent>>>,
    sim: &mut ImageTestPeer,
) -> usize {
    let mut count = 0;
    loop {
        match rx.try_lock().unwrap().try_recv() {
            Ok(event) => {
                count += 1;
                let _ = handle_net_event(event, sim);
            }
            Err(_) => break,
        }
    }
    count
}

#[tokio::test]
async fn test_image_send_and_download() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(42);

    let (router_a, ep_a, sk_a, gossip_a, pk_a, blob_store_a) =
        spawn_peer_with_blobs(&mut rng).await?;
    let (router_b, ep_b, sk_b, gossip_b, pk_b, blob_store_b) =
        spawn_peer_with_blobs(&mut rng).await?;

    println!("Peer A (sender):   {}", pk_a.fmt_short());
    println!("Peer B (receiver): {}", pk_b.fmt_short());

    let topic = TopicId::from_bytes(rng.random());

    // ── Peer A: subscribe ──
    println!("\n--- A: subscribing ---");
    let sub_a = gossip_a.subscribe(topic, vec![]).await?;
    let (sender_a, receiver_a) = sub_a.split();
    let (net_tx_a, net_rx_a) = tokio::sync::mpsc::unbounded_channel();
    let net_rx_a = Arc::new(Mutex::new(net_rx_a));
    task::spawn(forward_gossip_events(receiver_a, net_tx_a));

    let about_me = SignedMessage::sign_and_encode(
        &sk_a,
        &Message::AboutMe { name: "Alice".into() },
    ).unwrap();
    sender_a.broadcast(about_me).await?;

    // ── Peer B: subscribe with A as bootstrap ──
    sleep(Duration::from_millis(100)).await;
    println!("\n--- B: subscribing (with A as bootstrap) ---");
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

    let about_me_b =
        SignedMessage::sign_and_encode(&sk_b, &Message::AboutMe { name: "Bob".into() }).unwrap();
    sender_b.broadcast(about_me_b).await?;

    // ── Set up sim peers ──
    let mut sim_a = ImageTestPeer {
        local_public: pk_a,
        entries: vec![],
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        received_messages: vec![],
        pending_file: None,
        pending_image: None,
        blob_store: blob_store_a.clone(),
        endpoint: ep_a.clone(),
    };
    let mut sim_b = ImageTestPeer {
        local_public: pk_b,
        entries: vec![],
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        received_messages: vec![],
        pending_file: None,
        pending_image: None,
        blob_store: blob_store_b.clone(),
        endpoint: ep_b.clone(),
    };

    // ── Wait for gossip to connect ──
    let mut connected = false;
    for i in 0..60 {
        sleep(Duration::from_millis(200)).await;
        drain_net(&net_rx_a, &mut sim_a);
        drain_net(&net_rx_b, &mut sim_b);
        if sim_a.neighbors.len() > 0 && sim_b.neighbors.len() > 0 {
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
    println!("✓ Both peers connected");

    // Drain stale events
    drain_net(&net_rx_a, &mut sim_a);
    drain_net(&net_rx_b, &mut sim_b);

    // ── Sender (A) adds a small image to blob store and broadcasts ImageShare ──
    println!("\n--- A: adding image bytes to blob_store and broadcasting ImageShare ---");

    let image_data: Vec<u8> = b"fake-png-bytes-1234567890-abcdef".to_vec();

    // add_bytes returns AddProgress which implements IntoFuture -> RequestResult<TagInfo>
    use iroh_blobs::api::proto::TagInfo;
    let tag_info = blob_store_a.blobs().add_bytes(image_data.clone()).await
        .map_err(|e| format!("add_bytes error: {e}"))
        .unwrap();
    let blob_hash = tag_info.hash;
    println!("  Blob hash from add_bytes: {}", blob_hash);

    // Convert to MessageHash (like ExecuteImageSend does)
    let hash: MessageHash = *blob_hash.as_bytes();
    let msg = Message::ImageShare {
        name: "test-image.png".into(),
        hash,
    };
    let encoded = SignedMessage::sign_and_encode(&sk_a, &msg)
        .map_err(|e| format!("Failed to sign: {e}"))
        .unwrap();
    sender_a.broadcast(encoded).await?;

    println!("  ImageShare broadcast from A ✓");

    // ── Wait for B to receive the ImageShare ──
    sleep(Duration::from_secs(3)).await;
    let b_count = drain_net(&net_rx_b, &mut sim_b);
    println!("  B drained {b_count} events including ImageShare");

    // ── Check that B's pending_image is set ──
    let pending = sim_b.pending_image.take();
    assert!(
        pending.is_some(),
        "B should have pending_image set after receiving ImageShare.\n  B received: {:?}",
        sim_b.received_messages
    );
    let (img_name, img_hash, sender_pk) = pending.unwrap();
    println!(
        "  B's pending_image: name={:?}, hash={}, sender={}",
        img_name,
        hex::encode(img_hash),
        sender_pk.fmt_short()
    );
    assert_eq!(img_name, "test-image.png");
    assert_eq!(img_hash, hash);
    assert_eq!(sender_pk, pk_a);

    // ── Now simulate the Iced GUI download path (exact code from app.rs) ──
    println!("\n--- B: downloading blob from A (exact Iced GUI path) ---");

    let blob_hash_dl: iroh_blobs::Hash = img_hash.into();
    println!("  Using blob hash: {}", blob_hash_dl);

    // Download
    let candidates = download_candidates(sender_pk, &sim_b.neighbors);
    blob_store_b
        .downloader(&ep_b)
        .download(blob_hash_dl, candidates)
        .await
        .map_err(|e| format!("Download: {e}"))?;
    println!("  Download completed ✓");

    // Read back the blob
    use tokio::io::AsyncReadExt;
    let mut reader = blob_store_b.blobs().reader(blob_hash_dl);
    let mut downloaded = Vec::new();
    reader
        .read_to_end(&mut downloaded)
        .await
        .map_err(|e| format!("Read: {e}"))?;
    println!("  Read {} downloaded bytes ✓", downloaded.len());

    assert_eq!(
        downloaded, image_data,
        "Downloaded image bytes must match original"
    );
    println!("✓ Downloaded bytes match original!");

    // Cleanup
    drop(sender_a);
    drop(sender_b);
    drop(router_a);
    drop(router_b);

    println!("\n✓✓ IMAGE SEND AND DOWNLOAD END-TO-END VERIFIED ✓✓");
    Ok(())
}
