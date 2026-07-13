//! A 32-byte secret for private-room DHT discovery.
//!
//! [`DiscoverySecret`] is a random secret value used to control access to a
//! room's discovery namespace. Anyone who knows the secret can publish and
//! look up discovery records for that room, so it must never be leaked.
//!
//! The [`Display`] implementation intentionally shows only the first 4 hex
//! characters to prevent accidental secret leakage in logs.
//!
//! # Usage
//!
//! ```ignore
//! let secret = DiscoverySecret::generate();
//! let namespace = secret.as_namespace_id();
//! // publish / lookup records under `namespace`
//! ```

use crate::discovery_backend::NamespaceId;
use serde::{Deserialize, Serialize};

/// A 32-byte secret for private-room DHT discovery.
///
/// This value controls access to the room's discovery namespace on the DHT.
/// The [`Display`] and [`Debug`] impls show only the first 4 hex characters
/// to prevent accidental leakage in logs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiscoverySecret([u8; 32]);

impl DiscoverySecret {
    /// Generate a new random [`DiscoverySecret`] using the OS entropy source.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("OS entropy source failed");
        Self(bytes)
    }

    /// Create a [`DiscoverySecret`] from a raw 32-byte array.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Return a reference to the underlying 32-byte secret.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Derive the [`NamespaceId`] for this secret.
    ///
    /// The namespace identifier is a deterministic function of the secret
    /// bytes, enabling the holder to publish and look up discovery records
    /// under that namespace.
    pub fn as_namespace_id(&self) -> NamespaceId {
        NamespaceId::new(self.0)
    }
}

impl std::fmt::Display for DiscoverySecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let short = hex::encode(&self.0[..4]);
        write!(f, "DiscoverySecret({short}..)")
    }
}

impl std::fmt::Debug for DiscoverySecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Same partial display as Display — never leak the full secret.
        let short = hex::encode(&self.0[..4]);
        f.debug_tuple("DiscoverySecret")
            .field(&format_args!("{short}.."))
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Send + Sync are automatically satisfied because [u8; 32] is both.
// The struct is #[derive(...)]-only with no interior mutability.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// `generate()` produces non-zero output.
    #[test]
    fn generate_is_nonzero() {
        let secret = DiscoverySecret::generate();
        assert!(secret.0.iter().any(|&b| b != 0));
    }

    /// `generate()` is non-deterministic — two calls produce different values.
    #[test]
    fn generate_is_nondeterministic() {
        let a = DiscoverySecret::generate();
        let b = DiscoverySecret::generate();
        assert_ne!(a, b);
    }

    /// `from_bytes` round-trips through `as_bytes`.
    #[test]
    fn from_bytes_round_trip() {
        let input = [
            0xde, 0xad, 0xbe, 0xef, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a,
            0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
            0x19, 0x1a, 0x1b, 0x1c,
        ];
        let secret = DiscoverySecret::from_bytes(input);
        assert_eq!(secret.as_bytes(), &input);
    }

    /// `from_bytes` is deterministic — same input yields same secret.
    #[test]
    fn from_bytes_is_deterministic() {
        let bytes = [0x42u8; 32];
        let a = DiscoverySecret::from_bytes(bytes);
        let b = DiscoverySecret::from_bytes(bytes);
        assert_eq!(a, b);
    }

    /// `as_namespace_id()` returns the correct [`NamespaceId`].
    #[test]
    fn as_namespace_id_is_correct() {
        let bytes = [0x42u8; 32];
        let secret = DiscoverySecret::from_bytes(bytes);
        let ns = secret.as_namespace_id();
        assert_eq!(ns.as_bytes(), &bytes);
    }

    /// `Display` shows only the first 4 hex characters — no secret leak.
    #[test]
    fn display_shows_first_4_hex_chars_only() {
        let bytes = [
            0xab, 0xcd, 0xef, 0x01, 0xde, 0xad, 0xbe, 0xef, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];
        let secret = DiscoverySecret::from_bytes(bytes);
        let display = format!("{secret}");
        // The first 4 bytes (hex-encoded) are: ab cd ef 01 → "abcdef01"
        // So display should be "DiscoverySecret(abcdef01…)"
        assert!(
            display.starts_with("DiscoverySecret(abcdef01.."),
            "Display shows wrong content: {display}"
        );
        // Ensure the rest of the secret (bytes 4..32) is NOT in the display output.
        // The 5th byte is 0xde which would be "de" in hex.
        assert!(
            !display.contains("deadbeef"),
            "Display leaked secret bytes beyond the first 4: {display}"
        );
    }

    /// `Debug` shows only the first 4 hex characters — no secret leak.
    #[test]
    fn debug_shows_first_4_hex_chars_only() {
        let bytes = [
            0xbe, 0xef, 0xca, 0xfe, 0xde, 0xad, 0xbe, 0xef, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];
        let secret = DiscoverySecret::from_bytes(bytes);
        let debug = format!("{secret:?}");
        // First 4 bytes hex: beefcafe
        assert!(
            debug.contains("beefcafe"),
            "Debug should contain the first 4 bytes: {debug}"
        );
        // Should NOT contain the 5th byte (0xde → "de" in hex)
        assert!(
            !debug.contains("deadbeef"),
            "Debug leaked secret bytes beyond the first 4: {debug}"
        );
    }

    /// Serde round-trip via JSON preserves the secret.
    #[test]
    fn serde_round_trip_json() {
        let secret = DiscoverySecret::from_bytes([0x11u8; 32]);
        let json = serde_json::to_string(&secret).expect("serialize");
        let deserialized: DiscoverySecret = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(secret, deserialized);
    }

    /// The type is `Send + Sync` (compile-time check).
    #[test]
    fn is_send_sync() {
        fn assert_send<T: Send>(_: &T) {}
        fn assert_sync<T: Sync>(_: &T) {}

        let secret = DiscoverySecret::generate();
        assert_send(&secret);
        assert_sync(&secret);
    }
}
