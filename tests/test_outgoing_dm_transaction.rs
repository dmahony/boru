use boru_chat::{
    mailbox::MailboxIdentity,
    storage::{OutgoingDmFault, Storage},
};
use iroh::SecretKey;
use tempfile::TempDir;

fn setup() -> (TempDir, Storage, SecretKey, SecretKey) {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path()).unwrap();
    let sender = SecretKey::generate();
    let recipient = SecretKey::generate();
    (dir, storage, sender, recipient)
}

#[test]
fn queue_outgoing_dm_commits_visible_message_and_exact_encrypted_outbox() {
    let (_dir, storage, sender, recipient) = setup();
    let conversation_id = [7u8; 32];
    let result = storage
        .queue_outgoing_dm(
            conversation_id,
            sender.public(),
            "request-1",
            "hello",
            MailboxIdentity::from_secret(&recipient).public_key(),
            &sender,
        )
        .unwrap();

    assert_eq!(result.sequence, 1);
    assert_eq!(result.message_id.len(), 32);
    let message = storage.get_dm_message(&result.message_id).unwrap().unwrap();
    assert_eq!(message.plaintext, b"hello");
    let outbox = storage.get_dm_outbox(&result.message_id).unwrap().unwrap();
    assert_eq!(
        postcard::to_stdvec(&outbox.envelope).unwrap(),
        postcard::to_stdvec(&result.envelope).unwrap()
    );
    assert_eq!(outbox.recipient, recipient.public());
    assert_eq!(
        result.envelope.open(&recipient).unwrap(),
        result.logical_message
    );
}

#[test]
fn retry_with_same_request_key_is_idempotent_and_does_not_advance_sequence() {
    let (_dir, storage, sender, recipient) = setup();
    let key = MailboxIdentity::from_secret(&recipient).public_key();
    let first = storage
        .queue_outgoing_dm([1; 32], sender.public(), "same", "same", key, &sender)
        .unwrap();
    let second = storage
        .queue_outgoing_dm([1; 32], sender.public(), "same", "same", key, &sender)
        .unwrap();
    assert_eq!(first.message_id, second.message_id);
    assert_eq!(first.sequence, second.sequence);
    assert_eq!(first.logical_message, second.logical_message);
    assert_eq!(
        postcard::to_stdvec(&first.envelope).unwrap(),
        postcard::to_stdvec(&second.envelope).unwrap()
    );
    assert_eq!(
        storage.next_dm_sequence([1; 32], sender.public()).unwrap(),
        2
    );
}

#[test]
fn retry_with_same_request_key_but_different_recipient_is_rejected() {
    let (_dir, storage, sender, recipient) = setup();
    let other_recipient = SecretKey::generate();
    let first = storage
        .queue_outgoing_dm(
            [4; 32],
            sender.public(),
            "same",
            "same",
            MailboxIdentity::from_secret(&recipient).public_key(),
            &sender,
        )
        .unwrap();
    let error = storage
        .queue_outgoing_dm(
            [4; 32],
            sender.public(),
            "same",
            "same",
            MailboxIdentity::from_secret(&other_recipient).public_key(),
            &sender,
        )
        .unwrap_err();
    assert!(error.to_string().contains("idempotency key"));
    assert_eq!(storage.next_dm_sequence([4; 32], sender.public()).unwrap(), 2);
    assert!(storage.get_dm_message(&first.message_id).unwrap().is_some());
}

#[test]
fn conflicting_retry_rolls_back_without_advancing_sequence() {
    let (_dir, storage, sender, recipient) = setup();
    let key = MailboxIdentity::from_secret(&recipient).public_key();
    let first = storage
        .queue_outgoing_dm([3; 32], sender.public(), "same", "one", key, &sender)
        .unwrap();
    assert!(storage
        .queue_outgoing_dm([3; 32], sender.public(), "same", "different", key, &sender)
        .is_err());
    assert_eq!(storage.next_dm_sequence([3; 32], sender.public()).unwrap(), 2);
    assert_eq!(storage.get_dm_message(&first.message_id).unwrap().unwrap().plaintext, b"one");
}

#[test]
fn encryption_failure_rolls_back_all_outgoing_dm_rows() {
    let (_dir, storage, sender, recipient) = setup();
    let result = storage.queue_outgoing_dm_with_fault(
        [5; 32],
        sender.public(),
        "encryption-fails",
        "secret",
        MailboxIdentity::from_secret(&recipient).public_key(),
        &sender,
        OutgoingDmFault::Encryption,
    );
    assert!(result.is_err());
    assert_eq!(storage.next_dm_sequence([5; 32], sender.public()).unwrap(), 1);
}

#[test]
fn database_failure_rolls_back_all_outgoing_dm_rows() {
    let (_dir, storage, sender, recipient) = setup();
    let result = storage.queue_outgoing_dm_with_fault(
        [6; 32],
        sender.public(),
        "database-fails",
        "secret",
        MailboxIdentity::from_secret(&recipient).public_key(),
        &sender,
        OutgoingDmFault::Database,
    );
    assert!(result.is_err());
    assert_eq!(storage.next_dm_sequence([6; 32], sender.public()).unwrap(), 1);
}

#[test]
fn sequence_and_message_id_survive_restart() {
    let (dir, storage, sender, recipient) = setup();
    let key = MailboxIdentity::from_secret(&recipient).public_key();
    let first = storage
        .queue_outgoing_dm([2; 32], sender.public(), "a", "one", key, &sender)
        .unwrap();
    drop(storage);
    let storage = Storage::open(dir.path()).unwrap();
    let second = storage
        .queue_outgoing_dm([2; 32], sender.public(), "b", "two", key, &sender)
        .unwrap();
    assert_eq!(first.sequence, 1);
    assert_eq!(second.sequence, 2);
    assert!(storage.get_dm_message(&first.message_id).unwrap().is_some());
}

#[test]
fn concurrent_handles_allocate_distinct_sequences() {
    let dir = TempDir::new().unwrap();
    let first_storage = Storage::open(dir.path()).unwrap();
    let second_storage = Storage::open(dir.path()).unwrap();
    let sender = SecretKey::generate();
    let recipient = SecretKey::generate();
    let recipient_key = MailboxIdentity::from_secret(&recipient).public_key();
    let first_sender = sender.clone();
    let second_sender = sender;
    let first = std::thread::spawn(move || {
        first_storage
            .queue_outgoing_dm(
                [8; 32],
                first_sender.public(),
                "a",
                "one",
                recipient_key,
                &first_sender,
            )
            .unwrap()
            .sequence
    });
    let second = std::thread::spawn(move || {
        second_storage
            .queue_outgoing_dm(
                [8; 32],
                second_sender.public(),
                "b",
                "two",
                recipient_key,
                &second_sender,
            )
            .unwrap()
            .sequence
    });
    let mut sequences = [first.join().unwrap(), second.join().unwrap()];
    sequences.sort_unstable();
    assert_eq!(sequences, [1, 2]);
}
