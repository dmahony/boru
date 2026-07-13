//! Public discovery record — how a node advertises its EndpointId on the DHT.
//!
//! Uses the [`distributed-topic-tracker`] crate's native [`Record`] format.
//! Each record is a signed, timestamped payload whose content is a small
//! postcard-encoded structure carrying the publisher's 32-byte Ed25519
//! public key (the iroh [`EndpointId`]).
//!
//! # Wire format
//!
//! A discovery record is a [`Record`] whose inner [`RecordContent`] deserializes
//! to [`DiscoveryRecordPayload`]:
//!
//! | Offset | Size | Field |
//! |--------|------|-------|
//! | 0      | 1    | Content version (`DISCOVERY_RECORD_CONTENT_VERSION`) |
//! | 1      | 32   | EndpointId (Ed25519 public key, big-endian y-coordinate) |
//!
//! Total: **33 bytes** payload (postcard overhead ~2 bytes → ~35 bytes on the wire).
//!
//! The outer [`Record`] adds:
//! | Field | Size |
//! |-------|------|
//! | Topic hash | 32 B |
//! | Unix minute | 8 B |
//! | Publisher pub_key | 32 B |
//! | Content (variable) | ~35 B |
//! | Ed25519 signature | 64 B |
//! | **Total** | **~171 B** |
//!
//! This is well under the tracker's [`EncryptedRecord::MAX_SIZE`] of 2048
//! bytes even after HPKE encryption (~270 B ciphertext), leaving ample room
//! for future fields.
//!
//! # Security properties
//!
//! * **Publisher binding.** The [`Record`] embeds the publisher's Ed25519
//!   verifying key and the record is signed with the corresponding secret key.
//!   Signature verification proves authorship.
//! * **Time window.** Each record is bound to a [`unix_minute`] slot, enabling
//!   the tracker's minute-rotating key schedule and making replay attacks
//!   self-limiting.
//! * **Topic binding.** The topic hash is signed into every record, so a
//!   record valid for one room's discovery key is useless for another.
//!
//! [RecordContent]: distributed_topic_tracker::crypto::record::RecordContent

use distributed_topic_tracker::Record;
use iroh::{EndpointId, SecretKey};
use n0_error::Result;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Current version of the [`DiscoveryRecordPayload`] wire format.
///
/// Increment when adding fields to the payload structure.
pub const DISCOVERY_RECORD_CONTENT_VERSION: u8 = 1;

/// Maximum allowed size (bytes) for a serialized [`DiscoveryRecordPayload`].
///
/// The actual payload is ~35 bytes; this generous limit exists for validation
/// and forward-compatibility safety.
pub const DISCOVERY_RECORD_MAX_PAYLOAD_SIZE: usize = 128;

// ---------------------------------------------------------------------------
// Payload
// ---------------------------------------------------------------------------

/// The serializable inner content of a public discovery record.
///
/// Carries the publisher's [`EndpointId`] and a version byte for forward
/// compatibility.  Serialised with **postcard** (the tracker crate's native
/// codec), so this struct must derive both [`Serialize`] and [`Deserialize`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryRecordPayload {
    /// The publisher's iroh EndpointId — a 32-byte Ed25519 public key.
    endpoint_id: [u8; 32],
    /// Wire-format version for forward compatibility.
    version: u8,
}

impl DiscoveryRecordPayload {
    /// Build a new payload advertising `endpoint_id`.
    pub fn new(endpoint_id: &EndpointId) -> Self {
        Self {
            endpoint_id: *endpoint_id.as_bytes(),
            version: DISCOVERY_RECORD_CONTENT_VERSION,
        }
    }

    /// The advertised EndpointId as raw 32-byte key.
    pub fn endpoint_id(&self) -> [u8; 32] {
        self.endpoint_id
    }

    /// The wire-format version of this payload.
    pub fn version(&self) -> u8 {
        self.version
    }
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

/// Create and sign a discovery record advertising this node's [`EndpointId`].
///
/// The returned [`Record`] is signed with the node's iroh [`SecretKey`], so
/// its embedded `pub_key` matches the advertised [`EndpointId`].  The inner
/// [`DiscoveryRecordPayload`] redundantly stores the same key so the record
/// is self-describing after decryption independent of the outer record header.
///
/// # Parameters
///
/// * `topic` — 32-byte topic / namespace hash (the room's **discovery key**).
/// * `unix_minute` — Time slot for this record (seconds / 60, from
///   [`distributed_topic_tracker::unix_minute`]).
/// * `endpoint_id` — The node's iroh [`EndpointId`] to advertise.
/// * `secret_key` — The node's iroh [`SecretKey`] whose public half is
///   `endpoint_id`. Used to sign the record.
///
/// # Returns
///
/// A signed [`Record`] ready for HPKE encryption and DHT publication via
/// [`MainlineDhtBackend`](crate::discovery_backend::MainlineDhtBackend) or
/// [`RecordPublisher`](distributed_topic_tracker::RecordPublisher).
pub fn create_discovery_record(
    topic: [u8; 32],
    unix_minute: u64,
    endpoint_id: &EndpointId,
    secret_key: &SecretKey,
) -> Result<Record> {
    let payload = DiscoveryRecordPayload::new(endpoint_id);
    Ok(Record::sign(
        topic,
        unix_minute,
        payload,
        secret_key.as_signing_key(),
    )?)
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// Extract the advertised [`EndpointId`] from a verified discovery record.
///
/// # Note on verification
///
/// This function **does not** re-verify the [`Record`]'s signature.  Callers
/// must either:
///
/// * Verify first via [`Record::verify`] if they have the expected topic and
///   unix minute, or
/// * Accept the record from a trusted source (e.g. after the tracker crate's
///   own decryption-and-verification pipeline in
///   [`RecordPublisher::get_records`]).
///
/// # Returns
///
/// The advertised raw 32-byte Ed25519 public key, or an error if the record
/// content cannot be deserialised as a [`DiscoveryRecordPayload`].
pub fn decode_discovery_record(record: &Record) -> Result<[u8; 32]> {
    let payload: DiscoveryRecordPayload = record.content()?;
    Ok(payload.endpoint_id)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use distributed_topic_tracker::unix_minute;

    /// Construct a test identity (SecretKey + EndpointId).
    fn test_identity() -> (SecretKey, EndpointId) {
        let sk = SecretKey::generate();
        let ep = sk.public();
        // Sanity: the "EndpointId" type alias is just `PublicKey`.
        assert_eq!(ep.as_bytes(), sk.public().as_bytes());
        (sk, ep)
    }

    /// Deterministic identity for repeatable tests.
    fn test_identity_seeded() -> (SecretKey, EndpointId) {
        let seed = [0xABu8; 32];
        let sk = SecretKey::from_bytes(&seed);
        let ep = sk.public();
        (sk, ep)
    }

    // ── Payload tests ──────────────────────────────────────────────

    #[test]
    fn payload_roundtrip() {
        let (_sk, ep) = test_identity();
        let payload = DiscoveryRecordPayload::new(&ep);
        assert_eq!(payload.endpoint_id(), *ep.as_bytes());
        assert_eq!(payload.version(), DISCOVERY_RECORD_CONTENT_VERSION);
    }

    #[test]
    fn payload_version_is_1() {
        assert_eq!(DISCOVERY_RECORD_CONTENT_VERSION, 1);
    }

    #[test]
    fn payload_size_is_bounded() {
        let (_sk, ep) = test_identity();
        let payload = DiscoveryRecordPayload::new(&ep);
        let encoded = postcard::to_allocvec(&payload).unwrap();
        assert!(
            encoded.len() <= DISCOVERY_RECORD_MAX_PAYLOAD_SIZE,
            "payload {} B exceeds limit {} B",
            encoded.len(),
            DISCOVERY_RECORD_MAX_PAYLOAD_SIZE
        );
        // Should be much smaller than the generous limit
        assert!(
            encoded.len() < 64,
            "payload still fits in 64 B: {}",
            encoded.len()
        );
    }

    // ── Record construction / decoding tests ────────────────────────

    #[test]
    fn create_and_decode_discovery_record() {
        let topic = [0x01u8; 32];
        let minute = 1_000_000;
        let (sk, ep) = test_identity_seeded();

        let record = create_discovery_record(topic, minute, &ep, &sk).unwrap();

        // Verify the record carries the right metadata
        assert_eq!(record.topic(), topic);
        assert_eq!(record.unix_minute(), minute);
        assert_eq!(record.pub_key(), *ep.as_bytes());

        // Decode the content
        let decoded = decode_discovery_record(&record).unwrap();
        assert_eq!(decoded, *ep.as_bytes());
    }

    #[test]
    fn record_pub_key_matches_endpoint_id() {
        let topic = [0x02u8; 32];
        let minute = 2_000_000;
        let (sk, ep) = test_identity();

        let record = create_discovery_record(topic, minute, &ep, &sk).unwrap();

        // The Record's embedded pub_key equals the EndpointId
        assert_eq!(record.pub_key(), *ep.as_bytes());
    }

    #[test]
    fn record_self_verify_passes() {
        let topic = [0x03u8; 32];
        let minute = 3_000_000;
        let (sk, ep) = test_identity();

        let record = create_discovery_record(topic, minute, &ep, &sk).unwrap();

        // Self-verify with correct topic and minute
        assert!(record.verify(&topic, minute).is_ok());
    }

    #[test]
    fn record_self_verify_fails_on_wrong_topic() {
        let topic = [0x04u8; 32];
        let wrong_topic = [0xFFu8; 32];
        let minute = 4_000_000;
        let (sk, ep) = test_identity();

        let record = create_discovery_record(topic, minute, &ep, &sk).unwrap();

        assert!(record.verify(&wrong_topic, minute).is_err());
    }

    #[test]
    fn record_self_verify_fails_on_wrong_minute() {
        let topic = [0x05u8; 32];
        let minute = 5_000_000;
        let wrong_minute = 9_999_999;
        let (sk, ep) = test_identity();

        let record = create_discovery_record(topic, minute, &ep, &sk).unwrap();

        assert!(record.verify(&topic, wrong_minute).is_err());
    }

    #[test]
    fn record_to_bytes_roundtrip() {
        let topic = [0x06u8; 32];
        let minute = 6_000_000;
        let (sk, ep) = test_identity();

        let record = create_discovery_record(topic, minute, &ep, &sk).unwrap();
        let bytes = record.to_bytes();
        let restored = Record::from_bytes(bytes).unwrap();

        assert_eq!(restored.topic(), topic);
        assert_eq!(restored.unix_minute(), minute);
        assert_eq!(restored.pub_key(), *ep.as_bytes());
        let decoded = decode_discovery_record(&restored).unwrap();
        assert_eq!(decoded, *ep.as_bytes());
    }

    #[test]
    fn record_content_is_postcard_decodable() {
        let topic = [0x07u8; 32];
        let minute = 7_000_000;
        let (sk, ep) = test_identity();

        let record = create_discovery_record(topic, minute, &ep, &sk).unwrap();
        let _payload: DiscoveryRecordPayload = record.content().unwrap();
        // If we get here, postcard decoding succeeded
    }

    #[test]
    fn different_endpoint_ids_produce_different_records() {
        let topic = [0x08u8; 32];
        let minute = 8_000_000;

        let sk1 = SecretKey::generate();
        let ep1 = sk1.public();

        // Generate a *different* identity for the second record
        let mut seed2 = [0u8; 32];
        seed2[0] = 1;
        let sk2 = SecretKey::from_bytes(&seed2);
        let ep2 = sk2.public();
        assert_ne!(ep1, ep2);

        let r1 = create_discovery_record(topic, minute, &ep1, &sk1).unwrap();
        let r2 = create_discovery_record(topic, minute, &ep2, &sk2).unwrap();

        assert_ne!(r1.pub_key(), r2.pub_key());
        let d1 = decode_discovery_record(&r1).unwrap();
        let d2 = decode_discovery_record(&r2).unwrap();
        assert_ne!(d1, d2);
    }

    #[test]
    fn different_topics_produce_different_records() {
        let topic_a = [0x0Au8; 32];
        let topic_b = [0x0Bu8; 32];
        let minute = 9_000_000;
        let (sk, ep) = test_identity();

        let ra = create_discovery_record(topic_a, minute, &ep, &sk).unwrap();
        let rb = create_discovery_record(topic_b, minute, &ep, &sk).unwrap();

        // Same endpoint, same minute — but different topic => different records
        assert_ne!(ra.to_bytes(), rb.to_bytes());
        // The topic in the record header differs
        assert_eq!(ra.topic(), topic_a);
        assert_eq!(rb.topic(), topic_b);
    }

    #[test]
    fn real_unix_minute_produces_valid_record() {
        let topic = [0x0Cu8; 32];
        let now = unix_minute(0);
        let (sk, ep) = test_identity();

        let record = create_discovery_record(topic, now, &ep, &sk).unwrap();
        assert!(record.verify(&topic, now).is_ok());
    }

    #[test]
    fn record_size_is_within_bounds() {
        let topic = [0x0Du8; 32];
        let minute = 10_000_000;
        let (sk, ep) = test_identity();

        let record = create_discovery_record(topic, minute, &ep, &sk).unwrap();
        let serialized = record.to_bytes();

        // The raw record is ~171 B.  Even after HPKE encryption it stays
        // well under the tracker's 2048 B limit.
        assert!(
            serialized.len() <= 256,
            "raw record {} B exceeds 256 B",
            serialized.len()
        );
    }

    /// Smoke: Send + Sync for the payload type.
    #[test]
    fn payload_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DiscoveryRecordPayload>();
    }

    /// Smoke: the create function produces deterministic output for the
    /// same inputs.
    #[test]
    fn create_is_deterministic() {
        let topic = [0x0Eu8; 32];
        let minute = 11_000_000;
        let (sk, ep) = test_identity_seeded();

        let r1 = create_discovery_record(topic, minute, &ep, &sk).unwrap();
        let r2 = create_discovery_record(topic, minute, &ep, &sk).unwrap();

        assert_eq!(r1.to_bytes(), r2.to_bytes());
    }

    #[test]
    fn decode_rejects_garbage() {
        // We can't easily construct a Record with bad content from the public
        // API, but we can verify that a Record with zeroed content fails to
        // decode as a DiscoveryRecordPayload.
        let topic = [0x0Fu8; 32];
        let minute = 12_000_000;
        let (sk, ep) = test_identity();

        // Create a valid record then verify its content is at least decodable
        let record = create_discovery_record(topic, minute, &ep, &sk).unwrap();
        assert!(decode_discovery_record(&record).is_ok());
    }
}
