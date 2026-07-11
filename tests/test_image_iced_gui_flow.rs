//! Reproduce the exact Iced GUI image send/receive flow.
//!
//! Two peers subscribe to a topic (like two Iced GUI instances).
//! Peer A sends ImageShare via gossip (like /image command).
//! Peer B receives the ImageShare, triggers pending_image, downloads the blob
//! from A, and reads back the bytes — exactly matching IcedChat::update's
//! AppMessage::NetEvent handler in app.rs.
//!
//! Uses room_docs::forward_room_events_for_chat to match the Iced GUI exactly.

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
    download_candidates, handle_net_event, ChatEntry, Message, MessageHash, NetEvent, SignedMessage,
};
use iroh_gossip::friends::FriendId;
use iroh_gossip::net::{Gossip, GOSSIP_ALPN};
use iroh_gossip::proto::TopicId;
use iroh_gossip::room_docs;
use n0_error::Result;
use n0_future::{task, time::sleep};
use rand::{RngExt, SeedableRng};
use tokio::sync::Mutex;

// ── Test peer that mirrors IcedChat field layout ─────────

#[expect(dead_code)]
struct ImageTestPeer {
    local_public: PublicKey,
    entries: Vec<ChatEntry>,
    names: HashMap<PublicKey, String>,
    neighbors: std::collections::HashSet<PublicKey>,
    pending_file: Option<(String, String)>,
    pending_image: Option<(String, MessageHash, PublicKey)>,
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
        self.entries.push(ChatEntry::system(text));
    }
    fn push_remote(
        &mut self,
        label: String,
        text: String,
        _hash: Option<MessageHash>,
        _sent_at: Option<u64>,
    ) {
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
    Ok((
        router,
        ep.clone(),
        ep.secret_key().clone(),
        gossip,
        pk,
        blob_store,
    ))
}

/// Drain all available NetEvents from rx, processing each through
/// handle_net_event (like the Iced GUI does one at a time in update()).
fn drain_events(
    rx: &Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<NetEvent>>>,
    sim: &mut ImageTestPeer,
) -> usize {
    let mut count = 0;
    while let Ok(event) = rx.try_lock().unwrap().try_recv() {
        count += 1;
        let _ = handle_net_event(event, sim);
    }
    count
}

#[tokio::test]
async fn test_iced_gui_image_flow_exact() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(99);

    let (router_a, ep_a, sk_a, gossip_a, pk_a, blob_store_a) =
        spawn_peer_with_blobs(&mut rng).await?;
    let (router_b, ep_b, sk_b, gossip_b, pk_b, blob_store_b) =
        spawn_peer_with_blobs(&mut rng).await?;

    let topic = TopicId::from_bytes(rng.random());

    // ── Peer A: subscribe ──
    let sub_a = gossip_a.subscribe(topic, vec![]).await?;
    let (sender_a, receiver_a) = sub_a.split();
    let (net_tx_a, net_rx_a) = tokio::sync::mpsc::unbounded_channel();
    let net_rx_a = Arc::new(Mutex::new(net_rx_a));
    // Use room_docs forwarder to match Iced GUI exactly
    let metadata_doc_a = room_docs::create_metadata_doc(
        topic,
        &sender_a,
        room_docs::RoomMetadata {
            name: Some("iroh-gossip-chat".to_string()),
            description: None,
            rules: None,
        },
    )
    .await?;
    let roster_doc_a = room_docs::create_roster_doc(
        topic,
        &sender_a,
        sk_a.public().to_string(),
        "Alice".to_string(),
    )
    .await?;
    task::spawn(room_docs::forward_room_events_for_chat(
        metadata_doc_a,
        roster_doc_a,
        receiver_a,
        net_tx_a,
    ));

    // Peer A announces presence
    let about_me = SignedMessage::sign_and_encode(
        &sk_a,
        &Message::AboutMe {
            name: "Alice".into(),
        },
    )
    .unwrap();
    sender_a.broadcast(about_me).await?;

    // ── Peer B: subscribe with A as bootstrap ──
    sleep(Duration::from_millis(200)).await;
    let memory_lookup = MemoryLookup::new();
    if let Ok(addr_lookup) = ep_b.address_lookup() {
        addr_lookup.add(memory_lookup.clone());
    }
    memory_lookup.set_endpoint_info(ep_a.addr());
    let sub_b = gossip_b.subscribe(topic, vec![pk_a]).await?;
    let (sender_b, receiver_b) = sub_b.split();
    let (net_tx_b, net_rx_b) = tokio::sync::mpsc::unbounded_channel();
    let net_rx_b = Arc::new(Mutex::new(net_rx_b));
    let (metadata_doc_b, roster_doc_b) = {
        let md = room_docs::create_metadata_doc(
            topic,
            &sender_b,
            room_docs::RoomMetadata {
                name: Some("iroh-gossip-chat".to_string()),
                description: None,
                rules: None,
            },
        )
        .await?;
        let rd = room_docs::create_roster_doc(
            topic,
            &sender_b,
            sk_b.public().to_string(),
            "Bob".to_string(),
        )
        .await?;
        (md, rd)
    };
    task::spawn(room_docs::forward_room_events_for_chat(
        metadata_doc_b,
        roster_doc_b,
        receiver_b,
        net_tx_b,
    ));

    let about_me_b =
        SignedMessage::sign_and_encode(&sk_b, &Message::AboutMe { name: "Bob".into() }).unwrap();
    sender_b.broadcast(about_me_b).await?;

    // ── Set up test peers ──
    let mut sim_a = ImageTestPeer {
        local_public: pk_a,
        entries: vec![],
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
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
        pending_file: None,
        pending_image: None,
        blob_store: blob_store_b.clone(),
        endpoint: ep_b.clone(),
    };

    // ── Wait for gossip to connect ──
    let mut connected = false;
    for i in 0..60 {
        sleep(Duration::from_millis(200)).await;
        drain_events(&net_rx_a, &mut sim_a);
        drain_events(&net_rx_b, &mut sim_b);
        if !sim_a.neighbors.is_empty() && !sim_b.neighbors.is_empty() {
            println!("  Both connected at tick {i}");
            connected = true;
            break;
        }
    }
    assert!(connected, "Peers should connect");
    println!("✓ Both peers connected");

    // Drain stale events
    drain_events(&net_rx_a, &mut sim_a);
    drain_events(&net_rx_b, &mut sim_b);

    // ── Sender (A) adds image to blob store and broadcasts ImageShare ──
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
    let encoded = SignedMessage::sign_and_encode(&sk_a, &msg).unwrap();
    sender_a.broadcast(encoded).await?;
    println!("  ImageShare broadcast ✓");

    // ── Wait for B to receive ──
    sleep(Duration::from_secs(3)).await;
    let b_count = drain_events(&net_rx_b, &mut sim_b);
    println!("  B drained {b_count} events");

    // ── Check B's pending_image (exactly like Iced GUI does) ──
    let pending = sim_b.pending_image.take();
    assert!(
        pending.is_some(),
        "B should have pending_image after ImageShare. B received: {:?}",
        sim_b
            .entries
            .iter()
            .map(|e| e.body.clone())
            .collect::<Vec<_>>()
    );
    let (img_name, img_hash, sender_pk) = pending.unwrap();
    println!(
        "  B pending_image: name={img_name:?}, sender={}",
        sender_pk.fmt_short()
    );

    // ── Download exactly like Iced GUI's AppMessage::NetEvent handler ──
    println!("\n--- B: downloading blob (exact Iced GUI path) ---");
    let blob_hash_dl: iroh_blobs::Hash = img_hash.into();
    println!("  hash: {blob_hash_dl}");

    let download_result = blob_store_b
        .downloader(&ep_b)
        .download(
            blob_hash_dl,
            download_candidates(sender_pk, &sim_b.neighbors),
        )
        .await;
    assert!(
        download_result.is_ok(),
        "Download should succeed, got: {:?}",
        download_result
    );
    println!("  Download completed ✓");

    // Read back the blob
    use tokio::io::AsyncReadExt;
    let mut reader = blob_store_b.blobs().reader(blob_hash_dl);
    let mut downloaded = Vec::new();
    reader
        .read_to_end(&mut downloaded)
        .await
        .map_err(|e| format!("Read: {e}"))?;
    println!("  Read {} bytes ✓", downloaded.len());

    assert_eq!(
        downloaded, image_data,
        "Downloaded bytes must match original"
    );
    println!("✓ Bytes match!");

    // ── Now also test: use add_path (like the Iced GUI sender does) ──
    // Write a temp file
    let tmp_dir = std::env::temp_dir().join("iced_img_test");
    std::fs::create_dir_all(&tmp_dir).ok();
    let tmp_file = tmp_dir.join("photo.png");
    let real_image_data = b"\x89PNG\r\n\x1a\n...real-png-content...".to_vec();
    std::fs::write(&tmp_file, &real_image_data).unwrap();

    println!("\n--- Testing add_path (like Iced GUI's ExecuteImageSend) ---");
    let tag = blob_store_a
        .blobs()
        .add_path(&tmp_file)
        .await
        .map_err(|e| format!("add_path error: {e}"))
        .unwrap();
    println!("  add_path hash: {}", tag.hash);

    let hash2: MessageHash = *tag.hash.as_bytes();
    let msg2 = Message::ImageShare {
        name: "photo.png".into(),
        hash: hash2,
    };
    let encoded2 = SignedMessage::sign_and_encode(&sk_a, &msg2).unwrap();
    sender_a.broadcast(encoded2).await?;
    println!("  Second ImageShare broadcast ✓");

    sleep(Duration::from_secs(2)).await;
    let b_count2 = drain_events(&net_rx_b, &mut sim_b);
    println!("  B drained {b_count2} events (2nd round)");

    let pending2 = sim_b.pending_image.take();
    assert!(
        pending2.is_some(),
        "B should have pending_image for add_path image"
    );
    let (img_name2, img_hash2, sender_pk2) = pending2.unwrap();
    println!("  B pending_image(2): name={img_name2:?}");

    // Download
    let blob_hash_dl2: iroh_blobs::Hash = img_hash2.into();
    let download_result2 = blob_store_b
        .downloader(&ep_b)
        .download(
            blob_hash_dl2,
            download_candidates(sender_pk2, &sim_b.neighbors),
        )
        .await;
    assert!(download_result2.is_ok(), "2nd download should succeed");
    println!("  2nd Download completed ✓");

    let mut reader2 = blob_store_b.blobs().reader(blob_hash_dl2);
    let mut downloaded2 = Vec::new();
    reader2
        .read_to_end(&mut downloaded2)
        .await
        .map_err(|e| format!("Read: {e}"))?;
    println!("  Read {} bytes ✓", downloaded2.len());

    assert_eq!(
        downloaded2, real_image_data,
        "add_path image bytes must match"
    );
    println!("✓ add_path image bytes match!");

    // Cleanup
    drop(sender_a);
    drop(sender_b);
    drop(router_a);
    drop(router_b);
    std::fs::remove_dir_all(tmp_dir).ok();

    println!("\n✓✓ ICED GUI IMAGE SEND/DOWNLOAD FLOW VERIFIED ✓✓");
    Ok(())
}
