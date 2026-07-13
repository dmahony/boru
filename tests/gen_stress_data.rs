//! Generate stress test data on disk for the startup loading test.
//! Run: STRESS_DATA_DIR=/tmp/iroh-stress-test-data cargo test --test gen_stress_data --features net,test-utils -- --nocapture
use std::path::Path;

use boru_chat::chat_history::{ChatHistoryStore, HistoryEntry};
use boru_chat::conversations::{ConversationEntry, ConversationKind, ConversationStore};
use boru_chat::friends::{FriendId, FriendRecord, FriendRelationship, FriendStatus, FriendsStore};
use boru_chat::proto::TopicId;
use iroh::SecretKey;
use rand::RngExt;
use rand::SeedableRng;

#[test]
fn generate_stress_data() {
    let data_dir = std::env::var("STRESS_DATA_DIR")
        .unwrap_or_else(|_| "/tmp/iroh-stress-test-data".to_string());
    let path = Path::new(&data_dir);
    std::fs::create_dir_all(path).unwrap();

    let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(42);

    // 500 friends — use rng to generate unique keys (the wrapping_mul formula
    // only produces 256 unique values since u8 wraps at 256)
    let mut friends_store = FriendsStore::empty_at(path);
    let mut friend_keys: Vec<_> = (0..500)
        .map(|_| SecretKey::from_bytes(&rng.random::<[u8; 32]>()))
        .collect();
    // Sort to keep deterministic order
    friend_keys.sort_by_key(|sk| sk.public().to_string());
    for (i, sk) in friend_keys.iter().enumerate() {
        let pk = sk.public();
        let fid = FriendId::from_public_key(pk);
        friends_store.friends.insert(
            fid,
            FriendRecord {
                label: if i % 2 == 0 { Some(format!("Friend{i}")) } else { None },
                last_announced_name: None,
                last_announced_profile_image_ticket: Some(format!("ticket_{i}")),
                status: FriendStatus { online: i % 3 == 0, ..Default::default() },
                known_addrs: vec![],
                addrs_updated_at_unix_ms: None,
                relationship: FriendRelationship::NotFriend,
                rooms: Default::default(),
                direct_conversation: None,
                mailbox_public_key: None,
            },
        );
    }
    friends_store.save().unwrap();
    eprintln!("  friends.json: {} friends", friends_store.len());

    // 100 conversations
    let mut conv_store = ConversationStore::empty_at(path);
    let mut topic_keys = Vec::with_capacity(100);
    for i in 0..100 {
        let topic = TopicId::from_bytes(rng.random());
        topic_keys.push(topic);
        conv_store.upsert(ConversationEntry {
            topic,
            peer_id: String::new(),
            name: format!("Conversation_{i}"),
            kind: ConversationKind::Group,
            created_at_unix_ms: 1700000000000u64,
            last_seen_at_unix_ms: 1700000000000u64,
            archived: false,
        });
    }
    conv_store.save().unwrap();
    eprintln!("  conversations.json: {} conversations", conv_store.len());

    // 5,000 messages across all conversations
    let mut history_store = ChatHistoryStore::empty_at(path);
    for i in 0..5000 {
        let conv_idx = i % 100;
        let topic = topic_keys[conv_idx];
        // Build a signed message that looks realistic — just a placeholder
        let signed_bytes = format!("signed_bytes_for_message_{i}").into_bytes();
        let hash = format!("{:032x}", i32::MAX as u64 + i as u64);
        let ts = 1700000000000u64 + (i as u64 * 1000);
        let body = format!("Message #{i} in conversation {conv_idx} with some realistic padding content.");
        history_store.push(HistoryEntry {
            event_id: 0,
            hash,
            sender: format!("User_{}", i % 100),
            timestamp: ts,
            kind: "text".to_string(),
            topic,
            text_preview: body,
            signed_bytes,
            delivery_state: boru_chat::chat_history::DeliveryState::Sent,
            image_bytes: None,
            image_identifier: None,
        });
    }
    history_store.save().unwrap();
    eprintln!("  chat_history.json: {} entries", history_store.len());

    eprintln!("\nDone. Data at {data_dir}");
}
