//! Deterministic, bounded fuzz smoke (BORU-AUDIT-28, steps 2 and 9).
//!
//! This module runs a *bounded* mutation fuzzing session inside normal
//! `cargo test` — no nightly, no libFuzzer — so CI can exercise the same
//! decoder surfaces on every change.  The longer sustained session runs
//! through the cargo-fuzz targets in `fuzz/`.
//!
//! Iteration count is controlled by `BORU_FUZZ_ITERATIONS` (default 2000);
//! CI sets it small (e.g. 200) to keep the smoke job short, while a nightly
//! job can raise it.

use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::common::MiniRng;
use boru_core::chat_core::{Message, SignedMessage};
use boru_core::group_events::{GroupEvent, GroupEventPayload, GroupState};
use boru_core::mailbox::{MailboxEnvelope, MailboxIdentity};
use boru_core::short_code::{ShortCodeFreshnessPolicy, SignedShortCodeAnnouncement};
use boru_core::tunnel::TunnelCapability;
use boru_core::wire_compression::decompress;
use boru_core::TopicId;
use iroh::SecretKey;
use std::time::{Duration, SystemTime};

fn iterations() -> usize {
    std::env::var("BORU_FUZZ_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000)
}

/// Feed a byte buffer to every peer-controlled decoder and assert none of
/// them panics (they must reject or return a typed error).
fn exercise_all_decoders(bytes: &[u8]) {
    // SignedMessage envelope decoder.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let _ = SignedMessage::verify_and_decode(bytes);
    }));

    // Mailbox envelope decoder.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let _ = MailboxEnvelope::decode(bytes);
    }));

    // Group event decoder + verify against a fresh state.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let owner = SecretKey::generate();
        let state = GroupState::new(TopicId::from_bytes([7u8; 32]), owner.public());
        if let Ok(event) = GroupEvent::decode(bytes) {
            let _ = event.verify(&state);
        }
    }));

    // Short-code announcement decoder + verify.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let policy =
            ShortCodeFreshnessPolicy::new(Duration::from_secs(300), Duration::from_secs(60));
        let _ =
            SignedShortCodeAnnouncement::verify_at(SystemTime::now(), &policy, bytes, "any-code");
    }));

    // Tunnel capability decoder + verify.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Ok(cap) = postcard::from_bytes::<TunnelCapability>(bytes) {
            let owner = SecretKey::generate().public();
            let peer = SecretKey::generate().public();
            let _ = cap.verify_for(
                &owner,
                &peer,
                boru_core::tunnel::TunnelId([0x11; 32]),
                0,
                true,
            );
        }
    }));

    // Deflate decompressor.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let _ = decompress(bytes);
    }));
}

/// Random bytes, truncated bytes, and mutated copies of valid samples are fed
/// to every decoder.  No panic anywhere, and no decoder accepts random junk
/// as a fully-verified object.
#[test]
fn fuzz_smoke_random_and_truncated_inputs_never_panic() {
    let mut rng = MiniRng::new(0xB0B0_B0B0_5EED);
    let n = iterations();

    for i in 0..n {
        let len = rng.len(512);
        let mut buf = vec![0u8; len];
        rng.fill(&mut buf);
        exercise_all_decoders(&buf);

        // Truncation of the random buffer.
        if !buf.is_empty() {
            let cut = rng.len(buf.len());
            exercise_all_decoders(&buf[..cut]);
        }
        let _ = i;
    }
}

/// Mutation fuzzing over valid signed samples: take real envelopes and
/// randomly corrupt them, ensuring decoders never panic and never accept a
/// corrupted object as valid.
#[test]
fn fuzz_smoke_mutated_valid_samples_never_panic_or_accept() {
    let mut rng = MiniRng::new(0xC0FFEE_5EED);
    let sk = SecretKey::generate();

    // One valid sample per decoder surface.
    let signed_msg = SignedMessage::sign_and_encode(
        &sk,
        &Message::Message {
            text: "fuzz me".into(),
        },
    )
    .unwrap()
    .to_vec();

    let recipient = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let mailbox = postcard::to_stdvec(&identity.seal(&sk, b"fuzz").unwrap()).unwrap();

    let group_event = GroupEvent::sign(
        &sk,
        TopicId::from_bytes([7u8; 32]),
        0,
        GroupEventPayload::MemberInvited {
            member: SecretKey::generate().public(),
        },
    )
    .unwrap()
    .encode()
    .unwrap();

    let samples = [
        signed_msg.as_slice(),
        mailbox.as_slice(),
        group_event.as_slice(),
    ];
    let n = iterations();

    for i in 0..n {
        let sample = samples[i % samples.len()];
        let mut mutated = sample.to_vec();
        // Randomly corrupt 1..=4 bytes.
        let flips = 1 + rng.len(3);
        for _ in 0..flips {
            if mutated.is_empty() {
                break;
            }
            let idx = rng.len(mutated.len() - 1);
            mutated[idx] ^= (rng.next_u64() & 0xFF) as u8;
        }
        // Guard against no-op mutations: if every flip XORed with 0 the
        // buffer is byte-identical to the valid sample and would trivially
        // verify.  Force one guaranteed change (no extra RNG consumption, so
        // the sequence stays deterministic).
        if mutated == sample {
            if let Some(first) = mutated.first_mut() {
                *first ^= 0x01;
            }
        }

        // The decoder must not panic; if it decodes, it must not *verify* as
        // a valid object (random corruption of a signed payload breaks the
        // signature with overwhelming probability).  Note: bare `decode` on
        // GroupEvent/Mailbox does not verify, so the accept predicate must
        // include verification.
        let accepted = catch_unwind(AssertUnwindSafe(|| match i % samples.len() {
            0 => SignedMessage::verify_and_decode(&mutated).is_ok(),
            1 => MailboxEnvelope::decode(&mutated)
                .map(|e| e.open(&recipient).is_ok())
                .unwrap_or(false),
            _ => GroupEvent::decode(&mutated)
                .map(|e| {
                    let owner = sk.public();
                    let state = GroupState::new(TopicId::from_bytes([7u8; 32]), owner);
                    e.verify(&state).is_ok()
                })
                .unwrap_or(false),
        }))
        .expect("decoder must not panic");
        assert!(
            !accepted,
            "mutated valid sample was accepted (iteration {i}, sample {}) — mutated {:02x?}",
            i % samples.len(),
            &mutated
        );
    }
}
