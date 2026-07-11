//! End-to-end test: verify the receiver's exact GUI download path works.
//!
//! This mimics how the Iced GUI processes ImageShare + auto-download:
//!   1. Peer A broadcasts ImageShare with blob hash
//!   2. Peer B receives via NetEvent -> handle_net_event -> set_pending_image
//!   3. Peer B's update loop checks pending_image -> downloads blob from A
//!   4. Peer B reads bytes
//!   5. Final check: B's entries contain an image entry with matching bytes

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
    download_candidates, handle_net_event, ChatEntry, Message, MessageHash, NetEvent,
    SignedMessage,
};
use iroh_gossip::friends::FriendId;
use iroh_gossip::net::{Gossip, GOSSIP_ALPN};
use iroh_gossip::proto::TopicId;
use n0_error::Result;
use n0_future::{task, time::sleep};
use rand::{RngExt, SeedableRng};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;

/// Test peer that exactly mirrors IcedChat's field layout and download logic.
struct RxTestPeer {
    local_public: PublicKey,
    entries: Vec<ChatEntry>,
    names: HashMap<PublicKey, String>,
    neighbors: std::collections::HashSet<PublicKey>,
    pending_file: Option<(String, String)>,
    pending_image: Option<(String, MessageHash, PublicKey)>,
    blob_store: MemStore,
    endpoint: iroh::Endpoint,
    last_downloaded_image: Option<Vec<u8>>,
    /// accumulated system/error messages
    log: Vec<String>,
}

impl ChatCallbacks for RxTestPeer {
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
        self.log.push(format!("[sys] {text}"));
        self.entries.push(ChatEntry::system(text));
    }
    fn push_remote(
        &mut self,
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
            "[set_pending_image] name={name} hash={} from={}",
            hex::encode(hash),
            from.fmt_short()
        ));
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
    Ok((
        router,
        ep.clone(),
        ep.secret_key().clone(),
        gossip,
        pk,
        blob_store,
    ))
}

/// Process ONE NetEvent from the channel, exactly like the Iced GUI does.
/// After handle_net_event, check pending_image and auto-download if set.
/// Returns true if an event was processed.
async fn process_one_event(
    rx: &Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<NetEvent>>>,
    peer: &mut RxTestPeer,
) -> bool {
    let event = {
        let mut guard = rx.lock().await;
        guard.try_recv().ok()
    };
    match event {
        Some(event) => {
            let _ = handle_net_event(event, peer);
            // Exactly like Iced GUI: check pending_image after each event
            if let Some((name, hash, sender_pk)) = peer.pending_image.take() {
                peer.log.push(format!(
                    "[auto-download] starting download of {name} from {}",
                    sender_pk.fmt_short()
                ));
                let blob_hash: iroh_blobs::Hash = hash.into();
                let candidates = download_candidates(sender_pk, &peer.neighbors);
                match peer
                    .blob_store
                    .downloader(&peer.endpoint)
                    .download(blob_hash, candidates)
                    .await
                {
                    Ok(()) => {
                        peer.log
                            .push(format!("[auto-download] download completed for {name}"));
                        let mut reader = peer.blob_store.blobs().reader(blob_hash);
                        let mut buf = Vec::new();
                        match reader.read_to_end(&mut buf).await {
                            Ok(_) => {
                                peer.log.push(format!(
                                    "[auto-download] read {} bytes for {name}",
                                    buf.len()
                                ));
                                // Create a ChatEntry with image_bytes (like Iced GUI's ImageDownloaded handler)
                                let sender_name = peer
                                    .names
                                    .get(&sender_pk)
                                    .cloned()
                                    .unwrap_or_else(|| sender_pk.fmt_short().to_string());
                                peer.last_downloaded_image = Some(buf);
                                peer.entries.push(ChatEntry::remote(
                                    sender_name,
                                    format!("[Image: {name}]"),
                                ));
                                peer.log.push(format!(
                                    "[auto-download] ChatEntry created for {name} with image bytes"
                                ));
                            }
                            Err(e) => {
                                peer.log
                                    .push(format!("[auto-download] ERROR reading blob: {e}"));
                            }
                        }
                    }
                    Err(e) => {
                        peer.log
                            .push(format!("[auto-download] ERROR downloading {name}: {e}"));
                    }
                }
            }
            true
        }
        None => false,
    }
}

fn count_image_entries(entries: &[ChatEntry]) -> usize {
    entries
        .iter()
        .filter(|e| e.body.starts_with("[Image:"))
        .count()
}

#[tokio::test]
async fn test_receiver_downloads_image_entry() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(12345);

    let (router_a, ep_a, sk_a, gossip_a, pk_a, blob_store_a) =
        spawn_peer_with_blobs(&mut rng).await?;
    let (_router_b, ep_b, sk_b, gossip_b, pk_b, blob_store_b) =
        spawn_peer_with_blobs(&mut rng).await?;

    println!("Peer A (sender):   {}", pk_a.fmt_short());
    println!("Peer B (receiver): {}", pk_b.fmt_short());

    let topic = TopicId::from_bytes(rng.random());

    // Peer A subscribe
    let sub_a = gossip_a.subscribe(topic, vec![]).await?;
    let (sender_a, receiver_a) = sub_a.split();
    let (net_tx_a, net_rx_a) = tokio::sync::mpsc::unbounded_channel();
    let _net_rx_a = Arc::new(Mutex::new(net_rx_a));
    task::spawn(iroh_gossip::chat_core::forward_gossip_events(
        receiver_a, net_tx_a,
    ));

    let about_me = SignedMessage::sign_and_encode(
        &sk_a,
        &Message::AboutMe {
            name: "Alice".into(),
        },
    )
    .unwrap();
    sender_a.broadcast(about_me).await?;

    // Peer B subscribe with A as bootstrap
    sleep(Duration::from_millis(100)).await;
    let memory_lookup = MemoryLookup::new();
    if let Ok(addr_lookup) = ep_b.address_lookup() {
        addr_lookup.add(memory_lookup.clone());
    }
    memory_lookup.set_endpoint_info(ep_a.addr());
    let sub_b = gossip_b.subscribe(topic, vec![pk_a]).await?;
    let (_sender_b, receiver_b) = sub_b.split();
    let (net_tx_b, net_rx_b) = tokio::sync::mpsc::unbounded_channel();
    let net_rx_b = Arc::new(Mutex::new(net_rx_b));
    task::spawn(iroh_gossip::chat_core::forward_gossip_events(
        receiver_b, net_tx_b,
    ));

    let about_me_b =
        SignedMessage::sign_and_encode(&sk_b, &Message::AboutMe { name: "Bob".into() }).unwrap();
    _sender_b.broadcast(about_me_b).await?;

    // Create receiver peer state
    let sim = RxTestPeer {
        local_public: pk_b,
        entries: Vec::new(),
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        pending_file: None,
        pending_image: None,
        blob_store: blob_store_b.clone(),
        endpoint: ep_b.clone(),
        last_downloaded_image: None,
        log: Vec::new(),
    };

    let mut rx_sim = sim;
    let net_rx = net_rx_b;

    // Wait for connection
    let mut connected = false;
    for i in 0..60 {
        sleep(Duration::from_millis(200)).await;
        while process_one_event(&net_rx, &mut rx_sim).await {}
        if !rx_sim.neighbors.is_empty() {
            println!("  B connected at tick {i}");
            connected = true;
            break;
        }
    }
    assert!(connected, "B should connect to A");

    println!(
        "✓ B connected. entries={}, pending={:?}",
        rx_sim.entries.len(),
        rx_sim.pending_image
    );

    // Drain stale
    while process_one_event(&net_rx, &mut rx_sim).await {}

    // Sender A: add image and broadcast ImageShare
    println!("\n--- A: adding image and broadcasting ImageShare ---");
    let image_data: Vec<u8> = b"fake-png-bytes-1234567890-abcdef".to_vec();

    #[expect(unused_imports)]
    use iroh_blobs::api::proto::TagInfo;
    let tag_info = blob_store_a
        .blobs()
        .add_bytes(image_data.clone())
        .await
        .map_err(|e| format!("add_bytes error: {e}"))
        .unwrap();
    let blob_hash = tag_info.hash;
    println!("  Blob hash: {}", blob_hash);

    let hash: MessageHash = *blob_hash.as_bytes();
    let msg = Message::ImageShare {
        name: "test-image.png".into(),
        hash,
    };
    let encoded = SignedMessage::sign_and_encode(&sk_a, &msg)
        .map_err(|e| format!("Failed to sign: {e}"))
        .unwrap();
    sender_a.broadcast(encoded).await?;
    println!("  ImageShare broadcast ✓");

    // Wait for B to receive and auto-download
    sleep(Duration::from_secs(3)).await;

    // Process events ONE AT A TIME (like Iced GUI)
    let mut events_processed = 0;
    let mut image_downloaded = false;
    for _ in 0..30 {
        if process_one_event(&net_rx, &mut rx_sim).await {
            events_processed += 1;
            if count_image_entries(&rx_sim.entries) > 0 {
                image_downloaded = true;
                break;
            }
        } else {
            sleep(Duration::from_millis(200)).await;
        }
    }

    // Report results
    println!("\n=== RESULTS ===");
    println!("Events processed: {}", events_processed);
    println!("Total entries: {}", rx_sim.entries.len());
    println!("Image entries: {}", count_image_entries(&rx_sim.entries));
    println!("Log:");
    for line in &rx_sim.log {
        println!("  {line}");
    }

    // Assertions
    assert!(
        rx_sim.pending_image.is_none(),
        "pending_image should be consumed after auto-download"
    );
    assert!(
        image_downloaded,
        "Receiver should have a ChatEntry with image_bytes"
    );
    assert!(
        count_image_entries(&rx_sim.entries) > 0,
        "Receiver entries should contain an image entry"
    );

    // Verify image bytes match original
    let stored_bytes = rx_sim
        .last_downloaded_image
        .as_ref()
        .expect("downloaded image bytes should be stored");
    assert_eq!(
        stored_bytes, &image_data,
        "Downloaded image bytes must match original"
    );
    println!("\n✓✓ RECEIVER DOWNLOADED AND STORED IMAGE CORRECTLY ✓✓");

    // Cleanup
    drop(sender_a);
    drop(_sender_b);
    drop(router_a);
    drop(_router_b);

    Ok(())
}
