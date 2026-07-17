//! Crash-recovery integration tests for retry scheduling and sync response processing.
//!
//! These tests simulate crashes by deliberately closing and reopening the SQLite
//! database between operations, then verifying that the system recovers to a
//! consistent state — no lost messages, no duplicate deliveries, no corrupt
//! scheduler state, and idempotent re-execution of in-flight operations.
//!
//! Test categories:
//!
//! 1. **Retry scheduling crash recovery** —
//!    Process interruption during outbox enqueue, delivery attempt recording,
//!    ack processing, and expiry. Verifies stale leases recover, scheduled
//!    retries execute correctly after recovery, and the outbox never enters
//!    an inconsistent scheduling state.
//!
//! 2. **Sync response crash recovery** —
//!    Process interruption during inbox insertion from sync responses and
//!    sync cursor updates. Verifies committed messages survive, uncommitted
//!    messages are safe to replay, replay is idempotent, and cursors remain
//!    consistent.

use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use iroh::{PublicKey, SecretKey};

use boru_chat::{
    storage::Storage,
    store::{DeliveryStatus, MessageId, StoredEnvelope},
};

// ── Helpers ────────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = now_ms();
    dir.push(format!("boru-crash-recovery-{name}-{pid}-{nanos}"));
    dir
}

fn random_pk() -> PublicKey {
    SecretKey::generate().public()
}

fn make_msg_id(byte: u8) -> MessageId {
    [byte; 32]
}

fn make_conv_id(byte: u8) -> [u8; 32] {
    [byte; 32]
}

fn sample_envelope(msg_id: MessageId, conv_id: [u8; 32]) -> StoredEnvelope {
    StoredEnvelope {
        msg_id,
        conversation_id: conv_id,
        author_user_id: random_pk(),
        author_device_id: random_pk(),
        created_at_ms: now_ms(),
        expires_at_ms: now_ms() + 86_400_000, // 1 day
        ciphertext: bytes::Bytes::from_static(b"crash-recovery-test-ciphertext"),
        signature: [0u8; 64],
        acked_at_ms: None,
    }
}

// =========================================================================
// SECTION 1: RETRY SCHEDULING CRASH RECOVERY
// =========================================================================

/// Verify that an outbox entry enqueued before a crash survives reopen.
#[test]
fn retry_crash_enqueue_survives_reopen() {
    let dir = temp_dir("retry-enqueue-reopen");
    let msg_id = make_msg_id(1);
    let recipient = random_pk();
    let conv_id = make_conv_id(10);

    // Session 1: enqueue
    {
        let storage = Storage::open(&dir).expect("open session 1");
        let env = sample_envelope(msg_id, conv_id);
        storage.insert_inbox(&env).expect("insert inbox");
        storage
            .enqueue_outbox(&msg_id, recipient, 0)
            .expect("enqueue");
    }
    // simulate crash — drop storage without explicit cleanup

    // Session 2: reopen and verify
    let storage = Storage::open(&dir).expect("reopen session 2");
    let due = storage
        .fetch_due_outbox(now_ms() + 1)
        .expect("fetch due after reopen");
    let row = due.iter().find(|r| r.msg_id == msg_id);
    assert!(row.is_some(), "enqueued message should survive crash");
    assert_eq!(
        row.unwrap().status,
        DeliveryStatus::Pending,
        "freshly enqueued entry should be Pending"
    );
    assert_eq!(
        row.unwrap().attempts,
        0,
        "no delivery attempts should have been recorded"
    );
}

/// Verify that multiple outbox entries in various states survive a crash.
#[test]
fn retry_crash_multiple_entries_survive() {
    let dir = temp_dir("retry-multi-survive");
    let recipient = random_pk();
    let conv_id = make_conv_id(20);

    let msg_a = make_msg_id(0xA1);
    let msg_b = make_msg_id(0xB1);
    let msg_c = make_msg_id(0xC1);
    let msg_d = make_msg_id(0xD1);

    let t_base = now_ms();

    // Session 1: enqueue 4 messages with staggered next_attempt_at_ms
    {
        let storage = Storage::open(&dir).expect("open session 1");
        for env in [msg_a, msg_b, msg_c, msg_d]
            .iter()
            .map(|id| sample_envelope(*id, conv_id))
        {
            storage.insert_inbox(&env).expect("insert inbox");
        }
        storage
            .enqueue_outbox(&msg_a, recipient, t_base)
            .expect("enqueue A");
        storage
            .enqueue_outbox(&msg_b, recipient, t_base + 10_000)
            .expect("enqueue B");
        storage
            .enqueue_outbox(&msg_c, recipient, t_base + 60_000)
            .expect("enqueue C");
        storage
            .enqueue_outbox(&msg_d, recipient, t_base + 300_000)
            .expect("enqueue D");
    }
    // simulate crash

    // Session 2: reopen and verify all four survive
    let storage = Storage::open(&dir).expect("reopen session 2");
    let all_due = storage
        .fetch_due_outbox(t_base + 400_000)
        .expect("fetch all due");

    for (label, msg_id) in [("A", msg_a), ("B", msg_b), ("C", msg_c), ("D", msg_d)] {
        assert!(
            all_due.iter().any(|r| r.msg_id == msg_id),
            "message {label} should survive crash"
        );
    }

    // Verify ordering: A < B < C < D in next_attempt_at_ms
    let positions: Vec<usize> = [msg_a, msg_b, msg_c, msg_d]
        .iter()
        .filter_map(|id| all_due.iter().position(|r| r.msg_id == *id))
        .collect();
    assert_eq!(positions.len(), 4, "all 4 messages should be in due list");
    for i in 0..positions.len().saturating_sub(1) {
        assert!(
            positions[i] <= positions[i + 1],
            "messages should maintain staggered order after crash"
        );
    }
}

/// Simulate a crash during `record_attempt` and verify the outbox entry
/// is recovered to a consistent state after recovery.
///
/// The scheduler calls `record_attempt` which sets `status = Sent`,
/// increments `attempts`, and sets `next_attempt_at_ms` to a future
/// retry time. `Storage::open()` runs `recover_crash_state()` which
/// resets `Sent → Pending` and `next_attempt_at_ms → now` so the
/// retry worker can pick up where it left off.
#[test]
fn retry_crash_during_record_attempt_state_preserved() {
    let dir = temp_dir("retry-record-attempt-crash");
    let msg_id = make_msg_id(2);
    let recipient = random_pk();
    let conv_id = make_conv_id(30);

    let t_base = now_ms();
    let first_retry_ms = t_base + 5_000; // retry window set by record_attempt

    // Session 1: enqueue and record one attempt, then crash
    {
        let storage = Storage::open(&dir).expect("open session 1");
        let env = sample_envelope(msg_id, conv_id);
        storage.insert_inbox(&env).expect("insert inbox");
        storage
            .enqueue_outbox(&msg_id, recipient, t_base)
            .expect("enqueue");

        // Simulate retry worker starting: fetch due, then record attempt
        let due = storage.fetch_due_outbox(t_base + 1).expect("fetch due");
        assert_eq!(due.len(), 1, "should be due immediately");
        assert_eq!(due[0].attempts, 0, "no prior attempts");

        // Record the delivery attempt (sets status=Sent, increments attempts,
        // sets next_attempt_at_ms = first_retry_ms)
        storage
            .record_attempt(&msg_id, recipient, first_retry_ms, Some("timeout"))
            .expect("record attempt");
    }
    // simulate crash — the record_attempt SQL write was committed by SQLite
    // but the retry worker did not get a chance to complete the cycle

    // Session 2: reopen — `recover_crash_state()` automatically resets
    // Sent → Pending and next_attempt_at_ms → now.
    let storage = Storage::open(&dir).expect("reopen session 2");

    // After recovery, the entry is immediately due for retry
    let due = storage
        .fetch_due_outbox(now_ms() + 1)
        .expect("fetch due after reopen");
    let row = due
        .iter()
        .find(|r| r.msg_id == msg_id)
        .expect("entry should be due for retry after crash recovery");

    // Crash recovery resets status from Sent back to Pending
    assert_eq!(
        row.status,
        DeliveryStatus::Pending,
        "crash recovery should reset Sent -> Pending"
    );
    assert_eq!(row.attempts, 1, "attempt count should survive crash");
    assert!(
        row.last_attempt_at_ms.is_some(),
        "last_attempt_at_ms should survive crash"
    );
    // The recover_crash_state sets last_error_code to 'crash_recovered'
    assert_eq!(
        row.last_error_code.as_deref(),
        Some("crash_recovered"),
        "crash recovery should tag recovered entries"
    );

    // After recovery, the retry worker can attempt delivery again.
    // record_attempt works on Pending entries.
    storage
        .record_attempt(
            &msg_id,
            recipient,
            now_ms() + 30_000,
            Some("connection reset"),
        )
        .expect("record retry after recovery");

    // After the second record_attempt, status is Sent again
    let due2 = storage
        .fetch_due_outbox(now_ms() + 60_000)
        .expect("fetch due after recovery retry");
    let row2 = due2
        .iter()
        .find(|r| r.msg_id == msg_id)
        .expect("entry should exist after recovery");
    assert_eq!(
        row2.attempts, 2,
        "attempt count should increment after recovery"
    );

    // Successfully ack the message (end of retry lifecycle)
    storage
        .mark_acked(&msg_id, recipient)
        .expect("ack after recovery");
    let due3 = storage
        .fetch_due_outbox(now_ms() + 100_000)
        .expect("fetch due after ack");
    assert!(
        !due3.iter().any(|r| r.msg_id == msg_id),
        "acked message should not be due after full lifecycle"
    );
}

/// Verify that stale entries with status=Sent and a past `next_attempt_at_ms`
/// are correctly recovered by `recover_crash_state()` after a crash.
///
/// The recovery mechanism resets Sent → Pending and sets
/// `next_attempt_at_ms → now` so the entry is immediately due for retry.
#[test]
fn retry_crash_stale_sent_entry_is_due_after_reopen() {
    let dir = temp_dir("retry-stale-sent");
    let msg_id = make_msg_id(3);
    let recipient = random_pk();
    let conv_id = make_conv_id(40);

    let t_base = now_ms();
    // Simulate a past retry window (the crash took longer than the retry delay)
    let past_retry_ms = t_base.saturating_sub(10_000);

    // Session 1: enqueue and record attempt with a *past* retry time
    {
        let storage = Storage::open(&dir).expect("open session 1");
        let env = sample_envelope(msg_id, conv_id);
        storage.insert_inbox(&env).expect("insert inbox");
        storage
            .enqueue_outbox(&msg_id, recipient, t_base)
            .expect("enqueue");
        storage
            .record_attempt(&msg_id, recipient, past_retry_ms, Some("timeout"))
            .expect("record attempt with past retry time");
    }
    // simulate crash — app was down, `recover_crash_state` will reset
    // Sent → Pending and next_attempt_at_ms → now

    // Session 2: reopen — recover_crash_state runs automatically
    let storage = Storage::open(&dir).expect("reopen session 2");
    let due = storage
        .fetch_due_outbox(now_ms() + 1)
        .expect("fetch due after reopen");
    let row = due
        .iter()
        .find(|r| r.msg_id == msg_id)
        .expect("stale entry should be immediately due for retry after recovery");
    assert_eq!(
        row.status,
        DeliveryStatus::Pending,
        "crash recovery should reset Sent -> Pending"
    );
    assert_eq!(row.attempts, 1, "attempt count should be 1");
    assert_eq!(
        row.last_error_code.as_deref(),
        Some("crash_recovered"),
        "crash recovery should tag recovered entries"
    );

    // After recovery, the retry can proceed and eventually succeed
    storage
        .mark_acked(&msg_id, recipient)
        .expect("mark acked after recovery");
    let due_after_ack = storage
        .fetch_due_outbox(now_ms() + 100_000)
        .expect("fetch after ack");
    assert!(
        !due_after_ack.iter().any(|r| r.msg_id == msg_id),
        "acked message should not be due for retry"
    );
}

/// Verify that an outbox entry that was already Acked before a crash
/// does NOT reappear in fetch_due_outbox after recovery.
#[test]
fn retry_crash_acked_does_not_retry_after_reopen() {
    let dir = temp_dir("retry-acked");
    let msg_id = make_msg_id(4);
    let recipient = random_pk();
    let conv_id = make_conv_id(50);

    // Session 1: enqueue and ack
    {
        let storage = Storage::open(&dir).expect("open session 1");
        let env = sample_envelope(msg_id, conv_id);
        storage.insert_inbox(&env).expect("insert inbox");
        storage
            .enqueue_outbox(&msg_id, recipient, 0)
            .expect("enqueue");
        // Mark as acked (the ACK was received before crash)
        storage.mark_acked(&msg_id, recipient).expect("mark acked");
    }
    // simulate crash

    // Session 2: reopen — must not see the acked entry
    let storage = Storage::open(&dir).expect("reopen session 2");
    let due = storage
        .fetch_due_outbox(now_ms() + 100_000)
        .expect("fetch due after reopen");
    assert!(
        !due.iter().any(|r| r.msg_id == msg_id),
        "acked message must not reappear after crash"
    );
}

/// Verify that an expired outbox entry does NOT reappear after a crash.
#[test]
fn retry_crash_expired_does_not_retry_after_reopen() {
    let dir = temp_dir("retry-expired");
    let msg_id = make_msg_id(5);
    let recipient = random_pk();
    let conv_id = make_conv_id(60);

    let past = now_ms() - 100_000;

    // Session 1: enqueue with an expiry in the past, expire it, then crash
    {
        let storage = Storage::open(&dir).expect("open session 1");
        let mut env = sample_envelope(msg_id, conv_id);
        env.expires_at_ms = past; // already expired
        storage.insert_inbox(&env).expect("insert inbox");
        storage
            .enqueue_outbox(&msg_id, recipient, 0)
            .expect("enqueue");

        // Expire the outbox entry
        storage.expire_outbox(now_ms()).expect("expire outbox");
        let due_after_expire = storage
            .fetch_due_outbox(now_ms() + 1)
            .expect("fetch after expire");
        assert!(
            !due_after_expire.iter().any(|r| r.msg_id == msg_id),
            "expired message should not be due before crash"
        );
    }
    // simulate crash

    // Session 2: reopen — must still not see the expired entry
    let storage = Storage::open(&dir).expect("reopen session 2");
    let due = storage
        .fetch_due_outbox(now_ms() + 100_000)
        .expect("fetch due after reopen");
    assert!(
        !due.iter().any(|r| r.msg_id == msg_id),
        "expired message must not reappear after crash"
    );
}

/// Verify that the outbox expiry mechanism still works after a crash.
/// Entries that were enqueued but NOT expired before crash should be
/// expired correctly after recovery.
#[test]
fn retry_crash_expiry_still_works_after_reopen() {
    let dir = temp_dir("retry-expiry-after");
    let msg_id = make_msg_id(6);
    let recipient = random_pk();
    let conv_id = make_conv_id(70);

    let past = now_ms() - 100_000;

    // Session 1: enqueue with past expiry but DO NOT expire before crash
    {
        let storage = Storage::open(&dir).expect("open session 1");
        let mut env = sample_envelope(msg_id, conv_id);
        env.expires_at_ms = past;
        storage.insert_inbox(&env).expect("insert inbox");
        storage
            .enqueue_outbox(&msg_id, recipient, 0)
            .expect("enqueue");
    }
    // simulate crash before expiry ran

    // Session 2: reopen, run expiry, verify the expired entry is gone
    let storage = Storage::open(&dir).expect("reopen session 2");

    // Before expiring: entry should be due
    let due_before = storage
        .fetch_due_outbox(now_ms() + 1)
        .expect("fetch due before expire");
    assert!(
        due_before.iter().any(|r| r.msg_id == msg_id),
        "entry should be due before expiry after reopen"
    );

    // Run expiry
    storage
        .expire_outbox(now_ms())
        .expect("expire after reopen");

    // After expiry: entry must not be due
    let due_after = storage
        .fetch_due_outbox(now_ms() + 100_000)
        .expect("fetch due after expire");
    assert!(
        !due_after.iter().any(|r| r.msg_id == msg_id),
        "entry should be expired after reopen and expiry run"
    );
}

/// Verify that the full retry lifecycle — enqueue → crash → record_attempt
/// → crash → ack — works correctly across multiple crash cycles.
#[test]
fn retry_crash_full_lifecycle_across_crashes() {
    let dir = temp_dir("retry-full-lifecycle");
    let msg_id = make_msg_id(7);
    let recipient = random_pk();
    let conv_id = make_conv_id(80);

    let t_base = now_ms();

    // Crash cycle 1: enqueue
    {
        let storage = Storage::open(&dir).expect("cycle 1 open");
        let env = sample_envelope(msg_id, conv_id);
        storage.insert_inbox(&env).expect("insert inbox");
        storage
            .enqueue_outbox(&msg_id, recipient, t_base)
            .expect("enqueue");
    }
    // crash

    // Crash cycle 2: fetch due and record attempt (simulate retry)
    {
        let storage = Storage::open(&dir).expect("cycle 2 open");
        let due = storage
            .fetch_due_outbox(now_ms())
            .expect("fetch due in cycle 2");
        assert!(
            due.iter().any(|r| r.msg_id == msg_id),
            "message should be due in cycle 2"
        );
        storage
            .record_attempt(
                &msg_id,
                recipient,
                now_ms() + 30_000,
                Some("transient failure"),
            )
            .expect("record attempt in cycle 2");
    }
    // crash before ack received

    // Crash cycle 3: reopen — recover_crash_state resets Sent → Pending
    {
        let storage = Storage::open(&dir).expect("cycle 3 open");
        let due = storage
            .fetch_due_outbox(now_ms() + 60_000)
            .expect("fetch due in cycle 3");
        let row = due
            .iter()
            .find(|r| r.msg_id == msg_id)
            .expect("message should be due in cycle 3");
        assert_eq!(row.attempts, 1, "should have 1 attempt");
        // Crash recovery resets Sent → Pending so retry can resume
        assert_eq!(
            row.status,
            DeliveryStatus::Pending,
            "crash recovery should reset Sent -> Pending"
        );
        assert_eq!(
            row.last_error_code.as_deref(),
            Some("crash_recovered"),
            "crash recovery should tag recovered entries"
        );

        // This time, record attempt and then ACK before crash
        storage
            .record_attempt(&msg_id, recipient, now_ms() + 30_000, None)
            .expect("record attempt in cycle 3");
        storage
            .mark_acked(&msg_id, recipient)
            .expect("mark acked in cycle 3");
    }
    // crash after ack

    // Crash cycle 4: verify clean state
    let storage = Storage::open(&dir).expect("cycle 4 open");
    let due = storage
        .fetch_due_outbox(now_ms() + 100_000)
        .expect("fetch due in cycle 4");
    assert!(
        !due.iter().any(|r| r.msg_id == msg_id),
        "acked message should not be due in final cycle"
    );
}

/// Verify that the system does NOT enter an inconsistent scheduling state
/// when a crash occurs between enqueueing multiple messages and recording
/// attempts for only some of them.
#[test]
fn retry_crash_no_inconsistent_mixed_state() {
    let dir = temp_dir("retry-mixed-state");
    let recipient = random_pk();
    let conv_id = make_conv_id(90);

    let msg_enqueued = make_msg_id(0xE1);
    let msg_sent = make_msg_id(0xE2);
    let msg_acked = make_msg_id(0xE3);

    let t_base = now_ms();

    // Session: enqueue 3, record attempt for 2 (one succeeds, one
    // sets retry), ack 1, then crash
    {
        let storage = Storage::open(&dir).expect("open session");
        for id in [msg_enqueued, msg_sent, msg_acked] {
            let env = sample_envelope(id, conv_id);
            storage.insert_inbox(&env).expect("insert inbox");
        }
        storage
            .enqueue_outbox(&msg_enqueued, recipient, t_base)
            .expect("enqueue E1");
        storage
            .enqueue_outbox(&msg_sent, recipient, t_base)
            .expect("enqueue E2");
        storage
            .enqueue_outbox(&msg_acked, recipient, t_base)
            .expect("enqueue E3");

        // Record attempt for msg_sent (status→Sent)
        storage
            .record_attempt(&msg_sent, recipient, now_ms() + 30_000, Some("timeout"))
            .expect("record attempt E2");

        // Record attempt AND ack for msg_acked
        storage
            .record_attempt(&msg_acked, recipient, now_ms() + 30_000, None)
            .expect("record attempt E3");
        storage.mark_acked(&msg_acked, recipient).expect("ack E3");
    }
    // crash

    // Reopen: verify each entry is in the correct state
    let storage = Storage::open(&dir).expect("reopen");

    // msg_enqueued: should be Pending, 0 attempts, immediately due
    let due = storage.fetch_due_outbox(now_ms() + 1).expect("fetch due");
    let e1 = due.iter().find(|r| r.msg_id == msg_enqueued);
    assert!(
        e1.is_some(),
        "enqueued-only message should be due after crash"
    );
    assert_eq!(e1.unwrap().status, DeliveryStatus::Pending);
    assert_eq!(e1.unwrap().attempts, 0);

    // msg_sent: recover_crash_state resets Sent -> Pending with crash_recovered tag
    let e2 = due.iter().find(|r| r.msg_id == msg_sent);
    assert!(
        e2.is_some(),
        "sent message should be due after crash recovery"
    );
    assert_eq!(
        e2.unwrap().status,
        DeliveryStatus::Pending,
        "crash recovery should reset Sent -> Pending"
    );
    assert_eq!(e2.unwrap().attempts, 1);
    assert_eq!(
        e2.unwrap().last_error_code.as_deref(),
        Some("crash_recovered"),
        "crash recovery should tag previously-Sent entries"
    );

    // msg_acked: should NOT be due
    assert!(
        !due.iter().any(|r| r.msg_id == msg_acked),
        "acked message must not be due after crash"
    );

    // After recovery, the scheduler can resume: record another attempt
    // for msg_sent, ack msg_enqueued
    storage
        .record_attempt(
            &msg_sent,
            recipient,
            now_ms() + 60_000,
            Some("recovery retry"),
        )
        .expect("recovery retry E2");
    storage
        .mark_acked(&msg_enqueued, recipient)
        .expect("ack E1 after recovery");

    let due_final = storage
        .fetch_due_outbox(now_ms() + 100_000)
        .expect("fetch due final");
    assert!(
        !due_final.iter().any(|r| r.msg_id == msg_acked),
        "acked E3 must not reappear"
    );
    assert!(
        !due_final.iter().any(|r| r.msg_id == msg_enqueued),
        "acked E1 must not reappear"
    );
}

// =========================================================================
// SECTION 2: SYNC RESPONSE PROCESSING CRASH RECOVERY
// =========================================================================

/// Verify that inbox messages inserted from a sync response survive a crash.
///
/// Simulates: receiving a SyncResponse, inserting envelopes into the inbox,
/// then crashing before processing is complete. After recovery, the inserted
/// messages must still be present.
#[test]
fn sync_crash_inserted_envelopes_survive() {
    let dir = temp_dir("sync-insert-envelopes");
    let conv_id = make_conv_id(100);
    let msg_a = make_msg_id(0xA0);
    let msg_b = make_msg_id(0xB0);

    // Session 1: insert inbox messages (simulating sync response processing)
    {
        let storage = Storage::open(&dir).expect("open session 1");
        storage
            .insert_inbox(&sample_envelope(msg_a, conv_id))
            .expect("insert A");
        storage
            .insert_inbox(&sample_envelope(msg_b, conv_id))
            .expect("insert B");
    }
    // crash

    // Session 2: reopen and verify both messages survived
    let storage = Storage::open(&dir).expect("reopen session 2");
    assert!(
        storage.get_inbox(&msg_a).unwrap().is_some(),
        "message A should survive crash"
    );
    assert!(
        storage.get_inbox(&msg_b).unwrap().is_some(),
        "message B should survive crash"
    );

    // All messages should be present in list_inbox
    let all = storage.list_inbox(None).expect("list inbox");
    let matched: Vec<_> = all
        .iter()
        .filter(|e| e.msg_id == msg_a || e.msg_id == msg_b)
        .collect();
    assert_eq!(
        matched.len(),
        2,
        "both messages should be present after crash recovery"
    );
}

/// Verify that re-inserting the same sync response envelopes after a crash
/// is idempotent — no duplicate messages appear.
#[test]
fn sync_crash_idempotent_replay() {
    let dir = temp_dir("sync-idempotent");
    let conv_id = make_conv_id(110);
    let msg_a = make_msg_id(0xA1);
    let msg_b = make_msg_id(0xB1);

    // Session 1: insert from sync response
    {
        let storage = Storage::open(&dir).expect("open session 1");
        storage
            .insert_inbox(&sample_envelope(msg_a, conv_id))
            .expect("insert A");
        storage
            .insert_inbox(&sample_envelope(msg_b, conv_id))
            .expect("insert B");
    }
    // crash before acking or advancing cursor

    // Session 2: re-process the same sync response (same envelopes)
    {
        let storage = Storage::open(&dir).expect("reopen session 2");
        // Re-insert same messages — should be no-ops via ON CONFLICT(msg_id) DO NOTHING
        storage
            .insert_inbox(&sample_envelope(msg_a, conv_id))
            .expect("re-insert A -> should succeed (idempotent)");
        storage
            .insert_inbox(&sample_envelope(msg_b, conv_id))
            .expect("re-insert B -> should succeed (idempotent)");

        // Verify no duplicates
        let all = storage.list_inbox(None).expect("list inbox");
        let count_a = all.iter().filter(|e| e.msg_id == msg_a).count();
        let count_b = all.iter().filter(|e| e.msg_id == msg_b).count();
        assert_eq!(count_a, 1, "message A must not be duplicated");
        assert_eq!(count_b, 1, "message B must not be duplicated");
    }
    // crash again

    // Session 3: reopen and verify still no duplicates
    let storage = Storage::open(&dir).expect("reopen session 3");
    let all = storage.list_inbox(None).expect("list inbox");
    let count_a = all.iter().filter(|e| e.msg_id == msg_a).count();
    let count_b = all.iter().filter(|e| e.msg_id == msg_b).count();
    assert_eq!(
        count_a, 1,
        "message A must still be exactly 1 after 3 sessions"
    );
    assert_eq!(
        count_b, 1,
        "message B must still be exactly 1 after 3 sessions"
    );
}

/// Verify that messages acknowledged during sync response processing
/// (with acked_at_ms set) survive a crash correctly.
#[test]
fn sync_crash_acked_inbox_survives() {
    let dir = temp_dir("sync-acked-inbox");
    let conv_id = make_conv_id(120);
    let msg_id = make_msg_id(0xC0);

    // Session 1: insert inbox message with acked_at_ms set
    {
        let storage = Storage::open(&dir).expect("open session 1");
        let mut env = sample_envelope(msg_id, conv_id);
        env.acked_at_ms = Some(now_ms());
        storage.insert_inbox(&env).expect("insert acked inbox");
    }
    // crash after insert

    // Session 2: reopen and verify acked message is present
    let storage = Storage::open(&dir).expect("reopen session 2");
    let env = storage
        .get_inbox(&msg_id)
        .expect("get inbox after crash")
        .expect("acked message should survive crash");
    assert!(
        env.acked_at_ms.is_some(),
        "acked_at_ms should survive crash"
    );
}

/// Verify that sync cursor state survives a crash and is consistent
/// after recovery.
#[test]
fn sync_crash_cursor_survives_reopen() {
    let dir = temp_dir("sync-cursor");
    let peer = random_pk();
    let clock_bytes = b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f";
    let t_sync = now_ms();

    // Session 1: upsert sync cursor
    {
        let storage = Storage::open(&dir).expect("open session 1");
        storage
            .upsert_sync_cursor(&peer, Some(clock_bytes), t_sync)
            .expect("upsert sync cursor");
    }
    // crash

    // Session 2: reopen and verify cursor is still there
    let storage = Storage::open(&dir).expect("reopen session 2");
    let cursors = storage.list_sync_cursors().expect("list cursors");
    assert_eq!(cursors.len(), 1, "sync cursor should survive crash");
    assert_eq!(
        cursors[0].last_sync_at_ms, t_sync,
        "cursor timestamp should survive crash"
    );
    assert!(
        cursors[0].last_seen_msg_clock.is_some(),
        "cursor clock should survive crash"
    );

    // Update cursor after recovery (simulating successful sync)
    storage
        .upsert_sync_cursor(&peer, Some(b"\xff\xfe\xfd\xfc"), now_ms())
        .expect("update cursor after recovery");

    let cursors = storage
        .list_sync_cursors()
        .expect("list cursors after update");
    assert_eq!(cursors.len(), 1, "should still be 1 cursor");
    assert_eq!(
        cursors[0].last_seen_msg_clock.as_ref().map(|v| v.len()),
        Some(4),
        "cursor clock should be the updated value"
    );
}

/// Verify that multiple sync cursors (for different peers) all survive
/// a crash and can be individually recovered.
#[test]
fn sync_crash_multiple_cursors_survive() {
    let dir = temp_dir("sync-multi-cursor");
    let peer_a = random_pk();
    let peer_b = random_pk();
    let t_sync = now_ms();

    // Session 1: upsert two cursors
    {
        let storage = Storage::open(&dir).expect("open session 1");
        storage
            .upsert_sync_cursor(&peer_a, Some(b"clock_a"), t_sync)
            .expect("upsert cursor A");
        storage
            .upsert_sync_cursor(&peer_b, Some(b"clock_b"), t_sync)
            .expect("upsert cursor B");
    }
    // crash

    // Session 2: reopen and verify both cursors
    let storage = Storage::open(&dir).expect("reopen session 2");
    let cursors = storage.list_sync_cursors().expect("list cursors");
    assert_eq!(cursors.len(), 2, "both cursors should survive crash");

    // Verify each cursor's clock data
    let cursor_a = cursors
        .iter()
        .find(|c| c.last_seen_msg_clock.as_deref() == Some(b"clock_a"))
        .expect("cursor A should exist");
    assert_eq!(cursor_a.last_sync_at_ms, t_sync);

    let cursor_b = cursors
        .iter()
        .find(|c| c.last_seen_msg_clock.as_deref() == Some(b"clock_b"))
        .expect("cursor B should exist");
    assert_eq!(cursor_b.last_sync_at_ms, t_sync);
}

/// Verify that a fresh sync (no prior cursor) after a crash correctly
/// starts from scratch without stale state.
#[test]
fn sync_crash_fresh_start_has_no_cursors() {
    let dir = temp_dir("sync-fresh-start");

    // After a clean start (no prior session), there should be no cursors
    let storage = Storage::open(&dir).expect("open fresh storage");
    let cursors = storage.list_sync_cursors().expect("list cursors");
    assert!(
        cursors.is_empty(),
        "fresh storage should have no sync cursors"
    );
}

/// Verify that a sync cursor can be upserted after a crash — the cursor
/// value is correctly updated and persisted across another crash cycle.
#[test]
fn sync_crash_cursor_update_survives() {
    let dir = temp_dir("sync-cursor-update");
    let peer = random_pk();

    // Session 1: initial cursor
    {
        let storage = Storage::open(&dir).expect("open session 1");
        storage
            .upsert_sync_cursor(&peer, Some(b"clock_v1"), 1000)
            .expect("initial cursor");
    }
    // crash

    // Session 2: update cursor to a newer value
    {
        let storage = Storage::open(&dir).expect("open session 2");
        storage
            .upsert_sync_cursor(&peer, Some(b"clock_v2"), 2000)
            .expect("update cursor");
    }
    // crash

    // Session 3: verify the latest cursor value survived
    let storage = Storage::open(&dir).expect("open session 3");
    let cursors = storage.list_sync_cursors().expect("list cursors");
    assert_eq!(cursors.len(), 1, "should have one cursor");
    assert_eq!(
        cursors[0].last_seen_msg_clock.as_deref(),
        Some(&b"clock_v2"[..]),
        "cursor should have the latest clock value"
    );
    assert_eq!(
        cursors[0].last_sync_at_ms, 2000,
        "cursor should have the latest timestamp"
    );
}

/// Verify that after a crash during sync response processing, a subsequent
/// sync correctly advances the cursor past already-inserted messages
/// without requiring them to be re-processed.
#[test]
fn sync_crash_cursor_advances_after_recovery() {
    let dir = temp_dir("sync-cursor-advance");
    let peer = random_pk();
    let conv_id = make_conv_id(130);
    let msg_a = make_msg_id(0xD0);
    let msg_b = make_msg_id(0xD1);

    // Session 1: process sync response — insert messages and set cursor
    {
        let storage = Storage::open(&dir).expect("open session 1");
        storage
            .insert_inbox(&sample_envelope(msg_a, conv_id))
            .expect("insert A");
        storage
            .insert_inbox(&sample_envelope(msg_b, conv_id))
            .expect("insert B");
        // Set cursor to mark that we've synced up to clock "v1"
        storage
            .upsert_sync_cursor(&peer, Some(b"v1"), now_ms())
            .expect("upsert cursor v1");
    }
    // crash

    // Session 2: recovery — do a new sync, advance cursor to v2
    {
        let storage = Storage::open(&dir).expect("open session 2");

        // Verify existing messages are still there
        assert!(
            storage.get_inbox(&msg_a).unwrap().is_some(),
            "msg A should survive crash"
        );
        assert!(
            storage.get_inbox(&msg_b).unwrap().is_some(),
            "msg B should survive crash"
        );

        // Process new sync response messages (C and D) and advance cursor
        let msg_c = make_msg_id(0xD2);
        let msg_d = make_msg_id(0xD3);
        storage
            .insert_inbox(&sample_envelope(msg_c, conv_id))
            .expect("insert C");
        storage
            .insert_inbox(&sample_envelope(msg_d, conv_id))
            .expect("insert D");
        storage
            .upsert_sync_cursor(&peer, Some(&b"v2"[..]), now_ms())
            .expect("advance cursor to v2");
    }
    // crash

    // Session 3: verify all 4 messages and latest cursor
    let storage = Storage::open(&dir).expect("open session 3");
    for (label, id) in [
        ("A", msg_a),
        ("B", msg_b),
        ("C", make_msg_id(0xD2)),
        ("D", make_msg_id(0xD3)),
    ] {
        assert!(
            storage.get_inbox(&id).unwrap().is_some(),
            "message {label} should survive all crashes"
        );
    }

    let cursors = storage.list_sync_cursors().expect("list cursors");
    assert_eq!(cursors.len(), 1, "should have one cursor");
    assert_eq!(
        cursors[0].last_seen_msg_clock.as_deref(),
        Some(&b"v2"[..]),
        "cursor should be at the latest clock value"
    );
}

/// Verify that inserting a large batch of sync response envelopes is
/// idempotent across a crash boundary — all envelopes survive and
/// no duplicates are created.
#[test]
fn sync_crash_batch_envelope_idempotency() {
    let dir = temp_dir("sync-batch");
    let conv_id = make_conv_id(140);

    // Generate 20 unique message IDs
    let msg_ids: Vec<[u8; 32]> = (0u8..20).map(make_msg_id).collect();

    // Session 1: insert all 20 envelopes
    {
        let storage = Storage::open(&dir).expect("open session 1");
        for id in &msg_ids {
            storage
                .insert_inbox(&sample_envelope(*id, conv_id))
                .expect("insert batch envelope");
        }
    }
    // crash

    // Session 2: re-insert all 20 (idempotent replay)
    {
        let storage = Storage::open(&dir).expect("open session 2");
        for id in &msg_ids {
            storage
                .insert_inbox(&sample_envelope(*id, conv_id))
                .expect("re-insert batch envelope");
        }
    }

    // Session 3: verify all 20 exist and are not duplicated
    let storage = Storage::open(&dir).expect("open session 3");
    let all = storage.list_inbox(None).expect("list inbox");
    for id in &msg_ids {
        let count = all.iter().filter(|e| e.msg_id == *id).count();
        assert_eq!(
            count, 1,
            "message {:02x} should appear exactly once after crash recovery",
            id[0]
        );
    }
    assert_eq!(
        all.len(),
        msg_ids.len(),
        "total envelope count should match after crash recovery"
    );
}

/// Verify that inserting inbox messages and updating the sync cursor
/// in sequence survives a crash at any point (every step is durable).
#[test]
fn sync_crash_interleaved_operations() {
    let dir = temp_dir("sync-interleaved");
    let peer = random_pk();
    let conv_id = make_conv_id(150);

    let msg_a = make_msg_id(0xE0);
    let msg_b = make_msg_id(0xE1);

    // Session 1: insert A, advance cursor
    {
        let storage = Storage::open(&dir).expect("open session 1");
        storage
            .insert_inbox(&sample_envelope(msg_a, conv_id))
            .expect("insert A");
        storage
            .upsert_sync_cursor(&peer, Some(b"after_A"), now_ms())
            .expect("cursor after A");
    }
    // crash

    // Session 2: insert B (A already present from session 1)
    {
        let storage = Storage::open(&dir).expect("open session 2");
        // A should already be here
        assert!(
            storage.get_inbox(&msg_a).unwrap().is_some(),
            "msg A should survive from session 1"
        );
        storage
            .insert_inbox(&sample_envelope(msg_b, conv_id))
            .expect("insert B");
        storage
            .upsert_sync_cursor(&peer, Some(&b"after_B"[..]), now_ms())
            .expect("cursor after B");
    }
    // crash

    // Session 3: verify both A and B are present, cursor is at after_B
    let storage = Storage::open(&dir).expect("open session 3");
    assert!(
        storage.get_inbox(&msg_a).unwrap().is_some(),
        "msg A should survive both crashes"
    );
    assert!(
        storage.get_inbox(&msg_b).unwrap().is_some(),
        "msg B should survive second crash"
    );

    let cursors = storage.list_sync_cursors().expect("list cursors");
    assert_eq!(cursors.len(), 1, "one cursor");
    assert_eq!(
        cursors[0].last_seen_msg_clock.as_deref(),
        Some(&b"after_B"[..]),
        "cursor should be at after_B"
    );
}
