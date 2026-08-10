//! Canonical signed-object framing (BORU-AUDIT-27).
//!
//! Every Ed25519-authenticated Boru protocol object follows one rule:
//!
//! ```text
//! canonical bytes = postcard((protocol, version, security-relevant fields...))
//! ```
//!
//! * `protocol` is a stable ASCII domain-separation tag (e.g. `boru/mailbox-ack`)
//!   so a signature over one object family can never be replayed as a signature
//!   over another family.
//! * `version` is embedded **inside** the signed bytes, so the interpretation
//!   of the object is authenticated.  Unknown versions are rejected by
//!   verification, never guessed.
//! * `fields` is the deterministic postcard serialization of ALL
//!   security-relevant fields (identity, routing/topic, timestamps, nonces,
//!   hashes, capability scope, interpretation/version fields).
//!
//! Signing and verification MUST both go through `canonical_signed_bytes` so
//! the two sides can never drift.  During the migration window, verification
//! additionally accepts the pre-AUDIT-27 legacy framing via
//! `verify_canonical_or_legacy` — old persisted/wire objects keep verifying,
//! while new objects get the full domain-separated layout.  New objects are
//! always signed with the canonical layout.
//!
//! See `docs/protocol-signing.md` for the design note and per-object field
//! classification.

use iroh::PublicKey;
use serde::Serialize;

/// Length of an Ed25519 signature in bytes.
pub const SIGNATURE_LEN: usize = 64;

/// Build the canonical bytes that a signature MUST cover for a protocol object.
///
/// `postcard::to_stdvec((protocol, version, fields))`.  Postcard serializes
/// tuple fields in order and length-prefixes every variable-length value, so
/// the encoding is deterministic and unambiguous.
pub fn canonical_signed_bytes<F: Serialize>(
    protocol: &str,
    version: u16,
    fields: &F,
) -> postcard::Result<Vec<u8>> {
    postcard::to_stdvec(&(protocol, version, fields))
}

/// Verify a signature against the canonical bytes; if that fails, against the
/// legacy pre-AUDIT-27 framing.
///
/// The canonical bytes are tried first, so new objects are verified strictly.
/// Only when they fail is the legacy layout consulted, which keeps objects
/// signed before the standardization verifiable (migration window).  An
/// attacker cannot exploit the fallback because producing a *valid* legacy
/// signature still requires the signer's private key.
pub fn verify_canonical_or_legacy(
    key: &PublicKey,
    signature: &[u8],
    canonical: &[u8],
    legacy: &[u8],
) -> bool {
    verify(key, signature, canonical) || verify(key, signature, legacy)
}

/// Verify one Ed25519 signature against one byte string.
///
/// Returns `false` for malformed signatures (wrong length) instead of
/// panicking — fail closed.
pub fn verify(key: &PublicKey, signature: &[u8], data: &[u8]) -> bool {
    let Ok(sig_bytes) = <[u8; SIGNATURE_LEN]>::try_from(signature) else {
        return false;
    };
    key.verify(data, &iroh::Signature::from_bytes(&sig_bytes))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    /// Fixed fields must produce fixed canonical bytes (postcard is
    /// deterministic).  This golden vector pins the framing: protocol tag
    /// first, then version, then the fields — all inside one postcard tuple.
    #[test]
    fn canonical_signed_bytes_golden_vector() {
        let protocol = "boru/test-object";
        let version: u16 = 1;
        let fields = (
            PublicKey::from_bytes(&[0u8; 32]).expect("valid key"),
            42u64,
            vec![1u8, 2, 3],
        );
        let bytes = canonical_signed_bytes(protocol, version, &fields).expect("canonical bytes");
        let expected = "\
            10 62 6f 72 75 2f 74 65 73 74 2d 6f 62 6a 65 63 74 \
            01 \
            00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 \
            2a \
            03 01 02 03";
        let expected_bytes: Vec<u8> = expected
            .split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).expect("hex byte"))
            .collect();
        assert_eq!(
            bytes, expected_bytes,
            "canonical framing bytes must be stable (BORU-AUDIT-27)"
        );
    }

    /// Mutating any signed field invalidates verification.
    #[test]
    fn canonical_field_mutation_invalidates_verification() {
        let sk = SecretKey::generate();
        let pk = sk.public();
        let base = (PublicKey::from_bytes(&[1u8; 32]).expect("valid key"), 7u64);
        let canonical = canonical_signed_bytes("boru/test-object", 1, &base).expect("canonical");
        let sig = sk.sign(&canonical).to_bytes().to_vec();

        // Same bytes verify.
        assert!(verify(&pk, &sig, &canonical));
        // Mutated fields produce different canonical bytes → verification fails.
        let other_pk = SecretKey::generate().public();
        let mutated = (other_pk, 7u64);
        let canonical_mutated =
            canonical_signed_bytes("boru/test-object", 1, &mutated).expect("canonical");
        assert!(!verify(&pk, &sig, &canonical_mutated));
        // Different protocol tag → fails.
        let other_protocol =
            canonical_signed_bytes("boru/other-object", 1, &base).expect("canonical");
        assert!(!verify(&pk, &sig, &other_protocol));
        // Different version → fails.
        let other_version =
            canonical_signed_bytes("boru/test-object", 2, &base).expect("canonical");
        assert!(!verify(&pk, &sig, &other_version));
    }

    /// Public keys round-trip through bytes with no string-normalization
    /// ambiguity: `as_bytes` → `from_bytes` is the identity.
    #[test]
    fn public_key_bytes_round_trip_is_stable() {
        let sk = SecretKey::generate();
        let pk = sk.public();
        let bytes = pk.as_bytes();
        let pk2 = PublicKey::from_bytes(bytes).expect("valid key");
        assert_eq!(pk, pk2);
        assert_eq!(pk.as_bytes(), pk2.as_bytes());
    }
}
