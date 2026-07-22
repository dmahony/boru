//! Tests for the stable versioned room invitation format (RoomInviteV2).
//!
//! Covers:
//! - Round-trip encode/decode
//! - Prefix, version, length, and corruption validation
//! - Debug secrecy (secret redacted)
//! - Legacy ticket compatibility (no overlap)
//! - Expected prefix "boru1:"

use boru_core::chat_core::{RoomInvitation, RoomInviteV2, Ticket};
use boru_core::discovery_secret::DiscoverySecret;
use boru_core::proto::TopicId;

/// A known 32-byte secret for deterministic tests.
fn test_secret() -> DiscoverySecret {
    let bytes: [u8; 32] = [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
        0x1f, 0x20,
    ];
    DiscoverySecret::from_bytes(bytes)
}

/// A known topic for deterministic tests.
fn test_topic() -> TopicId {
    TopicId::from_bytes([
        0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad, 0xae, 0xaf,
        0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xbb, 0xbc, 0xbd, 0xbe,
        0xbf, 0xc0,
    ])
}

/// Valid round-trip: encode then decode yields the same topic and secret.
#[test]
fn valid_round_trip() {
    let invite = RoomInviteV2::new(test_topic(), test_secret());
    let encoded = invite.encode();
    assert!(
        encoded.starts_with("boru1:"),
        "should start with boru1: prefix"
    );
    assert!(encoded.len() > 100, "should be ~105 characters");

    let decoded = RoomInviteV2::parse(&encoded).unwrap();
    assert_eq!(decoded.topic, test_topic());
    assert_eq!(decoded.discovery_secret, test_secret());
}

/// Parsing accepts trimmed whitespace.
#[test]
fn parse_accepts_whitespace() {
    let invite = RoomInviteV2::new(test_topic(), test_secret());
    let encoded = invite.encode();
    let padded = format!("  \n  {encoded}  \n  ");
    let decoded = RoomInviteV2::parse(&padded).unwrap();
    assert_eq!(decoded.topic, test_topic());
    assert_eq!(decoded.discovery_secret, test_secret());
}

/// Missing prefix is rejected with a clear error.
#[test]
fn rejects_missing_prefix() {
    let err = RoomInviteV2::parse("nope123abc").unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("boru1:") || msg.contains("prefix"),
        "error should mention prefix: {msg}"
    );
}

/// Wrong prefix is rejected.
#[test]
fn rejects_wrong_prefix() {
    let err = RoomInviteV2::parse("foo1:abc123").unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("boru1:") || msg.contains("prefix"),
        "error should mention expected prefix: {msg}"
    );
}

/// Payload too short is rejected.
#[test]
fn rejects_short_payload() {
    let err = RoomInviteV2::parse("boru1:short").unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("too short") || msg.contains("payload"),
        "error should mention short payload: {msg}"
    );
}

/// Wrong payload length (not exactly 65 bytes decoded) is rejected.
#[test]
fn rejects_wrong_length() {
    // Encode a 1-byte payload to produce a valid base32 string that decodes
    // to the wrong length.
    let one_byte = data_encoding::BASE32_NOPAD.encode(&[0x42u8]);
    let input = format!("boru1:{}", one_byte.to_ascii_lowercase());
    let err = RoomInviteV2::parse(&input).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("65") || msg.contains("expected") || msg.contains("payload"),
        "error should mention expected length: {msg}"
    );
}

/// Wrong version byte is rejected.
#[test]
fn rejects_wrong_version() {
    // Build a 65-byte payload with version byte 0xff.
    let mut payload = Vec::with_capacity(65);
    payload.push(0xff); // unsupported version
    payload.extend_from_slice(test_topic().as_ref());
    payload.extend_from_slice(test_secret().as_bytes());
    let encoded = data_encoding::BASE32_NOPAD.encode(&payload);
    let input = format!("boru1:{}", encoded.to_ascii_lowercase());
    let err = RoomInviteV2::parse(&input).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("version") || msg.contains("1") || msg.contains("255"),
        "error should mention version mismatch: {msg}"
    );
}

/// Corrupted base32 input is rejected.
#[test]
fn rejects_corrupted_base32() {
    let invite = RoomInviteV2::new(test_topic(), test_secret());
    let mut encoded = invite.encode().into_bytes();
    // Corrupt one character in the middle with an invalid base32 char.
    let mid = encoded.len() / 2;
    encoded[mid] = b'@';
    let corrupt = String::from_utf8(encoded).unwrap();
    let err = RoomInviteV2::parse(&corrupt).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("base32") || msg.contains("decode"),
        "error should mention base32/decode error: {msg}"
    );
}

/// Debug output redacts the discovery secret.
#[test]
fn debug_redacts_secret() {
    let invite = RoomInviteV2::new(test_topic(), test_secret());
    let debug_str = format!("{invite:?}");
    assert!(
        debug_str.contains("[redacted]"),
        "Debug output should redact secret: {debug_str}"
    );
    // Secret bytes should NOT appear.
    for chunk in test_secret().as_bytes().chunks(4) {
        let hex_chunk = hex::encode(chunk);
        if hex_chunk != "01020304" {
            // skip the first 4 which might appear as "DiscoverySecret(0102..)"
            // but that's still safe.
        }
        assert!(
            !debug_str.contains(&hex_chunk),
            "secret chunk {hex_chunk} should not appear raw in Debug: {debug_str}"
        );
    }
}

/// Debug output still shows the topic.
#[test]
fn debug_shows_topic() {
    let invite = RoomInviteV2::new(test_topic(), test_secret());
    let debug_str = format!("{invite:?}");
    assert!(
        debug_str.contains("topic"),
        "Debug output should show topic field: {debug_str}"
    );
}

/// Encoded string does not include endpoint, relay, or creator info.
#[test]
fn no_endpoint_info() {
    let invite = RoomInviteV2::new(test_topic(), test_secret());
    let encoded = invite.encode();
    // The prefix "boru1:" contains exactly one colon
    assert_eq!(
        encoded.matches(':').count(),
        1,
        "only the prefix separator should have a colon: {encoded}"
    );
}

/// RoomInviteV2 format does NOT overlap with legacy Ticket format.
/// Legacy Ticket is base32-nopad with no prefix. RoomInviteV2 has boru1: prefix.
#[test]
fn no_overlap_with_legacy_format() {
    // A legacy ticket (base32 without prefix) should NOT parse as RoomInviteV2.
    let legacy_payload = data_encoding::BASE32_NOPAD.encode(&[0u8; 65]);
    let input = legacy_payload.to_ascii_lowercase();
    let err = RoomInviteV2::parse(&input).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("boru1:") || msg.contains("prefix"),
        "legacy string should not parse as RoomInviteV2: {msg}"
    );
}

/// Two different secrets produce different encoded strings.
#[test]
fn different_secrets_different_output() {
    let secret_a = test_secret();
    let mut secret_b_bytes = *secret_a.as_bytes();
    secret_b_bytes[0] ^= 0xff;
    let secret_b = DiscoverySecret::from_bytes(secret_b_bytes);

    let invite_a = RoomInviteV2::new(test_topic(), secret_a);
    let invite_b = RoomInviteV2::new(test_topic(), secret_b);

    assert_ne!(invite_a.encode(), invite_b.encode());
}

/// Two different topics produce different encoded strings.
#[test]
fn different_topics_different_output() {
    let mut topic_b_bytes: [u8; 32] = *test_topic().as_ref();
    topic_b_bytes[0] ^= 0xff;
    let topic_b = TopicId::from_bytes(topic_b_bytes);

    let invite_a = RoomInviteV2::new(test_topic(), test_secret());
    let invite_b = RoomInviteV2::new(topic_b, test_secret());

    assert_ne!(invite_a.encode(), invite_b.encode());
}

/// Uppercase base32 is accepted (normalised internally).
#[test]
fn accepts_uppercase_base32() {
    let invite = RoomInviteV2::new(test_topic(), test_secret());
    let encoded = invite.encode();
    let payload = &encoded[6..];
    let all_upper = format!("boru1:{}", payload.to_ascii_uppercase());
    let decoded = RoomInviteV2::parse(&all_upper).unwrap();
    assert_eq!(decoded.topic, test_topic());
    assert_eq!(decoded.discovery_secret, test_secret());
}

#[test]
fn detects_stable_invites_before_legacy_decoding() {
    let input = RoomInviteV2::new(test_topic(), test_secret()).encode();
    let parsed = RoomInvitation::parse(&input).unwrap();
    assert!(matches!(parsed, RoomInvitation::Stable(_)));
}

#[test]
fn malformed_stable_invite_does_not_fall_back_to_legacy() {
    let err = RoomInvitation::parse("boru1:not-a-valid-ticket").unwrap_err();
    assert!(format!("{err}").contains("invitation"));
}

#[test]
fn legacy_ticket_has_no_implicit_secret() {
    let legacy = Ticket::new(test_topic(), Vec::new());
    let parsed = RoomInvitation::parse(&legacy.to_string()).unwrap();
    assert!(matches!(parsed, RoomInvitation::Legacy(_)));
    assert!(parsed.bootstrap_peers().is_empty());
    assert!(parsed.discovery_secret().is_none());
}

#[test]
fn ticket_debug_redacts_discovery_secret() {
    let ticket = Ticket::with_discovery(test_topic(), Vec::new(), test_secret());
    let debug = format!("{ticket:?}");
    assert!(debug.contains("[redacted]"));
    assert!(!debug.contains("01020304"));
}
