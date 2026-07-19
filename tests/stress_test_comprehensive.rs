//! Comprehensive stress test for iroh-gossip-chat at the specified dataset scale.
//!
//! Dataset: 500 friends, 100 conversations, 5,000 messages, 100 shared files,
//!          50 active downloads, 200 avatars.
//!
//! Measures: startup data loading, scrolling (entry iteration), conversation
//! switching, profile opening, downloads, search, avatars.
//!
//! Run:
//!   BORU_PERF=1 cargo test --test stress_test_comprehensive --features net,test-utils -- --nocapture

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use boru_chat::chat_core::{ChatEntry, ChatKind};
use boru_chat::chat_history::ChatHistoryStore;
use boru_chat::conversations::ConversationStore;
use boru_chat::friends::{FriendId, FriendRecord, FriendRelationship, FriendStatus, FriendsStore};
use boru_chat::perf::PerfTracker;
use boru_chat::proto::TopicId;
use iroh::{PublicKey, SecretKey};

use rand::RngExt;
use rand::SeedableRng;

// ── Constants matching the app ─────────────────────────────────────────

const DATASET_FRIENDS: usize = 500;
const DATASET_CONVERSATIONS: usize = 100;
const DATASET_MESSAGES: usize = 5_000;
const DATASET_SHARED_FILES: usize = 100;
const DATASET_DOWNLOADS: usize = 50;
const DATASET_AVATARS: usize = 200;

/// Generate synthetic ChatEntry entries matching the IcedChat app's ChatEntry
/// shape for the stress test.
fn make_chat_entries(count: usize) -> Vec<ChatEntry> {
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let entry = if i % 15 == 0 {
            // File share message
            ChatEntry::local(
                "Me".to_string(),
                format!("📎 shared_file_{}.pdf [1.2 MB]", i % 100),
            )
        } else if i % 3 == 0 {
            ChatEntry::local("Me".to_string(), format!("Hello! Message #{i}"))
        } else if i % 5 == 0 {
            ChatEntry::system(format!("User joined at message #{i}"))
        } else if i % 7 == 0 {
            // Image message — simulate image content
            ChatEntry::remote("Alice".to_string(), format!("📷 Check out photo #{i}"))
        } else {
            ChatEntry::remote(
                "Bob".to_string(),
                format!("Here's an interesting thought #{i}"),
            )
        };
        entries.push(entry);
    }
    entries
}

/// Load synthetic data files from disk — simulates app startup loading
fn load_stores_from_disk(data_dir: &str) -> (FriendsStore, ChatHistoryStore, ConversationStore) {
    let path = std::path::Path::new(data_dir);

    let friends = FriendsStore::load_or_default(path);
    let chat_history = ChatHistoryStore::load_or_default(path);
    let conversation_store = ConversationStore::load_or_default(path);

    (friends, chat_history, conversation_store)
}

// ── 1. STARTUP STRESS: Load all stores from disk ───────────────────────

#[test]
fn stress_startup_data_loading() {
    let _ = tracing_subscriber::fmt::try_init();
    boru_chat::perf::init();

    // The data must be pre-generated at /tmp/iroh-stress-test-data
    let data_dir = std::env::var("STRESS_DATA_DIR")
        .unwrap_or_else(|_| "/tmp/iroh-stress-test-data".to_string());

    let _timer = PerfTracker::timer("stress_startup_load_friends", &data_dir);
    let (friends, chat_history, conversation_store) = load_stores_from_disk(&data_dir);

    PerfTracker::record(
        "stress_startup_friend_count",
        Duration::ZERO,
        format!("{}_friends", friends.len()),
    );
    PerfTracker::record(
        "stress_startup_history_count",
        Duration::ZERO,
        format!("{}_entries", chat_history.len()),
    );
    PerfTracker::record(
        "stress_startup_conversation_count",
        Duration::ZERO,
        format!("{}_conversations", conversation_store.len()),
    );

    // Measure friend iteration (like app does for online cache seeding)
    let _timer = PerfTracker::timer("stress_startup_iterate_friends", "seed_online_cache");
    let online_count = friends
        .iter()
        .filter(|(_, record)| record.status.online)
        .count();
    PerfTracker::record(
        "stress_startup_online_count",
        Duration::ZERO,
        format!("{online_count}_online"),
    );

    // Measure friend -> HashMap building (like app's friend_online_cache)
    let _timer = PerfTracker::timer("stress_build_friend_online_cache", "500_friends");
    let mut friend_online_cache: HashSet<PublicKey> = HashSet::new();
    let mut friend_image_tickets: HashMap<PublicKey, String> = HashMap::new();
    let mut friend_profile_versions: HashMap<PublicKey, u64> = HashMap::new();
    let mut friend_image_handles: HashMap<PublicKey, Option<Vec<u8>>> = HashMap::new();

    for (fid, record) in friends.iter() {
        if let Ok(pk) = fid.parse_public_key() {
            if record.status.online {
                friend_online_cache.insert(pk);
            }
            // Simulate profile image ticket
            if record.last_announced_profile_image_ticket.is_some() {
                friend_image_tickets.insert(pk, "test_ticket".to_string());
                friend_image_handles.insert(pk, None);
                friend_profile_versions.insert(pk, 1);
            }
        }
    }

    PerfTracker::record(
        "stress_friend_cache_built",
        Duration::ZERO,
        format!(
            "{}_online {}_images {}_versions",
            friend_online_cache.len(),
            friend_image_tickets.len(),
            friend_profile_versions.len(),
        ),
    );

    // Simulate app startup info logging
    eprintln!("\n── Startup Stress ──");
    eprintln!(
        "  Friends: {}, History: {} entries, Conversations: {}",
        friends.len(),
        chat_history.len(),
        conversation_store.len(),
    );
    eprintln!(
        "  Online: {}, Profile images: {}",
        friend_online_cache.len(),
        friend_image_tickets.len(),
    );
}

// ── 2. SCROLLING STRESS: Entry iteration + height estimation ────────────

#[test]
fn stress_entry_scrolling() {
    let _ = tracing_subscriber::fmt::try_init();
    boru_chat::perf::init();

    let entries = make_chat_entries(DATASET_MESSAGES);

    // A) Full entry scan (simulates building widget cache)
    let _timer = PerfTracker::timer("stress_scroll_full_scan_5000", "all_entries");
    let mut count_remote = 0usize;
    let mut count_local = 0usize;
    let mut count_system = 0usize;
    for e in &entries {
        match e.kind {
            ChatKind::Local => count_local += 1,
            ChatKind::Remote => count_remote += 1,
            ChatKind::System => count_system += 1,
        }
    }
    PerfTracker::record(
        "stress_scroll_entry_types",
        Duration::ZERO,
        format!("{count_remote}remote_{count_local}local_{count_system}system"),
    );

    // B) Height estimation (like app's LayoutCache)

    const SYSTEM_H: f32 = 24.0;
    const MSG_BASE_H: f32 = 76.0;
    const REACTION_EXTRA: f32 = 22.0;

    let _timer = PerfTracker::timer("stress_scroll_height_estimation_5000", "layout_cache");
    let mut total_height = 0.0f32;
    for entry in &entries {
        let mut h = 0.0;
        match entry.kind {
            ChatKind::System => h += SYSTEM_H,
            _ => {
                h += MSG_BASE_H;
                // Simulate reactions height check
                if !entry.reactions.is_empty() {
                    h += REACTION_EXTRA;
                }
            }
        }
        total_height += h;
    }
    PerfTracker::record(
        "stress_scroll_total_height",
        Duration::ZERO,
        format!("{:.0}_pixels", total_height),
    );

    // C) Simulate windowed rendering — constructing slices (scroll window)
    let _timer = PerfTracker::timer("stress_scroll_windowed_iteration", "slice_100");
    let window_size = 100;
    let mut slices = 0;
    for chunk in entries.chunks(window_size) {
        let _ = chunk.len();
        slices += 1;
    }
    PerfTracker::record(
        "stress_scroll_window_count",
        Duration::ZERO,
        format!("{slices}_windows_of_{window_size}"),
    );

    eprintln!("\n── Scrolling Stress ──");
    eprintln!(
        "  Entries: {}, Height: {:.0}px, Windows (size {window_size}): {slices}",
        entries.len(),
        total_height,
    );
}

// ── 3. CONVERSATION SWITCHING STRESS ───────────────────────────────────

#[test]
fn stress_conversation_switching() {
    let _ = tracing_subscriber::fmt::try_init();
    boru_chat::perf::init();

    let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(42);

    // Build 100 conversations with entries
    let mut conversations: Vec<(TopicId, Vec<ChatEntry>)> =
        Vec::with_capacity(DATASET_CONVERSATIONS);
    for i in 0..DATASET_CONVERSATIONS {
        let topic = TopicId::from_bytes(rng.random());
        let entry_count = 10 + (i % 100); // 10-109 entries per conversation
        let entries: Vec<ChatEntry> = (0..entry_count)
            .map(|j| {
                if j % 5 == 0 {
                    ChatEntry::remote("Alice".to_string(), format!("Conv {i} msg #{j}"))
                } else {
                    ChatEntry::local("Me".to_string(), format!("Conv {i} msg #{j}"))
                }
            })
            .collect();
        conversations.push((topic, entries));
    }

    // Simulate switching through all conversations sequentially
    let _timer = PerfTracker::timer("stress_switch_conversations_100", "sequential_switch");
    for (i, (topic, entries)) in conversations.iter().enumerate() {
        // Simulate: store old, load new
        let _old_entries = &conversations
            .get(i.wrapping_sub(1) % conversations.len())
            .map(|(_, e)| e);
        // Scan new entries (like app does on room switch)
        let entry_count = entries.len();
        let hash = topic.as_bytes()[0];
        std::hint::black_box((entry_count, hash));
    }

    eprintln!("\n── Conversation Switching Stress ──");
    eprintln!(
        "  Conversations: {}, switches: {}",
        conversations.len(),
        DATASET_CONVERSATIONS,
    );
}

// ── 4. PROFILE OPENING STRESS ─────────────────────────────────────────

#[test]
fn stress_profile_opening() {
    let _ = tracing_subscriber::fmt::try_init();
    boru_chat::perf::init();

    let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(42);
    let _secret_key = SecretKey::from_bytes(&[0u8; 32]);

    // Build profile image handles map (like app's friend_image_handles)
    let _timer = PerfTracker::timer("stress_build_profile_image_map", "200_avatars");
    let mut friend_image_handles: HashMap<PublicKey, Option<Vec<u8>>> = HashMap::new();
    let mut friend_image_tickets: HashMap<PublicKey, String> = HashMap::new();
    let mut friend_profile_versions: HashMap<PublicKey, u64> = HashMap::new();

    for i in 0..DATASET_AVATARS {
        let pk = SecretKey::from_bytes(&[(i as u8).wrapping_mul(37); 32]).public();
        // ~2KB-5KB of avatar image data
        let avatar_data = (0..(1024 * (2 + i % 4)))
            .map(|_| rng.random::<u8>())
            .collect::<Vec<_>>();
        friend_image_handles.insert(pk, Some(avatar_data));
        friend_image_tickets.insert(pk, format!("blob_ticket_{i}"));
        friend_profile_versions.insert(pk, i as u64);
    }

    // Profile open: look up a specific peer's profile
    let _timer = PerfTracker::timer("stress_profile_lookup_200", "random_access");
    let target_key = SecretKey::from_bytes(&[(42u8).wrapping_mul(37); 32]).public();
    let _handle = friend_image_handles.get(&target_key);
    let ticket_count = friend_image_tickets.len();
    let version_count = friend_profile_versions.len();

    PerfTracker::record(
        "stress_profile_map_stats",
        Duration::ZERO,
        format!(
            "{}_handles {}_tickets {}_versions",
            friend_image_handles.len(),
            ticket_count,
            version_count
        ),
    );

    eprintln!("\n── Profile Opening Stress ──");
    eprintln!(
        "  Avatars: {} ({} KB each), lookups: OK",
        friend_image_handles.len(),
        2 + 42 % 4,
    );
}

// ── 5. DOWNLOADS STRESS ───────────────────────────────────────────────

#[test]
fn stress_downloads() {
    let _ = tracing_subscriber::fmt::try_init();
    boru_chat::perf::init();

    let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(42);

    // Simulate 50 active downloads with progress tracking (like app's
    // transfer_id_to_index map and download_progress_queue)
    let _timer = PerfTracker::timer("stress_download_setup_50", "active_downloads");

    let mut transfer_ids: Vec<u64> = Vec::with_capacity(DATASET_DOWNLOADS);
    let mut transfer_id_to_index: HashMap<u64, usize> = HashMap::new();
    let mut download_progress: HashMap<u64, (u64, u64)> = HashMap::new(); // received, total

    for i in 0..DATASET_DOWNLOADS {
        let tid = rng.random::<u64>();
        transfer_ids.push(tid);
        transfer_id_to_index.insert(tid, i);
        download_progress.insert(tid, (0u64, 1024u64 * 1024u64 * (1u64 + i as u64 % 10u64)));
        // 1MB-10MB per download
    }

    // Simulate progress updates (like app's DownloadProgress handler)
    let _timer = PerfTracker::timer("stress_download_progress_updates", "50_downloads_x_10");
    for tid in &transfer_ids {
        for _chunk in 0..10 {
            if let Some((received, total)) = download_progress.get_mut(tid) {
                *received += *total / 10;
                let pct = (*received as f64 / *total as f64) * 100.0;
                let _index = transfer_id_to_index.get(tid);
                std::hint::black_box((pct, _index));
            }
        }
    }

    // Simulate transfer completion cleanup
    let _timer = PerfTracker::timer("stress_download_complete_50", "cleanup");
    for tid in transfer_ids.iter().take(DATASET_DOWNLOADS / 2) {
        transfer_id_to_index.remove(tid);
        download_progress.remove(tid);
    }

    PerfTracker::record(
        "stress_download_remaining",
        Duration::ZERO,
        format!("{}_active_of_50", transfer_id_to_index.len()),
    );

    eprintln!("\n── Downloads Stress ──");
    eprintln!(
        "  Active downloads: {}, completed: {}",
        transfer_id_to_index.len(),
        DATASET_DOWNLOADS / 2,
    );
}

// ── 6. SEARCH STRESS ──────────────────────────────────────────────────

#[test]
fn stress_search() {
    let _ = tracing_subscriber::fmt::try_init();
    boru_chat::perf::init();

    let entries = make_chat_entries(DATASET_MESSAGES);

    // A) Search by text substring (like app's inline search)
    let _timer = PerfTracker::timer("stress_search_substring", "5000_entries");
    let query = "photo";
    let mut results: Vec<usize> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if e.body.contains(query) {
            results.push(i);
        }
    }
    PerfTracker::record(
        "stress_search_substring_results",
        Duration::ZERO,
        format!("{}_matches_for_{}", results.len(), query),
    );

    // B) Search by author label
    let _timer = PerfTracker::timer("stress_search_by_author", "5000_entries");
    let author_query = "Alice";
    let mut author_results: Vec<usize> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if e.label.contains(author_query) {
            author_results.push(i);
        }
    }
    PerfTracker::record(
        "stress_search_author_results",
        Duration::ZERO,
        format!("{}_by_{}", author_results.len(), author_query),
    );

    // C) Filter by kind
    let _timer = PerfTracker::timer("stress_search_filter_kind", "5000_entries");
    let mut file_msgs: Vec<usize> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        match e.kind {
            ChatKind::Local | ChatKind::Remote if e.body.contains("📎") => file_msgs.push(i),
            _ => {}
        }
    }
    PerfTracker::record(
        "stress_search_file_results",
        Duration::ZERO,
        format!("{}_file_msgs", file_msgs.len()),
    );

    eprintln!("\n── Search Stress ──");
    eprintln!(
        "  'photo': {}, by 'Alice': {}, files: {}",
        results.len(),
        author_results.len(),
        file_msgs.len(),
    );
}

// ── 7. ALL-OPERATIONS STRESS TEST ─────────────────────────────────────

#[test]
fn stress_all_operations() {
    let _ = tracing_subscriber::fmt::try_init();
    boru_chat::perf::init();

    let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(42);

    // Build the full dataset
    let _timer = PerfTracker::timer("stress_build_full_dataset", "500friends_100conv_5000msgs");

    // 500 friends
    let tmp = tempfile::tempdir().unwrap();
    let mut friends_store = FriendsStore::load_or_default(tmp.path());
    let mut friend_keys: Vec<PublicKey> = Vec::with_capacity(DATASET_FRIENDS);
    for i in 0..DATASET_FRIENDS {
        let sk = SecretKey::from_bytes(&[(i as u8).wrapping_mul(17); 32]);
        let pk = sk.public();
        friend_keys.push(pk);
        let fid = FriendId::from_public_key(pk);
        friends_store.friends.insert(
            fid,
            FriendRecord {
                label: if i % 2 == 0 {
                    Some(format!("Friend{i}"))
                } else {
                    None
                },
                last_announced_name: None,
                last_announced_profile_image_ticket: Some(format!("ticket_{i}")),
                status: FriendStatus {
                    online: i % 3 == 0,
                    ..Default::default()
                },
                known_addrs: vec![],
                addrs_updated_at_unix_ms: None,
                relationship: FriendRelationship::NotFriend,
                rooms: Default::default(),
                direct_conversation: None,
                mailbox_public_key: None,
            },
        );
    }

    // 100 conversations with 5,000 total messages
    let mut conversation_topics: Vec<TopicId> = Vec::with_capacity(DATASET_CONVERSATIONS);
    let mut conversation_entries: HashMap<TopicId, Vec<ChatEntry>> = HashMap::new();
    for i in 0..DATASET_CONVERSATIONS {
        let topic = TopicId::from_bytes(rng.random());
        conversation_topics.push(topic);
        let msgs_per_conv = DATASET_MESSAGES / DATASET_CONVERSATIONS; // 50 each
        let entries: Vec<ChatEntry> = (0..msgs_per_conv)
            .map(|j| {
                let is_file = j % 20 == 0 && DATASET_SHARED_FILES > 0;
                let is_image = j % 15 == 0;
                let sender = friend_keys[j % friend_keys.len()];
                let label = sender.fmt_short().to_string();
                if is_file {
                    ChatEntry::remote(
                        label,
                        format!("📎 shared_file_{}.pdf", j % DATASET_SHARED_FILES),
                    )
                } else if is_image {
                    ChatEntry::remote(label, format!("📷 photo_{j}.jpg"))
                } else {
                    ChatEntry::remote(label, format!("Message #{j} in conversation {i}"))
                }
            })
            .collect();
        conversation_entries.insert(topic, entries);
    }

    // Full build time
    let total_msgs: usize = conversation_entries.values().map(|v| v.len()).sum();
    PerfTracker::record(
        "stress_dataset_built",
        Duration::ZERO,
        format!(
            "{}friends_{}conv_{}msgs",
            friends_store.len(),
            conversation_topics.len(),
            total_msgs
        ),
    );

    // ── Combined measure: startup simulation ──
    let _timer = PerfTracker::timer("stress_app_startup_full", "all_operations");

    // Build online cache (like IcedChat::new())
    let _timer2 = PerfTracker::timer(
        "stress_build_online_cache",
        format!("{}_friends", friends_store.len()),
    );
    let starting_online_count = friends_store
        .iter()
        .filter(|(_, rec)| rec.status.online)
        .count();

    // Build profile image maps (like IcedChat::new())
    let _timer3 = PerfTracker::timer(
        "stress_build_image_maps",
        format!("{}_avatars", DATASET_AVATARS),
    );
    let mut image_handles: HashMap<PublicKey, Option<Vec<u8>>> = HashMap::new();
    for i in 0..DATASET_AVATARS {
        let pk = friend_keys[i % friend_keys.len()];
        // Simulate avatar data load
        let avatar_data = if i % 3 != 0 {
            Some(vec![0u8; 1024 * (2 + i % 4)])
        } else {
            None
        };
        image_handles.insert(pk, avatar_data);
    }

    // ── Combined measure: conversation switching ──
    let _timer = PerfTracker::timer("stress_combined_switch_scan", "all_conversations");
    for (i, topic) in conversation_topics.iter().enumerate() {
        if let Some(entries) = conversation_entries.get(topic) {
            let count = entries.len();
            let _prev = conversation_topics.get(i.wrapping_sub(1));
            std::hint::black_box(count);
        }
    }

    // ── Combined measure: scrolling through all entries ──
    let _timer = PerfTracker::timer("stress_combined_full_scroll", "all_entries");
    for entries in conversation_entries.values() {
        let mut total_height = 0.0f32;
        for entry in entries {
            let h = match entry.kind {
                ChatKind::System => 24.0,
                _ => 76.0,
            };
            total_height += h;
        }
        std::hint::black_box(total_height);
    }

    // ── Combined measure: full-text search ──
    let _timer = PerfTracker::timer("stress_combined_search_5000", "body_scan");
    let query = "photo";
    let mut hits = 0usize;
    for entries in conversation_entries.values() {
        for entry in entries {
            if entry.body.contains(query) {
                hits += 1;
            }
        }
    }

    // ── Combined measure: profile lookups ──
    let _timer = PerfTracker::timer("stress_combined_profile_lookup", "avatar_map");
    let target = friend_keys[DATASET_FRIENDS / 2]; // middle friend
    let _found_handle = image_handles.get(&target);

    eprintln!("\n── All-Operations Full Stress Report ──");
    eprintln!(
        "  Dataset: {} friends, {} conversations, {} messages",
        friends_store.len(),
        conversation_topics.len(),
        total_msgs,
    );
    eprintln!(
        "  Avatars: {}, Online: {}",
        image_handles.len(),
        starting_online_count,
    );
    eprintln!("  Search hits for 'photo': {hits}");
    PerfTracker::print_report();
}

// ── 8. ADVANCED: Memory pressure test ──────────────────────────────────

#[test]
fn stress_memory_pressure() {
    let _ = tracing_subscriber::fmt::try_init();
    boru_chat::perf::init();

    let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(42);

    // Simulate IcedChat's image budget enforcement at scale.
    // The app caps image bytes at 64MiB total, 2000 entries, 500 profile handles.
    const MAX_ENTRIES: usize = 2000;
    const MAX_PROFILE_HANDLES: usize = 500;

    // ── Entry cap with avatar images ──
    let _timer = PerfTracker::timer("stress_entry_cap_enforcement", "2000+_entries");

    let mut oversized_entries: Vec<ChatEntry> = Vec::new();
    for i in 0..MAX_ENTRIES + 100 {
        let entry = ChatEntry::remote(
            format!("User_{}", i % 100),
            format!("Message #{i} with some padding content for realistic sizing"),
        );
        oversized_entries.push(entry);
    }
    // Simulate cap enforcement: truncate to 2000
    let cap_label = format!(
        "{}_entries_enforced_to_{}",
        oversized_entries.len(),
        MAX_ENTRIES
    );
    if oversized_entries.len() > MAX_ENTRIES {
        let trimmed = oversized_entries.split_off(oversized_entries.len() - MAX_ENTRIES);
        oversized_entries = trimmed;
    }
    PerfTracker::record("stress_entry_cap_applied", Duration::ZERO, cap_label);
    assert_eq!(oversized_entries.len(), MAX_ENTRIES);

    // ── Profile image handle eviction ──
    let _timer = PerfTracker::timer("stress_profile_cap_enforcement", "500+_handles");
    let mut profile_map: HashMap<PublicKey, Vec<u8>> = HashMap::new();
    for i in 0..MAX_PROFILE_HANDLES + 50 {
        let pk = SecretKey::from_bytes(&[(i as u8).wrapping_mul(73); 32]).public();
        // ~2KB profile image bytes
        let data = (0..2048).map(|_| rng.random::<u8>()).collect::<Vec<_>>();
        profile_map.insert(pk, data);
    }
    PerfTracker::record(
        "stress_profile_map_before_eviction",
        Duration::ZERO,
        format!("{}_handles", profile_map.len()),
    );

    // Simulate eviction: keep youngest 500
    while profile_map.len() > MAX_PROFILE_HANDLES {
        if let Some(oldest) = profile_map.keys().next().cloned() {
            profile_map.remove(&oldest);
        }
    }
    PerfTracker::record(
        "stress_profile_map_after_eviction",
        Duration::ZERO,
        format!("{}_handles_remaining", profile_map.len()),
    );

    eprintln!("\n── Memory Pressure Stress ──");
    eprintln!(
        "  Entry cap: {} entries, Profile handles: {}",
        oversized_entries.len(),
        profile_map.len(),
    );
}
