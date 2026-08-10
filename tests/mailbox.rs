use boru_core::mailbox::{MailboxAck, MailboxIdentity, MailboxStore, DEFAULT_MAILBOX_TTL};
use iroh::SecretKey;
use std::time::Duration;

#[test]
fn offline_mailbox_replays_and_ack_removes_once() {
    // A sealed offline envelope can be replayed from the mailbox and an
    // acknowledgement removes it exactly once.  (Cross-restart persistence
    // of offline messages is SQLite-backed since AUDIT-19 and is covered by
    // tests/security/restart.rs::mailbox_state_survives_restart; the legacy
    // JSON `save()`/`load()` round-trip is a deprecated no-op.)
    let dir = tempfile::tempdir().unwrap();
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let mut store = MailboxStore::with_ttl(dir.path(), Duration::from_secs(3600));

    let envelope = identity.seal(&sender, b"offline hello").unwrap();
    let id = envelope.message_id();
    store.enqueue(envelope.clone(), &[sender.public()]).unwrap();

    let replay = store.pending().unwrap();
    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].open(&recipient).unwrap(), b"offline hello");

    let ack = MailboxAck::sign(&recipient, id, sender.public());
    assert!(store.acknowledge(&ack).unwrap());
    assert!(!store.acknowledge(&ack).unwrap());
    assert!(store.pending().unwrap().is_empty());
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
    envelope.ciphertext_mut().unwrap()[0] ^= 1;
    assert!(envelope.open(&recipient).is_err());

    let dir = tempfile::tempdir().unwrap();
    let mut store = MailboxStore::empty_at(dir.path());
    let envelope = identity.seal(&sender, b"secret").unwrap();
    let id = envelope.message_id();
    store.enqueue(envelope, &[sender.public()]).unwrap();
    let bad_ack = MailboxAck::sign(&SecretKey::generate(), id, sender.public());
    assert!(store.acknowledge(&bad_ack).is_err());
}

#[test]
fn mailbox_accept_incoming_preserves_pending_for_recipient() {
    // accept_incoming retains the envelope so pending_for_recipient can
    // serve it later (the inbox SyncResponse path).  Cross-restart
    // persistence of the accepted envelope is SQLite-backed since AUDIT-19
    // and covered by tests/security/restart.rs; the legacy JSON
    // `save()`/`load()` round-trip is a deprecated no-op.
    let dir = tempfile::tempdir().unwrap();
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let mut store = MailboxStore::with_ttl(dir.path(), Duration::from_secs(3600));

    let envelope = identity.seal(&sender, b"pending for reconnect").unwrap();
    let (msg_id, _payload) = store
        .accept_incoming(&identity, envelope, &[sender.public()])
        .unwrap();

    let pending = store.pending_for_recipient(recipient.public());
    assert_eq!(
        pending.len(),
        1,
        "should have 1 pending envelope after accept_incoming"
    );
    assert_eq!(
        pending[0].message_id(),
        msg_id,
        "message id should match after accept_incoming"
    );
    // Verify we can decrypt the replayed envelope.
    assert_eq!(
        pending[0].open(&recipient).unwrap(),
        b"pending for reconnect"
    );
}

#[test]
fn mailbox_expired_messages_rejected_by_validate_for() {
    // Envelopes with created_at older than the TTL must be rejected by
    // validate_for. We seal at a well-past timestamp (so the signature is
    // still valid) to simulate a genuinely expired message.
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    // Set created_at far in the past so it exceeds even a generous TTL.
    let ancient = 1_000_000; // well before Unix epoch + 1M seconds
    let envelope = identity.seal_at(&sender, b"soon-to-expire", ancient).unwrap();

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
    // accept_incoming should reject an envelope whose created_at exceeds
    // the TTL, just like validate_for does.
    let dir = tempfile::tempdir().unwrap();
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let mut store = MailboxStore::with_ttl(dir.path(), Duration::from_secs(3600));

    // Seal at a far-past timestamp so it exceeds the 1-hour TTL while the
    // signature remains valid.
    let ancient = 1_000_000;
    let envelope = identity.seal_at(&sender, b"expired", ancient).unwrap();

    let result = store.accept_incoming(&identity, envelope, &[sender.public()]);
    assert!(
        result.is_err(),
        "accept_incoming must reject expired envelope"
    );
}

#[test]
fn mailbox_lost_ack_stays_pending_until_acknowledged() {
    // If an acknowledgement is never received, the envelope must remain
    // pending so it can be replayed again; acknowledging removes it.
    // (Cross-restart persistence is SQLite-backed since AUDIT-19 and
    // covered by tests/security/restart.rs; the legacy JSON `save()`/
    // `load()` round-trip is a deprecated no-op.)
    let dir = tempfile::tempdir().unwrap();
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let mut store = MailboxStore::with_ttl(dir.path(), Duration::from_secs(3600));

    let envelope = identity.seal(&sender, b"lost-ack").unwrap();
    let (msg_id, _payload) = store
        .accept_incoming(&identity, envelope, &[sender.public()])
        .unwrap();

    let pending = store.pending_for_recipient(recipient.public());
    assert_eq!(pending.len(), 1, "envelope persists without ack");
    assert_eq!(pending[0].message_id(), msg_id);

    // After ack, envelope is removed.
    let ack = MailboxAck::sign(&recipient, msg_id, sender.public());
    assert!(store.acknowledge(&ack).unwrap());
    assert!(
        store.pending_for_recipient(recipient.public()).is_empty(),
        "envelope removed after ack"
    );
}

#[test]
fn mailbox_pending_for_recipient_filters_by_identity() {
    // pending_for_recipient must return only envelopes addressed to the
    // specified recipient, and must return empty for a different key.
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
    // An envelope encrypted for one recipient must be rejected by
    // validate_for when provided with a different identity.
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
    // Envelopes with created_at more than 60 seconds in the future must
    // be rejected as invalid. Seal at the future timestamp so the
    // signature is valid and the failure is the skew check.
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let envelope = identity.seal_at(&sender, b"from future", u64::MAX).unwrap();

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
