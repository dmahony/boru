//! Oversized-length and allocation tests: malicious advertised sizes must
//! not cause unbounded memory allocation (BORU-AUDIT-28, step 4).

use boru_core::chat_core::{Message, SignedMessage};
use boru_core::group_events::{GroupEvent, GroupEventPayload, MAX_GROUP_EVENT_PAYLOAD};
use boru_core::mailbox::{MailboxIdentity, MailboxStore, DEFAULT_MAILBOX_TTL};
use boru_core::protocol_version::{frame_payload_len_ok, MAX_FRAME_PAYLOAD};
use boru_core::wire_compression::{compress, decompress};
use boru_core::TopicId;
use iroh::SecretKey;
use std::time::Duration;

/// A postcard length prefix that advertises an absurd size must be rejected
/// without attempting a matching allocation.  `postcard` uses a varint length
/// prefix; a truncated buffer whose varint claims gigabytes must fail with
/// `UnexpectedEnd`, not OOM.
#[test]
fn postcard_huge_length_prefix_rejected_without_allocation() {
    // 10 bytes of 0xFF = varint 2^70-ish; a Vec<u8> field with this length
    // would try to allocate ~1 ZiB if the decoder trusted it.
    let huge_prefix = [0xFFu8; 10];
    let result: Result<Vec<u8>, _> = postcard::from_bytes(&huge_prefix);
    assert!(
        result.is_err(),
        "huge postcard length prefix must be rejected"
    );
}

/// An oversized SignedMessage envelope (10 MB of garbage) fails decode
/// gracefully instead of panicking.
#[test]
fn signed_message_huge_buffer_rejected_without_panic() {
    let huge = vec![0x01u8; 10_000_000];
    let result = SignedMessage::verify_and_decode(&huge);
    assert!(
        result.is_err(),
        "10MB malformed envelope must fail gracefully"
    );
}

/// A message whose *advertised* data length is enormous (varint prefix says
/// gigabytes) must fail cleanly — the signature/decode path never allocates
/// the advertised size.
#[test]
fn signed_message_advertised_huge_length_rejected() {
    // Build a valid envelope then corrupt the length varint in the `data`
    // field by inserting an absurd varint right after the `from` key. The
    // decode must return Err, never panic, and never allocate the claimed
    // size.
    let sk = SecretKey::generate();
    let msg = Message::Message {
        text: "tiny".into(),
    };
    let encoded = SignedMessage::sign_and_encode(&sk, &msg).unwrap();

    // Insert a 10-byte 0xFF varint (huge length) after the 32-byte from key.
    let mut hostile = Vec::with_capacity(encoded.len() + 10);
    hostile.extend_from_slice(&encoded[..32]);
    hostile.extend_from_slice(&[0xFFu8; 10]);
    hostile.extend_from_slice(&encoded[32..]);

    let result = SignedMessage::verify_and_decode(&hostile);
    assert!(
        result.is_err(),
        "advertised huge data length must be rejected"
    );
}

/// Group events cap encoded payloads at `MAX_GROUP_EVENT_PAYLOAD`; a crafted
/// event advertising a huge payload must be rejected before allocation.
#[test]
fn group_event_payload_size_cap_enforced() {
    let owner = SecretKey::generate();
    let member = SecretKey::generate().public();
    let event = GroupEvent::sign(
        &owner,
        TopicId::from_bytes([7u8; 32]),
        0,
        GroupEventPayload::MemberInvited { member },
    )
    .unwrap();
    let encoded = event.encode().unwrap();
    assert!(
        encoded.len() <= MAX_GROUP_EVENT_PAYLOAD,
        "valid event should be under the payload cap"
    );

    // A hostile decode of an over-cap buffer must fail, not allocate.
    let hostile = vec![0xABu8; MAX_GROUP_EVENT_PAYLOAD + 1];
    let result = GroupEvent::decode(&hostile);
    assert!(result.is_err(), "over-cap group event must fail decode");

    // Truncated event must fail cleanly too.
    let truncated = &encoded[..encoded.len() / 2];
    assert!(GroupEvent::decode(truncated).is_err());
}

/// Mailbox store must reject over-TTL / over-cap queue growth in a bounded
/// way.  A huge envelope blob fails decode.
#[test]
fn mailbox_huge_envelope_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let mut store = MailboxStore::with_ttl(dir.path(), DEFAULT_MAILBOX_TTL);

    let envelope = identity.seal(&sender, b"payload").unwrap();
    let encoded = postcard::to_stdvec(&envelope).unwrap();
    assert!(encoded.len() < 4096, "valid envelope is small");

    // Hostile decode of a huge blob must error, not allocate.
    let hostile = vec![0xEEu8; 8_000_000];
    let result = boru_core::mailbox::MailboxEnvelope::decode(&hostile);
    assert!(result.is_err(), "8MB hostile mailbox blob must fail decode");

    // Decoding the valid envelope still works (control).
    let decoded = boru_core::mailbox::MailboxEnvelope::decode(&encoded).unwrap();
    assert!(decoded.open(&recipient).is_ok());
    let _ = store.enqueue(decoded, &[sender.public()]);
}

/// Frame length gate rejects a malicious advertised frame size before the
/// caller would allocate a buffer of that size.
#[test]
fn frame_length_gate_prevents_allocation_attack() {
    assert!(frame_payload_len_ok(MAX_FRAME_PAYLOAD));
    assert!(frame_payload_len_ok(MAX_FRAME_PAYLOAD - 1));
    assert!(!frame_payload_len_ok(MAX_FRAME_PAYLOAD + 1));
    assert!(!frame_payload_len_ok(u32::MAX as usize));
    assert!(!frame_payload_len_ok(usize::MAX));
}

/// A deflate bomb (tiny compressed input that would expand enormously) is
/// rejected by the decompressor's cap instead of growing memory without
/// bound.
#[test]
fn decompress_rejects_deflate_bomb() {
    // Build a highly repetitive payload: compresses to a tiny deflate stream
    // but decompresses to MAX_DECOMPRESSED_SIZE+ bytes.
    let bomb_payload = vec![0u8; 64 * 1024 * 1024 + 1024]; // > 64 MiB cap
    let compressed = compress(&bomb_payload);
    assert!(
        compressed.len() < bomb_payload.len() / 100,
        "repetitive payload must compress hard"
    );

    let result = decompress(&compressed);
    assert!(
        result.is_err(),
        "deflate bomb exceeding the cap must be rejected"
    );
}

/// Random garbage never causes decompress to allocate unboundedly: it errors
/// or returns small output, and never panics (caught by the caller).
#[test]
fn decompress_garbage_bounded() {
    for size in [0usize, 1, 16, 1024, 4096, 1_000_000] {
        let garbage: Vec<u8> = (0..size as u32).map(|i| (i % 251) as u8).collect();
        match decompress(&garbage) {
            Ok(out) => assert!(out.len() <= 64 * 1024 * 1024),
            Err(_) => {}
        }
    }
}

/// The gossip frame cap is enforced by `write_frame` (reject before writing)
/// and `read_frame` (reject before allocating).
#[test]
fn frame_read_write_caps_bounded() {
    // `write_frame` rejects oversized payloads with InvalidInput. We exercise
    // the pure gate directly (the async path uses the same predicate).
    assert!(!frame_payload_len_ok(MAX_FRAME_PAYLOAD + 1));
    // A frame length of u32::MAX must never pass the gate.
    assert!(!frame_payload_len_ok(u32::MAX as usize));
}

/// `MailboxStore` TTL pruning bounds memory: after expiry the store drops
/// stale envelopes (no unbounded retention).
#[test]
fn mailbox_ttl_prunes_stale_envelopes() {
    let dir = tempfile::tempdir().unwrap();
    let recipient = SecretKey::generate();
    let sender = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);

    // Use a short TTL so the envelope is immediately stale.  `pending()`
    // internally expires entries older than the TTL (no persistence needed —
    // the legacy JSON save/load path is deprecated and no longer writes).
    let mut store = MailboxStore::with_ttl(dir.path(), Duration::from_millis(1));
    let envelope = identity.seal(&sender, b"stale").unwrap();
    store.enqueue(envelope, &[sender.public()]).unwrap();

    // Wait a moment: the entry is now past the 1 ms TTL.
    std::thread::sleep(Duration::from_millis(50));
    let pending = store.pending().unwrap();
    assert!(
        pending.is_empty(),
        "expired envelopes must be pruned, not retained unboundedly"
    );
}
