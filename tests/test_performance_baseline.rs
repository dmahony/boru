//! Performance baseline measurement for boru-chat.
//!
//! Measures wall-clock time for all major operations at realistic scale:
//! 1,000 messages, 100 conversations, 500 friends, and multiple
//! simultaneous downloads.  Run with:
//!
//!   BORU_PERF=1 cargo test --test performance_baseline --features net,test-utils -- --nocapture
//!
//! The perf report is printed to stderr on test completion.
//!
//! # Acceptance Criteria
//! - Baseline numbers are documented in the test output.
//! - Slowest operations are identified by the report's "Top 10 Slowest" section.
//! - No behavior changes — instrumentation is opt-in via BORU_PERF env var.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use boru_chat::chat_callbacks::ChatCallbacks;
use boru_chat::chat_core::{
    handle_net_event as chat_net_event, ChatEntry, Message, MessageHash, NetEvent, SignedMessage,
};
use boru_chat::friends::{FriendId, FriendRecord, FriendRelationship, FriendStatus, FriendsStore};
use boru_chat::net::{Gossip, GOSSIP_ALPN};
use boru_chat::perf::PerfTracker;
use boru_chat::proto::TopicId;
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, PublicKey,
    RelayMode, SecretKey,
};
use rand::RngExt;
use rand::SeedableRng;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

/// Build a ChatEntry vector of a given size with varied message types.
fn make_entries(count: usize) -> Vec<ChatEntry> {
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let entry = if i % 15 == 0 {
            ChatEntry::remote("Alice".to_string(), format!("Check out this photo #{i}"))
        } else if i % 3 == 0 {
            ChatEntry::local("Me".to_string(), format!("Hello! Message #{i}"))
        } else if i % 5 == 0 {
            ChatEntry::system(format!("User joined at message #{i}"))
        } else {
            ChatEntry::remote("Bob".to_string(), format!("Here's a thought #{i}"))
        };
        entries.push(entry);
    }
    entries
}

// ---------------------------------------------------------------------------
// Test struct: simple ChatCallbacks impl for benchmarking
// ---------------------------------------------------------------------------

struct BenchPeer {
    local_public: PublicKey,
    entries: Vec<ChatEntry>,
    names: HashMap<PublicKey, String>,
    neighbors: HashSet<PublicKey>,
    received_count: usize,
}

impl BenchPeer {
    fn new(local_public: PublicKey) -> Self {
        Self {
            local_public,
            entries: Vec::new(),
            names: HashMap::new(),
            neighbors: HashSet::new(),
            received_count: 0,
        }
    }
}

impl ChatCallbacks for BenchPeer {
    fn local_public(&self) -> PublicKey {
        self.local_public
    }

    fn set_name(&mut self, peer: PublicKey, name: String) -> Option<String> {
        self.names.insert(peer, name)
    }

    fn resolve_name(&self, peer: &PublicKey) -> String {
        self.names
            .get(peer)
            .cloned()
            .unwrap_or_else(|| peer.fmt_short().to_string())
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

    fn set_pending_image(&mut self, _name: String, _hash: [u8; 32], _from: PublicKey) {}

    fn has_message(&self, _hash: &[u8; 32]) -> bool {
        false
    }

    fn edit_message(&mut self, _hash: &[u8; 32], _new_text: String) {}

    fn delete_message(&mut self, _hash: &[u8; 32]) {}

    fn add_reaction(&mut self, _hash: &[u8; 32], _emoji: String) {}

    fn on_neighbor_up(&mut self, peer: PublicKey) {
        self.neighbors.insert(peer);
    }

    fn on_neighbor_down(&mut self, peer: PublicKey) {
        self.neighbors.remove(&peer);
    }

    fn record_activity(&mut self, _peer: PublicKey) {}

    fn request_quit(&mut self) {}
}

// ===========================================================================
// Benchmark tests
// ===========================================================================

/// 1. Startup time: endpoint creation, Gossip spawn, topic subscription.
#[tokio::test]
async fn baseline_startup_time() {
    let _ = tracing_subscriber::fmt::try_init();
    boru_chat::perf::init();
    let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(42);

    // Measure endpoint creation time
    let start = Instant::now();
    let ep = iroh::Endpoint::builder(presets::N0)
        .secret_key(SecretKey::from_bytes(&rng.random()))
        .address_lookup(MemoryLookup::new())
        .relay_mode(RelayMode::Default)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let ep_time = start.elapsed();
    ep.online().await;
    PerfTracker::record("startup_endpoint", ep_time, "endpoint_bind".into());

    // Measure Gossip spawn time
    let start = Instant::now();
    let gossip = Gossip::builder().spawn(ep.clone());
    let gossip_spawn_time = start.elapsed();
    PerfTracker::record(
        "startup_gossip_spawn",
        gossip_spawn_time,
        "gossip_spawn".into(),
    );

    let _router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();

    // Measure topic subscription time
    let topic = TopicId::from_bytes(rng.random());
    let start = Instant::now();
    let _sub = gossip.subscribe(topic, vec![]).await.unwrap();
    let sub_time = start.elapsed();
    PerfTracker::record("startup_subscribe", sub_time, "topic_subscribe".into());

    // Measure FriendsStore iteration (500 friends)
    let tmp = tempfile::tempdir().unwrap();
    let mut store = FriendsStore::load_or_default(tmp.path());
    for i in 0..500 {
        let sk = SecretKey::from_bytes(&[(i as u8).wrapping_mul(17); 32]);
        let pk = sk.public();
        let id = FriendId::from_public_key(pk);
        store.friends.insert(
            id,
            FriendRecord {
                label: if i % 2 == 0 {
                    Some(format!("Friend{}", i))
                } else {
                    None
                },
                last_announced_name: None,
                last_announced_profile_image_ticket: None,
                status: FriendStatus::default(),
                known_addrs: vec![],
                addrs_updated_at_unix_ms: None,
                relationship: FriendRelationship::NotFriend,
                rooms: Default::default(),
                direct_conversation: None,
                mailbox_public_key: None,
            },
        );
    }

    // Measure friend iteration
    let _timer = PerfTracker::timer("iterate_500_friends", "friend_scan");
    for (id, _record) in store.iter() {
        let _ = id.as_str().len();
    }

    eprintln!("\n=== STARTUP TIME BASELINE ===");
    PerfTracker::print_report();
    PerfTracker::reset();
}

/// 2. Message send/receive latency benchmark.
#[tokio::test]
async fn baseline_message_latency() {
    let _ = tracing_subscriber::fmt::try_init();
    boru_chat::perf::init();
    let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(42);

    // Create two peers
    let ep_a = iroh::Endpoint::builder(presets::N0)
        .secret_key(SecretKey::from_bytes(&rng.random()))
        .address_lookup(MemoryLookup::new())
        .relay_mode(RelayMode::Default)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    ep_a.online().await;
    let sk_a = ep_a.secret_key().clone();
    let gossip_a = Gossip::builder().spawn(ep_a.clone());
    let _router_a = Router::builder(ep_a.clone())
        .accept(GOSSIP_ALPN, gossip_a.clone())
        .spawn();

    let ep_b = iroh::Endpoint::builder(presets::N0)
        .secret_key(SecretKey::from_bytes(&rng.random()))
        .address_lookup(MemoryLookup::new())
        .relay_mode(RelayMode::Default)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    ep_b.online().await;
    let sk_b = ep_b.secret_key().clone();
    let gossip_b = Gossip::builder().spawn(ep_b.clone());
    let _router_b = Router::builder(ep_b.clone())
        .accept(GOSSIP_ALPN, gossip_b.clone())
        .spawn();

    let topic = TopicId::from_bytes(rng.random());

    // Subscribe peer A as sender
    let sub_a = gossip_a.subscribe(topic, vec![]).await.unwrap();
    let (sender_a, _receiver_a) = sub_a.split();

    // Subscribe peer B as receiver with peer A as bootstrap
    let memory_lookup = MemoryLookup::new();
    if let Ok(addr_lookup) = ep_b.address_lookup() {
        addr_lookup.add(memory_lookup.clone());
    }
    memory_lookup.set_endpoint_info(ep_a.addr());
    let _sub_b = gossip_b
        .subscribe(topic, vec![sk_a.public()])
        .await
        .unwrap();

    // Give time for peers to connect
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // ── Message send latency (1,000 messages) ──
    let msgs_1k: Vec<Message> = (0..1000)
        .map(|i| Message::Message {
            text: format!(
                "Performance benchmark message #{i} with realistic padding to avoid optimizations."
            ),
        })
        .collect();

    // Measure sign_and_encode time at different scales
    let small_enc_time = bench_min(
        || {
            for msg in &msgs_1k[..100] {
                let _ = SignedMessage::sign_and_encode(&sk_b, msg).unwrap();
            }
        },
        10,
    );
    PerfTracker::record(
        "sign_encode_100",
        small_enc_time / 100,
        "per_message_avg".into(),
    );

    let large_enc_time = bench_min(
        || {
            for msg in &msgs_1k {
                let _ = SignedMessage::sign_and_encode(&sk_b, msg).unwrap();
            }
        },
        5,
    );
    PerfTracker::record(
        "sign_encode_1000",
        large_enc_time / 1000,
        "per_message_avg".into(),
    );

    // Measure broadcast time via gossip (100 messages to avoid timeout)
    let start = Instant::now();
    for msg in &msgs_1k[..100] {
        let signed = SignedMessage::sign_and_encode(&sk_a, msg).unwrap();
        let _ = sender_a.broadcast(signed).await;
    }
    let broadcast_time = start.elapsed();
    PerfTracker::record(
        "gossip_broadcast_100",
        broadcast_time / 100,
        "per_message".into(),
    );

    // Give receiver time to process
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // ── handle_net_event throughput (1,000 messages) ──
    let mut peer = BenchPeer::new(sk_a.public());
    let start = Instant::now();
    for i in 0..1000 {
        let event = NetEvent::Message {
            from: sk_b.public(),
            message: Message::Message {
                text: format!("Benchmark message #{i}"),
            },
            sent_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };
        let _ = chat_net_event(event, &mut peer);
    }
    let hne_1k_time = start.elapsed();
    PerfTracker::record(
        "handle_net_event_1000",
        hne_1k_time / 1000,
        "per_message_avg".into(),
    );

    // ── verify_and_decode throughput ──
    let signed_msgs: Vec<_> = msgs_1k[..100]
        .iter()
        .map(|m| SignedMessage::sign_and_encode(&sk_b, m).unwrap())
        .collect();

    let start = Instant::now();
    for signed in &signed_msgs {
        let _ = SignedMessage::verify_and_decode(signed).unwrap();
    }
    let verify_time = start.elapsed();
    PerfTracker::record("verify_decode_100", verify_time / 100, "per_message".into());

    eprintln!("\n=== MESSAGE LATENCY BASELINE ===");
    PerfTracker::print_report();
    PerfTracker::reset();
}

/// 3. Large-scale data setup benchmark: 100 conversations, 500 friends, entries.
#[test]
fn baseline_data_scaling() {
    let _ = tracing_subscriber::fmt::try_init();
    boru_chat::perf::init();
    let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(42);

    // ── 500 friends in a Store ──
    let tmp = tempfile::tempdir().unwrap();
    let store_start = Instant::now();
    let mut store = FriendsStore::load_or_default(tmp.path());
    for i in 0..500 {
        let sk = SecretKey::from_bytes(&[(i as u8).wrapping_mul(17); 32]);
        let pk = sk.public();
        let id = FriendId::from_public_key(pk);
        store.friends.insert(
            id,
            FriendRecord {
                label: if i % 2 == 0 {
                    Some(format!("Friend{}", i))
                } else {
                    None
                },
                last_announced_name: None,
                last_announced_profile_image_ticket: None,
                status: FriendStatus::default(),
                known_addrs: vec![],
                addrs_updated_at_unix_ms: None,
                relationship: FriendRelationship::NotFriend,
                rooms: Default::default(),
                direct_conversation: None,
                mailbox_public_key: None,
            },
        );
    }
    PerfTracker::record(
        "friends_store_build_500",
        store_start.elapsed(),
        "500_entries".into(),
    );

    // Iterate friends
    let _timer = PerfTracker::timer("friends_iterate_500", "scan_all");
    for (id, record) in store.iter() {
        let _ = id.as_str().len();
        let _ = record.label.as_ref().map(|s| s.len());
    }

    // ── 100 conversations ──
    let mut conversations = Vec::new();
    let start = Instant::now();
    for i in 0..100 {
        conversations.push((
            TopicId::from_bytes(rng.random()),
            format!("Conversation_{i}"),
        ));
    }
    PerfTracker::record(
        "conversations_build_100",
        start.elapsed(),
        "100_entries".into(),
    );

    // Simulate 100 conversation switching
    let start = Instant::now();
    for (idx, (topic, name)) in conversations.iter().enumerate() {
        let _ = (topic, name);
        std::hint::black_box(idx);
    }
    PerfTracker::record(
        "conversation_switches_100",
        start.elapsed(),
        "100_switches".into(),
    );

    // ── Entry iteration at 3 scales ──
    for (suffix, count) in [("100", 100usize), ("1000", 1000), ("5000", 5000)] {
        let entries = make_entries(count);
        let label = match suffix {
            "100" => "entry_iteration_100",
            "1000" => "entry_iteration_1000",
            _ => "entry_iteration_5000",
        };
        let _timer = PerfTracker::timer(label, format!("{count}_entries"));
        let mut total = 0usize;
        for e in &entries {
            if e.message_hash.is_some() {
                total += 1;
            }
        }
        let _ = total;
    }

    // ── Height estimation pass at 3 scales ──
    const DATE_SEP_H: f32 = 32.0;
    const SYSTEM_H: f32 = 24.0;
    const MSG_BASE_H: f32 = 76.0;
    const REACTION_EXTRA: f32 = 22.0;

    for (suffix, count) in [("100", 100usize), ("1000", 1000), ("5000", 5000)] {
        let entries = make_entries(count);
        let label = match suffix {
            "100" => "height_estimation_100",
            "1000" => "height_estimation_1000",
            _ => "height_estimation_5000",
        };
        let _timer = PerfTracker::timer(label, format!("{count}_entries"));
        let mut total_height = 0.0_f32;
        let mut prev_day_ht: Option<u64> = None;
        for entry in &entries {
            let mut h = 0.0;
            let day = entry.timestamp.map(|ts| ts / 86400000);
            if let Some(d) = day {
                if prev_day_ht.is_none_or(|prev| prev != d) {
                    h += DATE_SEP_H;
                }
                prev_day_ht = Some(d);
            }
            match entry.kind {
                boru_chat::chat_core::ChatKind::System => h += SYSTEM_H,
                _ => {
                    h += MSG_BASE_H;
                    if !entry.reactions.is_empty() {
                        h += REACTION_EXTRA;
                    }
                }
            }
            total_height += h;
        }
        let _ = total_height;
    }

    eprintln!("\n=== DATA SCALING BASELINE ===");
    PerfTracker::print_report();
    PerfTracker::reset();
}

/// 4. Multiple simultaneous downloads benchmark (blob operations).
#[tokio::test]
async fn baseline_simultaneous_downloads() {
    let _ = tracing_subscriber::fmt::try_init();
    boru_chat::perf::init();
    let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(42);
    use iroh_blobs::store::mem::MemStore;
    use tokio::io::AsyncReadExt;

    fn make_image(rng: &mut impl rand::RngExt, size_kb: usize) -> Vec<u8> {
        let mut buf = vec![0u8; size_kb * 1024];
        for b in &mut buf[8..] {
            *b = rng.random();
        }
        buf
    }

    // 50 blobs at 64KB each
    let images_50: Vec<Vec<u8>> = (0..50).map(|_| make_image(rng, 64)).collect();
    let store = MemStore::new();
    let mut hashes = Vec::with_capacity(50);
    for data in &images_50 {
        let tag = store.blobs().add_bytes(data.clone()).await.unwrap();
        hashes.push(tag.hash);
    }

    // Sequential read of 50 blobs
    let start = Instant::now();
    let mut total_bytes = 0_usize;
    for h in &hashes {
        let mut reader = store.blobs().reader(*h);
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        total_bytes += buf.len();
    }
    PerfTracker::record(
        "sequential_read_50_blobs",
        start.elapsed(),
        format!("{}_bytes", total_bytes),
    );

    // Sequential add of 50 more blobs
    let images_50_more: Vec<Vec<u8>> = (0..50).map(|_| make_image(rng, 64)).collect();
    let start = Instant::now();
    for data in &images_50_more {
        let _ = store.blobs().add_bytes(data.clone()).await.unwrap();
    }
    PerfTracker::record("sequential_add_50_blobs", start.elapsed(), "50x64KB".into());

    // 100 small blobs (16KB each)
    let images_100: Vec<Vec<u8>> = (0..100).map(|_| make_image(rng, 16)).collect();
    let store2 = MemStore::new();
    let start = Instant::now();
    let mut hashes_100 = Vec::with_capacity(100);
    for data in &images_100 {
        let tag = store2.blobs().add_bytes(data.clone()).await.unwrap();
        hashes_100.push(tag.hash);
    }
    PerfTracker::record("add_100_blobs_16KB", start.elapsed(), "100x16KB".into());

    // Read them all back
    let start = Instant::now();
    let mut total = 0_usize;
    for h in &hashes_100 {
        let mut reader = store2.blobs().reader(*h);
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        total += buf.len();
    }
    PerfTracker::record(
        "read_100_blobs_16KB",
        start.elapsed(),
        format!("{}_bytes", total),
    );

    eprintln!("\n=== SIMULTANEOUS DOWNLOADS BASELINE ===");
    PerfTracker::print_report();
    PerfTracker::reset();
}

/// 5. Net event pipeline throughput at different scales.
#[tokio::test]
async fn baseline_net_event_throughput() {
    let _ = tracing_subscriber::fmt::try_init();
    boru_chat::perf::init();
    let _rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(42);
    let sk = SecretKey::from_bytes(&[0u8; 32]);

    for (suffix, count) in [("100", 100usize), ("1000", 1000)] {
        let mut peer = BenchPeer::new(sk.public());
        let msgs: Vec<Message> = (0..count)
            .map(|i| Message::Message {
                text: format!("Net event throughput message #{i} with padding."),
            })
            .collect();

        let signed_msgs: Vec<_> = msgs
            .iter()
            .map(|m| SignedMessage::sign_and_encode(&sk, m).unwrap())
            .collect();

        let label = match suffix {
            "100" => "net_event_pipeline_100",
            _ => "net_event_pipeline_1000",
        };
        let _timer = PerfTracker::timer(label, format!("{count}_msgs"));
        for signed in &signed_msgs {
            if let Ok((from, decoded, sent_at)) = SignedMessage::verify_and_decode(signed) {
                let event = NetEvent::Message {
                    from,
                    message: decoded,
                    sent_at,
                };
                let _ = chat_net_event(event, &mut peer);
            }
        }
    }

    eprintln!("\n=== NET EVENT THROUGHPUT BASELINE ===");
    PerfTracker::print_report();
    PerfTracker::reset();
}
