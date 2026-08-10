//! Mutation tests: flip / truncate / extend every byte of every signed
//! protocol object and assert a clean rejection without panic (BORU-AUDIT-28,
//! steps 3 and 10).

use crate::common::{now_secs, sweep_mutations};
use boru_core::chat_core::{
    sign_advertisement, verify_advertisement, Message, RoomAdvertisement, SignedMessage,
};
use boru_core::contact::{ContactAction, SignedContactMessage};
use boru_core::file_access_protocol::{
    sign_download_descriptor, verify_download_descriptor, BlobFormat,
};
use boru_core::group_events::{GroupEvent, GroupEventPayload, GroupState};
use boru_core::inbox::{AuthorDeleteProof, InboxPayload, SignedInboxMessage};
use boru_core::mailbox::MailboxIdentity;
use boru_core::short_code::{
    ShortCodeAnnouncement, ShortCodeFreshnessPolicy, SignedShortCodeAnnouncement,
};
use boru_core::tunnel::{TunnelCapability, TunnelId};
use boru_core::wire_compression::{compress, decompress};
use boru_core::TopicId;
use iroh::{PublicKey, SecretKey};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn group_id() -> TopicId {
    TopicId::from_bytes([7u8; 32])
}

// ── SignedMessage ─────────────────────────────────────────────────────────

#[test]
fn signed_message_flip_truncate_extend_rejected_without_panic() {
    let sk = SecretKey::generate();
    // Use a payload large enough that deflate actually shrinks it
    // (compression = 1).  A tiny message encodes with compression = 0, whose
    // trailing `compression` byte is *optional* in the legacy framing — a
    // truncation that drops just that byte decodes and verifies by design
    // (AUDIT-27 migration window).  A compressed envelope has no such
    // ambiguity: dropping the compression byte yields compressed data that
    // cannot decode as a Message, so every strict truncation is rejected.
    let text = "hello peer, this message body is large enough to compress well ".repeat(8);
    let msg = Message::Message { text };
    let encoded = SignedMessage::sign_and_encode_compressed(&sk, &msg)
        .unwrap()
        .to_vec();

    sweep_mutations("SignedMessage", &encoded, |bytes| {
        SignedMessage::verify_and_decode(bytes).is_ok()
    });
}

// ── Mailbox envelope ──────────────────────────────────────────────────────

#[test]
fn mailbox_envelope_flip_truncate_extend_rejected_without_panic() {
    let sender = SecretKey::generate();
    let recipient = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let envelope = identity.seal(&sender, b"offline payload").unwrap();
    let encoded = postcard::to_stdvec(&envelope).unwrap();

    // Accepting a mutation here means the envelope both decodes AND opens with
    // the recipient key. Mutating the ciphertext/signature/nonce must fail.
    sweep_mutations("MailboxEnvelope", &encoded, |bytes| {
        match boru_core::mailbox::MailboxEnvelope::decode(bytes) {
            Ok(envelope) => envelope.open(&recipient).is_ok(),
            Err(_) => false,
        }
    });
}

// ── Group event ───────────────────────────────────────────────────────────

#[test]
fn group_event_flip_truncate_extend_rejected_without_panic() {
    let owner = SecretKey::generate();
    let member = SecretKey::generate().public();
    let event = GroupEvent::sign(
        &owner,
        group_id(),
        0,
        GroupEventPayload::MemberInvited { member },
    )
    .unwrap();
    let encoded = event.encode().unwrap();

    // A mutated event must either fail to decode or fail to verify against a
    // fresh state (or be a replay of the exact same bytes — impossible after
    // mutation).
    sweep_mutations("GroupEvent", &encoded, |bytes| {
        match GroupEvent::decode(bytes) {
            Ok(decoded) => {
                let mut state = GroupState::new(group_id(), owner.public());
                decoded.verify(&state).is_ok() && decoded.apply(&mut state).is_ok()
            }
            Err(_) => false,
        }
    });
}

// ── Download descriptor ───────────────────────────────────────────────────

#[test]
fn download_descriptor_flip_truncate_extend_rejected_without_panic() {
    let owner = SecretKey::generate();
    let requester = SecretKey::generate().public();
    let descriptor = sign_download_descriptor(
        &owner,
        requester,
        "shared-file-1".into(),
        [0xAB; 32],
        4096,
        BlobFormat::Raw,
        now_secs() * 1000,
        (now_secs() + 3600) * 1000,
    );
    let encoded = postcard::to_stdvec(&descriptor).unwrap();

    sweep_mutations(
        "SignedDownloadDescriptor",
        &encoded,
        |bytes| match postcard::from_bytes::<
            boru_core::file_access_protocol::SignedDownloadDescriptor,
        >(bytes)
        {
            Ok(desc) => {
                matches!(
                    verify_download_descriptor(
                        &desc,
                        &owner.public(),
                        &requester,
                        now_secs() * 1000,
                    ),
                    boru_core::file_access_protocol::DescriptorVerification::Valid
                )
            }
            Err(_) => false,
        },
    );
}

// ── Short-code announcement ────────────────────────────────────────────────

#[test]
fn short_code_announcement_flip_truncate_extend_rejected_without_panic() {
    let sk = SecretKey::generate();
    let announcement = ShortCodeAnnouncement {
        code: "ABC123".into(),
        name: "report.pdf".into(),
        ticket: "blob:iroh:abc".into(),
        size: 1024,
        created_at_ms: now_secs() * 1000,
    };
    let encoded = SignedShortCodeAnnouncement::sign(&sk, &announcement).unwrap();
    let policy =
        ShortCodeFreshnessPolicy::new(Duration::from_secs(5 * 60), Duration::from_secs(60));

    sweep_mutations("ShortCodeAnnouncement", &encoded, |bytes| {
        SignedShortCodeAnnouncement::verify_at(SystemTime::now(), &policy, bytes, "ABC123").is_ok()
    });
}

// ── Contact message ───────────────────────────────────────────────────────

#[test]
fn contact_message_flip_truncate_extend_rejected_without_panic() {
    let sk = SecretKey::generate();
    let action = ContactAction::FriendRequest {
        name: Some("Alice".into()),
    };
    let encoded = SignedContactMessage::sign(&sk, &action).unwrap();

    sweep_mutations("SignedContactMessage", &encoded, |bytes| {
        SignedContactMessage::verify(bytes, Some(sk.public())).is_ok()
    });
}

// ── Room advertisement ────────────────────────────────────────────────────

#[test]
fn room_advertisement_flip_truncate_extend_rejected_without_panic() {
    let sk = SecretKey::generate();
    let ad = RoomAdvertisement {
        room_name: "Lobby".into(),
        description: "public".into(),
        topic: group_id(),
        ticket: "blob:iroh:xyz".into(),
        member_count: 1,
        last_activity: now_secs() * 1000,
    };
    let sig = sign_advertisement(&ad, &sk);
    let mut encoded = postcard::to_stdvec(&ad).unwrap();
    encoded.extend_from_slice(&sig);

    sweep_mutations("RoomAdvertisement", &encoded, |bytes| {
        // The mutation may land in the advertisement or the trailing signature;
        // acceptance requires both decode and a valid signature over it.
        if bytes.len() < sig.len() {
            return false;
        }
        let (ad_bytes, sig_bytes) = bytes.split_at(bytes.len() - sig.len());
        match postcard::from_bytes::<RoomAdvertisement>(ad_bytes) {
            Ok(ad) => verify_advertisement(&ad, sig_bytes, sk.public()),
            Err(_) => false,
        }
    });
}

// ── Tunnel capability ─────────────────────────────────────────────────────

#[test]
fn tunnel_capability_flip_truncate_extend_rejected_without_panic() {
    let owner = SecretKey::generate();
    let peer = SecretKey::generate().public();
    let tunnel = TunnelId([0x11; 32]);
    let now = now_secs() * 1000;
    let cap = TunnelCapability::sign(&owner, peer, tunnel, now, now + 60_000);
    let encoded = postcard::to_stdvec(&cap).unwrap();

    sweep_mutations(
        "TunnelCapability",
        &encoded,
        |bytes| match postcard::from_bytes::<TunnelCapability>(bytes) {
            Ok(cap) => cap
                .verify_for(&owner.public(), &peer, tunnel, now + 1_000, true)
                .is_ok(),
            Err(_) => false,
        },
    );
}

// ── Inbox message ─────────────────────────────────────────────────────────

#[test]
fn inbox_message_flip_truncate_extend_rejected_without_panic() {
    let sk = SecretKey::generate();
    let payload = InboxPayload::SyncRequest { since_ms: 0 };
    let encoded = SignedInboxMessage::sign(&sk, payload).unwrap();

    sweep_mutations("SignedInboxMessage", &encoded, |bytes| {
        SignedInboxMessage::verify(bytes, Some(sk.public())).is_ok()
    });
}

// ── Author-delete proof ───────────────────────────────────────────────────

#[test]
fn author_delete_proof_flip_truncate_extend_rejected_without_panic() {
    let author = SecretKey::generate();
    let proof = AuthorDeleteProof::sign(&author, [0x42; 32], [0x24; 32]);
    let encoded = postcard::to_stdvec(&proof).unwrap();

    sweep_mutations(
        "AuthorDeleteProof",
        &encoded,
        |bytes| match postcard::from_bytes::<AuthorDeleteProof>(bytes) {
            Ok(proof) => proof.verify().is_ok(),
            Err(_) => false,
        },
    );
}

// ── Compression stream ────────────────────────────────────────────────────

#[test]
fn compression_stream_flip_truncate_extend_rejected_without_panic() {
    // Compressed payloads are peer-controlled bytes; decompress must never
    // panic on corruption.  A flipped deflate byte either breaks the stream
    // (Err) or decodes to *different* bytes — it must never reproduce the
    // original payload.  A strict truncation similarly cannot round-trip the
    // full payload (deflate is a stream; there is no trailing framing the
    // decoder tolerates).  So exact round-trip is the accept predicate; the
    // harness additionally asserts no panic for every mutation.
    let payload = b"the quick brown fox jumps over the lazy dog".repeat(8);
    let compressed = compress(&payload);
    sweep_mutations("decompress", &compressed, |bytes| match decompress(bytes) {
        Ok(out) => out == payload,
        Err(_) => false,
    });
}

// ── Cross-check: valid samples still verify ───────────────────────────────

#[test]
fn valid_samples_verify_as_control() {
    // Sanity: the mutation sweeps above are only meaningful if the pristine
    // samples are accepted by the same predicates.
    let sk = SecretKey::generate();
    let msg = Message::Message {
        text: "control".into(),
    };
    let encoded = SignedMessage::sign_and_encode(&sk, &msg).unwrap();
    assert!(SignedMessage::verify_and_decode(&encoded).is_ok());

    let owner = SecretKey::generate();
    let member = SecretKey::generate().public();
    let event = GroupEvent::sign(
        &owner,
        group_id(),
        0,
        GroupEventPayload::MemberInvited { member },
    )
    .unwrap();
    let state = GroupState::new(group_id(), owner.public());
    assert!(event.verify(&state).is_ok());

    let recipient = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let envelope = identity.seal(&sk, b"ok").unwrap();
    assert_eq!(envelope.open(&recipient).unwrap(), b"ok");

    let _ = PublicKey::from_bytes(&[0u8; 32]);
    let _ = UNIX_EPOCH;
}
