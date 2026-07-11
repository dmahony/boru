use iroh::SecretKey;
use iroh_gossip::mailbox::{MailboxAck, MailboxIdentity, MailboxStore};
use std::time::Duration;

#[test]
fn offline_mailbox_replays_after_restart_and_ack_removes_once() {
    let dir = tempfile::tempdir().unwrap();
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let mut store = MailboxStore::with_ttl(dir.path(), Duration::from_secs(3600));

    let envelope = identity.seal(&sender, b"offline hello").unwrap();
    let id = envelope.message_id();
    store.enqueue(envelope.clone(), &[sender.public()]).unwrap();
    store.save().unwrap();

    let mut restarted = MailboxStore::load(dir.path()).unwrap().unwrap();
    let replay = restarted.pending().unwrap();
    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].open(&recipient).unwrap(), b"offline hello");

    let ack = MailboxAck::sign(&recipient, id);
    assert!(restarted.acknowledge(&ack).unwrap());
    assert!(!restarted.acknowledge(&ack).unwrap());
    restarted.save().unwrap();
    assert!(MailboxStore::load(dir.path())
        .unwrap()
        .unwrap()
        .pending()
        .unwrap()
        .is_empty());
}

#[test]
fn mailbox_rejects_unauthorized_and_duplicate_messages() {
    let dir = tempfile::tempdir().unwrap();
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let stranger = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let mut store = MailboxStore::empty_at(dir.path());
    let envelope = identity.seal(&sender, b"one").unwrap();

    assert!(store
        .enqueue(envelope.clone(), &[stranger.public()])
        .is_err());
    store.enqueue(envelope.clone(), &[sender.public()]).unwrap();
    assert!(store.enqueue(envelope, &[sender.public()]).is_err());
}

#[test]
fn mailbox_rejects_tampering_and_wrong_ack_signer() {
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let mut envelope = identity.seal(&sender, b"secret").unwrap();
    envelope.ciphertext[0] ^= 1;
    assert!(envelope.open(&recipient).is_err());

    let dir = tempfile::tempdir().unwrap();
    let mut store = MailboxStore::empty_at(dir.path());
    let envelope = identity.seal(&sender, b"secret").unwrap();
    let id = envelope.message_id();
    store.enqueue(envelope, &[sender.public()]).unwrap();
    let bad_ack = MailboxAck::sign(&SecretKey::generate(), id);
    assert!(store.acknowledge(&bad_ack).is_err());
}
