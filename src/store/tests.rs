//! Unit tests for the decomposed `crate::store` module.
//!
//! Consolidated verbatim from the pre-split `store.rs` (BORU-CORE-004). The
//! `use super::*` glob keeps the facade's MessageStore type, its public
//! types, and the shared helpers in scope.

use super::*;

#[test]
fn group_history_merges_known_epochs_without_rewriting_messages() {
    let store = MessageStore::memory().unwrap();
    let group_id = [7u8; 32];
    let topic_a = [1u8; 32];
    let topic_b = [2u8; 32];
    let sender = [9u8; 32];
    let local = [0u8; 32];
    store.register_group_epoch(group_id, 1, topic_a).unwrap();
    store.register_group_epoch(group_id, 2, topic_b).unwrap();
    assert!(store
        .insert_chat_message(
            &[11u8; 32],
            &topic_a,
            &sender,
            20,
            "text",
            "old",
            None,
            None,
            &local
        )
        .unwrap());
    assert!(store
        .insert_chat_message(
            &[12u8; 32],
            &topic_b,
            &sender,
            10,
            "text",
            "new",
            None,
            None,
            &local
        )
        .unwrap());

    let messages = store.get_messages_for_group(&group_id, 0, 10).unwrap();
    assert_eq!(
        messages.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(),
        ["new", "old"]
    );
    assert_eq!(store.count_messages_for_topic(&topic_a).unwrap(), 1);
    assert_eq!(store.count_messages_for_topic(&topic_b).unwrap(), 1);
}

#[test]
fn incoming_acceptance_persists_replay_metadata_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let local = other_public_key(8);
    let env = make_envelope([8u8; 32], [6u8; 32], random_public_key());
    {
        let store = MessageStore::open(dir.path().join("messages.db")).unwrap();
        assert_eq!(
            store.accept_incoming_message(&env, &local).unwrap(),
            IncomingMessageResult::Inserted
        );
        assert_eq!(
            store.accept_incoming_message(&env, &local).unwrap(),
            IncomingMessageResult::Duplicate
        );
    }
    let reopened = MessageStore::open(dir.path().join("messages.db")).unwrap();
    let replay = reopened
        .get_incoming_replay_metadata(&env.msg_id)
        .unwrap()
        .unwrap();
    assert_eq!(replay.receive_count, 2);
    assert_eq!(
        reopened.get_unread_count(&env.conversation_id).unwrap(),
        Some(1)
    );
}

#[test]
fn incoming_acceptance_rejects_tombstoned_message() {
    let store = MessageStore::memory().unwrap();
    let env = make_envelope([4u8; 32], [5u8; 32], random_public_key());
    store
        .insert_tombstone(&env.msg_id, &env.conversation_id, &env.author_user_id, &[])
        .unwrap();
    assert_eq!(
        store
            .accept_incoming_message(&env, &other_public_key(8))
            .unwrap(),
        IncomingMessageResult::Rejected
    );
    assert!(store
        .get_conversation_meta(&env.conversation_id)
        .unwrap()
        .is_none());
}

#[test]
fn incoming_acceptance_is_idempotent_and_detects_conflicts() {
    let store = MessageStore::memory().unwrap();
    let local = other_public_key(8);
    let remote = random_public_key();
    let conv = [9u8; 32];
    let env = make_envelope([7u8; 32], conv, remote);

    assert_eq!(
        store.accept_incoming_message(&env, &local).unwrap(),
        IncomingMessageResult::Inserted
    );
    let first = store.get_conversation_meta(&conv).unwrap().unwrap();
    assert_eq!(first.unread_count, 1);
    assert_eq!(
        store.accept_incoming_message(&env, &local).unwrap(),
        IncomingMessageResult::Duplicate
    );
    let second = store.get_conversation_meta(&conv).unwrap().unwrap();
    assert_eq!(second.unread_count, 1);
    assert_eq!(second.last_activity_at_ms, first.last_activity_at_ms);

    let mut conflict = env.clone();
    conflict.ciphertext = Bytes::from(vec![99, 98]);
    assert_eq!(
        store.accept_incoming_message(&conflict, &local).unwrap(),
        IncomingMessageResult::Conflict
    );
    assert_eq!(store.get_unread_count(&conv).unwrap(), Some(1));
}

fn random_public_key() -> PublicKey {
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = (i % 256) as u8;
    }
    PublicKey::from_bytes(&bytes).unwrap()
}

fn other_public_key(base: u8) -> PublicKey {
    // Derive a deterministic but valid Ed25519 public key from a seed.
    use iroh::SecretKey;
    let seed = [base; 32];
    SecretKey::from_bytes(&seed).public()
}

fn make_envelope(msg_id: [u8; 32], conversation_id: [u8; 32], author: PublicKey) -> StoredEnvelope {
    StoredEnvelope {
        msg_id,
        conversation_id,
        author_user_id: author,
        author_device_id: author,
        created_at_ms: 1000,
        expires_at_ms: 5000,
        ciphertext: Bytes::from(vec![1, 2, 3]),
        signature: [3u8; 64],
        acked_at_ms: None,
    }
}

// ── Existing tests ─────────────────────────────────────────────────

#[test]
fn test_store_idempotent_insert() {
    let store = MessageStore::memory().unwrap();

    let msg_id = [1u8; 32];
    let envelope = StoredEnvelope {
        msg_id,
        conversation_id: [2u8; 32],
        author_user_id: random_public_key(),
        author_device_id: random_public_key(),
        created_at_ms: 1000,
        expires_at_ms: 5000,
        ciphertext: Bytes::from(vec![1, 2, 3]),
        signature: [3u8; 64],
        acked_at_ms: None,
    };

    assert!(store.insert_inbox(&envelope).unwrap()); // new insert
    assert!(!store.insert_inbox(&envelope).unwrap()); // duplicate

    let fetched = store.get_inbox(&msg_id).unwrap().unwrap();
    assert_eq!(fetched.msg_id, envelope.msg_id);
}

#[test]
fn test_outbox_flow() {
    let store = MessageStore::memory().unwrap();

    let msg_id = [1u8; 32];
    let recipient = random_public_key();

    store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();

    let due = store.fetch_due_outbox(500).unwrap();
    assert!(due.is_empty());

    let due = store.fetch_due_outbox(1500).unwrap();
    assert_eq!(due.len(), 1);

    store
        .record_attempt(&msg_id, recipient, 3000, Some("timeout"))
        .unwrap();

    let due = store.fetch_due_outbox(1500).unwrap();
    assert!(due.is_empty()); // Next attempt is 3000

    store.mark_acked(&msg_id, recipient).unwrap();
    let due = store.fetch_due_outbox(3500).unwrap();
    assert!(due.is_empty()); // Acked messages shouldn't be retried
}

// ── Conversation meta table tests (Step 11) ────────────────────────

#[test]
fn test_conversation_meta_table_created_on_init() {
    let store = MessageStore::memory().unwrap();
    // Schema init should not error; meta table exists implicitly.
    // Verify by inserting a meta row manually.
    let conn = store.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO conversation_meta (conversation_id, last_activity_at_ms)
         VALUES (X'aa', 1000)",
        [],
    )
    .unwrap();
    let count: u32 = conn
        .query_row("SELECT COUNT(*) FROM conversation_meta", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_insert_inbox_returns_new_vs_duplicate() {
    let store = MessageStore::memory().unwrap();
    let env = make_envelope([1u8; 32], [2u8; 32], random_public_key());

    // First insert should return true
    assert!(store.insert_inbox(&env).unwrap());
    // Duplicate insert should return false
    assert!(!store.insert_inbox(&env).unwrap());
}

#[test]
fn test_insert_with_conversation_update_and_unread() {
    let store = MessageStore::memory().unwrap();
    let local = random_public_key();
    let remote = other_public_key(42);
    let conv_id = [2u8; 32];
    let msg_id1 = [1u8; 32];
    let msg_id2 = [2u8; 32];

    // 1. Remote sends a message → unread should be 1
    let env1 = make_envelope(msg_id1, conv_id, remote);
    assert!(store
        .insert_inbox_with_conversation_update(&env1, &local)
        .unwrap());
    let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
    assert_eq!(meta.unread_count, 1);
    assert!(!meta.is_muted);
    assert!(!meta.is_archived);
    assert!(!meta.is_deleted);

    // 2. Duplicate of same message → unread should NOT increment
    assert!(!store
        .insert_inbox_with_conversation_update(&env1, &local)
        .unwrap());
    let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
    assert_eq!(meta.unread_count, 1);

    // 3. Second remote message → unread should be 2
    let env2 = make_envelope(msg_id2, conv_id, remote);
    assert!(store
        .insert_inbox_with_conversation_update(&env2, &local)
        .unwrap());
    let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
    assert_eq!(meta.unread_count, 2);
}

#[test]
fn test_self_sent_does_not_increment_unread() {
    let store = MessageStore::memory().unwrap();
    let local = random_public_key();
    let conv_id = [2u8; 32];

    // Local user sends a message → unread should be 0
    let env = make_envelope([1u8; 32], conv_id, local);
    assert!(store
        .insert_inbox_with_conversation_update(&env, &local)
        .unwrap());
    let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
    assert_eq!(meta.unread_count, 0);
}

#[test]
fn test_mark_conversation_read() {
    let store = MessageStore::memory().unwrap();
    let local = random_public_key();
    let remote = other_public_key(42);
    let conv_id = [2u8; 32];

    // Send two messages from remote
    let env1 = make_envelope([1u8; 32], conv_id, remote);
    let env2 = make_envelope([2u8; 32], conv_id, remote);
    store
        .insert_inbox_with_conversation_update(&env1, &local)
        .unwrap();
    store
        .insert_inbox_with_conversation_update(&env2, &local)
        .unwrap();

    // Mark read
    let prev = store.mark_conversation_read(&conv_id).unwrap().unwrap();
    assert_eq!(prev, 2);

    // Unread should now be 0
    let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
    assert_eq!(meta.unread_count, 0);
}

#[test]
fn test_mark_read_non_existent_conversation() {
    let store = MessageStore::memory().unwrap();
    let conv_id = [99u8; 32];
    // No meta row yet → returns None
    let prev = store.mark_conversation_read(&conv_id).unwrap();
    assert!(prev.is_none());
}

#[test]
fn test_get_unread_count() {
    let store = MessageStore::memory().unwrap();
    let local = random_public_key();
    let remote = other_public_key(42);
    let conv_id = [2u8; 32];

    // No messages → no meta row → returns None
    assert!(store.get_unread_count(&conv_id).unwrap().is_none());

    // After a remote message → returns Some(1)
    let env = make_envelope([1u8; 32], conv_id, remote);
    store
        .insert_inbox_with_conversation_update(&env, &local)
        .unwrap();
    assert_eq!(store.get_unread_count(&conv_id).unwrap().unwrap(), 1);
}

#[test]
fn test_total_unread_count() {
    let store = MessageStore::memory().unwrap();
    let local = random_public_key();
    let remote = other_public_key(42);

    let conv_a = [1u8; 32];
    let conv_b = [2u8; 32];

    assert_eq!(store.total_unread_count().unwrap(), 0);

    // 2 unread in conv_a, 1 in conv_b
    store
        .insert_inbox_with_conversation_update(&make_envelope([1u8; 32], conv_a, remote), &local)
        .unwrap();
    store
        .insert_inbox_with_conversation_update(&make_envelope([2u8; 32], conv_a, remote), &local)
        .unwrap();
    store
        .insert_inbox_with_conversation_update(&make_envelope([3u8; 32], conv_b, remote), &local)
        .unwrap();

    assert_eq!(store.total_unread_count().unwrap(), 3);
}

#[test]
fn test_archive_and_unarchive() {
    let store = MessageStore::memory().unwrap();
    let conv_id = [2u8; 32];

    // Archive
    store.set_conversation_archived(&conv_id, true).unwrap();
    let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
    assert!(meta.is_archived);

    // Unarchive
    store.set_conversation_archived(&conv_id, false).unwrap();
    let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
    assert!(!meta.is_archived);
}

#[test]
fn test_mute_and_unmute() {
    let store = MessageStore::memory().unwrap();
    let conv_id = [2u8; 32];

    // Mute
    store.set_conversation_muted(&conv_id, true).unwrap();
    let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
    assert!(meta.is_muted);

    // Unmute
    store.set_conversation_muted(&conv_id, false).unwrap();
    let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
    assert!(!meta.is_muted);
}

#[test]
fn test_delete_conversation_removes_inbox_but_not_pending_outgoing() {
    let store = MessageStore::memory().unwrap();
    let local = random_public_key();
    let remote = other_public_key(42);
    let conv_id = [2u8; 32];
    let recipient = random_public_key();

    // Insert inbox messages
    let env1 = make_envelope([1u8; 32], conv_id, remote);
    let env2 = make_envelope([2u8; 32], conv_id, remote);
    store
        .insert_inbox_with_conversation_update(&env1, &local)
        .unwrap();
    store
        .insert_inbox_with_conversation_update(&env2, &local)
        .unwrap();

    // Enqueue a pending outgoing message for the same conversation
    store.enqueue_outbox(&[3u8; 32], recipient, 1000).unwrap();

    // Delete conversation
    let removed = store.delete_conversation(&conv_id).unwrap();
    assert_eq!(removed, 2); // Two inbox messages removed

    // Verify inbox is empty for this conversation
    assert!(store.get_inbox(&[1u8; 32]).unwrap().is_none());
    assert!(store.get_inbox(&[2u8; 32]).unwrap().is_none());

    // Verify outbox still has the pending message
    let due = store.fetch_due_outbox(2000).unwrap();
    assert_eq!(due.len(), 1);

    // Verify meta is soft-deleted
    let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
    assert!(meta.is_deleted);
}

#[test]
fn test_list_conversations_filters_correctly() {
    let store = MessageStore::memory().unwrap();
    let local = random_public_key();
    let remote = other_public_key(42);

    let conv_active = [1u8; 32];
    let conv_archived = [2u8; 32];

    // Create two conversations
    store
        .insert_inbox_with_conversation_update(
            &make_envelope([1u8; 32], conv_active, remote),
            &local,
        )
        .unwrap();
    store
        .insert_inbox_with_conversation_update(
            &make_envelope([2u8; 32], conv_archived, remote),
            &local,
        )
        .unwrap();

    // Archive the second one
    store
        .set_conversation_archived(&conv_archived, true)
        .unwrap();

    // Without archived → only 1
    let list = store.list_conversations(false).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].conversation_id, conv_active);

    // With archived → 2
    let list = store.list_conversations(true).unwrap();
    assert_eq!(list.len(), 2);

    // Delete one → it should disappear from both lists
    store.delete_conversation(&conv_active).unwrap();
    let list = store.list_conversations(true).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].conversation_id, conv_archived);
}

#[test]
fn test_conversation_state_survives_reopen() {
    // Use a temp file so we can re-open
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("store.db");

    let local = random_public_key();
    let remote = other_public_key(42);
    let conv_id = [2u8; 32];

    // First session
    {
        let store = MessageStore::open(&db_path).unwrap();
        store
            .insert_inbox_with_conversation_update(
                &make_envelope([1u8; 32], conv_id, remote),
                &local,
            )
            .unwrap();
        store
            .insert_inbox_with_conversation_update(
                &make_envelope([2u8; 32], conv_id, remote),
                &local,
            )
            .unwrap();

        // Mark one as read with preview
        store.mark_conversation_read(&conv_id).unwrap();
        store.set_conversation_archived(&conv_id, true).unwrap();
        store.set_conversation_muted(&conv_id, true).unwrap();

        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert_eq!(meta.unread_count, 0);
        assert!(meta.is_archived);
        assert!(meta.is_muted);
    }

    // Second session — reopen
    {
        let store = MessageStore::open(&db_path).unwrap();

        // All state should be restored
        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert_eq!(meta.unread_count, 0);
        assert!(meta.is_archived);
        assert!(meta.is_muted);
        assert!(!meta.is_deleted);

        // Inbox messages should be present
        assert!(store.get_inbox(&[1u8; 32]).unwrap().is_some());
        assert!(store.get_inbox(&[2u8; 32]).unwrap().is_some());
    }
}

#[test]
fn test_hard_delete_conversation_removes_everything() {
    let store = MessageStore::memory().unwrap();
    let local = random_public_key();
    let remote = other_public_key(42);
    let conv_id = [2u8; 32];
    let recipient = random_public_key();

    // Insert messages
    let env = make_envelope([1u8; 32], conv_id, remote);
    store
        .insert_inbox_with_conversation_update(&env, &local)
        .unwrap();

    // Enqueue a pending outgoing
    store.enqueue_outbox(&[1u8; 32], recipient, 1000).unwrap();

    // Hard delete
    let removed = store.hard_delete_conversation(&conv_id).unwrap();
    assert_eq!(removed, 1);

    // Meta row is gone
    assert!(store.get_conversation_meta(&conv_id).unwrap().is_none());

    // Inbox is empty
    assert!(store.get_inbox(&[1u8; 32]).unwrap().is_none());

    // Outbox is empty too (the msg_id matched the inbox query)
    let due = store.fetch_due_outbox(2000).unwrap();
    assert_eq!(due.len(), 0);
}

#[test]
fn test_duplicate_remote_does_not_increment_unread() {
    let store = MessageStore::memory().unwrap();
    let local = random_public_key();
    let remote = other_public_key(42);
    let conv_id = [2u8; 32];

    let env = make_envelope([1u8; 32], conv_id, remote);

    // First insert → unread = 1
    store
        .insert_inbox_with_conversation_update(&env, &local)
        .unwrap();
    assert_eq!(store.get_unread_count(&conv_id).unwrap().unwrap(), 1);

    // Duplicate (e.g. from restart replay) → unread stays 1
    store
        .insert_inbox_with_conversation_update(&env, &local)
        .unwrap();
    assert_eq!(store.get_unread_count(&conv_id).unwrap().unwrap(), 1);
}

#[test]
fn test_delete_pending_outgoing_explicit() {
    let store = MessageStore::memory().unwrap();
    let local = random_public_key();
    let remote = other_public_key(42);
    let conv_id = [2u8; 32];
    let recipient = random_public_key();

    // Insert inbox message to establish the conversation
    let env = make_envelope([1u8; 32], conv_id, remote);
    store
        .insert_inbox_with_conversation_update(&env, &local)
        .unwrap();

    // Enqueue pending outgoing
    store.enqueue_outbox(&[1u8; 32], recipient, 1000).unwrap();

    // Explicitly delete pending outgoing for this conversation
    let removed = store
        .delete_pending_outgoing_for_conversation(&conv_id)
        .unwrap();
    assert_eq!(removed, 1);

    // Outbox should be empty now
    let due = store.fetch_due_outbox(2000).unwrap();
    assert_eq!(due.len(), 0);
}

#[test]
fn test_update_last_message_preview() {
    let store = MessageStore::memory().unwrap();
    let local = random_public_key();
    let remote = other_public_key(42);
    let conv_id = [2u8; 32];

    let env = make_envelope([1u8; 32], conv_id, remote);
    store
        .insert_inbox_with_conversation_update(&env, &local)
        .unwrap();

    // Initial preview is "[3 bytes]"
    let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
    assert_eq!(meta.last_message_preview, "[3 bytes]");

    // Update to actual decrypted preview
    store
        .update_last_message_preview(&conv_id, "Hello, world!")
        .unwrap();
    let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
    assert_eq!(meta.last_message_preview, "Hello, world!");
}

#[test]
fn test_last_author_and_message_id_tracking() {
    let store = MessageStore::memory().unwrap();
    let local = random_public_key();
    let remote = other_public_key(42);
    let conv_id = [2u8; 32];

    // First message from remote
    let env1 = make_envelope([1u8; 32], conv_id, remote);
    store
        .insert_inbox_with_conversation_update(&env1, &local)
        .unwrap();

    let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
    assert_eq!(meta.last_message_id, Some([1u8; 32]));
    assert_eq!(meta.last_author_user_id, Some(remote));

    // Second message overwrites
    let env2 = make_envelope([2u8; 32], conv_id, remote);
    store
        .insert_inbox_with_conversation_update(&env2, &local)
        .unwrap();

    let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
    assert_eq!(meta.last_message_id, Some([2u8; 32]));
}

// ── Deletion and tombstone tests (Step 12) ────────────────────────

#[test]
fn test_delete_message_local() {
    let store = MessageStore::memory().unwrap();
    let msg_id = [1u8; 32];
    let conv_id = [2u8; 32];
    let author = random_public_key();

    // Insert a message
    let env = make_envelope(msg_id, conv_id, author);
    assert!(store.insert_inbox(&env).unwrap());
    assert!(store.get_inbox(&msg_id).unwrap().is_some());

    // Delete it locally
    assert!(store.delete_message(&msg_id).unwrap());

    // Inbox should be gone
    assert!(store.get_inbox(&msg_id).unwrap().is_none());

    // Tombstone should exist
    assert!(store.is_tombstoned(&msg_id).unwrap());

    // Cannot re-insert a tombstoned message
    assert!(!store.insert_inbox(&env).unwrap());

    // Deleting a non-existent message returns false
    assert!(!store.delete_message(&[99u8; 32]).unwrap());
}

#[test]
fn test_delete_message_with_pending_outbound() {
    let store = MessageStore::memory().unwrap();
    let msg_id = [1u8; 32];
    let conv_id = [2u8; 32];
    let author = random_public_key();
    let recipient = random_public_key();

    let env = make_envelope(msg_id, conv_id, author);
    assert!(store.insert_inbox(&env).unwrap());

    // Enqueue pending outbound
    store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();

    // Delete should cancel the pending outbound
    assert!(store.delete_message(&msg_id).unwrap());

    // Outbox should not have this message as due (it's now Expired)
    let due = store.fetch_due_outbox(2000).unwrap();
    assert!(!due.iter().any(|r| r.msg_id == msg_id));
}

#[test]
fn test_cancel_pending_outbound() {
    let store = MessageStore::memory().unwrap();
    let msg_id = [1u8; 32];
    let recipient = other_public_key(1);
    let recipient2 = other_public_key(2);

    // Enqueue pending outbounds for same message to two recipients
    store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();
    store.enqueue_outbox(&msg_id, recipient2, 1000).unwrap();

    // Cancel pending outbound
    let affected = store.cancel_pending_outbound(&msg_id).unwrap();
    assert_eq!(affected, 2);

    // Should not appear in due outbox
    let due = store.fetch_due_outbox(2000).unwrap();
    assert!(!due.iter().any(|r| r.msg_id == msg_id));
}

#[test]
fn test_cancel_pending_outbound_already_acked() {
    let store = MessageStore::memory().unwrap();
    let msg_id = [1u8; 32];
    let recipient = random_public_key();

    store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();
    store.mark_acked(&msg_id, recipient).unwrap();

    // Canceling an already-acked message should not affect it
    let affected = store.cancel_pending_outbound(&msg_id).unwrap();
    assert_eq!(affected, 0);

    // Should still not appear in due (it's acked)
    let due = store.fetch_due_outbox(2000).unwrap();
    assert!(!due.iter().any(|r| r.msg_id == msg_id));
}

#[test]
fn test_insert_tombstone_remote() {
    let store = MessageStore::memory().unwrap();
    let msg_id = [1u8; 32];
    let conv_id = [2u8; 32];
    let author = random_public_key();
    let signature = [4u8; 64];

    // Insert a message
    let env = make_envelope(msg_id, conv_id, author);
    assert!(store.insert_inbox(&env).unwrap());
    assert!(store.get_inbox(&msg_id).unwrap().is_some());

    // Insert a remote tombstone
    let is_new = store
        .insert_tombstone(&msg_id, &conv_id, &author, &signature)
        .unwrap();
    assert!(is_new);

    // Inbox should be gone
    assert!(store.get_inbox(&msg_id).unwrap().is_none());

    // Tombstone should exist
    assert!(store.is_tombstoned(&msg_id).unwrap());

    // Cannot re-insert
    assert!(!store.insert_inbox(&env).unwrap());

    // Duplicate tombstone returns false
    let is_new2 = store
        .insert_tombstone(&msg_id, &conv_id, &author, &signature)
        .unwrap();
    assert!(!is_new2);
}

#[test]
fn test_tombstone_rejects_backfill_after_restart() {
    // Simulate: message deleted, DB reopened, backfill tries to re-insert.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("store.db");

    let msg_id = [1u8; 32];
    let conv_id = [2u8; 32];
    let author = random_public_key();

    // First session: insert, then delete
    {
        let store = MessageStore::open(&db_path).unwrap();
        let env = make_envelope(msg_id, conv_id, author);
        assert!(store.insert_inbox(&env).unwrap());
        assert!(store.delete_message(&msg_id).unwrap());
    }

    // Second session: try to re-insert (simulating backfill after restart)
    {
        let store = MessageStore::open(&db_path).unwrap();
        let env = make_envelope(msg_id, conv_id, author);

        // Tombstone should block re-insertion
        assert!(!store.insert_inbox(&env).unwrap());
        assert!(!store
            .insert_inbox_with_conversation_update(&env, &author)
            .unwrap());

        // get_inbox should return None
        assert!(store.get_inbox(&msg_id).unwrap().is_none());

        // is_tombstoned should still be true
        assert!(store.is_tombstoned(&msg_id).unwrap());
    }
}

#[test]
fn test_get_inbox_returns_none_for_tombstoned() {
    let store = MessageStore::memory().unwrap();
    let msg_id = [1u8; 32];
    let conv_id = [2u8; 32];
    let author = random_public_key();

    let env = make_envelope(msg_id, conv_id, author);
    assert!(store.insert_inbox(&env).unwrap());
    assert!(store.get_inbox(&msg_id).unwrap().is_some());

    // Delete
    assert!(store.delete_message(&msg_id).unwrap());

    // get_inbox should return None even though the msg still exists in tombstone table
    assert!(store.get_inbox(&msg_id).unwrap().is_none());
}

#[test]
fn test_is_tombstoned_non_existent() {
    let store = MessageStore::memory().unwrap();
    // Non-existent message is not tombstoned
    assert!(!store.is_tombstoned(&[42u8; 32]).unwrap());
}

#[test]
fn test_record_attempt_guards_against_expired() {
    let store = MessageStore::memory().unwrap();
    let msg_id = [1u8; 32];
    let conv_id = [2u8; 32];
    let author = random_public_key();
    let recipient = random_public_key();

    // Insert and enqueue
    let env = make_envelope(msg_id, conv_id, author);
    assert!(store.insert_inbox(&env).unwrap());
    store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();

    // Cancel (set to Expired)
    store.cancel_pending_outbound(&msg_id).unwrap();

    // record_attempt should not resurrect an Expired message
    store
        .record_attempt(&msg_id, recipient, 2000, Some("timeout"))
        .unwrap();

    // Should still not appear as due
    let due = store.fetch_due_outbox(3000).unwrap();
    assert!(!due.iter().any(|r| r.msg_id == msg_id));
}

// ── Additional edge-case tests (Step 12) ──────────────────────────

#[test]
fn test_tombstone_survives_reopen() {
    // Verify tombstones persist across store reopens.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("store.db");
    let msg_id = [1u8; 32];
    let conv_id = [2u8; 32];
    let author = random_public_key();

    // First session: insert and delete
    {
        let store = MessageStore::open(&db_path).unwrap();
        let env = make_envelope(msg_id, conv_id, author);
        assert!(store.insert_inbox(&env).unwrap());
        assert!(store.delete_message(&msg_id).unwrap());
        assert!(store.is_tombstoned(&msg_id).unwrap());
    }

    // Second session: tombstone should still block re-insertion
    {
        let store = MessageStore::open(&db_path).unwrap();
        assert!(store.is_tombstoned(&msg_id).unwrap());
        let env = make_envelope(msg_id, conv_id, author);
        assert!(!store.insert_inbox(&env).unwrap());
        assert!(!store
            .insert_inbox_with_conversation_update(&env, &author)
            .unwrap());
        assert!(store.get_inbox(&msg_id).unwrap().is_none());
    }
}

#[test]
fn test_remote_tombstone_survives_reopen() {
    // Verify remote tombstones persist across store reopens.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("store.db");
    let msg_id = [1u8; 32];
    let conv_id = [2u8; 32];
    let author = random_public_key();
    let signature = [4u8; 64];

    // First session: insert and apply remote tombstone
    {
        let store = MessageStore::open(&db_path).unwrap();
        let env = make_envelope(msg_id, conv_id, author);
        assert!(store.insert_inbox(&env).unwrap());
        assert!(store
            .insert_tombstone(&msg_id, &conv_id, &author, &signature)
            .unwrap());
        assert!(store.is_tombstoned(&msg_id).unwrap());
    }

    // Second session: tombstone persists
    {
        let store = MessageStore::open(&db_path).unwrap();
        assert!(store.is_tombstoned(&msg_id).unwrap());
        assert!(store.get_inbox(&msg_id).unwrap().is_none());
    }
}

#[test]
fn test_remote_tombstone_cancels_pending_outbound() {
    // A remote delete tombstone should cancel pending outbound deliveries.
    let store = MessageStore::memory().unwrap();
    let msg_id = [1u8; 32];
    let conv_id = [2u8; 32];
    let author = random_public_key();
    let recipient = random_public_key();
    let signature = [4u8; 64];

    let env = make_envelope(msg_id, conv_id, author);
    assert!(store.insert_inbox(&env).unwrap());

    // Enqueue pending outbound
    store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();

    // Apply remote tombstone
    assert!(store
        .insert_tombstone(&msg_id, &conv_id, &author, &signature)
        .unwrap());

    // Outbound should be cancelled (not due for retry)
    let due = store.fetch_due_outbox(2000).unwrap();
    assert!(!due.iter().any(|r| r.msg_id == msg_id));
}

#[test]
fn test_ack_tombstoned_message_is_safe() {
    // ACKing a message that has been tombstoned should not resurrect it.
    let store = MessageStore::memory().unwrap();
    let msg_id = [1u8; 32];
    let conv_id = [2u8; 32];
    let author = random_public_key();
    let recipient = random_public_key();

    let env = make_envelope(msg_id, conv_id, author);
    assert!(store.insert_inbox(&env).unwrap());
    store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();

    // Delete locally (tombstone)
    assert!(store.delete_message(&msg_id).unwrap());
    assert!(store.is_tombstoned(&msg_id).unwrap());

    // ACK should still work on the outbox row (doesn't touch tombstone)
    store.mark_acked(&msg_id, recipient).unwrap();

    // Message should remain tombstoned
    assert!(store.is_tombstoned(&msg_id).unwrap());
    assert!(store.get_inbox(&msg_id).unwrap().is_none());

    // Outbox should still not show as due (it's acked)
    let due = store.fetch_due_outbox(2000).unwrap();
    assert!(!due.iter().any(|r| r.msg_id == msg_id));
}

#[test]
fn test_local_and_remote_tombstones_coexist() {
    // Verify the store handles both local and remote tombstones.
    let store = MessageStore::memory().unwrap();
    let author = random_public_key();
    let conv_id = [2u8; 32];

    // Two messages in same conversation
    let msg_local = [1u8; 32];
    let msg_remote = [2u8; 32];
    let env_local = make_envelope(msg_local, conv_id, author);
    let env_remote = make_envelope(msg_remote, conv_id, author);
    assert!(store.insert_inbox(&env_local).unwrap());
    assert!(store.insert_inbox(&env_remote).unwrap());

    // Delete one locally
    assert!(store.delete_message(&msg_local).unwrap());

    // Tombstone the other remotely
    let signature = [5u8; 64];
    assert!(store
        .insert_tombstone(&msg_remote, &conv_id, &author, &signature)
        .unwrap());

    // Both should be tombstoned
    assert!(store.is_tombstoned(&msg_local).unwrap());
    assert!(store.is_tombstoned(&msg_remote).unwrap());

    // Neither should be re-insertable
    assert!(!store.insert_inbox(&env_local).unwrap());
    assert!(!store.insert_inbox(&env_remote).unwrap());
}

#[test]
fn test_durable_replay_rejects_tombstoned_message() {
    // Simulate: message is received, then a duplicate arrives after
    // the message was locally deleted. The duplicate must be rejected.
    let store = MessageStore::memory().unwrap();
    let msg_id = [1u8; 32];
    let conv_id = [2u8; 32];
    let author = random_public_key();

    let env = make_envelope(msg_id, conv_id, author);

    // Insert and then delete
    assert!(store.insert_inbox(&env).unwrap());
    assert!(store.delete_message(&msg_id).unwrap());

    // A duplicate arriving later should be rejected (tombstone check)
    assert!(!store.insert_inbox(&env).unwrap());
}

#[test]
fn test_backfill_after_local_delete_is_rejected() {
    // Simulate: backfill tries to re-insert a message that was
    // locally deleted before a restart.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("store.db");
    let msg_id = [1u8; 32];
    let conv_id = [2u8; 32];
    let author = random_public_key();

    // Session 1: insert and locally delete
    {
        let store = MessageStore::open(&db_path).unwrap();
        let env = make_envelope(msg_id, conv_id, author);
        assert!(store.insert_inbox(&env).unwrap());
        assert!(store.delete_message(&msg_id).unwrap());
    }

    // Session 2: backfill tries to insert the same message
    {
        let store = MessageStore::open(&db_path).unwrap();
        let env = make_envelope(msg_id, conv_id, author);

        // Both insert paths must reject
        assert!(!store.insert_inbox(&env).unwrap());
        assert!(!store
            .insert_inbox_with_conversation_update(&env, &author)
            .unwrap());
    }
}

#[test]
fn test_redelivery_after_remote_tombstone_is_rejected() {
    // Simulate: a remote delete tombstone is received, then the
    // same message is redelivered (e.g. from another device's sync).
    let store = MessageStore::memory().unwrap();
    let msg_id = [1u8; 32];
    let conv_id = [2u8; 32];
    let author = random_public_key();
    let signature = [6u8; 64];

    let env = make_envelope(msg_id, conv_id, author);

    // Insert, then apply remote tombstone
    assert!(store.insert_inbox(&env).unwrap());
    assert!(store
        .insert_tombstone(&msg_id, &conv_id, &author, &signature)
        .unwrap());

    // Redelivery attempt should be rejected
    assert!(!store.insert_inbox(&env).unwrap());
    assert!(!store
        .insert_inbox_with_conversation_update(&env, &author)
        .unwrap());
}

#[test]
fn test_cancel_pending_outbound_on_tombstoned_message() {
    // Cancelling pending outbound on a message that was already
    // tombstoned should work (idempotent).
    let store = MessageStore::memory().unwrap();
    let msg_id = [1u8; 32];
    let conv_id = [2u8; 32];
    let author = random_public_key();
    let recipient = random_public_key();

    let env = make_envelope(msg_id, conv_id, author);
    assert!(store.insert_inbox(&env).unwrap());
    store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();

    // Delete (creates tombstone + cancels outbound)
    assert!(store.delete_message(&msg_id).unwrap());

    // Cancel again — should be a no-op (already Expired)
    let affected = store.cancel_pending_outbound(&msg_id).unwrap();
    assert_eq!(affected, 0);
}

#[test]
fn test_get_inbox_preserves_non_tombstoned_messages() {
    // Verify that get_inbox still works for non-tombstoned messages
    // when other messages in the same conversation are tombstoned.
    let store = MessageStore::memory().unwrap();
    let author = random_public_key();
    let conv_id = [2u8; 32];

    let msg_alive = [1u8; 32];
    let msg_dead = [2u8; 32];

    let env_alive = make_envelope(msg_alive, conv_id, author);
    let env_dead = make_envelope(msg_dead, conv_id, author);

    assert!(store.insert_inbox(&env_alive).unwrap());
    assert!(store.insert_inbox(&env_dead).unwrap());

    // Delete one
    assert!(store.delete_message(&msg_dead).unwrap());

    // Alive message should still be retrievable
    assert!(store.get_inbox(&msg_alive).unwrap().is_some());

    // Dead message should not
    assert!(store.get_inbox(&msg_dead).unwrap().is_none());
}

#[test]
fn received_message_survives_store_reopen() {
    let path = std::env::temp_dir().join(format!(
        "boru-message-store-durability-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let hash = [7u8; 32];
    let topic = [8u8; 32];
    let sender = [9u8; 32];
    let local = [0u8; 32];
    {
        let store = MessageStore::open(&path).unwrap();
        assert!(store
            .insert_chat_message(
                &hash,
                &topic,
                &sender,
                42_000,
                "text",
                "received group message",
                Some(b"authenticated-wire-payload"),
                None,
                &local,
            )
            .unwrap());
    }
    {
        let reopened = MessageStore::open(&path).unwrap();
        let row = reopened.find_message_by_hash(&hash).unwrap().unwrap();
        assert_eq!(row.body, "received group message");
        assert_eq!(row.topic, topic);
        assert_eq!(row.sender, sender);
        assert_eq!(
            row.signed_bytes.as_deref(),
            Some(b"authenticated-wire-payload".as_slice())
        );
    }
    let _ = std::fs::remove_file(path);
}
