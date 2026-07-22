//! Versioned, serializable peer invitation for boru-chat pairing.
//!
//! This module defines the [`PeerInvitation`] struct — a compact, versioned
//! payload used to encode a pairing invitation that can be shared out-of-band
//! (QR code, URL, clipboard text, etc.).
//!
//! # URI format
//!
//! ```text
//! boru-chat://pair/<base64url-nopad-encoded-postcard-payload>
//! ```
//!
//! The `boru-chat://pair/` prefix identifies the payload as a boru-chat
//! peer invitation. The remainder is a compact serialization:
//!
//! 1. [`postcard`] encode the [`PeerInvitation`] struct.
//! 2. URL-safe base64 encode (no padding).
//!
//! # Security properties
//!
//! The invitation payload is **authenticated** by the fact it must be
//! presented alongside the friend-request flow.  It does **not** contain
//! secret key material, filesystem paths, private DB info, or long-lived
//! bearer credentials.  An invitation leaks:
//!
//! - The inviting peer's public key (public by design).
//! - The inviting peer's chosen display name.
//! - Network hints (relay/gossip URLs, direct addresses).
//! - Optionally: avatar content hash, a single-use pairing token, an
//!   expiration time.
//!
//! # Validation rules
//!
//! | Check | Behaviour |
//! |-------|-----------|
//! | Unknown version | `validate` returns an error |
//! | Empty display name | `validate` returns an error |
//! | Display name > 64 chars | `validate` returns an error |
//! | > 10 relay URLs | `validate` returns an error |
//! | > 10 direct addresses | `validate` returns an error |
//! | Payload > 4096 bytes (before base64) | `decode` returns an error |
//! | Expired invitation | `validate` returns an error |
//! | Self-invitation (peer_id matches ours) | `validate` returns an error |
//! | Malformed base64 input | `decode` returns an error |
//! | Unparsable postcard bytes | `decode` returns an error |

use std::time::{SystemTime, UNIX_EPOCH};

use iroh::PublicKey;
use serde::{Deserialize, Serialize};

/// Current invitation protocol version.
const CURRENT_VERSION: u8 = 1;

/// Maximum allowed length for display names.
///
/// Matches the limit in [`crate::user_profile::MAX_DISPLAY_NAME_LENGTH`].
const MAX_DISPLAY_NAME_LENGTH: usize = 64;

/// Maximum number of relay/gossip bootstrap URLs.
const MAX_RELAY_URLS: usize = 10;

/// Maximum number of direct connection addresses.
const MAX_DIRECT_ADDRESSES: usize = 10;

/// Maximum serialized payload size in bytes (before base64 encoding).
///
/// 4 KiB is generous for the payload shape: a public key (~32 B), display
/// name (≤64 B), a handful of addresses (≤~500 B total), and optionals.
/// The limit prevents oversized payload attacks via QR scans or pasted text.
const MAX_PAYLOAD_SIZE: usize = 4096;

/// URI scheme prefix for boru-chat peer invitations.
///
/// The full URI is `boru-chat://pair/<base64url-nopad-encoded-payload>`.
/// Use [`PeerInvitation::to_uri`] and [`PeerInvitation::from_uri`] to
/// convert to/from the full URI string.
const URI_PREFIX: &str = "boru-chat://pair/";

/// A versioned, serializable peer invitation for boru-chat pairing.
///
/// Designed to be shared out-of-band (QR code, URL, clipboard).  Encoded
/// as a compact postcard payload wrapped in URL-safe base64 (no padding).
///
/// # Example
///
/// ```rust
/// use boru_chat::peer_invitation::PeerInvitation;
///
/// let inv = PeerInvitation::builder()
///     .display_name("Alice")
///     .build();
///
/// let uri = inv.to_uri();
/// let decoded = PeerInvitation::from_uri(&uri).unwrap();
/// assert_eq!(inv, decoded);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInvitation {
    /// Protocol version (currently `1`).
    pub version: u8,
    /// Public key of the inviting peer.
    pub peer_id: PublicKey,
    /// Display name of the inviting peer.
    pub display_name: String,
    /// Optional content hash of the peer's avatar.
    pub avatar_hash: Option<String>,
    /// Relay/gossip bootstrap URLs the invitee may use to connect.
    pub relay_urls: Vec<String>,
    /// Direct connection addresses (endpoint format).
    pub direct_addresses: Vec<String>,
    /// Optional token for pre-authenticated friend request pairing.
    pub friend_request_token: Option<String>,
    /// Optional UNIX timestamp (in seconds) after which this invitation expires.
    pub expires_at: Option<i64>,
}

impl PeerInvitation {
    /// Serialise this invitation into a URL-safe base64 string (no padding).
    ///
    /// This is the *payload* portion of the URI (everything after the prefix).
    /// Use [`Self::to_uri`] for the full URI string.
    pub fn encode(&self) -> Result<String, EncodeError> {
        let bytes = postcard::to_stdvec(self).map_err(|e| EncodeError::Serialize(e.to_string()))?;
        Ok(data_encoding::BASE64URL_NOPAD.encode(&bytes))
    }

    /// Deserialise an invitation from a URL-safe base64 string (no padding).
    ///
    /// Accepts the *payload* portion only (the part after the URI prefix).
    /// Use [`Self::from_uri`] to parse a full `boru-chat://pair/...` URI.
    ///
    /// Returns an error if:
    /// - The input is not valid base64 (URL-safe, no padding).
    /// - The decoded bytes exceed [`MAX_PAYLOAD_SIZE`].
    /// - The decoded bytes are not a valid [`PeerInvitation`] (wrong version,
    ///   corrupt postcard data, etc.).
    pub fn decode(input: &str) -> Result<Self, DecodeError> {
        let bytes = data_encoding::BASE64URL_NOPAD
            .decode(input.as_bytes())
            .map_err(|e| DecodeError::Base64(e.to_string()))?;

        if bytes.len() > MAX_PAYLOAD_SIZE {
            return Err(DecodeError::OversizedPayload(bytes.len()));
        }

        let invitation: Self =
            postcard::from_bytes(&bytes).map_err(|e| DecodeError::Deserialize(e.to_string()))?;

        Ok(invitation)
    }

    /// Validate this invitation against all structural and semantic rules.
    ///
    /// If `our_pubkey` is `Some`, the invitation is also checked for
    /// self-invitation (the peer_id matches our own public key).
    pub fn validate(&self, our_pubkey: Option<&PublicKey>) -> Result<(), ValidationError> {
        // --- Version check ---
        if self.version != CURRENT_VERSION {
            return Err(ValidationError::UnsupportedVersion(self.version));
        }

        // --- Display name checks ---
        let name_len = self.display_name.chars().count();
        if name_len == 0 {
            return Err(ValidationError::EmptyDisplayName);
        }
        if name_len > MAX_DISPLAY_NAME_LENGTH {
            return Err(ValidationError::DisplayNameTooLong(name_len));
        }

        // --- Address count bounds ---
        if self.relay_urls.len() > MAX_RELAY_URLS {
            return Err(ValidationError::TooManyRelayUrls(self.relay_urls.len()));
        }
        if self.direct_addresses.len() > MAX_DIRECT_ADDRESSES {
            return Err(ValidationError::TooManyDirectAddresses(
                self.direct_addresses.len(),
            ));
        }

        // --- Expiration check ---
        if let Some(expires_at) = self.expires_at {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            if expires_at < now {
                return Err(ValidationError::Expired);
            }
        }

        // --- Self-invitation check ---
        if let Some(ours) = our_pubkey {
            if &self.peer_id == ours {
                return Err(ValidationError::SelfInvitation);
            }
        }

        Ok(())
    }

    /// Encode this invitation as a full `boru-chat://pair/...` URI.
    pub fn to_uri(&self) -> Result<String, EncodeError> {
        let encoded = self.encode()?;
        Ok(format!("{URI_PREFIX}{encoded}"))
    }

    /// Parse a full `boru-chat://pair/...` URI into a [`PeerInvitation`].
    ///
    /// Returns `None` if the prefix doesn't match or the payload is invalid.
    pub fn from_uri(uri: &str) -> Option<Self> {
        let payload = uri.strip_prefix(URI_PREFIX)?;
        Self::decode(payload).ok()
    }

    /// Create a builder-initialised invitation with sensible defaults.
    ///
    /// The builder sets `version = CURRENT_VERSION` and leaves optional
    /// fields as `None` / empty.  You must set at least `display_name`
    /// and `peer_id`.
    pub fn builder() -> PeerInvitationBuilder {
        PeerInvitationBuilder::new()
    }
}

/// Builder for constructing a [`PeerInvitation`] with a fluent API.
#[derive(Debug, Default)]
pub struct PeerInvitationBuilder {
    peer_id: Option<PublicKey>,
    display_name: Option<String>,
    avatar_hash: Option<String>,
    relay_urls: Vec<String>,
    direct_addresses: Vec<String>,
    friend_request_token: Option<String>,
    expires_at: Option<i64>,
}

impl PeerInvitationBuilder {
    fn new() -> Self {
        Self::default()
    }

    /// Set the inviting peer's public key.
    pub fn peer_id(mut self, peer_id: PublicKey) -> Self {
        self.peer_id = Some(peer_id);
        self
    }

    /// Set the inviting peer's display name.
    pub fn display_name(mut self, name: impl Into<String>) -> Self {
        self.display_name = Some(name.into());
        self
    }

    /// Set the optional avatar content hash.
    pub fn avatar_hash(mut self, hash: impl Into<String>) -> Self {
        self.avatar_hash = Some(hash.into());
        self
    }

    /// Add a relay/gossip bootstrap URL.
    pub fn relay_url(mut self, url: impl Into<String>) -> Self {
        self.relay_urls.push(url.into());
        self
    }

    /// Add a direct connection address.
    pub fn direct_address(mut self, addr: impl Into<String>) -> Self {
        self.direct_addresses.push(addr.into());
        self
    }

    /// Set the optional friend-request pairing token.
    pub fn friend_request_token(mut self, token: impl Into<String>) -> Self {
        self.friend_request_token = Some(token.into());
        self
    }

    /// Set the expiration timestamp (UNIX seconds).
    pub fn expires_at(mut self, timestamp: i64) -> Self {
        self.expires_at = Some(timestamp);
        self
    }

    /// Build the [`PeerInvitation`].
    ///
    /// # Panics
    ///
    /// Panics if `peer_id` or `display_name` were not set.
    /// In non-test code, prefer constructing the struct directly
    /// or use the public fields.
    pub fn build(self) -> PeerInvitation {
        PeerInvitation {
            version: CURRENT_VERSION,
            peer_id: self.peer_id.expect("peer_id is required"),
            display_name: self.display_name.expect("display_name is required"),
            avatar_hash: self.avatar_hash,
            relay_urls: self.relay_urls,
            direct_addresses: self.direct_addresses,
            friend_request_token: self.friend_request_token,
            expires_at: self.expires_at,
        }
    }
}

// ── Error types ──────────────────────────────────────────────────────────

/// Errors that can occur when encoding a [`PeerInvitation`].
#[derive(Debug)]
pub enum EncodeError {
    /// Postcard serialisation failed.
    Serialize(String),
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialize(msg) => write!(f, "failed to serialize invitation: {msg}"),
        }
    }
}

impl std::error::Error for EncodeError {}

/// Errors that can occur when decoding a [`PeerInvitation`].
#[derive(Debug)]
pub enum DecodeError {
    /// Base64 decoding failed (invalid characters, wrong alphabet).
    Base64(String),
    /// Decoded payload exceeds the maximum allowed size.
    OversizedPayload(usize),
    /// Postcard deserialisation failed (corrupt data, version mismatch, etc.).
    Deserialize(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Base64(msg) => write!(f, "base64 decode error: {msg}"),
            Self::OversizedPayload(size) => {
                write!(
                    f,
                    "decoded payload too large: {size} bytes (max {MAX_PAYLOAD_SIZE})"
                )
            }
            Self::Deserialize(msg) => write!(f, "failed to deserialize invitation: {msg}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Errors from [`PeerInvitation::validate`].
#[derive(Debug)]
pub enum ValidationError {
    /// The invitation uses an unsupported protocol version.
    UnsupportedVersion(u8),
    /// The display name is empty.
    EmptyDisplayName,
    /// The display name exceeds the maximum allowed length.
    DisplayNameTooLong(usize),
    /// Too many relay URLs.
    TooManyRelayUrls(usize),
    /// Too many direct addresses.
    TooManyDirectAddresses(usize),
    /// The invitation has expired (current time is past `expires_at`).
    Expired,
    /// The invitation's `peer_id` matches our own public key.
    SelfInvitation,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion(v) => write!(f, "unsupported invitation version: {v}"),
            Self::EmptyDisplayName => write!(f, "display name must not be empty"),
            Self::DisplayNameTooLong(len) => {
                write!(
                    f,
                    "display name too long: {len} chars (max {MAX_DISPLAY_NAME_LENGTH})"
                )
            }
            Self::TooManyRelayUrls(count) => {
                write!(f, "too many relay URLs: {count} (max {MAX_RELAY_URLS})")
            }
            Self::TooManyDirectAddresses(count) => {
                write!(
                    f,
                    "too many direct addresses: {count} (max {MAX_DIRECT_ADDRESSES})"
                )
            }
            Self::Expired => write!(f, "invitation has expired"),
            Self::SelfInvitation => write!(f, "cannot pair with yourself"),
        }
    }
}

impl std::error::Error for ValidationError {}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a valid invitation for testing.
    fn valid_invitation() -> PeerInvitation {
        let sk = iroh::SecretKey::generate();
        PeerInvitation {
            version: CURRENT_VERSION,
            peer_id: sk.public(),
            display_name: "Alice".to_string(),
            avatar_hash: None,
            relay_urls: vec![],
            direct_addresses: vec![],
            friend_request_token: None,
            expires_at: None,
        }
    }

    /// Helper: create a different peer's identity for self-invitation checks.
    fn other_pubkey() -> PublicKey {
        let sk = iroh::SecretKey::generate();
        sk.public()
    }

    // ── Valid round trip ────────────────────────────────────────────────────

    #[test]
    fn valid_round_trip() {
        let inv = valid_invitation();
        let encoded = inv.encode().expect("encode should succeed");
        let decoded = PeerInvitation::decode(&encoded).expect("decode should succeed");
        assert_eq!(inv, decoded);
    }

    #[test]
    fn valid_round_trip_uri() {
        let inv = valid_invitation();
        let uri = inv.to_uri().expect("to_uri should succeed");
        let decoded = PeerInvitation::from_uri(&uri).expect("from_uri should succeed");
        assert_eq!(inv, decoded);
    }

    #[test]
    fn valid_round_trip_with_all_optionals() {
        let sk = iroh::SecretKey::generate();
        let inv = PeerInvitation {
            version: CURRENT_VERSION,
            peer_id: sk.public(),
            display_name: "Bob".to_string(),
            avatar_hash: Some("sha3:abc123def456".to_string()),
            relay_urls: vec!["relay.example.com:1234".to_string()],
            direct_addresses: vec!["192.168.1.42:9876".to_string()],
            friend_request_token: Some("tok_abc123".to_string()),
            expires_at: Some(i64::MAX),
        };
        let encoded = inv.encode().expect("encode should succeed");
        let decoded = PeerInvitation::decode(&encoded).expect("decode should succeed");
        assert_eq!(inv, decoded);

        // Validation should pass
        let pubkey = other_pubkey();
        assert!(inv.validate(Some(&pubkey)).is_ok());
    }

    #[test]
    fn valid_invitation_no_optionals() {
        let sk = iroh::SecretKey::generate();
        let inv = PeerInvitation {
            version: CURRENT_VERSION,
            peer_id: sk.public(),
            display_name: "Charlie".to_string(),
            avatar_hash: None,
            relay_urls: vec![],
            direct_addresses: vec![],
            friend_request_token: None,
            expires_at: None,
        };
        assert!(inv.validate(Some(&other_pubkey())).is_ok());
    }

    // ── Validation tests ────────────────────────────────────────────────────

    #[test]
    fn rejects_unsupported_version() {
        let mut inv = valid_invitation();
        inv.version = 99;
        let err = inv.validate(Some(&other_pubkey())).unwrap_err();
        assert!(
            matches!(err, ValidationError::UnsupportedVersion(99)),
            "expected UnsupportedVersion(99), got {err:?}"
        );
    }

    #[test]
    fn rejects_empty_display_name() {
        let mut inv = valid_invitation();
        inv.display_name = String::new();
        let err = inv.validate(Some(&other_pubkey())).unwrap_err();
        assert!(
            matches!(err, ValidationError::EmptyDisplayName),
            "expected EmptyDisplayName, got {err:?}"
        );
    }

    #[test]
    fn rejects_excessively_long_display_name() {
        let mut inv = valid_invitation();
        inv.display_name = "x".repeat(MAX_DISPLAY_NAME_LENGTH + 1);
        let err = inv.validate(Some(&other_pubkey())).unwrap_err();
        assert!(
            matches!(err, ValidationError::DisplayNameTooLong(len) if len == MAX_DISPLAY_NAME_LENGTH + 1),
            "expected DisplayNameTooLong({}), got {err:?}",
            MAX_DISPLAY_NAME_LENGTH + 1
        );
    }

    #[test]
    fn rejects_too_many_relay_urls() {
        let mut inv = valid_invitation();
        inv.relay_urls = (0..=MAX_RELAY_URLS)
            .map(|i| format!("relay{i}.example.com"))
            .collect();
        let err = inv.validate(Some(&other_pubkey())).unwrap_err();
        assert!(
            matches!(err, ValidationError::TooManyRelayUrls(count) if count == MAX_RELAY_URLS + 1),
            "expected TooManyRelayUrls({}), got {err:?}",
            MAX_RELAY_URLS + 1
        );
    }

    #[test]
    fn rejects_too_many_direct_addresses() {
        let mut inv = valid_invitation();
        inv.direct_addresses = (0..=MAX_DIRECT_ADDRESSES)
            .map(|i| format!("192.168.1.{i}:9876"))
            .collect();
        let err = inv.validate(Some(&other_pubkey())).unwrap_err();
        assert!(
            matches!(err, ValidationError::TooManyDirectAddresses(count) if count == MAX_DIRECT_ADDRESSES + 1),
            "expected TooManyDirectAddresses({}), got {err:?}",
            MAX_DIRECT_ADDRESSES + 1
        );
    }

    #[test]
    fn rejects_expired_invitation() {
        let mut inv = valid_invitation();
        // Set expiration to 1 second after UNIX epoch — definitely in the past.
        inv.expires_at = Some(1);
        let err = inv.validate(Some(&other_pubkey())).unwrap_err();
        assert!(
            matches!(err, ValidationError::Expired),
            "expected Expired, got {err:?}"
        );
    }

    #[test]
    fn accepts_future_expiration() {
        let mut inv = valid_invitation();
        // Set expiration far in the future.
        inv.expires_at = Some(i64::MAX);
        assert!(inv.validate(Some(&other_pubkey())).is_ok());
    }

    #[test]
    fn rejects_self_invitation() {
        let sk = iroh::SecretKey::generate();
        let our_pubkey = sk.public();
        let inv = PeerInvitation {
            version: CURRENT_VERSION,
            peer_id: our_pubkey,
            display_name: "Me".to_string(),
            avatar_hash: None,
            relay_urls: vec![],
            direct_addresses: vec![],
            friend_request_token: None,
            expires_at: None,
        };
        let err = inv.validate(Some(&our_pubkey)).unwrap_err();
        assert!(
            matches!(err, ValidationError::SelfInvitation),
            "expected SelfInvitation, got {err:?}"
        );
    }

    #[test]
    fn self_invitation_not_detected_without_our_pubkey() {
        let sk = iroh::SecretKey::generate();
        let our_pubkey = sk.public();
        let inv = PeerInvitation {
            version: CURRENT_VERSION,
            peer_id: our_pubkey,
            display_name: "Me".to_string(),
            avatar_hash: None,
            relay_urls: vec![],
            direct_addresses: vec![],
            friend_request_token: None,
            expires_at: None,
        };
        // When our_pubkey is None, self-invitation is not reported.
        assert!(inv.validate(None).is_ok());
    }

    // ── Decode error tests ──────────────────────────────────────────────────

    #[test]
    fn rejects_malformed_base64() {
        let err = PeerInvitation::decode("!!!not-valid-base64!!!").unwrap_err();
        assert!(
            matches!(err, DecodeError::Base64(_)),
            "expected Base64 error, got {err:?}"
        );
    }

    #[test]
    fn rejects_oversized_payload() {
        // Create a large payload by encoding a valid invitation, then
        // appending extra data before re-encoding.
        let inv = valid_invitation();
        let mut bytes = postcard::to_stdvec(&inv).expect("postcard should succeed");
        // Pad to exceed the limit (but keep it valid base64 later).
        bytes.resize(MAX_PAYLOAD_SIZE + 1, 0);
        let oversized = data_encoding::BASE64URL_NOPAD.encode(&bytes);
        let err = PeerInvitation::decode(&oversized).unwrap_err();
        assert!(
            matches!(err, DecodeError::OversizedPayload(size) if size == MAX_PAYLOAD_SIZE + 1),
            "expected OversizedPayload({}), got {err:?}",
            MAX_PAYLOAD_SIZE + 1
        );
    }

    #[test]
    fn rejects_corrupt_postcard() {
        // Encode something that is valid base64 but not a valid PeerInvitation.
        let corrupt = data_encoding::BASE64URL_NOPAD.encode(b"some random bytes");
        let err = PeerInvitation::decode(&corrupt).unwrap_err();
        assert!(
            matches!(err, DecodeError::Deserialize(_)),
            "expected Deserialize error, got {err:?}"
        );
    }

    #[test]
    fn rejects_non_uri_input() {
        let result = PeerInvitation::from_uri("not-a-valid-uri");
        assert!(result.is_none(), "expected None for non-URI input");
    }

    #[test]
    fn rejects_wrong_uri_prefix() {
        let result = PeerInvitation::from_uri("boru://invite/abc123");
        assert!(result.is_none(), "expected None for wrong prefix");
    }

    // ── Invitation with relay hints ─────────────────────────────────────────

    #[test]
    fn invitation_with_relay_hints() {
        let sk = iroh::SecretKey::generate();
        let inv = PeerInvitation {
            version: CURRENT_VERSION,
            peer_id: sk.public(),
            display_name: "RelayUser".to_string(),
            avatar_hash: None,
            relay_urls: vec![
                "relay1.boru-chat.example.com:443".to_string(),
                "relay2.boru-chat.example.com:443".to_string(),
            ],
            direct_addresses: vec![],
            friend_request_token: None,
            expires_at: None,
        };
        assert!(inv.validate(Some(&other_pubkey())).is_ok());

        let encoded = inv.encode().expect("encode should succeed");
        let decoded = PeerInvitation::decode(&encoded).expect("decode should succeed");
        assert_eq!(inv, decoded);
        assert_eq!(decoded.relay_urls.len(), 2);
    }

    // ── Builder tests ───────────────────────────────────────────────────────

    #[test]
    fn builder_constructs_valid_invitation() {
        let sk = iroh::SecretKey::generate();
        let inv = PeerInvitation::builder()
            .peer_id(sk.public())
            .display_name("BuilderTest")
            .build();
        assert_eq!(inv.version, CURRENT_VERSION);
        assert_eq!(inv.display_name, "BuilderTest");
        assert!(inv.validate(Some(&other_pubkey())).is_ok());
    }

    #[test]
    fn builder_with_all_fields() {
        let sk = iroh::SecretKey::generate();
        let inv = PeerInvitation::builder()
            .peer_id(sk.public())
            .display_name("Full Builder")
            .avatar_hash("sha3:xyz789")
            .relay_url("relay.example.com")
            .relay_url("relay2.example.com")
            .direct_address("10.0.0.1:9000")
            .friend_request_token("tok_secret")
            .expires_at(i64::MAX)
            .build();
        assert_eq!(inv.display_name, "Full Builder");
        assert_eq!(inv.avatar_hash, Some("sha3:xyz789".to_string()));
        assert_eq!(inv.relay_urls.len(), 2);
        assert_eq!(inv.direct_addresses.len(), 1);
        assert_eq!(inv.friend_request_token, Some("tok_secret".to_string()));
        assert_eq!(inv.expires_at, Some(i64::MAX));
    }
}
