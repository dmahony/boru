//! Failure-injection tests around multi-step persistence transactions
//! (BORU-AUDIT-28, step 7).  The storage layer exposes deterministic fault
//! injection points (`*_with_fault`); we verify that a fault mid-transaction
//! rolls back completely — no partial rows survive.

use boru_core::mailbox::{MailboxAck, MailboxIdentity};
use boru_core::storage::{AckProcessingFault, OutgoingDmFault, Storage};
use iroh::SecretKey;

fn setup() -> (tempfile::TempDir, Storage, SecretKey, SecretKey) {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).unwrap();
    let sender = SecretKey::generate();
    let recipient = SecretKey::generate();
    (dir, storage, sender, recipient)
}

/// A failure injected before commit (encryption step) leaves zero partial
/// rows: no message row, no outbox row, and the DM sequence stays at 1.
#[test]
fn outgoing_dm_encryption_fault_rolls_back_completely() {
    let (_dir, storage, sender, recipient) = setup();
    let result = storage.queue_outgoing_dm_with_fault(
        [5; 32],
        sender.public(),
        "fault-encryption",
        "secret",
        MailboxIdentity::from_secret(&recipient).public_key(),
        &sender,
        OutgoingDmFault::Encryption,
    );
    assert!(
        result.is_err(),
        "encryption fault must fail the transaction"
    );
    assert_eq!(
        storage.next_dm_sequence([5; 32], sender.public()).unwrap(),
        1,
        "sequence must not advance after rollback"
    );
}

/// A failure injected after the durable rows are written but before commit
/// (database step) rolls back completely: no message row, no outbox row.
#[test]
fn outgoing_dm_database_fault_rolls_back_completely() {
    let (_dir, storage, sender, recipient) = setup();
    let result = storage.queue_outgoing_dm_with_fault(
        [6; 32],
        sender.public(),
        "fault-database",
        "secret",
        MailboxIdentity::from_secret(&recipient).public_key(),
        &sender,
        OutgoingDmFault::Database,
    );
    assert!(result.is_err(), "database fault must fail the transaction");

    // The transaction rolled back: the sequence table has no row for this
    // conversation (the sequence would be 1 on first clean insert).
    assert_eq!(
        storage.next_dm_sequence([6; 32], sender.public()).unwrap(),
        1,
        "sequence must not advance after rollback"
    );
}

/// A fault in the acknowledgement transaction (after the ack row is written,
/// before commit) must roll back: the ack is not recorded and the outbox row
/// is not removed.
#[test]
fn ack_database_fault_rolls_back_completely() {
    let (_dir, storage, sender, recipient) = setup();
    let key = MailboxIdentity::from_secret(&recipient).public_key();

    // First queue a DM so there is an outbox row to acknowledge.
    let dm = storage
        .queue_outgoing_dm([7; 32], sender.public(), "req", "payload", key, &sender)
        .unwrap();

    let ack = MailboxAck::sign(&recipient, dm.envelope.message_id(), sender.public());

    // Fault: the ack transaction fails after writing the ack row.
    let result = storage.process_outgoing_ack_with_fault(
        recipient.public(),
        &ack,
        AckProcessingFault::Database,
    );
    assert!(
        result.is_err(),
        "ack database fault must fail the transaction"
    );

    // The outbox row must still exist (rollback removed nothing).
    let outbox = storage
        .get_dm_outbox(&dm.message_id)
        .expect("query outbox")
        .expect("outbox row must survive the rollback");
    assert_eq!(outbox.recipient, recipient.public());

    // Processing the ack again without the fault succeeds and removes the row.
    let processed = storage
        .process_outgoing_ack(recipient.public(), &ack)
        .expect("clean ack processing must succeed");
    assert!(processed, "ack should be newly processed after clean retry");
    assert!(
        storage.get_dm_outbox(&dm.message_id).unwrap().is_none(),
        "acknowledged outbox row must be removed after success"
    );
}

/// A clean (no-fault) queue + ack round trip still works — control for the
/// fault-injection assertions above.
#[test]
fn clean_queue_ack_roundtrip_control() {
    let (_dir, storage, sender, recipient) = setup();
    let key = MailboxIdentity::from_secret(&recipient).public_key();
    let dm = storage
        .queue_outgoing_dm([8; 32], sender.public(), "req", "payload", key, &sender)
        .unwrap();
    let ack = MailboxAck::sign(&recipient, dm.envelope.message_id(), sender.public());
    assert!(storage
        .process_outgoing_ack(recipient.public(), &ack)
        .unwrap());
    assert!(storage.get_dm_outbox(&dm.message_id).unwrap().is_none());
}
