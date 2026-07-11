use iroh::SecretKey;
use iroh_gossip::mailbox::{MailboxAck, MailboxIdentity, MailboxStore, DEFAULT_MAILBOX_TTL};
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

#[test]
fn mailbox_recipient_restart_preserves_pending_for_reconnect() {
    /// After accept_incoming + save, reloading the store and calling
    /// pending_for_recipient must return the accepted envelope. This
    /// simulates a recipient restart that needs to serve pending
    /// envelopes via the inbox SyncResponse handler.
    let dir = tempfile::tempdir().unwrap();
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let mut store = MailboxStore::with_ttl(dir.path(), Duration::from_secs(3600));

    let envelope = identity.seal(&sender, b"pending for reconnect").unwrap();
    let (msg_id, _payload) = store
        .accept_incoming(&identity, envelope, &[sender.public()])
        .unwrap();

    // Simulate restart: drop store, load from disk.
    let mut loaded = MailboxStore::load(dir.path()).unwrap().unwrap();
    let pending = loaded.pending_for_recipient(recipient.public());
    assert_eq!(
        pending.len(),
        1,
        "should have 1 pending envelope after restart"
    );
    assert_eq!(
        pending[0].message_id(),
        msg_id,
        "message id should match after restart"
    );
    // Verify we can decrypt the replayed envelope.
    assert_eq!(
        pending[0].open(&recipient).unwrap(),
        b"pending for reconnect"
    );
}

#[test]
fn mailbox_expired_messages_rejected_by_validate_for() {
    /// Envelopes with created_at older than the TTL must be rejected by
    /// validate_for. We create an envelope with a well-past timestamp to
    /// simulate an expired message.
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let mut envelope = identity.seal(&sender, b"soon-to-expire").unwrap();

    // Set created_at far in the past so it exceeds even a generous TTL.
    let ancient = 1_000_000; // well before Unix epoch + 1M seconds
    envelope.created_at = ancient;

    // A 1-hour TTL — the envelope is more than 1 hour old.
    let result = envelope.validate_for(&identity, &[sender.public()], Duration::from_secs(3600));
    assert!(
        result.is_err(),
        "envelope with ancient timestamp must be rejected"
    );
    assert!(
        result.unwrap_err().to_string().contains("expired"),
        "error must mention expiry"
    );
}

#[test]
fn mailbox_accept_incoming_handles_expired_envelope() {
    /// accept_incoming should reject an envelope whose created_at exceeds
    /// the TTL, just like validate_for does.
    let dir = tempfile::tempdir().unwrap();
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let mut store = MailboxStore::with_ttl(dir.path(), Duration::from_secs(3600));

    let mut envelope = identity.seal(&sender, b"expired").unwrap();
    // Set created_at far in the past so it exceeds the 1-hour TTL.
    let ancient = 1_000_000;
    envelope.created_at = ancient;

    let result = store.accept_incoming(&identity, envelope, &[sender.public()]);
    assert!(
        result.is_err(),
        "accept_incoming must reject expired envelope"
    );
}

#[test]
fn mailbox_lost_ack_stays_pending_across_restart() {
    /// If an acknowledgement is never received, the envelope must remain
    /// in the mailbox across restarts so it can be replayed again.
    let dir = tempfile::tempdir().unwrap();
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let mut store = MailboxStore::with_ttl(dir.path(), Duration::from_secs(3600));

    let envelope = identity.seal(&sender, b"lost-ack").unwrap();
    let (msg_id, _payload) = store
        .accept_incoming(&identity, envelope, &[sender.public()])
        .unwrap();
    store.save().unwrap();

    // Simulate restart — no ack was ever sent.
    let mut loaded = MailboxStore::load(dir.path()).unwrap().unwrap();
    let pending = loaded.pending_for_recipient(recipient.public());
    assert_eq!(pending.len(), 1, "envelope persists without ack");
    assert_eq!(pending[0].message_id(), msg_id);

    // After ack, envelope is removed.
    let ack = MailboxAck::sign(&recipient, msg_id);
    assert!(loaded.acknowledge(&ack).unwrap());
    assert!(
        loaded.pending_for_recipient(recipient.public()).is_empty(),
        "envelope removed after ack"
    );
}

#[test]
fn mailbox_pending_for_recipient_filters_by_identity() {
    /// pending_for_recipient must return only envelopes addressed to the
    /// specified recipient, and must return empty for a different key.
    let dir = tempfile::tempdir().unwrap();
    let recipient = SecretKey::generate();
    let sender_a = SecretKey::generate();
    let sender_b = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let mut store = MailboxStore::with_ttl(dir.path(), Duration::from_secs(3600));

    let env_a = identity.seal(&sender_a, b"from A").unwrap();
    let env_b = identity.seal(&sender_b, b"from B").unwrap();

    store.enqueue(env_a, &[sender_a.public()]).unwrap();
    store.enqueue(env_b, &[sender_b.public()]).unwrap();

    // All entries are for the configured recipient.
    let all = store.pending_for_recipient(recipient.public());
    assert_eq!(all.len(), 2, "both envelopes for this recipient");

    // A different key returns no entries.
    let different = SecretKey::generate();
    let none = store.pending_for_recipient(different.public());
    assert!(none.is_empty(), "no entries for a different recipient key");
}

#[test]
fn mailbox_invalid_identity_rejected_by_validate_for() {
    /// An envelope encrypted for one recipient must be rejected by
    /// validate_for when provided with a different identity.
    let client = SecretKey::generate();
    let server_a = SecretKey::generate();
    let server_b = SecretKey::generate();
    let identity_a = MailboxIdentity::from_secret(&server_a);
    let identity_b = MailboxIdentity::from_secret(&server_b);

    // Seal for server_a's advertised key.
    let envelope = identity_a.seal(&client, b"for A only").unwrap();

    // Try to validate with server_b's identity — must fail.
    let result = envelope.validate_for(&identity_b, &[client.public()], DEFAULT_MAILBOX_TTL);
    assert!(
        result.is_err(),
        "validate_for must reject envelope not addressed to this identity"
    );
    assert!(
        result.unwrap_err().to_string().contains("recipient"),
        "error must mention recipient mismatch"
    );
}

#[test]
fn mailbox_envelope_rejects_future_timestamp() {
    /// Envelopes with created_at more than 60 seconds in the future must
    /// be rejected as invalid.
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let mut envelope = identity.seal(&sender, b"from future").unwrap();

    // Set created_at far in the future.
    envelope.created_at = u64::MAX;

    let result = envelope.validate_for(&identity, &[sender.public()], DEFAULT_MAILBOX_TTL);
    assert!(
        result.is_err(),
        "future-timestamp envelope must be rejected"
    );
    assert!(
        result.unwrap_err().to_string().contains("expired"),
        "error must mention expiry"
    );
}
