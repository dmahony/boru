//! KLIPY-07 regression tests: user-uploaded GIF / animated WebP / MP4
//! attachments must keep flowing through Boru's existing encrypted
//! attachment pipeline exactly as before.
//!
//! What is verified here:
//!   1. A real animated GIF sent via `Message::ImageShare` between two
//!      localhost peers round-trips byte-for-byte (the encrypted attachment
//!      pipeline — no GIF conversion, no provider involvement).
//!   2. A PNG uses the same pipeline and round-trips unchanged (other file
//!      types are unaffected).
//!   3. The wire form of `Message::ImageShare` contains only `name`/`hash` —
//!      no provider, URL, or KLIPY fields (user-uploaded GIFs can
//!      never become provider-GIF messages).
//!   4. A `.mp4`-named `FileShare` download emits `TransferProgress::Started`
//!      and `Completed` events (progress/retry path still works).
//!   5. `download_blob_with_safety` still enforces attachment permissions
//!      (a `PublicRoomSafety` blob-size cap rejects an oversized GIF).
//!   6. `ImageStore` still stores a `.gif` under the `gif` extension.
//!
//! None of these tests contact any external GIF provider (KLIPY) — the
//! only network is two localhost iroh peers on a memory lookup.

use std::{collections::HashMap, sync::Arc, time::Duration};

use boru_core::{
    chat_callbacks::{ChatCallbacks, TransferKind, TransferProgress},
    chat_core::{
        download_blob_to_file, download_blob_with_safety, download_candidates,
        forward_gossip_events, handle_net_event, Message, MessageHash, NetEvent, SignedMessage,
    },
    friends::FriendId,
    image_store::ImageStore,
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    public_room_config::PublicRoomConfig,
    public_room_safety::PublicRoomSafety,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, PublicKey,
    RelayMode, SecretKey,
};
use iroh_blobs::{store::mem::MemStore, BlobsProtocol};
use n0_error::Result;
use n0_future::{task, time::sleep};
use tokio::sync::Mutex;

// ── Tiny animation/image fixtures built in-test (no external files) ────

/// Build a real 3-frame animated GIF (4×4, red/green/blue frames) using the
/// `image` crate encoder — the same decoder/encoder family the app uses.
fn make_animated_gif() -> Vec<u8> {
    use image::{
        codecs::gif::{GifEncoder, Repeat},
        Delay, Frame, RgbaImage,
    };
    let mut bytes = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut bytes);
        enc.set_repeat(Repeat::Infinite).unwrap();
        for i in 0..3u8 {
            let img = RgbaImage::from_pixel(4, 4, image::Rgba([i * 40, 0, 0, 255]));
            let frame = Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(1, 10));
            enc.encode_frame(frame).unwrap();
        }
    }
    bytes
}

/// Build a small static PNG (4×4).
fn make_png() -> Vec<u8> {
    use image::ImageEncoder;
    let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([10, 20, 30, 255]));
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(img.as_raw(), 4, 4, image::ExtendedColorType::Rgba8)
        .unwrap();
    bytes
}

// ── Two-peer harness (mirrors test_image_send_download.rs) ─────────────

#[expect(dead_code)]
struct TestPeer {
    router: Router,
    endpoint: iroh::Endpoint,
    secret_key: SecretKey,
    gossip: Gossip,
    public_key: PublicKey,
    blobs: Arc<iroh_blobs::api::Store>,
}

async fn spawn_peer(seed: u8) -> Result<TestPeer> {
    let ep = iroh::Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::from_bytes(&[seed; 32]))
        .address_lookup(MemoryLookup::new())
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    let pk = ep.secret_key().public();
    let gossip = Gossip::builder().spawn(ep.clone());
    let blobs: Arc<iroh_blobs::api::Store> = Arc::new(MemStore::new().into());
    let router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .accept(iroh_blobs::ALPN, BlobsProtocol::new(&blobs, None))
        .spawn();
    Ok(TestPeer {
        router,
        endpoint: ep.clone(),
        secret_key: ep.secret_key().clone(),
        gossip,
        public_key: pk,
        blobs,
    })
}

/// ChatCallbacks stub that records pending image/file downloads like the
/// Iced GUI does.
struct RecordPeer {
    local_public: PublicKey,
    names: HashMap<PublicKey, String>,
    neighbors: std::collections::HashSet<PublicKey>,
    pending_file: Option<(String, String)>,
    pending_image: Option<(String, MessageHash, PublicKey)>,
    received: Vec<String>,
}

impl ChatCallbacks for RecordPeer {
    fn local_public(&self) -> PublicKey {
        self.local_public
    }
    fn set_name(&mut self, peer: PublicKey, name: String) -> Option<String> {
        self.names.insert(peer, name)
    }
    fn is_friend(&self, peer: &PublicKey) -> bool {
        // The receiver treats the sender as an accepted friend, matching the
        // GUI's ImageShare authorisation gate (chat_core.rs:2039:
        // `is_friend || accepts_group_peer`). KLIPY-07 tests the attachment
        // pipeline, not the friendship model.
        *peer != self.local_public
    }
    fn friend_mark_online(&mut self, _fid: FriendId) {}
    fn friend_mark_offline(&mut self, _fid: FriendId) {}
    fn friend_set_name(&mut self, _fid: FriendId, _name: String) {}
    fn mark_friends_dirty(&mut self) {}
    fn push_system(&mut self, text: String) {
        self.received.push(format!("[sys] {text}"));
    }
    fn push_remote(
        &mut self,
        _peer: PublicKey,
        label: String,
        text: String,
        _hash: Option<MessageHash>,
        _sent_at: Option<u64>,
    ) {
        self.received.push(format!("[{label}] {text}"));
    }
    fn set_pending_file(
        &mut self,
        name: String,
        ticket: String,
        _size: u64,
        _thumbnail: Option<MessageHash>,
        _sender_label: Option<String>,
    ) {
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

fn drain_net(
    rx: &Arc<Mutex<tokio::sync::mpsc::Receiver<NetEvent>>>,
    sim: &mut RecordPeer,
) -> usize {
    let mut count = 0;
    while let Ok(event) = rx.try_lock().unwrap().try_recv() {
        count += 1;
        let _ = handle_net_event(event, sim);
    }
    count
}

/// Connect two peers on a fresh topic and wait for the gossip mesh.
///
/// Returns (peer_a, peer_b, topic, sender_a, sender_b, rx_a, rx_b) — the
/// senders stay alive so tests can broadcast; the receivers feed
/// `handle_net_event` via `drain_net`.
#[allow(clippy::type_complexity)]
async fn connect_two_peers() -> Result<(
    TestPeer,
    TestPeer,
    TopicId,
    boru_core::api::GossipSender,
    boru_core::api::GossipSender,
    Arc<Mutex<tokio::sync::mpsc::Receiver<NetEvent>>>,
    Arc<Mutex<tokio::sync::mpsc::Receiver<NetEvent>>>,
)> {
    let peer_a = spawn_peer(0xA1).await?;
    let peer_b = spawn_peer(0xB2).await?;

    // Route A's endpoint address to B through a shared memory lookup
    // (same pattern as test_image_send_download.rs).
    let memory_lookup = MemoryLookup::new();
    if let Ok(addr_lookup) = peer_b.endpoint.address_lookup() {
        addr_lookup.add(memory_lookup.clone());
    }
    memory_lookup.set_endpoint_info(peer_a.endpoint.addr());

    let topic = TopicId::from_bytes([0x4Bu8; 32]);
    let sub_a = peer_a.gossip.subscribe(topic, vec![]).await?;
    let (sender_a, receiver_a) = sub_a.split();
    let (net_tx_a, net_rx_a) = tokio::sync::mpsc::channel(64);
    let net_rx_a = Arc::new(Mutex::new(net_rx_a));
    task::spawn(forward_gossip_events(receiver_a, net_tx_a));
    let about_a = SignedMessage::sign_and_encode(
        &peer_a.secret_key,
        &Message::AboutMe {
            name: "A".into(),
            profile_image_ticket: None,
        },
    )
    .unwrap();
    sender_a.broadcast(about_a).await?;

    let sub_b = peer_b
        .gossip
        .subscribe(topic, vec![peer_a.public_key])
        .await?;
    let (sender_b, receiver_b) = sub_b.split();
    let (net_tx_b, net_rx_b) = tokio::sync::mpsc::channel(64);
    let net_rx_b = Arc::new(Mutex::new(net_rx_b));
    task::spawn(forward_gossip_events(receiver_b, net_tx_b));
    let about_b = SignedMessage::sign_and_encode(
        &peer_b.secret_key,
        &Message::AboutMe {
            name: "B".into(),
            profile_image_ticket: None,
        },
    )
    .unwrap();
    sender_b.broadcast(about_b).await?;

    let mut sim_a = RecordPeer {
        local_public: peer_a.public_key,
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        pending_file: None,
        pending_image: None,
        received: vec![],
    };
    let mut sim_b = RecordPeer {
        local_public: peer_b.public_key,
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        pending_file: None,
        pending_image: None,
        received: vec![],
    };

    let mut connected = false;
    for _ in 0..60 {
        sleep(Duration::from_millis(200)).await;
        drain_net(&net_rx_a, &mut sim_a);
        drain_net(&net_rx_b, &mut sim_b);
        if !sim_a.neighbors.is_empty() && !sim_b.neighbors.is_empty() {
            connected = true;
            break;
        }
    }
    assert!(connected, "peers should connect over gossip");
    drain_net(&net_rx_a, &mut sim_a);
    drain_net(&net_rx_b, &mut sim_b);

    Ok((
        peer_a, peer_b, topic, sender_a, sender_b, net_rx_a, net_rx_b,
    ))
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

/// A user-selected animated GIF flows through the encrypted ImageShare
/// attachment pipeline byte-for-byte (exact app flow: add_bytes → ImageShare
/// → gossip → download_blob_with_safety). No conversion, no provider.
#[tokio::test]
async fn gif_attachment_roundtrip_preserves_bytes() -> Result<()> {
    let (peer_a, peer_b, _topic, sender_a, _sender_b, _rx_a, rx_b) = connect_two_peers().await?;
    let gif = make_animated_gif();
    assert!(gif.len() > 16, "fixture GIF should be non-trivial");

    // ── Sender: add GIF bytes and broadcast ImageShare (ExecuteImageSend
    //    does exactly this; the GIF bytes are NOT converted).
    let tag = peer_a
        .blobs
        .blobs()
        .add_bytes(gif.clone())
        .await
        .map_err(|e| format!("add_bytes: {e}"))?;
    let hash: MessageHash = *tag.hash.as_bytes();
    let msg = Message::ImageShare {
        name: "anim.gif".into(),
        hash,
    };
    let encoded = SignedMessage::sign_and_encode(&peer_a.secret_key, &msg).unwrap();
    sender_a.broadcast(encoded).await?;

    // ── Receiver: wait for pending_image.
    let mut sim_b = RecordPeer {
        local_public: peer_b.public_key,
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        pending_file: None,
        pending_image: None,
        received: vec![],
    };
    let mut found = false;
    for _ in 0..30 {
        sleep(Duration::from_millis(200)).await;
        drain_net(&rx_b, &mut sim_b);
        if sim_b.pending_image.is_some() {
            found = true;
            break;
        }
    }
    assert!(found, "receiver should get pending_image for the GIF");
    let (name, img_hash, sender_pk) = sim_b.pending_image.take().unwrap();
    assert_eq!(name, "anim.gif");
    assert_eq!(img_hash, hash);
    assert_eq!(sender_pk, peer_a.public_key);

    // ── Download using the exact GUI path (download_blob_with_safety).
    let candidates = download_candidates(sender_pk, &sim_b.neighbors);
    let downloaded = download_blob_with_safety(
        &peer_b.blobs,
        &peer_b.endpoint,
        img_hash.into(),
        candidates,
        name.clone(),
        TransferKind::Image,
        |_| {},
        None,
        sender_pk,
    )
    .await
    .map_err(|e| format!("GIF download failed: {e}"))?;

    assert_eq!(
        downloaded, gif,
        "user-uploaded GIF must round-trip byte-for-byte through the encrypted attachment pipeline"
    );
    // The receiver-side renderer (decode_gif_frames) reads these same bytes.
    use image::AnimationDecoder;
    let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&downloaded)).unwrap();
    let frame_count = decoder.into_frames().count();
    assert!(
        frame_count > 1,
        "downloaded GIF should still be a multi-frame animated GIF"
    );
    println!(
        "✓ GIF ({}) round-tripped byte-for-byte; {frame_count} animation frames preserved",
        downloaded.len()
    );
    Ok(())
}

/// PNG (a non-animation image) uses the same attachment pipeline and is
/// unaffected by the GIF special-casing.
#[tokio::test]
async fn png_attachment_roundtrip_preserves_bytes() -> Result<()> {
    let (peer_a, peer_b, _topic, sender_a, _sender_b, _rx_a, rx_b) = connect_two_peers().await?;
    let png = make_png();

    let tag = peer_a
        .blobs
        .blobs()
        .add_bytes(png.clone())
        .await
        .map_err(|e| format!("add_bytes: {e}"))?;
    let hash: MessageHash = *tag.hash.as_bytes();
    let msg = Message::ImageShare {
        name: "photo.png".into(),
        hash,
    };
    let encoded = SignedMessage::sign_and_encode(&peer_a.secret_key, &msg).unwrap();
    sender_a.broadcast(encoded).await?;

    let mut sim_b = RecordPeer {
        local_public: peer_b.public_key,
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        pending_file: None,
        pending_image: None,
        received: vec![],
    };
    let mut found = false;
    for _ in 0..30 {
        sleep(Duration::from_millis(200)).await;
        drain_net(&rx_b, &mut sim_b);
        if sim_b.pending_image.is_some() {
            found = true;
            break;
        }
    }
    assert!(found, "receiver should get pending_image for the PNG");
    let (name, img_hash, sender_pk) = sim_b.pending_image.take().unwrap();
    assert_eq!(name, "photo.png");

    let candidates = download_candidates(sender_pk, &sim_b.neighbors);
    let downloaded = download_blob_with_safety(
        &peer_b.blobs,
        &peer_b.endpoint,
        img_hash.into(),
        candidates,
        name.clone(),
        TransferKind::Image,
        |_| {},
        None,
        sender_pk,
    )
    .await
    .map_err(|e| format!("PNG download failed: {e}"))?;

    assert_eq!(
        downloaded, png,
        "PNG must be unaffected by the GIF byte-for-byte branch"
    );
    println!(
        "✓ PNG ({}) round-tripped byte-for-byte via ImageShare",
        downloaded.len()
    );
    Ok(())
}

/// The wire form of `Message::ImageShare` carries exactly `name` + `hash` —
/// no provider, URL, or KLIPY metadata. User-uploaded GIFs therefore
/// cannot be turned into provider-GIF messages.
#[test]
fn image_share_wire_message_has_no_provider_fields() {
    let msg = Message::ImageShare {
        name: "anim.gif".into(),
        hash: [7u8; 32],
    };
    let json = serde_json::to_value(&msg).unwrap();
    let obj = json.get("ImageShare").and_then(|v| v.as_object());
    let obj = obj.expect("ImageShare variant should serialize as an object");
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["hash", "name"],
        "ImageShare must carry only name+hash"
    );
    let raw = serde_json::to_string(&msg).unwrap().to_lowercase();
    for forbidden in ["provider", "klipy", "url", "external"] {
        assert!(
            !raw.contains(forbidden),
            "ImageShare wire message must not contain {forbidden}"
        );
    }
}

/// A `.mp4`-named FileShare still downloads through `download_blob_to_file`
/// with progress events (Started → Completed), proving the file-transfer
/// progress/retry path is untouched.
#[tokio::test]
async fn mp4_file_share_progress_emits_started_and_completed() -> Result<()> {
    let (peer_a, peer_b, _topic, sender_a, _sender_b, _rx_a, _rx_b) = connect_two_peers().await?;
    let mp4_bytes: Vec<u8> = b"fake-mp4-container-bytes-for-progress-test".to_vec();

    let tag = peer_a
        .blobs
        .blobs()
        .add_bytes(mp4_bytes.clone())
        .await
        .map_err(|e| format!("add_bytes: {e}"))?;
    let hash = tag.hash;

    // Build the same BlobTicket the GUI's ExecuteFileSend constructs.
    let addr = peer_a.endpoint.addr();
    let ticket =
        iroh_blobs::ticket::BlobTicket::new(addr, hash, iroh_blobs::BlobFormat::Raw).to_string();
    let msg = Message::FileShare {
        name: "movie.mp4".into(),
        ticket: ticket.clone(),
        size: mp4_bytes.len() as u64,
        thumbnail_hash: None,
        collection_hash: None,
        collection_entries: 0,
    };
    let encoded = SignedMessage::sign_and_encode(&peer_a.secret_key, &msg).unwrap();
    sender_a.broadcast(encoded).await?;

    // Receiver records the pending file (like set_pending_file in the GUI).
    let mut sim_b = RecordPeer {
        local_public: peer_b.public_key,
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        pending_file: None,
        pending_image: None,
        received: vec![],
    };
    let mut found = false;
    for _ in 0..30 {
        sleep(Duration::from_millis(200)).await;
        drain_net(&_rx_b, &mut sim_b);
        if sim_b.pending_file.is_some() {
            found = true;
            break;
        }
    }
    assert!(found, "receiver should get pending_file for the MP4");
    let (fname, f_ticket) = sim_b.pending_file.take().unwrap();
    assert_eq!(fname, "movie.mp4");
    assert_eq!(f_ticket, ticket);

    // Download with the exact GUI path (download_blob_to_file) and collect
    // progress events.
    let parsed: iroh_blobs::ticket::BlobTicket = f_ticket.parse().unwrap();
    let (t_addr, t_hash, _fmt) = parsed.into_parts();
    let candidates = download_candidates(t_addr.id, &sim_b.neighbors);
    let dir = tempfile::tempdir().unwrap();
    // BORU-AUDIT-21: reserve the destination atomically (O_EXCL) instead of
    // checking a path and reopening it later.
    let mut destination = match boru_core::safe_destination::reserve_download_destination(
        dir.path(),
        "movie.mp4",
        "download",
        boru_core::safe_destination::OverwritePolicy::KeepBoth,
    )
    .unwrap()
    {
        boru_core::safe_destination::Reservation::Use(dest) => dest,
        boru_core::safe_destination::Reservation::Skip => {
            panic!("fresh temp dir must not skip");
        }
    };
    let save_path = destination.final_path().to_path_buf();

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_cb = events.clone();
    download_blob_to_file(
        &peer_b.blobs,
        &peer_b.endpoint,
        t_hash,
        candidates,
        fname.clone(),
        TransferKind::Video,
        &mut destination,
        None,
        move |ev| {
            if let Ok(mut guard) = events_cb.try_lock() {
                guard.push(ev);
            }
        },
        None,
    )
    .await
    .map_err(|e| format!("mp4 download failed: {e}"))?;
    destination.publish().unwrap();

    let events = events.lock().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, TransferProgress::Started { .. })),
        "expected a Started progress event, got: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, TransferProgress::Completed { .. })),
        "expected a Completed progress event, got: {events:?}"
    );
    assert_eq!(
        std::fs::read(&save_path).unwrap(),
        mp4_bytes,
        "downloaded .mp4 bytes must match the original"
    );
    println!(
        "✓ MP4 FileShare downloaded with {} progress events (Started → Completed)",
        events.len()
    );
    Ok(())
}

/// Attachment permissions stay enforced for GIF downloads: a
/// `PublicRoomSafety` blob-size cap rejects an oversized GIF exactly like any
/// other blob.
#[tokio::test]
async fn gif_download_permissions_enforced_by_safety_size_cap() -> Result<()> {
    let (peer_a, peer_b, _topic, _sender_a, _sender_b, _rx_a, _rx_b) = connect_two_peers().await?;
    let gif = make_animated_gif();
    assert!(gif.len() > 8, "fixture must exceed the tiny cap");

    let tag = peer_a
        .blobs
        .blobs()
        .add_bytes(gif.clone())
        .await
        .map_err(|e| format!("add_bytes: {e}"))?;
    let hash = tag.hash;

    // Public room safety with a deliberately tiny blob cap (8 bytes).
    let mut cfg = PublicRoomConfig::default();
    cfg.max_blob_size_bytes = 8;
    let safety = PublicRoomSafety::new(cfg);

    let result = download_blob_with_safety(
        &peer_b.blobs,
        &peer_b.endpoint,
        hash,
        vec![peer_a.public_key],
        "anim.gif".to_string(),
        TransferKind::Image,
        |_| {},
        Some(&safety),
        peer_a.public_key,
    )
    .await;

    assert!(
        result.is_err(),
        "oversized GIF download must be rejected by the safety blob-size cap"
    );
    let err = format!("{}", result.unwrap_err());
    assert!(
        err.contains("exceeds size limit") || err.contains("too large") || err.contains("limit"),
        "error should mention the size limit: {err}"
    );
    println!("✓ Oversized GIF rejected by safety cap: {err}");
    Ok(())
}

/// ImageStore storage behaviour for `.gif` is unchanged: the gif extension is
/// preserved and the bytes are stored content-addressed under the user dir.
#[test]
fn image_store_saves_gif_with_gif_extension() {
    let dir = tempfile::tempdir().unwrap();
    let store = ImageStore::at(dir.path());
    let gif = make_animated_gif();

    let id = store.save_image("alice", "anim.gif", &gif).unwrap();
    assert!(id.ends_with(".gif"), "identifier must keep gif ext: {id}");
    let abs = store.resolve_absolute_path("alice", &id).unwrap();
    assert!(abs.is_file());
    assert_eq!(std::fs::read(&abs).unwrap(), gif);
    println!("✓ ImageStore preserved .gif extension: {id}");
}
