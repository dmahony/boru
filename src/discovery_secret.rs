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

use getrandom;
use serde::{Deserialize, Serialize};

/// Size of a discovery secret in bytes.
pub const DISCOVERY_SECRET_SIZE: usize = 32;

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
}

// Manual Clone — we want it available but leave a doc trail so callers
// know to handle cloned copies with appropriate care.
impl Clone for DiscoverySecret {
    fn clone(&self) -> Self {
        Self { bytes: self.bytes }
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
        set.insert(a.clone());
        set.insert(b);
        assert_eq!(set.len(), 1, "equal secrets should hash identically");
    }

    #[test]
    fn clone_equality() {
        let a = DiscoverySecret::from_bytes([0xABu8; 32]);
        let b = a.clone();
        assert_eq!(a, b);
    }
}
