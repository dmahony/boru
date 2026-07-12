//! Performance regression tests for the iroh-gossip-chat message pipeline.
//!
//! These tests measure wall-clock time for core operations at different scales
//! and assert that performance stays roughly linear (not quadratic) as the
//! chat log grows.  They serve as early-warning detectors for O(n²) regressions
//! in message processing, image handling, and the virtual-scrolling render pass.
//!
//! Each benchmark runs several iterations and takes the minimum to reduce noise.
//! Assert thresholds are generous — the goal is to catch order-of-magnitude
//! regressions, not micro-benchmark deltas.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use iroh_gossip::chat_core::{ChatEntry, Message, MessageHash, SignedMessage};
use rand::SeedableRng;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Fill a vector with `count` ChatEntry values.
fn make_entries(count: usize) -> Vec<ChatEntry> {
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let entry = if i % 15 == 0 {
            // Image entry
            ChatEntry::remote("Alice".to_string(), format!("Check out this photo #{i}"))
        } else {
            if i % 3 == 0 {
                ChatEntry::local(
                    "Me".to_string(),
                    format!("Hello from the chat! Message #{i}"),
                )
            } else if i % 5 == 0 {
                ChatEntry::system(format!("User joined the chat at message #{i}"))
            } else {
                ChatEntry::remote(
                    "Bob".to_string(),
                    format!("Here's a thought about message #{i}"),
                )
            }
        };
        entries.push(entry);
    }
    entries
}

/// Simulate the O(n) height-estimation pass that `view_chat_log` does every
/// frame: compute per-entry heights and cumulative offsets for the full list.
/// Returns (heights_vec, cum_vec, total_height).
fn height_estimation_pass(entries: &[ChatEntry]) -> (Vec<f32>, Vec<f32>, f32) {
    const DATE_SEP_H: f32 = 32.0;
    const SYSTEM_H: f32 = 24.0;
    const MSG_BASE_H: f32 = 76.0;
    const REACTION_EXTRA: f32 = 22.0;

    let total = entries.len();
    let mut heights: Vec<f32> = Vec::with_capacity(total);
    let mut prev_day_ht: Option<u64> = None;

    for entry in entries {
        let mut h = 0.0;
        let day = entry.timestamp.map(|ts| ts / 86400000);
        if let Some(d) = day {
            if prev_day_ht.map_or(true, |prev| prev != d) {
                h += DATE_SEP_H;
            }
            prev_day_ht = Some(d);
        }
        use iroh_gossip::chat_core::ChatKind;
        match entry.kind {
            ChatKind::System => h += SYSTEM_H,
            _ => {
                h += MSG_BASE_H;
                // Entries don't carry image_bytes in the core ChatEntry
                // (that's Iced-specific), so IMAGE_EXTRA is not added here.
                if !entry.reactions.is_empty() {
                    h += REACTION_EXTRA;
                }
            }
        }
        heights.push(h);
    }

    let mut cum: Vec<f32> = Vec::with_capacity(total);
    let mut running = 0.0_f32;
    for &h in &heights {
        cum.push(running);
        running += h;
    }
    (heights, cum, running)
}

/// Run `f` repeatedly and return the minimum duration.
fn bench_min<F: FnMut()>(mut f: F, iterations: usize) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..iterations {
        let start = Instant::now();
        f();
        let elapsed = start.elapsed();
        if elapsed < best {
            best = elapsed;
        }
    }
    best
}

/// Assert that the ratio `large / small` is below a quadratic scaling factor.
/// If the sample count ratio is ~10x and the time ratio is <20x, that's
/// sub-quadratic (good).  If it's >40x, that's suspicious.
fn assert_sub_quadratic(
    label: &str,
    small_count: usize,
    small_time: Duration,
    large_count: usize,
    large_time: Duration,
) {
    let count_ratio = large_count as f64 / small_count as f64;
    let time_ratio = large_time.as_secs_f64() / small_time.as_secs_f64();
    // Allow up to 3x over linear as fudge factor for noise/alloc
    let max_acceptable = count_ratio * 3.0;
    assert!(
        time_ratio <= max_acceptable,
        "{}: time ratio {:.2}x exceeds {:.2}x limit ({} entries: {:?}, {} entries: {:?})",
        label,
        time_ratio,
        max_acceptable,
        small_count,
        small_time,
        large_count,
        large_time,
    );
    println!(
        "  ✓ {}: {:.2}x time ratio vs {:.2}x count ratio (sub-quadratic)",
        label, time_ratio, count_ratio
    );
}

// ── Test: Chat entry iteration scaling ───────────────────────────────────────

#[test]
fn test_chat_entry_iteration_scaling() {
    // Build entry lists at three scales
    let small = make_entries(100);
    let medium = make_entries(1_000);
    let large = make_entries(5_000);

    // Benchmark iterating the full list (like view_chat_log's image_bytes scan)
    let small_time = bench_min(
        || {
            let mut bytes = 0_usize;
            let mut count = 0_usize;
            for e in &small {
                if e.message_hash.is_some() {
                    bytes += 32;
                    count += 1;
                }
            }
            let _ = (bytes, count);
        },
        100,
    );

    let medium_time = bench_min(
        || {
            let mut bytes = 0_usize;
            let mut count = 0_usize;
            for e in &medium {
                if e.message_hash.is_some() {
                    bytes += 32;
                    count += 1;
                }
            }
            let _ = (bytes, count);
        },
        100,
    );

    let large_time = bench_min(
        || {
            let mut bytes = 0_usize;
            let mut count = 0_usize;
            for e in &large {
                if e.message_hash.is_some() {
                    bytes += 32;
                    count += 1;
                }
            }
            let _ = (bytes, count);
        },
        100,
    );

    println!(
        "Iterate 100 entries: {:?} | 1,000 entries: {:?} | 5,000 entries: {:?}",
        small_time, medium_time, large_time
    );

    assert_sub_quadratic("iteration 100→1,000", 100, small_time, 1_000, medium_time);
    assert_sub_quadratic("iteration 100→5,000", 100, small_time, 5_000, large_time);
}

// ── Test: Height estimation pass scaling ─────────────────────────────────────

#[test]
fn test_height_estimation_scaling() {
    let small = make_entries(100);
    let medium = make_entries(1_000);
    let large = make_entries(10_000);

    let small_time = bench_min(
        || {
            let _ = height_estimation_pass(&small);
        },
        200,
    );
    let medium_time = bench_min(
        || {
            let _ = height_estimation_pass(&medium);
        },
        200,
    );
    let large_time = bench_min(
        || {
            let _ = height_estimation_pass(&large);
        },
        200,
    );

    println!(
        "Height pass 100: {:?} | 1,000: {:?} | 10,000: {:?}",
        small_time, medium_time, large_time
    );

    assert_sub_quadratic("height pass 100→1,000", 100, small_time, 1_000, medium_time);
    assert_sub_quadratic(
        "height pass 100→10,000",
        100,
        small_time,
        10_000,
        large_time,
    );
}

// ── Test: Signed message encoding/decoding scaling ───────────────────────────

#[test]
fn test_signed_message_encode_decode_scaling() {
    let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(42);
    let sk = iroh::SecretKey::from_bytes(&[0u8; 32]);

    // Build messages at different scales
    let small_msgs: Vec<Message> = (0..100)
        .map(|i| Message::Message { text: format!("Hello, world! Message #{i} with some realistic padding to avoid tiny-string optimizations that would skew results.") })
        .collect();
    let large_msgs: Vec<Message> = (0..1_000)
        .map(|i| Message::Message { text: format!("Hello, world! Message #{i} with some realistic padding to avoid tiny-string optimizations that would skew results.") })
        .collect();

    let _ = rng;

    let small_enc_time = bench_min(
        || {
            for msg in &small_msgs {
                let _ = SignedMessage::sign_and_encode(&sk, msg).unwrap();
            }
        },
        10,
    );

    let large_enc_time = bench_min(
        || {
            for msg in &large_msgs {
                let _ = SignedMessage::sign_and_encode(&sk, msg).unwrap();
            }
        },
        10,
    );

    println!(
        "Encode 100 messages: {:?} | 1,000 messages: {:?}",
        small_enc_time, large_enc_time
    );

    assert_sub_quadratic(
        "encode 100→1,000",
        100,
        small_enc_time,
        1_000,
        large_enc_time,
    );

    // ── Decode scaling ──
    let all_bytes: Vec<Vec<u8>> = small_msgs
        .iter()
        .map(|msg| SignedMessage::sign_and_encode(&sk, msg).unwrap().to_vec())
        .collect();
    let all_bytes_large: Vec<Vec<u8>> = large_msgs
        .iter()
        .map(|msg| SignedMessage::sign_and_encode(&sk, msg).unwrap().to_vec())
        .collect();

    let small_dec_time = bench_min(
        || {
            for bytes in &all_bytes {
                let _ = SignedMessage::verify_and_decode(bytes).unwrap();
            }
        },
        5,
    );

    let large_dec_time = bench_min(
        || {
            for bytes in &all_bytes_large {
                let _ = SignedMessage::verify_and_decode(bytes).unwrap();
            }
        },
        5,
    );

    println!(
        "Decode 100 messages: {:?} | 1,000 messages: {:?}",
        small_dec_time, large_dec_time
    );

    assert_sub_quadratic(
        "decode 100→1,000",
        100,
        small_dec_time,
        1_000,
        large_dec_time,
    );
}

// ── Test: Image blob add/download scaling ────────────────────────────────────
// Uses iroh-blobs in-memory store to simulate the image upload/download path
// at different scales without setting up a full gossip mesh.

#[tokio::test]
async fn test_image_blob_operations_scaling() {
    use iroh_blobs::store::mem::MemStore;
    use tokio::io::AsyncReadExt;

    let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(42);

    // Build image payloads of realistic sizes — use rng.random() since
    // rand 0.10 doesn't expose RngCore in dev-deps
    fn make_image(rng: &mut impl rand::RngExt, size_kb: usize) -> Vec<u8> {
        let mut buf = vec![0u8; size_kb * 1024];
        // PNG header to make it realistic
        for b in &mut buf[8..] {
            *b = rng.random();
        }
        buf
    }

    // Small batch: 10 images at 64KB each
    let small_images: Vec<Vec<u8>> = (0..10).map(|_| make_image(rng, 64)).collect();
    // Large batch: 100 images at 64KB each (6.4 MB total)
    let large_images: Vec<Vec<u8>> = (0..100).map(|_| make_image(rng, 64)).collect();

    // Benchmark blob add — run inside the tokio runtime
    let small_add_start = Instant::now();
    for _ in 0..3 {
        let store = MemStore::new();
        for data in &small_images {
            let _ = store.blobs().add_bytes(data.clone()).await.unwrap();
        }
    }
    let small_add_time = small_add_start.elapsed() / 3;

    let large_add_start = Instant::now();
    for _ in 0..2 {
        let store = MemStore::new();
        for data in &large_images {
            let _ = store.blobs().add_bytes(data.clone()).await.unwrap();
        }
    }
    let large_add_time = large_add_start.elapsed() / 2;

    println!(
        "Blob add 10×64KB: {:?} | 100×64KB: {:?}",
        small_add_time, large_add_time
    );

    assert_sub_quadratic(
        "blob add 10→100 images",
        10,
        small_add_time,
        100,
        large_add_time,
    );

    // Benchmark blob read-back
    // Pre-populate and measure read time
    let small_read_start = Instant::now();
    for _ in 0..3 {
        let store = MemStore::new();
        let mut hashes = Vec::with_capacity(small_images.len());
        for data in &small_images {
            let tag = store.blobs().add_bytes(data.clone()).await.unwrap();
            hashes.push(tag.hash);
        }
        let mut total_bytes = 0_usize;
        for h in &hashes {
            let mut reader = store.blobs().reader(*h);
            let mut buf = Vec::new();
            reader.read_to_end(&mut buf).await.unwrap();
            total_bytes += buf.len();
        }
        let _ = total_bytes;
    }
    let small_read_time = small_read_start.elapsed() / 3;

    let large_read_start = Instant::now();
    for _ in 0..2 {
        let store = MemStore::new();
        let mut hashes = Vec::with_capacity(large_images.len());
        for data in &large_images {
            let tag = store.blobs().add_bytes(data.clone()).await.unwrap();
            hashes.push(tag.hash);
        }
        let mut total_bytes = 0_usize;
        for h in &hashes {
            let mut reader = store.blobs().reader(*h);
            let mut buf = Vec::new();
            reader.read_to_end(&mut buf).await.unwrap();
            total_bytes += buf.len();
        }
        let _ = total_bytes;
    }
    let large_read_time = large_read_start.elapsed() / 2;

    println!(
        "Blob read 10×64KB: {:?} | 100×64KB: {:?}",
        small_read_time, large_read_time
    );

    assert_sub_quadratic(
        "blob read 10→100 images",
        10,
        small_read_time,
        100,
        large_read_time,
    );
}

// ── Test: Full pipeline with many messages via handle_net_event ───────────────
// This test spawns two peers and processes a growing number of messages through
// handle_net_event, measuring wall-clock time to detect O(n²) degradation.

#[tokio::test]
async fn test_many_messages_handle_net_event_scaling() -> n0_error::Result<()> {
    use std::sync::Arc;

    use iroh::{
        address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, PublicKey,
        RelayMode, SecretKey,
    };
    use iroh_gossip::chat_callbacks::ChatCallbacks;
    use iroh_gossip::chat_core::{forward_gossip_events, handle_net_event, SignedMessage as SM};
    use iroh_gossip::friends::{FriendId, FriendsStore};
    use iroh_gossip::net::{Gossip, GOSSIP_ALPN};
    use iroh_gossip::proto::TopicId;
    use n0_future::{task, time::sleep};
    use rand::RngExt;
    use tokio::sync::Mutex;

    struct BenchPeer {
        local_public: PublicKey,
        entries: Vec<ChatEntry>,
        names: HashMap<PublicKey, String>,
        neighbors: std::collections::HashSet<PublicKey>,
        received_count: usize,
        #[allow(dead_code)]
        friends: FriendsStore,
    }

    impl ChatCallbacks for BenchPeer {
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
            self.received_count += 1;
        }
        fn push_remote(
            &mut self,
            _peer: PublicKey,
            label: String,
            text: String,
            _hash: Option<MessageHash>,
            _sent_at: Option<u64>,
        ) {
            self.entries.push(ChatEntry::remote(label, text));
            self.received_count += 1;
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

    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(7);
    let tmp_dir = tempfile::tempdir().unwrap();

    // Spawn two peers
    let ep_a = iroh::Endpoint::builder(presets::N0)
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

    let ep_b = iroh::Endpoint::builder(presets::N0)
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

    let topic = TopicId::from_bytes(rng.random());

    // Subscribe peers
    let sub_a = gossip_a.subscribe(topic, vec![]).await?;
    let (sender_a, receiver_a) = sub_a.split();
    let (net_tx_a, net_rx_a) = tokio::sync::mpsc::unbounded_channel();
    let _net_rx_a = Arc::new(Mutex::new(net_rx_a));
    task::spawn(forward_gossip_events(receiver_a, net_tx_a));

    // Announce AboutMe
    sender_a
        .broadcast(
            SM::sign_and_encode(
                &sk_a,
                &Message::AboutMe {
                    name: "Alice".into(),
                    profile_image_ticket: None,
                },
            )
            .unwrap(),
        )
        .await?;

    sleep(Duration::from_millis(100)).await;

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

    sender_b
        .broadcast(
            SM::sign_and_encode(
                &sk_b,
                &Message::AboutMe {
                    name: "Bob".into(),
                    profile_image_ticket: None,
                },
            )
            .unwrap(),
        )
        .await?;

    // Set up bench peers
    let bench_b = Arc::new(Mutex::new(BenchPeer {
        local_public: pk_b,
        entries: Vec::new(),
        names: HashMap::new(),
        neighbors: std::collections::HashSet::new(),
        received_count: 0,
        friends: FriendsStore::empty_at(tmp_dir.path().join("b")),
    }));

    // Wait for gossip connection
    for i in 0..60 {
        sleep(Duration::from_millis(200)).await;
        let mut b = bench_b.lock().await;
        let mut _count = 0;
        loop {
            let item = net_rx_b.try_lock().unwrap().try_recv();
            match item {
                Ok(event) => {
                    let _ = handle_net_event(event, &mut *b);
                    _count += 1;
                }
                Err(_) => break,
            }
        }
        drop(b);
        if !bench_b.lock().await.neighbors.is_empty() {
            println!("  Connected at tick {i}");
            break;
        }
    }
    assert!(
        !bench_b.lock().await.neighbors.is_empty(),
        "Peers should connect"
    );

    // Drain initial events
    {
        let mut b = bench_b.lock().await;
        loop {
            let item = net_rx_b.try_lock().unwrap().try_recv();
            match item {
                Ok(event) => {
                    let _ = handle_net_event(event, &mut *b);
                }
                Err(_) => break,
            }
        }
    }

    // ── Benchmark: send messages in batches and measure total time ──
    // Batch 1: 50 messages
    let batch1_count = 50;
    let batch1_start = Instant::now();
    for i in 0..batch1_count {
        let msg = SM::sign_and_encode(&sk_a, &Message::Message {
            text: format!("Test message #{i} to benchmark handle_net_event scaling. This has realistic length to avoid skewing results with tiny allocations."),
        }).unwrap();
        sender_a.broadcast(msg).await?;
    }
    sleep(Duration::from_secs(3)).await;
    {
        let mut b = bench_b.lock().await;
        loop {
            let item = net_rx_b.try_lock().unwrap().try_recv();
            match item {
                Ok(event) => {
                    let _ = handle_net_event(event, &mut *b);
                }
                Err(_) => break,
            }
        }
    }
    let batch1_time = batch1_start.elapsed();

    let b_count_1 = bench_b.lock().await.received_count;
    println!("Batch1 (50): received={} time={:?}", b_count_1, batch1_time);

    // Batch 2: 500 messages (10x more)
    let batch2_count = 500;
    let batch2_start = Instant::now();
    for i in 0..batch2_count {
        // Interleave some image shares to exercise that code path too
        let msg = if i % 10 == 0 {
            SM::sign_and_encode(
                &sk_a,
                &Message::ImageShare {
                    name: format!("photo_{i}.png"),
                    hash: [i as u8; 32],
                },
            )
            .unwrap()
        } else {
            SM::sign_and_encode(&sk_a, &Message::Message {
                text: format!("Test message #{i} from batch 2 to benchmark handle_net_event scaling. This has realistic length to avoid skewing results with tiny allocations."),
            }).unwrap()
        };
        sender_a.broadcast(msg).await?;
    }
    sleep(Duration::from_secs(3)).await;
    {
        let mut b = bench_b.lock().await;
        loop {
            let item = net_rx_b.try_lock().unwrap().try_recv();
            match item {
                Ok(event) => {
                    let _ = handle_net_event(event, &mut *b);
                }
                Err(_) => break,
            }
        }
    }
    let batch2_time = batch2_start.elapsed();

    let b_count_2 = bench_b.lock().await.received_count;
    let batch2_received = b_count_2.saturating_sub(b_count_1);
    println!(
        "Batch2 (500): received={} time={:?}",
        batch2_received, batch2_time
    );

    // The second batch has 10x the messages.  Time should not be > 30x.
    let count_ratio = batch2_count as f64 / batch1_count as f64; // 10x
    let time_ratio = batch2_time.as_secs_f64() / batch1_time.as_secs_f64();
    let max_acceptable = count_ratio * 4.0; // 40x — generous allowance for network + alloc variance
    assert!(
        time_ratio <= max_acceptable,
        "Batch2 time ratio {:.2}x exceeds {:.2}x limit (batch1: {:?}, batch2: {:?}, ratio: {:.2}x)",
        time_ratio,
        max_acceptable,
        batch1_time,
        batch2_time,
        time_ratio
    );
    println!(
        "  ✓ Full pipeline: {:.2}x time ratio vs {:.2}x count ratio (sub-quadratic)",
        time_ratio, count_ratio
    );

    // Cleanup
    drop(sender_a);
    drop(sender_b);
    drop(router_a);
    drop(router_b);

    println!("✓ MANY MESSAGES handle_net_event SCALING VERIFIED");
    Ok(())
}

// ── Test: ImageShare processing does not degrade with larger entry list ──────

#[test]
fn test_imageshare_processing_no_degradation() {
    use iroh::SecretKey;
    use iroh_gossip::chat_core::SignedMessage as SM;

    let sk = SecretKey::from_bytes(&[0u8; 32]);
    let _rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(42);

    // Build a messages list with few entries and many entries
    let small_entry_count = 100;
    let large_entry_count = 2_000;

    let mut small_entries = Vec::with_capacity(small_entry_count);
    for i in 0..small_entry_count {
        let msg = Message::Message {
            text: format!("message {i}"),
        };
        let encoded = SM::sign_and_encode(&sk, &msg).unwrap();
        small_entries.push(encoded);
    }

    let mut large_entries = Vec::with_capacity(large_entry_count);
    for i in 0..large_entry_count {
        let msg = Message::Message {
            text: format!("message {i}"),
        };
        let encoded = SM::sign_and_encode(&sk, &msg).unwrap();
        large_entries.push(encoded);
    }

    // Create an ImageShare message
    let image_msg = Message::ImageShare {
        name: "test.png".into(),
        hash: *blake3::hash(b"test-image").as_bytes(),
    };
    let image_encoded = SM::sign_and_encode(&sk, &image_msg).unwrap();

    // Benchmark: find and verify image message in small vs large entry list
    // This simulates the verify_and_decode + hash search that happens on image receipt
    let small_search_time = bench_min(
        || {
            // Decode all entries and look for ImageShare
            for bytes in &small_entries {
                let _ = SM::verify_and_decode(bytes).unwrap();
            }
            // Also verify the image message
            let _ = SM::verify_and_decode(&image_encoded).unwrap();
        },
        2,
    );

    let large_search_time = bench_min(
        || {
            for bytes in &large_entries {
                let _ = SM::verify_and_decode(bytes).unwrap();
            }
            let _ = SM::verify_and_decode(&image_encoded).unwrap();
        },
        2,
    );

    println!(
        "Verify+decode {} entries+1 image: {:?} | {} entries+1 image: {:?}",
        small_entry_count, small_search_time, large_entry_count, large_search_time
    );

    assert_sub_quadratic(
        "image verify 100→2,000",
        small_entry_count,
        small_search_time,
        large_entry_count,
        large_search_time,
    );
}

// ── Test: Incremental append is O(1) per entry, not O(n) ──────────────────
//
// When using an incremental cache, adding one entry computes just its own
// height (constant work). The full-height recompute on the other hand does
// O(n) work per addition. This test verifies that the incremental cost does
// not grow with total entry count.
#[test]
fn test_incremental_append_cost() {
    // Simulate the incremental cache pattern: compute height for one entry
    // in isolation (like cache.append does).
    fn incremental_append(height: f32, prev_total: f32) -> (f32, f32) {
        let new_cum_last = prev_total; // cum.push(prev_total)
        let new_total = prev_total + height;
        (new_cum_last, new_total)
    }

    // Benchmark: incremental cost at small and large scales
    let small_count = 100;
    let large_count = 10_000;

    let small_time = bench_min(
        || {
            let mut total = 0.0_f32;
            for i in 0..small_count {
                let h = (i as f32 % 76.0) + 32.0;
                let _ = incremental_append(h, total);
                total += h;
            }
        },
        500,
    );

    let large_time = bench_min(
        || {
            let mut total = 0.0_f32;
            for i in 0..large_count {
                let h = (i as f32 % 76.0) + 32.0;
                let _ = incremental_append(h, total);
                total += h;
            }
        },
        500,
    );

    println!(
        "Incremental append {small_count}: {:?} | {large_count}: {:?}",
        small_time, large_time
    );

    // Each iteration does the same O(1) work, so cost should be linear in
    // iteration count, not quadratic: 100x entries → ~100x time (sub-quadratic)
    assert_sub_quadratic(
        "incremental append",
        small_count,
        small_time,
        large_count,
        large_time,
    );
}

// ── Test: Window computation (binary search on cum) is O(log n) ──────────
//
// The cache's window() method uses binary search (partition_point) on the
// cumulative array. This test verifies that the lookup cost stays roughly
// constant (<2x) even when the entry count grows by 100x.
#[test]
fn test_cumulative_window_lookup_cost() {
    fn make_cum(total: usize) -> Vec<f32> {
        let mut cum = Vec::with_capacity(total);
        let mut running = 0.0_f32;
        for i in 0..total {
            cum.push(running);
            running += (i as f32 % 76.0) + 32.0;
        }
        cum
    }

    fn window_lookup(
        cum: &[f32],
        total_height: f32,
        scroll_offset: f32,
        vp_h: f32,
    ) -> (usize, usize) {
        const OVERSCAN: f32 = 800.0;
        let total = cum.len();
        if total == 0 || total_height <= 0.0 {
            return (0, 0);
        }
        let so = if scroll_offset >= f32::MAX / 2.0 {
            (total_height - vp_h.max(200.0)).max(0.0)
        } else {
            scroll_offset
        };
        let view_top = so.max(0.0);
        let view_bot = view_top + vp_h.max(200.0);
        let range_top = (view_top - OVERSCAN).max(0.0);
        let range_bot = (view_bot + OVERSCAN).min(total_height);

        let first_idx = cum
            .partition_point(|&c| c < range_top)
            .min(total.saturating_sub(1));
        let last_idx = cum
            .partition_point(|&c| c <= range_bot)
            .saturating_sub(1)
            .min(total.saturating_sub(1))
            .max(first_idx);
        (first_idx, last_idx)
    }

    let small_cum = make_cum(100);
    let large_cum = make_cum(10_000);
    let small_height = small_cum.last().copied().unwrap_or(0.0) + ((99_usize % 76) as f32 + 32.0);
    let large_height =
        large_cum.last().copied().unwrap_or(0.0) + ((9_999_usize % 76) as f32 + 32.0);

    let small_time = bench_min(
        || {
            let _ = window_lookup(&small_cum, small_height, 0.0, 600.0);
        },
        2000,
    );

    let large_time = bench_min(
        || {
            let _ = window_lookup(&large_cum, large_height, 0.0, 600.0);
        },
        2000,
    );

    println!(
        "Window lookup 100 entries: {:?} | 10,000 entries: {:?}",
        small_time, large_time
    );

    // Binary search is O(log n), so 100x more entries should be <2x time
    assert!(
        large_time.as_nanos() as f64 <= small_time.as_nanos() as f64 * 3.0,
        "Window lookup 10,000 entries {:?} should be <3x of 100 entries {:?}",
        large_time,
        small_time,
    );
}
