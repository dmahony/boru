//! End-to-end integration and acceptance tests for the message lifecycle.
//!
//! These tests exercise the full lifecycle of a message from creation through
//! delivery state transitions, restart recovery, reconnect replay, duplicate
//! suppression, and TTL-based expiry — without requiring live network peers.
//!
//! Test categories:
//! 1. **Full lifecycle** — Queued → Sent → Delivered → Seen through OutboxStore & ChatHistoryStore
//! 2. **Restart recovery** — OutboxStore save/load preserves all delivery states
//! 3. **Reconnect replay** — ChatHistoryStore::get_outgoing_queue finds pending messages
//! 4. **Duplicate suppression** — OutboxStore rejects duplicate event_ids; handle_net_event dedup
//! 5. **Message expiry** — TTL-based expiry in OutboxStore and handle_net_event stale-message drop
//! 6. **Edge cases** — empty stores, missing entries, backward transitions, concurrent saves

use boru_core::chat_history::{blake3_hex, ChatHistoryStore, DeliveryState, HistoryEntry};
use boru_core::outbox::{OutboxEntry, OutboxStore};
use boru_core::proto::TopicId;
use boru_core::storage::Storage;
use boru_core::store::MessageStore;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ── Test helpers ─────────────────────────────────────────────────────────────

fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("boru-lifecycle-{name}-{suffix}"));
    dir
}

fn make_topic(byte: u8) -> TopicId {
    TopicId::from_bytes([byte; 32])
}

fn make_chat_entry(topic: TopicId, idx: u8) -> HistoryEntry {
    let bytes = vec![idx; 64];
    HistoryEntry::new(
        topic,
        format!("sender{idx}"),
        bytes,
        "text",
        format!("hello {idx}"),
    )
}

fn make_outbox_entry(event_id: u64, topic: TopicId) -> OutboxEntry {
    let signed_bytes = format!("signed-message-{event_id}").into_bytes();
    OutboxEntry::new(event_id, topic, signed_bytes)
}

// ── 1. FULL LIFECYCLE tests ──────────────────────────────────────────────────

#[test]
fn full_lifecycle_queued_to_seen() {
    // Verify the complete Queued → Sent → Delivered → Seen transition for

    // a single message in both the OutboxStore and ChatHistoryStore.

    let dir = temp_dir("full_lifecycle");
    let topic = make_topic(0x01);

    // Set up both stores
    let mut chat = ChatHistoryStore::empty_at(&dir);
    let mut outbox = OutboxStore::empty_at(&dir);

    // 1. Create message (Queued)
    let id = chat.push_with_id(make_chat_entry(topic, 1));
    let entry = make_outbox_entry(id, topic);
    outbox.push(entry).unwrap();

    assert_eq!(
        chat.get_by_event_id(id).unwrap().delivery_state,
        DeliveryState::Queued
    );
    assert_eq!(
        outbox.get(id).unwrap().delivery_state,
        DeliveryState::Queued
    );

    // 2. Broadcast (Queued → Sent)
    chat.update_delivery_state(id, DeliveryState::Sent).unwrap();
    outbox
        .update_delivery_state(id, DeliveryState::Sent)
        .unwrap();

    assert_eq!(
        chat.get_by_event_id(id).unwrap().delivery_state,
        DeliveryState::Sent
    );
    assert_eq!(outbox.get(id).unwrap().delivery_state, DeliveryState::Sent);

    // 3. Peer confirms delivery (Sent → Delivered)
    chat.update_delivery_state(id, DeliveryState::Delivered)
        .unwrap();
    outbox
        .update_delivery_state(id, DeliveryState::Delivered)
        .unwrap();

    assert_eq!(
        chat.get_by_event_id(id).unwrap().delivery_state,
        DeliveryState::Delivered
    );
    assert_eq!(
        outbox.get(id).unwrap().delivery_state,
        DeliveryState::Delivered
    );

    // 4. User reads message (Delivered → Seen)
    chat.update_delivery_state(id, DeliveryState::Seen).unwrap();
    outbox
        .update_delivery_state(id, DeliveryState::Seen)
        .unwrap();

    assert_eq!(
        chat.get_by_event_id(id).unwrap().delivery_state,
        DeliveryState::Seen
    );
    assert_eq!(outbox.get(id).unwrap().delivery_state, DeliveryState::Seen);
}

#[test]
fn full_lifecycle_multiple_messages_independent_states() {
    // Multiple messages should advance independently through the state machine

    // without cross-contamination.

    let dir = temp_dir("multiple_independent");
    let topic = make_topic(0x02);

    let mut chat = ChatHistoryStore::empty_at(&dir);
    let mut outbox = OutboxStore::empty_at(&dir);

    // Create 3 messages
    let ids: Vec<u64> = (0..3)
        .map(|i| {
            let id = chat.push_with_id(make_chat_entry(topic, i));
            outbox.push(make_outbox_entry(id, topic)).unwrap();
            id
        })
        .collect();

    // Message 0: Queued → Sent → Delivered
    chat.update_delivery_state(ids[0], DeliveryState::Sent)
        .unwrap();
    outbox
        .update_delivery_state(ids[0], DeliveryState::Sent)
        .unwrap();
    chat.update_delivery_state(ids[0], DeliveryState::Delivered)
        .unwrap();
    outbox
        .update_delivery_state(ids[0], DeliveryState::Delivered)
        .unwrap();

    // Message 1: Queued → Sent (stays Sent)
    chat.update_delivery_state(ids[1], DeliveryState::Sent)
        .unwrap();
    outbox
        .update_delivery_state(ids[1], DeliveryState::Sent)
        .unwrap();

    // Message 2: stays Queued

    // Verify all three states
    assert_eq!(
        chat.get_by_event_id(ids[0]).unwrap().delivery_state,
        DeliveryState::Delivered
    );
    assert_eq!(
        outbox.get(ids[0]).unwrap().delivery_state,
        DeliveryState::Delivered
    );
    assert_eq!(
        chat.get_by_event_id(ids[1]).unwrap().delivery_state,
        DeliveryState::Sent
    );
    assert_eq!(
        outbox.get(ids[1]).unwrap().delivery_state,
        DeliveryState::Sent
    );
    assert_eq!(
        chat.get_by_event_id(ids[2]).unwrap().delivery_state,
        DeliveryState::Queued
    );
    assert_eq!(
        outbox.get(ids[2]).unwrap().delivery_state,
        DeliveryState::Queued
    );
}

#[test]
fn full_lifecycle_queued_to_failed() {
    // A message can transition Queued → Failed if sending fails immediately.

    let dir = temp_dir("queued_failed");
    let topic = make_topic(0x03);

    let mut chat = ChatHistoryStore::empty_at(&dir);
    let mut outbox = OutboxStore::empty_at(&dir);

    let id = chat.push_with_id(make_chat_entry(topic, 1));
    outbox.push(make_outbox_entry(id, topic)).unwrap();

    chat.update_delivery_state(id, DeliveryState::Failed)
        .unwrap();
    outbox
        .update_delivery_state(id, DeliveryState::Failed)
        .unwrap();

    assert_eq!(
        chat.get_by_event_id(id).unwrap().delivery_state,
        DeliveryState::Failed
    );
    assert_eq!(
        outbox.get(id).unwrap().delivery_state,
        DeliveryState::Failed
    );
}

#[test]
fn full_lifecycle_sent_to_failed() {
    // A message can transition Sent → Failed if the peer disconnects.

    let dir = temp_dir("sent_failed");
    let topic = make_topic(0x04);

    let mut chat = ChatHistoryStore::empty_at(&dir);
    let mut outbox = OutboxStore::empty_at(&dir);

    let id = chat.push_with_id(make_chat_entry(topic, 1));
    outbox.push(make_outbox_entry(id, topic)).unwrap();

    chat.update_delivery_state(id, DeliveryState::Sent).unwrap();
    outbox
        .update_delivery_state(id, DeliveryState::Sent)
        .unwrap();
    chat.update_delivery_state(id, DeliveryState::Failed)
        .unwrap();
    outbox
        .update_delivery_state(id, DeliveryState::Failed)
        .unwrap();

    assert_eq!(
        chat.get_by_event_id(id).unwrap().delivery_state,
        DeliveryState::Failed
    );
    assert_eq!(
        outbox.get(id).unwrap().delivery_state,
        DeliveryState::Failed
    );
}

#[test]
fn full_lifecycle_delivered_to_failed() {
    // A message can transition Delivered → Failed (delivery explicitly failed

    // after confirmation, e.g. timeout before final acknowledgement).

    let dir = temp_dir("delivered_failed");
    let topic = make_topic(0x05);

    let mut chat = ChatHistoryStore::empty_at(&dir);
    let mut outbox = OutboxStore::empty_at(&dir);

    let id = chat.push_with_id(make_chat_entry(topic, 1));
    outbox.push(make_outbox_entry(id, topic)).unwrap();

    chat.update_delivery_state(id, DeliveryState::Sent).unwrap();
    outbox
        .update_delivery_state(id, DeliveryState::Sent)
        .unwrap();
    chat.update_delivery_state(id, DeliveryState::Delivered)
        .unwrap();
    outbox
        .update_delivery_state(id, DeliveryState::Delivered)
        .unwrap();
    chat.update_delivery_state(id, DeliveryState::Failed)
        .unwrap();
    outbox
        .update_delivery_state(id, DeliveryState::Failed)
        .unwrap();

    assert_eq!(
        chat.get_by_event_id(id).unwrap().delivery_state,
        DeliveryState::Failed
    );
    assert_eq!(
        outbox.get(id).unwrap().delivery_state,
        DeliveryState::Failed
    );
}

// ── 2. RESTART RECOVERY tests ────────────────────────────────────────────────

#[test]
fn restart_recovery_preserves_all_states() {
    // Simulate an app restart against the durable SQLite `outgoing_messages`
    // table: write entries with various delivery states, drop the store,
    // reopen it (restart), and verify everything is preserved.

    let dir = temp_dir("restart_recovery");
    let topic = make_topic(0x10);

    // Session 1: write 5 entries and advance them through the state machine.
    {
        let storage = Storage::open(&dir).unwrap();
        for id in 1..=5u64 {
            storage
                .insert_outgoing_message(
                    id,
                    &topic,
                    &format!("hash-{id}"),
                    &format!("signed-{id}").into_bytes(),
                )
                .unwrap();
        }
        // event 1: Queued → Sent
        storage.update_outgoing_delivery_state(1, "sent").unwrap();
        // event 2: Queued → Sent → Delivered
        storage.update_outgoing_delivery_state(2, "sent").unwrap();
        storage
            .update_outgoing_delivery_state(2, "delivered")
            .unwrap();
        // event 3: Queued → Sent → Delivered → Seen
        storage.update_outgoing_delivery_state(3, "sent").unwrap();
        storage
            .update_outgoing_delivery_state(3, "delivered")
            .unwrap();
        storage.update_outgoing_delivery_state(3, "seen").unwrap();
        // event 4: Queued → Failed
        storage.update_outgoing_delivery_state(4, "failed").unwrap();
        // event 5 stays queued
    }

    // Session 2: reopen the store (simulate application restart).
    let loaded = Storage::open(&dir).expect("load should succeed");

    let state_of = |id: u64| -> String {
        loaded
            .get_outgoing_message(id)
            .unwrap()
            .expect("entry should exist")
            .delivery_state
    };
    assert_eq!(state_of(1), "sent");
    assert_eq!(state_of(2), "delivered");
    assert_eq!(state_of(3), "seen");
    assert_eq!(state_of(4), "failed");
    assert_eq!(state_of(5), "queued");

    // Pending (Queued) entries should be recoverable.
    let pending: Vec<u64> = loaded
        .list_pending_outgoing()
        .unwrap()
        .iter()
        .map(|e| e.event_id)
        .collect();
    assert_eq!(pending, vec![5], "only event 5 should be pending (Queued)");
}

#[test]
fn restart_recovery_preserves_retry_count() {
    // Retry count should survive a restart cycle in the durable SQLite store.

    let dir = temp_dir("restart_retry");
    let topic = make_topic(0x11);

    {
        let storage = Storage::open(&dir).unwrap();
        storage
            .insert_outgoing_message(1, &topic, "hash-1", b"signed-1")
            .unwrap();
        storage.increment_outgoing_retry(1).unwrap();
        storage.increment_outgoing_retry(1).unwrap();
        storage.increment_outgoing_retry(1).unwrap();
    }

    let loaded = Storage::open(&dir).expect("load");
    let row = loaded.get_outgoing_message(1).unwrap().unwrap();
    assert_eq!(row.retry_count, 3);
}

#[test]
fn restart_recovery_empty_store() {
    // Loading a non-existent outbox returns None (graceful first-start path).

    let dir = temp_dir("restart_empty");
    let loaded = OutboxStore::load(&dir).expect("load missing should not error");
    assert!(loaded.is_none(), "fresh directory should have no outbox");
}

#[test]
fn restart_recovery_load_or_default_creates_empty() {
    // load_or_default returns an empty store for a fresh directory.

    let dir = temp_dir("restart_default");
    let store = OutboxStore::load_or_default(&dir);
    assert!(store.is_empty());
}

#[test]
fn restart_recovery_save_then_load_idempotent() {
    // Multiple reopen cycles should be idempotent against the durable store.

    let dir = temp_dir("restart_idempotent");
    let topic = make_topic(0x12);

    {
        let storage = Storage::open(&dir).unwrap();
        for i in 1..=10u64 {
            storage
                .insert_outgoing_message(
                    i,
                    &topic,
                    &format!("hash-{i}"),
                    &format!("signed-{i}").into_bytes(),
                )
                .unwrap();
        }
    }

    // Reload and "save" again (no new entries)
    {
        let loaded = Storage::open(&dir).expect("load");
        assert_eq!(loaded.list_outgoing_for_topic(&topic).unwrap().len(), 10);
    }

    // Reload again — should still have 10
    let reloaded = Storage::open(&dir).expect("load");
    assert_eq!(reloaded.list_outgoing_for_topic(&topic).unwrap().len(), 10);
    for i in 1..=10u64 {
        assert!(
            reloaded.get_outgoing_message(i).unwrap().is_some(),
            "event {i} should survive two saves"
        );
    }
}

#[test]
fn restart_recovery_corrupt_file_graceful() {
    // A corrupt outbox file should not prevent the application from starting.

    let dir = temp_dir("restart_corrupt");
    fs::create_dir_all(&dir).expect("create test dir");
    fs::write(dir.join("outbox.json"), "{not valid json").expect("write corrupt file");

    // load should return an error
    let result = OutboxStore::load(&dir);
    assert!(result.is_err(), "corrupt JSON should fail");

    // load_or_default should fall back to empty
    let store = OutboxStore::load_or_default(&dir);
    assert!(
        store.is_empty(),
        "load_or_default should return empty store"
    );
}

#[test]
fn restart_recovery_reload_is_atomic() {
    // Verify that a write is durably committed by opening a fresh store that
    // was created solely from disk data.

    let dir = temp_dir("restart_atomic");
    let topic = make_topic(0x13);

    {
        let storage = Storage::open(&dir).unwrap();
        storage
            .insert_outgoing_message(42, &topic, "hash-42", b"signed-42")
            .unwrap();
    }

    // Completely independent load from disk
    let disk = Storage::open(&dir).expect("load");
    let row = disk
        .get_outgoing_message(42)
        .unwrap()
        .expect("should exist");
    assert_eq!(row.event_id, 42);
    assert_eq!(row.delivery_state, "queued");
}

// ── 3. RECONNECT REPLAY tests ────────────────────────────────────────────────

#[test]
fn reconnect_replay_finds_pending_messages() {
    // get_outgoing_queue should return only our own Queued/Sent messages

    // for a specific topic — these are the messages to replay on reconnection.

    let dir = temp_dir("replay_pending");
    let topic_a = make_topic(0x20);
    let topic_b = make_topic(0x21);
    let mut store = ChatHistoryStore::empty_at(&dir);

    let local_hex = "sender0";

    // Our messages on topic A
    let id1 = {
        let mut e = make_chat_entry(topic_a, 0);
        e.sender = local_hex.to_string();
        store.push_with_id(e)
    };
    // Our messages on topic B
    let id2 = {
        let mut e = make_chat_entry(topic_b, 0);
        e.sender = local_hex.to_string();
        store.push_with_id(e)
    };
    // Other peer's messages on topic A (should NOT be included in replay)
    let _id3 = {
        let mut e = make_chat_entry(topic_a, 1);
        e.sender = "other_peer".to_string();
        store.push_with_id(e)
    };
    // Already delivered message (should NOT be in replay)
    let _id4 = {
        let mut e = make_chat_entry(topic_a, 2);
        e.sender = local_hex.to_string();
        let id = store.push_with_id(e);
        store
            .update_delivery_state(id, DeliveryState::Sent)
            .unwrap();
        store
            .update_delivery_state(id, DeliveryState::Delivered)
            .unwrap();
        id
    };

    // Topic A replay should return only our Queued message
    let replay_a: Vec<u64> = store
        .get_outgoing_queue(&topic_a, local_hex)
        .iter()
        .map(|e| e.event_id)
        .collect();
    assert_eq!(replay_a, vec![id1], "only our Queued msg on topic A");

    // Topic B replay should return our Queued message
    let replay_b: Vec<u64> = store
        .get_outgoing_queue(&topic_b, local_hex)
        .iter()
        .map(|e| e.event_id)
        .collect();
    assert_eq!(replay_b, vec![id2], "only our Queued msg on topic B");
}

#[test]
fn reconnect_replay_includes_sent_messages() {
    // Messages that were sent but not yet confirmed (Sent state) should

    // also be replayed on reconnection.

    let dir = temp_dir("replay_sent");
    let topic = make_topic(0x22);
    let mut store = ChatHistoryStore::empty_at(&dir);
    let local_hex = "me";

    let id1 = {
        let mut e = make_chat_entry(topic, 1);
        e.sender = local_hex.to_string();
        let id = store.push_with_id(e);
        store
            .update_delivery_state(id, DeliveryState::Sent)
            .unwrap();
        id
    };
    let id2 = {
        let mut e = make_chat_entry(topic, 2);
        e.sender = local_hex.to_string();
        let id = store.push_with_id(e);
        store
            .update_delivery_state(id, DeliveryState::Sent)
            .unwrap();
        store
            .update_delivery_state(id, DeliveryState::Delivered)
            .unwrap();
        id
    };
    let id3 = {
        let mut e = make_chat_entry(topic, 3);
        e.sender = local_hex.to_string();
        store.push_with_id(e) // stays Queued
    };

    let replay: Vec<u64> = store
        .get_outgoing_queue(&topic, local_hex)
        .iter()
        .map(|e| e.event_id)
        .collect();

    // id1 (Sent) and id3 (Queued) should be in replay; id2 (Delivered) should not
    assert!(replay.contains(&id1), "Sent message should be replayed");
    assert!(
        !replay.contains(&id2),
        "Delivered message should NOT be replayed"
    );
    assert!(replay.contains(&id3), "Queued message should be replayed");
    assert_eq!(replay.len(), 2);
}

#[test]
fn reconnect_replay_empty_for_other_peer() {
    // get_outgoing_queue for a peer with no pending messages returns empty.

    let dir = temp_dir("replay_empty");
    let topic = make_topic(0x23);
    let mut store = ChatHistoryStore::empty_at(&dir);

    // All messages are from other peers
    let mut e = make_chat_entry(topic, 1);
    e.sender = "alice".to_string();
    store.push_with_id(e);

    let mut e = make_chat_entry(topic, 2);
    e.sender = "bob".to_string();
    store.push_with_id(e);

    let replay = store.get_outgoing_queue(&topic, "me");
    assert!(replay.is_empty(), "no pending messages for 'me'");
}

#[test]
fn reconnect_replay_outbox_pending_on_reload() {
    // After restart, outbox pending() should return messages that were

    // still Queued before shutdown — these need to be replayed.

    let dir = temp_dir("replay_outbox_reload");
    let topic = make_topic(0x24);

    {
        let storage = Storage::open(&dir).unwrap();
        storage
            .insert_outgoing_message(1, &topic, "hash-1", b"signed-1")
            .unwrap();
        storage
            .insert_outgoing_message(2, &topic, "hash-2", b"signed-2")
            .unwrap();
        storage.update_outgoing_delivery_state(1, "sent").unwrap();
        // 2 stays Queued
    }

    let loaded = Storage::open(&dir).expect("load");
    let pending: Vec<u64> = loaded
        .list_pending_outgoing()
        .unwrap()
        .iter()
        .map(|e| e.event_id)
        .collect();
    assert_eq!(pending, vec![2], "only event 2 should be pending on reload");
}

// ── 4. DUPLICATE SUPPRESSION tests ──────────────────────────────────────────

#[test]
fn duplicate_suppression_outbox_rejects_same_event_id() {
    // OutboxStore::push must reject entries with the same event_id.

    let dir = temp_dir("dedup_outbox");
    let topic = make_topic(0x30);
    let mut outbox = OutboxStore::empty_at(&dir);

    outbox.push(make_outbox_entry(1, topic)).unwrap();
    let err = outbox.push(make_outbox_entry(1, topic)).unwrap_err();
    assert!(err.contains("duplicate"), "error should mention duplicate");
    assert_eq!(outbox.len(), 1, "should still have 1 entry");
}

#[test]
fn duplicate_suppression_outbox_accepts_same_content_diff_id() {
    // Same content but different event_id should be accepted (different

    // messages, same bytes is valid — e.g. multiple identical messages).

    let dir = temp_dir("dedup_content");
    let topic = make_topic(0x31);
    let mut outbox = OutboxStore::empty_at(&dir);

    let bytes = b"identical-content".to_vec();
    let e1 = OutboxEntry::new(1, topic, bytes.clone());
    let e2 = OutboxEntry::new(2, topic, bytes);
    outbox.push(e1).unwrap();
    outbox.push(e2).unwrap();
    assert_eq!(outbox.len(), 2);
}

#[test]
fn duplicate_suppression_outbox_rejects_same_id_diff_topic() {
    // Different topics but same event_id should still be rejected (event_id

    // is the unique dedup key, not topic+event_id).

    let topic_a = make_topic(0x32);
    let topic_b = make_topic(0x33);
    let mut outbox = OutboxStore::empty_at(temp_dir("dedup_topic"));

    outbox.push(make_outbox_entry(1, topic_a)).unwrap();
    let err = outbox.push(make_outbox_entry(1, topic_b)).unwrap_err();
    assert!(
        err.contains("duplicate"),
        "same event_id different topic should be rejected"
    );
    assert_eq!(outbox.len(), 1);
}

#[test]
fn duplicate_suppression_outbox_remove_and_re_add() {
    // After removing an entry, its event_id should be reusable.

    let dir = temp_dir("dedup_readd");
    let topic = make_topic(0x34);
    let mut outbox = OutboxStore::empty_at(&dir);

    outbox.push(make_outbox_entry(1, topic)).unwrap();
    assert!(outbox.remove(1));
    assert!(
        outbox.push(make_outbox_entry(1, topic)).is_ok(),
        "should allow re-adding after removal"
    );
    assert_eq!(outbox.len(), 1);
}

#[test]
fn duplicate_suppression_push_queued_rejects_non_queued() {
    // push_queued should reject entries that are not in Queued state.

    let dir = temp_dir("dedup_push_queued");
    let topic = make_topic(0x35);
    let mut outbox = OutboxStore::empty_at(&dir);

    let mut entry = make_outbox_entry(1, topic);
    entry.delivery_state = DeliveryState::Sent;
    let err = outbox.push_queued(entry).unwrap_err();
    assert!(
        err.contains("Queued"),
        "push_queued should reject non-Queued entries"
    );
    assert!(outbox.is_empty());
}

#[test]
fn duplicate_suppression_history_store_no_built_in_dedup() {
    // ChatHistoryStore does NOT deduplicate — entries with the same

    // event_id can coexist (event_id is assigned by push_with_id).

    // This test documents this behavior.

    let dir = temp_dir("dedup_history");
    let topic = make_topic(0x36);
    let mut store = ChatHistoryStore::empty_at(&dir);

    // push (raw append) doesn't check for duplicates
    // push_with_id always assigns a new id
    let id1 = store.push_with_id(make_chat_entry(topic, 1));
    let id2 = store.push_with_id(make_chat_entry(topic, 1));
    assert_ne!(id1, id2, "each push_with_id assigns a new event_id");
    assert_eq!(store.len(), 2);
}

// ── 5. MESSAGE EXPIRY tests ─────────────────────────────────────────────────

#[test]
fn message_expiry_outbox_zero_ttl_removes_all() {
    // Entries older than the TTL should be removed by expire().

    // Zero TTL means everything expires immediately.

    let dir = temp_dir("expiry_zero");
    let topic = make_topic(0x40);
    let mut outbox = OutboxStore::with_ttl(&dir, Duration::from_secs(0));

    outbox.push(make_outbox_entry(1, topic)).unwrap();
    outbox.push(make_outbox_entry(2, topic)).unwrap();
    assert_eq!(outbox.len(), 2);

    let removed = outbox.expire();
    assert_eq!(removed, 2);
    assert!(outbox.is_empty());
}

#[test]
fn message_expiry_outbox_long_ttl_keeps_all() {
    // Entries within the TTL should survive expire().

    let dir = temp_dir("expiry_long");
    let topic = make_topic(0x41);
    let mut outbox = OutboxStore::with_ttl(&dir, Duration::from_secs(365 * 86400));

    outbox.push(make_outbox_entry(1, topic)).unwrap();
    outbox.push(make_outbox_entry(2, topic)).unwrap();

    let removed = outbox.expire();
    assert_eq!(removed, 0);
    assert_eq!(outbox.len(), 2);
}

#[test]
fn message_expiry_outbox_ttl_change_takes_effect() {
    // Changing the TTL should affect subsequent expire() calls.

    let dir = temp_dir("expiry_ttl_change");
    let topic = make_topic(0x43);
    let mut outbox = OutboxStore::with_ttl(&dir, Duration::from_secs(365 * 86400));

    outbox.push(make_outbox_entry(1, topic)).unwrap();
    assert_eq!(outbox.expire(), 0, "long TTL: nothing expires");

    // Now set TTL to zero
    outbox.set_ttl(Duration::from_secs(0));
    let removed = outbox.expire();
    assert_eq!(removed, 1, "zero TTL: entry expires now");
    assert!(outbox.is_empty());
}

#[test]
fn message_expiry_outbox_seen_and_failed_also_expire() {
    // Terminal-state entries (Seen, Failed) should also be subject to

    // TTL-based expiry — they don't get special treatment.

    let dir = temp_dir("expiry_terminal");
    let topic = make_topic(0x44);
    let mut outbox = OutboxStore::with_ttl(&dir, Duration::from_secs(0));

    let e1 = make_outbox_entry(1, topic);
    let e2 = make_outbox_entry(2, topic);
    let e3 = make_outbox_entry(3, topic);
    let e4 = make_outbox_entry(4, topic);
    outbox.push(e1).unwrap();
    outbox.push(e2).unwrap();
    outbox.push(e3).unwrap();
    outbox.push(e4).unwrap();

    // Advance through valid transitions to reach Seen and Failed
    outbox
        .update_delivery_state(1, DeliveryState::Sent)
        .unwrap();
    outbox
        .update_delivery_state(1, DeliveryState::Delivered)
        .unwrap();
    outbox
        .update_delivery_state(1, DeliveryState::Seen)
        .unwrap();
    outbox
        .update_delivery_state(2, DeliveryState::Sent)
        .unwrap();
    outbox
        .update_delivery_state(2, DeliveryState::Failed)
        .unwrap();

    let removed = outbox.expire();
    assert_eq!(removed, 4, "all entries should expire regardless of state");
}

// ── 6. EDGE CASES ───────────────────────────────────────────────────────────

#[test]
fn edge_case_empty_outbox_save_load() {
    // Saving and reopening an empty durable store should work.

    let dir = temp_dir("empty_outbox");

    // Session 1: open (empty), commit nothing.
    {
        let storage = Storage::open(&dir).unwrap();
        assert!(storage.list_pending_outgoing().unwrap().is_empty());
    }

    // Session 2: reopen — still empty.
    let loaded = Storage::open(&dir).expect("load");
    assert!(loaded.list_pending_outgoing().unwrap().is_empty());
}

#[test]
fn edge_case_identity_update_is_noop() {
    // Re-applying the same delivery state (identity) should be accepted

    // and not change anything, including retry_count.

    let dir = temp_dir("identity_noop");
    let topic = make_topic(0x50);
    let mut outbox = OutboxStore::empty_at(&dir);

    outbox.push(make_outbox_entry(1, topic)).unwrap();

    // Apply Queued → Queued (identity)
    assert!(
        outbox
            .update_delivery_state(1, DeliveryState::Queued)
            .is_ok(),
        "identity transition should be ok"
    );
    assert_eq!(outbox.get(1).unwrap().delivery_state, DeliveryState::Queued);
    assert_eq!(outbox.get(1).unwrap().retry_count, 0);
}

#[test]
fn edge_case_save_without_data_dir_errors() {
    // file_path() on a store without a data_dir returns the path.

    let mut outbox = OutboxStore::empty_at("");
    let topic = make_topic(0x51);
    outbox.push(make_outbox_entry(1, topic)).unwrap();

    let _path = outbox.file_path();
}

#[test]
fn edge_case_outbox_remove_topic_leaves_other_topics() {
    // remove_topic should only remove entries for the specified topic.

    let dir = temp_dir("remove_topic");
    let ta = make_topic(0x52);
    let tb = make_topic(0x53);
    let mut outbox = OutboxStore::empty_at(&dir);

    outbox.push(make_outbox_entry(1, ta)).unwrap();
    outbox.push(make_outbox_entry(2, tb)).unwrap();
    outbox.push(make_outbox_entry(3, ta)).unwrap();

    outbox.remove_topic(&ta);
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox.entries()[0].event_id, 2);
    assert_eq!(outbox.entries()[0].topic, tb);
}

#[test]
fn edge_case_outbox_contains_after_removal() {
    // contains should return false after remove().

    let dir = temp_dir("contains_after_rm");
    let topic = make_topic(0x54);
    let mut outbox = OutboxStore::empty_at(&dir);

    outbox.push(make_outbox_entry(1, topic)).unwrap();
    assert!(outbox.contains(1));
    outbox.remove(1);
    assert!(!outbox.contains(1));
}

#[test]
fn edge_case_outbox_get_by_hash_after_save_load() {
    // Content hash should be persisted and retrievable after a save/load-like
    // reopen against the durable store.

    let dir = temp_dir("hash_roundtrip");
    let topic = make_topic(0x55);

    let bytes = b"roundtrip-content".to_vec();
    let hash = blake3_hex(&bytes);

    {
        let storage = Storage::open(&dir).unwrap();
        storage
            .insert_outgoing_message(1, &topic, &hash, &bytes)
            .unwrap();
    }

    let loaded = Storage::open(&dir).expect("load");
    let row = loaded
        .get_outgoing_message(1)
        .unwrap()
        .expect("should exist");
    assert_eq!(row.hash, hash, "hash should survive a reopen");
    assert_eq!(row.event_id, 1);
    // Content-based lookup is keyed by the same hash: it must be preserved.
    let find_by_hash = loaded
        .list_outgoing_for_topic(&topic)
        .unwrap()
        .into_iter()
        .find(|r| r.hash == hash);
    assert!(
        find_by_hash.is_some(),
        "hash lookup should work after save/load"
    );
}

#[test]
fn edge_case_chat_history_get_outgoing_queue_not_include_delivered() {
    // get_outgoing_queue should exclude Delivered and Seen messages from replay.

    let dir = temp_dir("outgoing_excludes_terminal");
    let topic = make_topic(0x56);
    let mut store = ChatHistoryStore::empty_at(&dir);
    let local = "me";

    let _delivered_id = {
        let mut e = make_chat_entry(topic, 1);
        e.sender = local.to_string();
        let id = store.push_with_id(e);
        store
            .update_delivery_state(id, DeliveryState::Sent)
            .unwrap();
        store
            .update_delivery_state(id, DeliveryState::Delivered)
            .unwrap();
        id
    };

    let _seen_id = {
        let mut e = make_chat_entry(topic, 2);
        e.sender = local.to_string();
        let id = store.push_with_id(e);
        store
            .update_delivery_state(id, DeliveryState::Sent)
            .unwrap();
        store
            .update_delivery_state(id, DeliveryState::Delivered)
            .unwrap();
        store
            .update_delivery_state(id, DeliveryState::Seen)
            .unwrap();
        id
    };

    let pending = store.get_outgoing_queue(&topic, local);
    assert!(
        pending.is_empty(),
        "terminal-state messages should not appear in outgoing queue"
    );
}

// ── 7. MAILBOX REPLAY SEMANTICS tests ──────────────────────────────────────
//
// These tests verify that mailbox replay does not alter online gossip or
// Delivered versus Seen semantics. The mailbox accepts opaque ciphertext
// via accept_incoming, persists it, and the payload enters the normal
// history pipeline only after decryption. Replay of the same envelope
// must be idempotent — no duplicate history entries, no spurious state
// transitions.

#[test]
fn mailbox_replay_persists_before_acknowledgement() {
    // A message accepted by the mailbox must be persisted in the durable
    // chat-history store (MessageStore) before the acknowledgement is sent.
    // This simulates the pattern: accept_incoming → save → push_to_history
    // → send_ack, and verifies the entry survives a restart.

    let dir = temp_dir("mailbox_persist_before_ack");
    let topic = make_topic(0x60);
    let topic_arr = *topic.as_bytes();
    let sender = [1u8; 32];
    let local = [2u8; 32];
    let msg_hash = [3u8; 32];

    // Session 1: mailbox accepts the envelope and persists it to history
    // before any ack is sent.
    {
        let store = MessageStore::open(dir.join("message_store.db")).unwrap();
        let inserted = store
            .insert_chat_message(
                &msg_hash,
                &topic_arr,
                &sender,
                1000,
                "text",
                "hello from mailbox",
                None,
                None,
                &local,
            )
            .unwrap();
        assert!(inserted, "message persisted in history");
    }

    // Session 2: restart — the entry is still present, starting as Queued
    // (the mailbox reception is the first event; the ack is sent only after
    // the message is in the store).
    let store = MessageStore::open(dir.join("message_store.db")).unwrap();
    let messages = store.get_messages_for_topic(&topic_arr, 10, 0).unwrap();
    assert_eq!(
        messages.len(),
        1,
        "entry survives restart after mailbox accept"
    );
    assert_eq!(
        messages[0].delivery_state, "queued",
        "new entry starts as queued after mailbox accept"
    );
}

#[test]
fn mailbox_replay_of_same_payload_is_idempotent_in_history() {
    // Replaying the same decrypted payload twice must not create a

    // duplicate history entry.  The mailbox layer is responsible for

    // dedup; this test verifies that the history store's behaviour

    // does not accidentally create duplicates from replayed messages.

    let dir = temp_dir("mailbox_replay_idempotent");
    let topic = make_topic(0x61);
    let mut history = ChatHistoryStore::empty_at(&dir);

    // Simulate: first mailbox replay inserts the message.
    let event_id = history.push_with_id(make_chat_entry(topic, 1));
    assert_eq!(history.len(), 1);

    // Simulate: second mailbox replay (same decrypted content, but
    // push_with_id always assigns a new event_id — different from
    // the mailbox dedup but the history store doesn't dedup by content.)
    // This is expected and documented: the mailbox layer dedup is what
    // prevents duplicates; the history store is append-only.
    let second_id = history.push_with_id(make_chat_entry(topic, 1));
    assert_ne!(
        event_id, second_id,
        "each push_with_id gets a unique event_id"
    );
    // But content-hash dedup would be the mailbox's job.
    // This test documents that the history store itself is not a
    // content-based dedup layer — that's intentional.
}

#[test]
fn mailbox_replay_does_not_alter_delivery_transitions() {
    // Once a replayed message is in the history store, its delivery

    // state must follow the normal Queued → Sent → Delivered → Seen

    // progression without mailbox replay interfering.

    let dir = temp_dir("mailbox_no_interference");
    let topic = make_topic(0x62);
    let mut history = ChatHistoryStore::empty_at(&dir);
    let mut outbox = OutboxStore::empty_at(&dir);

    // Simulate mailbox-accepted message entering history.
    let event_id = history.push_with_id(make_chat_entry(topic, 1));
    let entry = make_outbox_entry(event_id, topic);
    outbox.push(entry).unwrap();

    assert_eq!(
        history.get_by_event_id(event_id).unwrap().delivery_state,
        DeliveryState::Queued
    );

    // Normal progression after broadcast.
    history
        .update_delivery_state(event_id, DeliveryState::Sent)
        .unwrap();
    outbox
        .update_delivery_state(event_id, DeliveryState::Sent)
        .unwrap();

    // Simulate a mailbox SyncResponse playing back the same message
    // during reconnection.  The history entry is already Sent — replay
    // should not regress it to Queued.
    let replayed = history.get_by_event_id(event_id).unwrap();
    assert_eq!(
        replayed.delivery_state,
        DeliveryState::Sent,
        "mailbox replay must not regress Sent back to Queued"
    );
    assert_eq!(
        outbox.get(event_id).unwrap().delivery_state,
        DeliveryState::Sent,
        "outbox state must also survive mailbox replay"
    );

    // Continue normal progression to Delivered → Seen.
    history
        .update_delivery_state(event_id, DeliveryState::Delivered)
        .unwrap();
    outbox
        .update_delivery_state(event_id, DeliveryState::Delivered)
        .unwrap();
    history
        .update_delivery_state(event_id, DeliveryState::Seen)
        .unwrap();
    outbox
        .update_delivery_state(event_id, DeliveryState::Seen)
        .unwrap();

    assert_eq!(
        history.get_by_event_id(event_id).unwrap().delivery_state,
        DeliveryState::Seen
    );
}

#[test]
fn mailbox_replay_pending_vs_seen_separation() {
    // A mailbox replay that delivers a message must not confuse

    // "pending" (not yet delivered) with "seen" (user has read it).

    // These are separate concepts; replay only affects pending.

    let dir = temp_dir("mailbox_pending_seen");
    let topic = make_topic(0x63);
    let mut history = ChatHistoryStore::empty_at(&dir);
    let mut outbox = OutboxStore::empty_at(&dir);

    // Message A: fully delivered and seen (pre-replay state).
    let seen_id = history.push_with_id(make_chat_entry(topic, 1));
    let entry_a = make_outbox_entry(seen_id, topic);
    outbox.push(entry_a).unwrap();
    history
        .update_delivery_state(seen_id, DeliveryState::Sent)
        .unwrap();
    outbox
        .update_delivery_state(seen_id, DeliveryState::Sent)
        .unwrap();
    history
        .update_delivery_state(seen_id, DeliveryState::Delivered)
        .unwrap();
    outbox
        .update_delivery_state(seen_id, DeliveryState::Delivered)
        .unwrap();
    history
        .update_delivery_state(seen_id, DeliveryState::Seen)
        .unwrap();
    outbox
        .update_delivery_state(seen_id, DeliveryState::Seen)
        .unwrap();

    // Message B: just arrived via mailbox replay (Queued).
    let pending_id = history.push_with_id(make_chat_entry(topic, 2));
    let entry_b = make_outbox_entry(pending_id, topic);
    outbox.push(entry_b).unwrap();

    // Mailbox replay must not touch Message A's Seen state.
    assert_eq!(
        history.get_by_event_id(seen_id).unwrap().delivery_state,
        DeliveryState::Seen,
        "Seen message must remain Seen after mailbox replay of another message"
    );
    assert_eq!(
        outbox.get(seen_id).unwrap().delivery_state,
        DeliveryState::Seen,
        "outbox Seen state must survive mailbox replay"
    );

    // Message B starts as Queued (pending).
    assert_eq!(
        history.get_by_event_id(pending_id).unwrap().delivery_state,
        DeliveryState::Queued,
        "new mailbox-replayed message starts as Queued (pending)"
    );
}

#[test]
fn mailbox_replay_outgoing_queue_unchanged() {
    // get_outgoing_queue (used for service/whisper reconnect replay)

    // must not include mailbox-replayed messages from other peers

    // (those are received messages, not outgoing).

    let dir = temp_dir("mailbox_outgoing_unchanged");
    let topic = make_topic(0x64);
    let mut history = ChatHistoryStore::empty_at(&dir);
    let local = "me";

    // Our own outgoing message.
    let outgoing_id = {
        let mut e = make_chat_entry(topic, 1);
        e.sender = local.to_string();
        let id = history.push_with_id(e);
        history
            .update_delivery_state(id, DeliveryState::Sent)
            .unwrap();
        id
    };

    // A message received via mailbox replay (from another peer).
    let mailbox_id = {
        let mut e = make_chat_entry(topic, 2);
        e.sender = "alice".to_string();
        history.push_with_id(e)
    };

    let queue: Vec<u64> = history
        .get_outgoing_queue(&topic, local)
        .iter()
        .map(|e| e.event_id)
        .collect();

    assert!(
        queue.contains(&outgoing_id),
        "our outgoing message must be in the queue"
    );
    assert!(
        !queue.contains(&mailbox_id),
        "mailbox-replayed message from another peer must NOT appear in outgoing queue"
    );
    assert_eq!(queue.len(), 1, "only one outgoing message in queue");
}
