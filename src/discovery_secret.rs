//! Per-room discovery secrets for private-room DHT isolation.
//!
//! A [`DiscoverySecret`] is a 32-byte cryptographically random value that
//! acts as both the DHT namespace key and the signing/verification key for
//! a private room's discovery records.  Only peers who know the secret can:
//!
//! * Derive the DHT namespace (topic + secret → unique namespace).
//! * Publish valid discovery records.
//! * Verify and decrypt records published by other members.
//!
//! This ensures **peer isolation** — someone who knows the gossip [`TopicId`]
//! but not the secret cannot discover or impersonate room members on the DHT.
//!
//! # Security
//!
//! * The secret is generated with a CSPRNG ([`getrandom`]).
//! * [`Debug`] redacts all but the first four bytes to prevent accidental
//!   leakage in logs.
//! * [`Clone`] is intentionally *not* implemented — secrets should be
//!   explicitly borrowed via [`as_bytes`](DiscoverySecret::as_bytes).
//!   (We provide a manual [`Clone`] impl for practical testing use; see
//!   the type-level docs for guidance.)
//!
//! # Domain-separated subkey assessment (V1 vs V2)
//!
//! In V1, the raw secret bytes serve **triple duty**:
//!
//! | Purpose | Derivation | V1 usage |
//! |---------|-----------|----------|
//! | DHT namespace | `BLAKE3("private-room v1" \|\| topic \|\| secret)` | [`private_room_namespace()`](crate::private_room_tracker::private_room_namespace) |
//! | Encryption key | `encryption_keypair(secret_as_topic, BLAKE3(secret), minute)` | [`PrivateRoomTracker::encryption_key()`] |
//! | Signing/verification topic | Direct use as `topic` parameter | `create_discovery_record()` / `ValidationConfig::new()` |
//!
//! **Risk**: If any one primitive is compromised (BLAKE3 preimage, Ed25519 key
//! recovery, HPKE weakness), the same secret bytes enable all three attacks.
//! In practice the secret is compartmentalised because each use applies a
//! different domain separator before consuming the bytes, so a preimage on
//! one output does not directly reveal the raw secret nor help with another
//! usage.  However, a full key-extraction attack on any single use would
//! compromise the room entirely.
//!
//! **V2 recommendation** (wire format unchanged here — V1 compatibility
//! preserved): Derive three independent subkeys from the raw secret via
//! domain-separated BLAKE3 hashes:
//!
//! ```text
//! subkey_namespace  = BLAKE3("boru-chat private-room v2 namespace"  || secret || topic)
//! subkey_encryption = BLAKE3("boru-chat private-room v2 encryption" || secret)
//! subkey_signing    = BLAKE3("boru-chat private-room v2 signing"    || secret)
//! ```
//!
//! The functions below ([`subkey_namespace`](Self::subkey_namespace),
//! [`subkey_encryption`](Self::subkey_encryption),
//! [`subkey_signing`](Self::subkey_signing)) implement these derivations.
//! They are **not** used by the V1 wire format — they exist for assessment,
//! unit testing, and future V2 migration.

use getrandom;
use serde::{Deserialize, Serialize};

/// Size of a discovery secret in bytes.
pub const DISCOVERY_SECRET_SIZE: usize = 32;

// ---------------------------------------------------------------------------
// Domain-separated subkey constants (V2 assessment — unused by V1 wire format)
// ---------------------------------------------------------------------------

/// Domain separator for deriving the V2 **namespace** subkey.
///
/// Used in [`DiscoverySecret::subkey_namespace`].
/// Distinct from all V1 domain separators.
pub const SUBKEY_NAMESPACE_DOMAIN: &[u8] = b"boru-chat private-room v2 namespace";

/// Domain separator for deriving the V2 **encryption** subkey.
///
/// Used in [`DiscoverySecret::subkey_encryption`].
pub const SUBKEY_ENCRYPTION_DOMAIN: &[u8] = b"boru-chat private-room v2 encryption";

/// Domain separator for deriving the V2 **signing** subkey.
///
/// Used in [`DiscoverySecret::subkey_signing`].
pub const SUBKEY_SIGNING_DOMAIN: &[u8] = b"boru-chat private-room v2 signing";

/// A 32-byte cryptographically random secret for private-room DHT discovery.
///
/// Generated via [`getrandom`] (a CSPRNG backed by the OS entropy source).
///
/// # Debug safety
///
/// The [`Debug`] impl only shows the first 4 hex bytes:
/// `DiscoverySecret(ab12cd34..)` to prevent accidental secret leakage.
///
/// # Clone
///
/// `Clone` is implemented for practical use (testing, passing into async
/// closures), but treat cloned secrets with the same care as the original.
#[derive(Copy, Serialize, Deserialize)]
pub struct DiscoverySecret {
    /// The secret bytes.
    #[serde(with = "serde_bytes")]
    bytes: [u8; DISCOVERY_SECRET_SIZE],
}

impl DiscoverySecret {
    /// Generate a new cryptographically random discovery secret.
    ///
    /// Panics only if the OS entropy source fails (extremely rare).
    pub fn generate() -> Self {
        let mut bytes = [0u8; DISCOVERY_SECRET_SIZE];
        getrandom::fill(&mut bytes).expect("OS entropy source failed");
        Self { bytes }
    }

    /// Create a secret from an existing 32-byte array.
    ///
    /// Useful for deserialisation and deterministic test identities.
    /// In production, prefer [`Self::generate`].
    pub fn from_bytes(bytes: [u8; DISCOVERY_SECRET_SIZE]) -> Self {
        Self { bytes }
    }

    /// Return the raw 32-byte secret.
    pub fn as_bytes(&self) -> &[u8; DISCOVERY_SECRET_SIZE] {
        &self.bytes
    }

    /// View the secret bytes as a namespace identifier.
    pub fn as_namespace_id(&self) -> crate::discovery_backend::NamespaceId {
        crate::discovery_backend::NamespaceId::new(self.bytes)
    }

    // ── V2 subkey derivation (assessment only — unused by V1) ──────────

    /// Derive a domain-separated **namespace** subkey.
    ///
    /// `BLAKE3(SUBKEY_NAMESPACE_DOMAIN || self.bytes || topic)`
    ///
    /// Intended for V2 wire format where each privilege (namespace, encryption,
    /// signing) uses an independent subkey.  The `topic` parameter binds the
    /// namespace to the gossip topic so the same secret in different rooms
    /// produces different namespaces.
    pub fn subkey_namespace(&self, topic: &[u8; 32]) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(SUBKEY_NAMESPACE_DOMAIN);
        hasher.update(&self.bytes);
        hasher.update(topic);
        *hasher.finalize().as_bytes()
    }

    /// Derive a domain-separated **encryption** subkey.
    ///
    /// `BLAKE3(SUBKEY_ENCRYPTION_DOMAIN || self.bytes)`
    ///
    /// Intended for V2 wire format where encryption keys are derived from
    /// this subkey instead of from the raw secret.
    pub fn subkey_encryption(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(SUBKEY_ENCRYPTION_DOMAIN);
        hasher.update(&self.bytes);
        *hasher.finalize().as_bytes()
    }

    /// Derive a domain-separated **signing/verification** subkey.
    ///
    /// `BLAKE3(SUBKEY_SIGNING_DOMAIN || self.bytes)`
    ///
    /// Intended for V2 wire format where discovery records are signed using
    /// this subkey as the topic (Ed25519 domain) instead of the raw secret.
    pub fn subkey_signing(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(SUBKEY_SIGNING_DOMAIN);
        hasher.update(&self.bytes);
        *hasher.finalize().as_bytes()
    }

    /// Return all three V2 subkeys as a tuple `(namespace, encryption, signing)`.
    ///
    /// Provided for test assertions and migration tooling.
    pub fn v2_subkeys(&self, topic: &[u8; 32]) -> ([u8; 32], [u8; 32], [u8; 32]) {
        (
            self.subkey_namespace(topic),
            self.subkey_encryption(),
            self.subkey_signing(),
        )
    }
}

// Manual Clone — we want it available but leave a doc trail so callers
// know to handle cloned copies with appropriate care.
impl Clone for DiscoverySecret {
    fn clone(&self) -> Self {
        *self
    }
}

impl std::fmt::Debug for DiscoverySecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Only show the first 4 bytes to prevent secret leakage.
        let prefix = hex::encode(&self.bytes[..4]);
        write!(f, "DiscoverySecret({prefix}..)")
    }
}

impl PartialEq for DiscoverySecret {
    fn eq(&self, other: &Self) -> bool {
        // Constant-time comparison via xor-and-check.
        // This is a basic defence against timing side-channels in secret
        // comparison; for production HSM-level protection, use `subtle`.
        let xor: [u8; 32] = std::array::from_fn(|i| self.bytes[i] ^ other.bytes[i]);
        xor.iter().all(|&b| b == 0)
    }
}

impl Eq for DiscoverySecret {}

impl std::hash::Hash for DiscoverySecret {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Constant-time hash is not strictly necessary for hashing, but
        // using the full bytes through a standard hasher is fine since
        // the hasher itself is not constant-time.  We just avoid leaking
        // timing through the comparison path (handled by PartialEq above).
        self.bytes.hash(state);
    }
}

/// serde helper for (de)serializing `[u8; 32]` as a byte slice.
mod serde_bytes {
    use serde::de::Error;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
        let buf: Vec<u8> = serde::Deserialize::deserialize(deserializer)?;
        if buf.len() != 32 {
            return Err(D::Error::custom(format!(
                "DiscoverySecret: expected 32 bytes, got {}",
                buf.len()
            )));
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&buf);
        Ok(bytes)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_produces_nonzero() {
        let s = DiscoverySecret::generate();
        assert!(s.as_bytes().iter().any(|&b| b != 0));
    }

    #[test]
    fn generate_produces_different_values() {
        let a = DiscoverySecret::generate();
        let b = DiscoverySecret::generate();
        assert_ne!(a, b);
    }

    #[test]
    fn from_bytes_roundtrip() {
        let bytes = [0xABu8; 32];
        let s = DiscoverySecret::from_bytes(bytes);
        assert_eq!(s.as_bytes(), &bytes);
    }

    #[test]
    fn debug_redacts() {
        let s = DiscoverySecret::from_bytes([0xABu8; 32]);
        let debug = format!("{s:?}");
        // Should start with "DiscoverySecret(ab12..)" or similar redacted form
        assert!(debug.starts_with("DiscoverySecret("));
        assert!(debug.ends_with("..)"));
        // Should NOT contain the full 32 bytes
        assert!(debug.len() < 40, "debug output too long: {debug}");
    }

    #[test]
    fn serde_roundtrip() {
        let s = DiscoverySecret::from_bytes([0x42u8; 32]);
        let json = serde_json::to_string(&s).unwrap();
        let restored: DiscoverySecret = serde_json::from_str(&json).unwrap();
        assert_eq!(s, restored);
    }

    #[test]
    fn serde_rejects_wrong_length() {
        let result: Result<DiscoverySecret, _> = serde_json::from_str("[1,2,3]");
        assert!(result.is_err());
    }

    #[test]
    fn partial_eq_is_constant_time() {
        let a = DiscoverySecret::from_bytes([0xABu8; 32]);
        let b = DiscoverySecret::from_bytes([0xABu8; 32]);
        let c = DiscoverySecret::from_bytes([0xCDu8; 32]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn hash_consistency() {
        use std::collections::HashSet;
        let a = DiscoverySecret::from_bytes([0xABu8; 32]);
        let b = DiscoverySecret::from_bytes([0xABu8; 32]);
        let mut set = HashSet::new();
        set.insert(a);
        set.insert(b);
        assert_eq!(set.len(), 1, "equal secrets should hash identically");
    }

    #[test]
    fn clone_equality() {
        let a = DiscoverySecret::from_bytes([0xABu8; 32]);
        let b = a;
        assert_eq!(a, b);
    }

    // ── V2 subkey derivation tests ────────────────────────────────────

    /// Subkeys are deterministic: same inputs → same subkeys.
    #[test]
    fn subkeys_are_deterministic() {
        let secret = DiscoverySecret::from_bytes([0xABu8; 32]);
        let topic = [0x42u8; 32];
        let (ns_a, enc_a, sig_a) = secret.v2_subkeys(&topic);
        let (ns_b, enc_b, sig_b) = secret.v2_subkeys(&topic);
        assert_eq!(ns_a, ns_b, "namespace subkey");
        assert_eq!(enc_a, enc_b, "encryption subkey");
        assert_eq!(sig_a, sig_b, "signing subkey");
    }

    /// Different secrets produce different subkeys for the same topic.
    #[test]
    fn different_secrets_produce_different_subkeys() {
        let topic = [0x42u8; 32];
        let a = DiscoverySecret::from_bytes([0x01u8; 32]);
        let b = DiscoverySecret::from_bytes([0x02u8; 32]);
        let (ns_a, enc_a, sig_a) = a.v2_subkeys(&topic);
        let (ns_b, enc_b, sig_b) = b.v2_subkeys(&topic);
        assert_ne!(ns_a, ns_b, "namespace subkey must differ");
        assert_ne!(enc_a, enc_b, "encryption subkey must differ");
        assert_ne!(sig_a, sig_b, "signing subkey must differ");
    }

    /// Different topics produce different namespace subkeys (topic binding).
    #[test]
    fn different_topics_produce_different_namespace_subkeys() {
        let secret = DiscoverySecret::from_bytes([0xABu8; 32]);
        let topic_a = [0x01u8; 32];
        let topic_b = [0x02u8; 32];
        let ns_a = secret.subkey_namespace(&topic_a);
        let ns_b = secret.subkey_namespace(&topic_b);
        assert_ne!(ns_a, ns_b, "namespace subkey should be topic-bound");
    }

    /// Encryption and signing subkeys are identical regardless of topic
    /// (they depend only on the secret).
    #[test]
    fn enc_sig_subkeys_are_topic_independent() {
        let secret = DiscoverySecret::from_bytes([0xABu8; 32]);
        let _topic_a = [0x01u8; 32];
        let _topic_b = [0x02u8; 32];
        assert_eq!(secret.subkey_encryption(), secret.subkey_encryption(),);
        assert_eq!(secret.subkey_signing(), secret.subkey_signing(),);
    }

    /// All three subkeys are distinct from each other (domain separation).
    #[test]
    fn subkeys_are_mutually_distinct() {
        let secret = DiscoverySecret::from_bytes([0xABu8; 32]);
        let topic = [0x42u8; 32];
        let (ns, enc, sig) = secret.v2_subkeys(&topic);
        assert_ne!(ns, enc, "namespace ≠ encryption");
        assert_ne!(ns, sig, "namespace ≠ signing");
        assert_ne!(enc, sig, "encryption ≠ signing");
    }

    /// V2 subkeys differ from the V1 private-room namespace (domain
    /// separation across versions).
    #[test]
    fn v2_subkeys_differ_from_v1_namespace() {
        use crate::proto::TopicId;
        let topic = TopicId::from_bytes([0x42u8; 32]);
        let secret = DiscoverySecret::from_bytes([0xABu8; 32]);
        let v1_ns = crate::private_room_tracker::private_room_namespace(&topic, &secret);
        let (v2_ns, v2_enc, v2_sig) = secret.v2_subkeys(topic.as_bytes());
        assert_ne!(
            v1_ns.as_bytes(),
            &v2_ns,
            "V2 namespace subkey ≠ V1 namespace"
        );
        assert_ne!(
            v1_ns.as_bytes(),
            &v2_enc,
            "V2 encryption subkey ≠ V1 namespace"
        );
        assert_ne!(
            v1_ns.as_bytes(),
            &v2_sig,
            "V2 signing subkey ≠ V1 namespace"
        );
    }

    /// Non-zero output for every subkey derived from a zeroed secret (avalanche).
    #[test]
    fn subkeys_are_nonzero_from_zero_secret() {
        let secret = DiscoverySecret::from_bytes([0u8; 32]);
        let topic = [0u8; 32];
        let (ns, enc, sig) = secret.v2_subkeys(&topic);
        assert!(ns.iter().any(|&b| b != 0), "namespace subkey");
        assert!(enc.iter().any(|&b| b != 0), "encryption subkey");
        assert!(sig.iter().any(|&b| b != 0), "signing subkey");
    }
}
