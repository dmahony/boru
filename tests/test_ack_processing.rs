use boru_core::{
    mailbox::{MailboxAck, MailboxIdentity},
    storage::{AckProcessingFault, Storage},
};
use iroh::SecretKey;
use tempfile::TempDir;

fn setup() -> (
    TempDir,
    Storage,
    SecretKey,
    SecretKey,
    boru_core::storage::OutgoingDm,
) {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path()).unwrap();
    let sender = SecretKey::generate();
    let recipient = SecretKey::generate();
    let outgoing = storage
        .queue_outgoing_dm(
            [9; 32],
            sender.public(),
            "ack-test",
            "payload",
            MailboxIdentity::from_secret(&recipient).public_key(),
            &sender,
        )
        .unwrap();
    (dir, storage, sender, recipient, outgoing)
}

fn ack(
    outgoing: &boru_core::storage::OutgoingDm,
    sender: &SecretKey,
    recipient: &SecretKey,
) -> MailboxAck {
    MailboxAck::sign(recipient, outgoing.envelope.message_id(), sender.public())
}

#[test]
fn valid_ack_is_transactional_and_duplicate_is_harmless() {
    let (_dir, storage, sender, recipient, outgoing) = setup();
    let a = ack(&outgoing, &sender, &recipient);
    assert!(storage
        .process_outgoing_ack(recipient.public(), &a)
        .unwrap());
    assert!(storage
        .get_dm_outbox(&outgoing.message_id)
        .unwrap()
        .is_none());
    assert!(storage.dm_acknowledged(&outgoing.message_id).unwrap());
    assert!(!storage
        .process_outgoing_ack(recipient.public(), &a)
        .unwrap());
}

#[test]
fn wrong_signer_and_wrong_recipient_do_not_change_state() {
    let (_dir, storage, sender, recipient, outgoing) = setup();
    let wrong = SecretKey::generate();
    let signer_ack = ack(&outgoing, &sender, &wrong);
    assert!(storage
        .process_outgoing_ack(wrong.public(), &signer_ack)
        .is_err());
    assert!(storage
        .get_dm_outbox(&outgoing.message_id)
        .unwrap()
        .is_some());
    assert!(!storage.dm_acknowledged(&outgoing.message_id).unwrap());

    let recipient_ack = ack(&outgoing, &sender, &recipient);
    assert!(storage
        .process_outgoing_ack(wrong.public(), &recipient_ack)
        .is_err());
    assert!(storage
        .get_dm_outbox(&outgoing.message_id)
        .unwrap()
        .is_some());
}

#[test]
fn unknown_message_and_malformed_signature_do_not_change_state() {
    let (_dir, storage, sender, recipient, outgoing) = setup();
    let unknown = MailboxAck::sign(&recipient, "00".repeat(32), sender.public());
    assert!(storage
        .process_outgoing_ack(recipient.public(), &unknown)
        .is_err());

    let mut malformed = ack(&outgoing, &sender, &recipient);
    let mut signature = *malformed.signature;
    signature[0] ^= 1;
    malformed.signature = serde_byte_array::ByteArray::new(signature);
    assert!(storage
        .process_outgoing_ack(recipient.public(), &malformed)
        .is_err());
    assert!(storage
        .get_dm_outbox(&outgoing.message_id)
        .unwrap()
        .is_some());
    assert!(!storage.dm_acknowledged(&outgoing.message_id).unwrap());
}

#[test]
fn acknowledgement_rollback_leaves_all_durable_state_untouched() {
    let (_dir, storage, sender, recipient, outgoing) = setup();
    let a = ack(&outgoing, &sender, &recipient);
    assert!(storage
        .process_outgoing_ack_with_fault(recipient.public(), &a, AckProcessingFault::Database)
        .is_err());
    assert!(storage
        .get_dm_outbox(&outgoing.message_id)
        .unwrap()
        .is_some());
    assert!(!storage.dm_acknowledged(&outgoing.message_id).unwrap());
    assert!(storage
        .process_outgoing_ack(recipient.public(), &a)
        .unwrap());
}

#[test]
fn acknowledgement_survives_sender_restart() {
    let (dir, storage, sender, recipient, outgoing) = setup();
    drop(storage);
    let restarted = Storage::open(dir.path()).unwrap();
    let a = ack(&outgoing, &sender, &recipient);
    assert!(restarted
        .process_outgoing_ack(recipient.public(), &a)
        .unwrap());
    assert!(restarted
        .get_dm_outbox(&outgoing.message_id)
        .unwrap()
        .is_none());
    assert!(restarted.dm_acknowledged(&outgoing.message_id).unwrap());
}
