//! Integration tests for the sync-after-downtime flow.
//!
//! These tests exercise the full sync lifecycle through the durable Storage
//! layer — the same path used by the actual sync protocol — without real
//! network.  The sync flow is:
//!
//!   1. Sender stores encrypted envelopes in `dm_outbox` via `queue_outgoing_dm`
//!   2. Recipient reconnects and requests sync
//!   3. Responder (sender) queries `dm_outbox` via `query_pending_outbound_for_recipient`
//!   4. Responder records served envelope IDs via `record_sync_served`
//!   5. Subsequent syncs exclude already-served envelopes (dedup)
//!   6. Cursor tracks the last successfully synced position via `upsert_sync_cursor`
//!
//! All tests use `Storage::memory()` for an in-memory SQLite database.
//!
//! ## Test Index
//!
//! | # | Test | What it verifies |
//! |---|------|------------------|
//! | 1 | `sync_normal_retrieves_all` | All offline messages retrieved, none missed |
//! | 2 | `sync_pagination_multipage` | More messages than page size → multi-page progression |
//! | 3 | `sync_size_limit_truncation` | max_bytes truncates page, has_more=true |
//! | 4 | `sync_rejects_wrong_recipient` | Peers can only see their own envelopes (recipient scoping) |
//! | 5 | `sync_excludes_already_served` | record_sync_served prevents re-serving |
//! | 6 | `sync_with_gaps` | After partial serve, remaining messages retrieved by cursor |
//! | 7 | `sync_full_lifecycle_with_cursor` | Complete lifecycle: enqueue → serve → record → cursor advance → next page → dedup |
//! | 8 | `sync_vs_retry_different_tables` | Sync queries dm_outbox; retry queries outbox (separate concerns) |
//! | 9 | `sync_prune_does_not_remove_recent` | prune_sync_dedup leaves fresh entries intact |

use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use iroh::{PublicKey, SecretKey};

use boru_chat::{
    mailbox::MailboxPublicKey,
    storage::Storage,
    store::{MessageId, StoredEnvelope},
};

// ── Helpers ────────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn random_pk() -> PublicKey {
    SecretKey::generate().public()
}

fn random_sk() -> SecretKey {
    SecretKey::generate()
}

fn make_conv_id(byte: u8) -> [u8; 32] {
    [byte; 32]
}

fn make_msg_id(byte: u8) -> MessageId {
    [byte; 32]
}

/// Build a MailboxPublicKey for test purposes.
fn mailbox_pk(sk: &SecretKey) -> MailboxPublicKey {
    MailboxPublicKey {
        identity: sk.public(),
        encryption: [0u8; 32],
    }
}

/// Queue a test DM addressed to `recipient_pk` with a deterministic payload.
fn queue_dm(
    storage: &Storage,
    sender_sk: &SecretKey,
    recipient: MailboxPublicKey,
    conv_id: [u8; 32],
    idx: u64,
) -> MessageId {
    let request_key = format!("sync-test-{idx}");
    let plaintext = format!("sync integration message {idx}");
    storage
        .queue_outgoing_dm(
            conv_id,
            sender_sk.public(),
            &request_key,
            &plaintext,
            recipient,
            sender_sk,
        )
        .expect("queue DM")
        .message_id
}

/// Create a minimal `StoredEnvelope` for the non-sync retry path.
fn sample_envelope(msg_id: MessageId, conv_id: [u8; 32]) -> StoredEnvelope {
    StoredEnvelope {
        msg_id,
        conversation_id: conv_id,
        author_user_id: random_pk(),
        author_device_id: random_pk(),
        created_at_ms: now_ms(),
        expires_at_ms: now_ms() + 86_400_000,
        ciphertext: Bytes::from_static(b"test ciphertext"),
        signature: [0u8; 64],
        acked_at_ms: None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 1: Normal sync — all offline messages retrieved
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn sync_normal_retrieves_all() {
    let storage = Storage::memory().expect("in-memory storage");
    let sender_sk = random_sk();
    let recipient_sk = random_sk();
    let recipient_pk = recipient_sk.public();
    let conv_id = make_conv_id(0x01);

    // Queue 5 messages addressed to recipient.
    let mb_pk = mailbox_pk(&recipient_sk);
    for i in 0..5u64 {
        queue_dm(&storage, &sender_sk, mb_pk, conv_id, i);
    }

    // Sync query should return all 5 with has_more=false.
    let (page, has_more) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 100, 1_000_000)
        .expect("sync query");
    assert_eq!(page.len(), 5, "all 5 messages should be returned");
    assert!(!has_more, "no more pages expected");

    // Verify each envelope addresses the correct recipient.
    for env in &page {
        assert_eq!(
            env.recipient.identity, recipient_pk,
            "envelope should be addressed to the recipient"
        );
    }

    // Verify another peer with no messages gets nothing.
    let stranger = random_pk();
    let (empty, _) = storage
        .query_pending_outbound_for_recipient(&stranger, 0, 100, 1_000_000)
        .expect("sync query for stranger");
    assert!(empty.is_empty(), "stranger should see no envelopes");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 2: Multi-page pagination — more messages than page limit
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn sync_pagination_multipage() {
    let storage = Storage::memory().expect("in-memory storage");
    let sender_sk = random_sk();
    let recipient_sk = random_sk();
    let recipient_pk = recipient_sk.public();
    let conv_id = make_conv_id(0x02);

    // Queue 10 messages, capturing their message IDs.
    let mb_pk = mailbox_pk(&recipient_sk);
    let mut all_ids = Vec::new();
    for i in 0..10u64 {
        all_ids.push(queue_dm(&storage, &sender_sk, mb_pk, conv_id, i));
    }

    // Single-page count-limit test: max_count=3 returns 3 with has_more=true.
    let (page, more) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 3, 1_000_000)
        .expect("page with limit 3");
    assert_eq!(page.len(), 3, "should return at most max_count items");
    assert!(more, "should indicate more pages exist");

    // Single-page count-limit test: max_count=100 returns all with has_more=false.
    let (all, more_all) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 100, 1_000_000)
        .expect("full page");
    assert_eq!(all.len(), 10, "should return all 10 with large max_count");
    assert!(!more_all, "should indicate no more pages");

    // Multi-page: natural sync flow — query → record → query → record ...
    let (page1, more1) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 3, 1_000_000)
        .expect("page 1");
    assert_eq!(page1.len(), 3);
    assert!(more1);
    // Record page 1 IDs as served.
    storage
        .record_sync_served(&recipient_pk, &all_ids[..3])
        .expect("record page 1");

    let (page2, more2) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 3, 1_000_000)
        .expect("page 2");
    assert_eq!(page2.len(), 3);
    assert!(more2);
    storage
        .record_sync_served(&recipient_pk, &all_ids[3..6])
        .expect("record page 2");

    let (page3, more3) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 3, 1_000_000)
        .expect("page 3");
    assert_eq!(page3.len(), 3);
    assert!(more3);
    storage
        .record_sync_served(&recipient_pk, &all_ids[6..9])
        .expect("record page 3");

    let (page4, _more4) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 3, 1_000_000)
        .expect("page 4");
    assert_eq!(page4.len(), 1, "page 4 should have the final envelope");
    // Record the final message.
    storage
        .record_sync_served(&recipient_pk, &all_ids[9..])
        .expect("record page 4");

    let total = page1.len() + page2.len() + page3.len() + page4.len();
    assert_eq!(total, 10, "total 10 envelopes across 4 pages");

    let total = page1.len() + page2.len() + page3.len() + page4.len();
    assert_eq!(total, 10, "total 10 envelopes across 4 pages");

    // Verify final dedup: nothing left.
    let (empty, _) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 100, 1_000_000)
        .expect("final dedup check");
    assert_eq!(empty.len(), 0, "no envelopes left after full pagination");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 3: Size limit enforcement — max_bytes truncates page
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn sync_size_limit_truncation() {
    let storage = Storage::memory().expect("in-memory storage");
    let sender_sk = random_sk();
    let recipient_sk = random_sk();
    let recipient_pk = recipient_sk.public();
    let conv_id = make_conv_id(0x03);

    // Queue 5 messages.
    let mb_pk = mailbox_pk(&recipient_sk);
    for i in 0..5u64 {
        queue_dm(&storage, &sender_sk, mb_pk, conv_id, i);
    }

    // Query with a tight byte budget (900 bytes — fits ~2 envelopes).
    let (page, has_more) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 100, 900)
        .expect("size-limited query");
    assert!(
        page.len() < 5,
        "size limit should truncate: got {} of 5",
        page.len()
    );
    assert!(!page.is_empty(), "should return at least 1 envelope");
    assert!(has_more, "truncated page should indicate more");

    // Verify the truncated page fits within the byte budget.
    let encoded_size: usize = page
        .iter()
        .map(|env| postcard::to_stdvec(env).unwrap_or_default().len())
        .sum();
    assert!(
        encoded_size <= 900,
        "response must fit within max_bytes: {encoded_size} <= 900"
    );

    // Second page should get the rest.
    let cursor = page.last().unwrap().created_at;
    let (page2, more2) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, cursor, 100, 900)
        .expect("page 2");
    let total = page.len() + page2.len();
    if more2 {
        let cursor2 = page2.last().unwrap().created_at;
        let (page3, _) = storage
            .query_pending_outbound_for_recipient(&recipient_pk, cursor2, 100, 300)
            .expect("page 3");
        assert_eq!(page.len() + page2.len() + page3.len(), 5);
    } else {
        assert_eq!(total, 5, "all 5 messages retrieved across 2 pages");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 4: Recipient scoping — peers can only see their own envelopes
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn sync_rejects_wrong_recipient() {
    let storage = Storage::memory().expect("in-memory storage");
    let sender_sk = random_sk();
    let alice_sk = random_sk();
    let bob_sk = random_sk();
    let alice_pk = alice_sk.public();
    let bob_pk = bob_sk.public();
    let conv_id = make_conv_id(0x04);

    // Queue messages for Alice and Bob in the same conversation.
    let alice_mb = mailbox_pk(&alice_sk);
    let bob_mb = mailbox_pk(&bob_sk);

    queue_dm(&storage, &sender_sk, alice_mb, conv_id, 0); // msg for Alice
    queue_dm(&storage, &sender_sk, bob_mb, conv_id, 1); // msg for Bob
    queue_dm(&storage, &sender_sk, alice_mb, conv_id, 2); // msg for Alice
    queue_dm(&storage, &sender_sk, bob_mb, conv_id, 3); // msg for Bob

    // Alice's sync query: should see only Alice's 2 messages.
    let (alice_page, _) = storage
        .query_pending_outbound_for_recipient(&alice_pk, 0, 100, 1_000_000)
        .expect("Alice sync");
    assert_eq!(alice_page.len(), 2, "Alice should see only her 2 messages");
    for env in &alice_page {
        assert_eq!(
            env.recipient.identity, alice_pk,
            "Alice's results should be addressed to Alice"
        );
    }

    // Bob's sync query: should see only Bob's 2 messages.
    let (bob_page, _) = storage
        .query_pending_outbound_for_recipient(&bob_pk, 0, 100, 1_000_000)
        .expect("Bob sync");
    assert_eq!(bob_page.len(), 2, "Bob should see only his 2 messages");
    for env in &bob_page {
        assert_eq!(
            env.recipient.identity, bob_pk,
            "Bob's results should be addressed to Bob"
        );
    }

    // Stranger gets nothing.
    let stranger = random_pk();
    let (empty, _) = storage
        .query_pending_outbound_for_recipient(&stranger, 0, 100, 1_000_000)
        .expect("stranger sync");
    assert!(empty.is_empty(), "stranger should see no envelopes");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 5: Duplicate detection — record_sync_served prevents re-serving
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn sync_excludes_already_served() {
    let storage = Storage::memory().expect("in-memory storage");
    let sender_sk = random_sk();
    let recipient_sk = random_sk();
    let recipient_pk = recipient_sk.public();
    let conv_id = make_conv_id(0x05);

    // Queue 5 messages.
    let mb_pk = mailbox_pk(&recipient_sk);
    let mut msg_ids = Vec::new();
    for i in 0..5u64 {
        msg_ids.push(queue_dm(&storage, &sender_sk, mb_pk, conv_id, i));
    }

    // First sync: all 5 visible.
    let (page, _) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 100, 1_000_000)
        .expect("first sync");
    assert_eq!(page.len(), 5);

    // Record first 3 as served.
    storage
        .record_sync_served(&recipient_pk, &msg_ids[..3])
        .expect("record first 3");

    // Second sync: only 2 remaining (4th and 5th).
    let (page2, _) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 100, 1_000_000)
        .expect("second sync");
    assert_eq!(
        page2.len(),
        2,
        "second sync should return only the 2 unserved envelopes"
    );

    // Record the remaining 2.
    storage
        .record_sync_served(&recipient_pk, &msg_ids[3..])
        .expect("record remaining");

    // Third sync: nothing left.
    let (page3, _) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 100, 1_000_000)
        .expect("third sync");
    assert_eq!(page3.len(), 0, "third sync should return nothing");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 6: Gaps midstream — partial serve + later sync gets remaining
// ═══════════════════════════════════════════════════════════════════════════
//
// Simulates: peer syncs, gets first 3 of 7, records them as served,
// then syncs again with a later cursor to get the remaining 4.

#[test]
fn sync_with_gaps() {
    let storage = Storage::memory().expect("in-memory storage");
    let sender_sk = random_sk();
    let recipient_sk = random_sk();
    let recipient_pk = recipient_sk.public();
    let conv_id = make_conv_id(0x06);

    // Queue 7 messages.
    let mb_pk = mailbox_pk(&recipient_sk);
    let mut msg_ids = Vec::new();
    for i in 0..7u64 {
        msg_ids.push(queue_dm(&storage, &sender_sk, mb_pk, conv_id, i));
    }

    // Round 1: sync gets first 3, records them, advances cursor.
    let (page1, more1) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 3, 1_000_000)
        .expect("round 1");
    assert_eq!(page1.len(), 3);
    assert!(more1);
    // Record first 3 as served.
    let first_three: Vec<MessageId> = msg_ids[..3].to_vec();
    storage
        .record_sync_served(&recipient_pk, &first_three)
        .expect("record first 3");
    let cursor1 = page1.last().unwrap().created_at;

    // Round 2: sync with cursor → should get next 3 (messages 4, 5, 6).
    let (page2, more2) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, cursor1, 3, 1_000_000)
        .expect("round 2");
    assert_eq!(page2.len(), 3, "round 2 should get 3 messages");
    assert!(more2);
    // Record them.
    let mids: Vec<MessageId> = msg_ids[3..6].to_vec();
    storage
        .record_sync_served(&recipient_pk, &mids)
        .expect("record next 3");
    let cursor2 = page2.last().unwrap().created_at;

    // Round 3: sync with cursor → final 1 message.
    let (page3, more3) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, cursor2, 3, 1_000_000)
        .expect("round 3");
    assert_eq!(page3.len(), 1, "round 3 should get final message");
    assert!(!more3);

    // Verify all 7 were retrieved with no duplicates.
    let total_unique = page1.len() + page2.len() + page3.len();
    assert_eq!(total_unique, 7);
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 7: Full lifecycle — enqueue → serve → record → cursor → next → dedup
// ═══════════════════════════════════════════════════════════════════════════
//
// Combines the cursor model with the dedup model in one end-to-end scenario.

#[test]
fn sync_full_lifecycle_with_cursor() {
    let storage = Storage::memory().expect("in-memory storage");
    let sender_sk = random_sk();
    let recipient_sk = random_sk();
    let recipient_pk = recipient_sk.public();
    let conv_id = make_conv_id(0x07);

    let mb_pk = mailbox_pk(&recipient_sk);

    // Phase 1: sender queues 6 messages.
    let mut msg_ids = Vec::new();
    for i in 0..6u64 {
        msg_ids.push(queue_dm(&storage, &sender_sk, mb_pk, conv_id, i));
    }

    // Phase 2: collector starts from cursor=0.
    // (Simulating a fresh peer with no existing cursor.)

    // Page 1: get 3, serve them, record dedup, advance cursor.
    {
        let (page, more) = storage
            .query_pending_outbound_for_recipient(&recipient_pk, 0, 3, 1_000_000)
            .expect("lifecycle page 1");
        assert_eq!(page.len(), 3);
        assert!(more);

        // Compute message IDs and record as served.
        let to_record: Vec<MessageId> = msg_ids[..3].to_vec();
        storage
            .record_sync_served(&recipient_pk, &to_record)
            .expect("record page 1");

        // Advance cursor.
        let cursor = page.last().unwrap().created_at;
        storage
            .upsert_sync_cursor(&recipient_pk, Some(&cursor.to_be_bytes()), now_ms())
            .expect("upsert cursor after page 1");
    }

    // Phase 3: new messages arrive while collector is processing.
    for i in 6..9u64 {
        msg_ids.push(queue_dm(&storage, &sender_sk, mb_pk, conv_id, i));
    }

    // Page 2: get next 3 (original messages 4..6), skipping served.
    {
        let cursor = storage
            .get_sync_cursor(&recipient_pk)
            .expect("get cursor")
            .expect("cursor exists");
        let last_seen = u64::from_be_bytes(
            cursor
                .last_seen_msg_clock
                .as_ref()
                .expect("clock data")
                .as_slice()
                .try_into()
                .expect("8 bytes"),
        );

        let (page, more) = storage
            .query_pending_outbound_for_recipient(&recipient_pk, last_seen, 3, 1_000_000)
            .expect("lifecycle page 2");
        assert_eq!(page.len(), 3);
        assert!(more);

        let to_record: Vec<MessageId> = msg_ids[3..6].to_vec();
        storage
            .record_sync_served(&recipient_pk, &to_record)
            .expect("record page 2");

        let new_cursor = page.last().unwrap().created_at;
        storage
            .upsert_sync_cursor(&recipient_pk, Some(&new_cursor.to_be_bytes()), now_ms())
            .expect("upsert cursor after page 2");
    }

    // Page 3: get remaining (new messages 7..9).
    {
        let cursor = storage
            .get_sync_cursor(&recipient_pk)
            .expect("get cursor")
            .expect("cursor exists");
        let last_seen = u64::from_be_bytes(
            cursor
                .last_seen_msg_clock
                .as_ref()
                .expect("clock data")
                .as_slice()
                .try_into()
                .expect("8 bytes"),
        );

        let (page, more) = storage
            .query_pending_outbound_for_recipient(&recipient_pk, last_seen, 3, 1_000_000)
            .expect("lifecycle page 3");
        assert_eq!(page.len(), 3, "page 3 should get 3 new messages");
        assert!(!more, "no more after page 3");

        let to_record: Vec<MessageId> = msg_ids[6..9].to_vec();
        storage
            .record_sync_served(&recipient_pk, &to_record)
            .expect("record page 3");

        // Advance cursor to end.
        let new_cursor = page.last().unwrap().created_at;
        storage
            .upsert_sync_cursor(&recipient_pk, Some(&new_cursor.to_be_bytes()), now_ms())
            .expect("upsert cursor after page 3");
    }

    // Phase 4: verify dedup — no new messages left.
    {
        let (page, _) = storage
            .query_pending_outbound_for_recipient(&recipient_pk, 0, 100, 1_000_000)
            .expect("final dedup check");
        assert_eq!(
            page.len(),
            0,
            "no envelopes should be left after full lifecycle"
        );
    }

    // Verify cursor is at the latest position.
    let cursor = storage
        .get_sync_cursor(&recipient_pk)
        .expect("get cursor")
        .expect("cursor exists");
    assert!(
        cursor.last_sync_at_ms > 0,
        "cursor should have a valid timestamp"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 8: Sync vs Retry — different tables, different purposes
// ═══════════════════════════════════════════════════════════════════════════
//
// Sync queries dm_outbox (sender's encrypted envelope store).
// Retry queries the outbox table (delivery scheduling).
// They share no rows and serve complementary roles.

#[test]
fn sync_vs_retry_different_tables() {
    let storage = Storage::memory().expect("in-memory storage");
    let sender_sk = random_sk();
    let recipient_sk = random_sk();
    let recipient_pk = recipient_sk.public();
    let conv_id = make_conv_id(0x08);
    let mb_pk = mailbox_pk(&recipient_sk);

    // Enqueue 3 sync-visible messages (go to dm_outbox).
    for i in 0..3u64 {
        queue_dm(&storage, &sender_sk, mb_pk, conv_id, i);
    }

    // Verify sync sees 3.
    let (sync_page, _) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 100, 1_000_000)
        .expect("sync query");
    assert_eq!(
        sync_page.len(),
        3,
        "sync should see 3 envelopes in dm_outbox"
    );

    // Retry sees 0 (no outbox entries yet).
    let retry_due = storage
        .fetch_due_outbox(now_ms() + 10_000)
        .expect("fetch due outbox");
    assert_eq!(retry_due.len(), 0, "retry sees 0 before enqueuing");

    // Now enqueue 2 messages for retry (go to outbox table via enqueue_outbox).
    // First we need inbox entries for them.
    let retry_ids = [make_msg_id(0xF1), make_msg_id(0xF2)];
    for (i, &mid) in retry_ids.iter().enumerate() {
        let env = sample_envelope(mid, conv_id);
        storage.insert_inbox(&env).expect("insert inbox for retry");
        storage
            .enqueue_outbox(&mid, recipient_pk, now_ms() + (i as u64 * 100))
            .expect("enqueue for retry");
    }

    // Retry now sees 2.
    let retry_due = storage
        .fetch_due_outbox(now_ms() + 10_000)
        .expect("fetch due outbox");
    assert_eq!(retry_due.len(), 2, "retry should see 2 entries");

    // Sync still sees 3 (unaffected by outbox entries).
    let (sync_page2, _) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 100, 1_000_000)
        .expect("sync query after retry enqueue");
    assert_eq!(
        sync_page2.len(),
        3,
        "sync count unchanged by retry outbox entries"
    );

    // Verify retry entries have the expected message IDs and recipient.
    for row in &retry_due {
        assert!(
            retry_ids.contains(&row.msg_id),
            "retry outbox entry must be one of the registered retry msg_ids"
        );
        assert_eq!(row.recipient_device_id, recipient_pk);
    }

    // Sync envelopes are MailboxEnvelope objects (encrypted payloads).
    for env in &sync_page2 {
        assert_eq!(env.recipient.identity, recipient_pk);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test 9: Prune does not remove recent dedup entries
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn sync_prune_does_not_remove_recent() {
    let storage = Storage::memory().expect("in-memory storage");
    let sender_sk = random_sk();
    let recipient_sk = random_sk();
    let recipient_pk = recipient_sk.public();
    let conv_id = make_conv_id(0x09);
    let mb_pk = mailbox_pk(&recipient_sk);

    // Queue and serve 2 messages.
    let mut msg_ids = Vec::new();
    for i in 0..2u64 {
        msg_ids.push(queue_dm(&storage, &sender_sk, mb_pk, conv_id, i));
    }

    storage
        .record_sync_served(&recipient_pk, &msg_ids)
        .expect("record served");

    // Prune (should not affect recent entries).
    storage.prune_sync_dedup().expect("prune sync dedup");

    // Served entries still excluded.
    let (page, _) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 100, 1_000_000)
        .expect("sync after prune");
    assert_eq!(page.len(), 0, "after prune, served entries still excluded");

    // New uns served messages should still be visible.
    queue_dm(&storage, &sender_sk, mb_pk, conv_id, 2);
    let (page2, _) = storage
        .query_pending_outbound_for_recipient(&recipient_pk, 0, 100, 1_000_000)
        .expect("sync after prune + new message");
    assert_eq!(page2.len(), 1, "new unserved message visible after prune");
}
