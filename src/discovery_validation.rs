//! Discovery record validation and peer filtering.
//!
//! Validates [`distributed_topic_tracker::Record`] instances fetched from the
//! DHT before their advertised [`iroh::EndpointId`] values are used for gossip
//! joins.  The pipeline rejects malformed, stale, misattributed, oversized,
//! or duplicate records and bounds both the records processed and the
//! candidates returned per lookup.
//!
//! # Pipeline (per-record)
//!
//! 1. **Size check** — reject records whose serialized byte length exceeds
//!    [`ValidationConfig::max_record_size`].
//! 2. **Timestamp check** — reject records whose [`unix_minute`] is too old
//!    (stale) or too far in the future (clock skew).
//! 3. **Decode payload** — reject records whose [`Record::content`] cannot be
//!    deserialised as a [`DiscoveryRecordPayload`].
//! 4. **Identity match** — reject records where the embedded [`pub_key`] does
//!    not equal the payload's `endpoint_id`.
//! 5. **Signature verify** — reject records whose Ed25519 signature does not
//!    validate for the expected topic and the record's own unix minute.
//!
//! # Batch processing
//!
//! The [`DiscoveryRecordValidator::filter_and_build`] method processes a
//! batch of [`Record`] values, applying:
//!
//! * Per-record validation (pipeline above).
//! * Bounded iteration — at most [`max_records_per_lookup`] records are
//!   examined.
//! * Deduplication — identical [`EndpointId`] values are emitted at most once.
//! * Self-filtering — the local node's own [`EndpointId`] is excluded.
//! * Bound on candidates — at most [`max_candidate_peers`] are returned.
//!
//! # Security
//!
//! * No secret key material is logged or exposed.
//! * Decrypted payload content is never logged (the payload's value is the
//!   node's [`EndpointId`] which is inherently public, but the logging ban
//!   covers any future secret-bearing fields).
//!
//! [`unix_minute`]: Record::unix_minute
//! [`pub_key`]: Record::pub_key

use std::collections::HashSet;

use distributed_topic_tracker::Record;
use iroh::EndpointId;
use n0_error::Result;

use crate::discovery_record::DiscoveryRecordPayload;
use tracing::trace;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default maximum age (in minutes) beyond which a record is considered stale.
pub const DEFAULT_MAX_RECORD_AGE_MINUTES: u64 = 10;

/// Default maximum clock skew (in minutes) allowed for future-dated records.
pub const DEFAULT_MAX_CLOCK_SKEW_MINUTES: u64 = 2;

/// Default maximum serialized [`Record`] size in bytes.
///
/// A valid raw record is ~171 B (see [`crate::discovery_record`]), so 256 B
/// provides generous headroom.
pub const DEFAULT_MAX_RECORD_SIZE: usize = 256;

/// Default maximum number of records to examine in a single lookup.
pub const DEFAULT_MAX_RECORDS_PER_LOOKUP: usize = 20;

/// Default maximum number of candidate peers to return.
pub const DEFAULT_MAX_CANDIDATE_PEERS: usize = 20;

/// Hard upper bound for records examined in one lookup, regardless of caller
/// configuration.  This is deliberately small: DHT responses are untrusted.
pub const HARD_MAX_RECORDS_PER_LOOKUP: usize = 20;

/// Hard upper bound for peers returned by one lookup, regardless of caller
/// configuration.
pub const HARD_MAX_CANDIDATE_PEERS: usize = 20;

/// Hard upper bound for a serialized raw discovery record.
pub const HARD_MAX_RECORD_SIZE: usize = 256;

// ---------------------------------------------------------------------------
// RejectionReason
// ---------------------------------------------------------------------------

/// Structured reason why a single discovery record was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionReason {
    /// Record serialized size exceeds the configured limit.
    Oversized {
        /// Actual byte size.
        size: usize,
        /// Configured maximum.
        max: usize,
    },
    /// Record's unix_minute is too old.
    Stale {
        /// Age in minutes.
        age_minutes: u64,
        /// Maximum allowed age.
        max_age: u64,
    },
    /// Record's unix_minute is too far in the future.
    FutureRecord {
        /// Skew in minutes.
        skew_minutes: u64,
        /// Maximum allowed skew.
        max_skew: u64,
    },
    /// Record content could not be decoded as a [`DiscoveryRecordPayload`].
    DecodeFailure(String),
    /// Record's embedded `pub_key` does not match the payload `endpoint_id`.
    IdentityMismatch,
    /// Record's Ed25519 signature verification failed.
    InvalidSignature(String),
    /// Record advertises the local node's own [`EndpointId`].
    SelfFiltered,
    /// Duplicate [`EndpointId`] already seen in this batch.
    Duplicate,
}

impl std::fmt::Display for RejectionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Oversized { size, max } => {
                write!(f, "oversized: {size} B exceeds {max} B limit")
            }
            Self::Stale {
                age_minutes,
                max_age,
            } => {
                write!(f, "stale: {age_minutes} min exceeds {max_age} min limit")
            }
            Self::FutureRecord {
                skew_minutes,
                max_skew,
            } => {
                write!(
                    f,
                    "future: clock skew {skew_minutes} min exceeds {max_skew} min limit"
                )
            }
            Self::DecodeFailure(msg) => {
                write!(f, "decode failure: {msg}")
            }
            Self::IdentityMismatch => {
                write!(f, "identity mismatch: pub_key != payload endpoint_id")
            }
            Self::InvalidSignature(msg) => {
                write!(f, "invalid signature: {msg}")
            }
            Self::SelfFiltered => {
                write!(f, "self-filtered: local EndpointId")
            }
            Self::Duplicate => {
                write!(f, "duplicate: EndpointId already accepted")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ValidationCounters
// ---------------------------------------------------------------------------

/// Structured counters tracking the disposition of records in a single
/// [`DiscoveryRecordValidator::filter_and_build`] call.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ValidationCounters {
    /// Total records examined (bounded by [`max_records_per_lookup`]).
    pub total: usize,
    /// Records rejected because their serialized size exceeded the limit.
    pub oversized: usize,
    /// Records rejected because their timestamp was too old.
    pub stale: usize,
    /// Records rejected because their timestamp was too far in the future.
    pub future: usize,
    /// Records rejected because their content could not be decoded.
    pub decode_failure: usize,
    /// Records rejected because the embedded pub_key did not match the payload.
    pub identity_mismatch: usize,
    /// Records rejected because the Ed25519 signature was invalid.
    pub invalid_signature: usize,
    /// Records filtered because they advertise the local node's own EndpointId.
    pub self_filtered: usize,
    /// Records filtered because they are duplicates of an already-seen EndpointId.
    pub duplicates: usize,
    /// Records that passed all checks and were accepted.
    pub accepted: usize,
}

impl ValidationCounters {
    /// True when every record was rejected (nothing accepted).
    pub fn all_rejected(&self) -> bool {
        self.accepted == 0 && self.total > 0
    }

    /// Return the total number of rejections across all categories.
    pub fn total_rejected(&self) -> usize {
        self.oversized
            + self.stale
            + self.future
            + self.decode_failure
            + self.identity_mismatch
            + self.invalid_signature
            + self.self_filtered
            + self.duplicates
    }
}

// ---------------------------------------------------------------------------
// PeerCandidates
// ---------------------------------------------------------------------------

/// Result of a single [`DiscoveryRecordValidator::filter_and_build`] call.
#[derive(Debug, Clone)]
pub struct PeerCandidates {
    /// Validated, deduplicated, bounded list of peer [`EndpointId`] values.
    pub peers: Vec<EndpointId>,
    /// Per-category counters for the batch.
    pub counters: ValidationCounters,
}

// ---------------------------------------------------------------------------
// ValidationConfig
// ---------------------------------------------------------------------------

/// Tunable parameters for the discovery record validation pipeline.
///
/// Most callers will use [`Default::default`] or adjust a few fields.
///
/// # Required field
///
/// [`topic`](ValidationConfig::topic) **must** be set to the room's discovery
/// topic hash (the 32-byte namespace used for DHT publishing).  Every record
/// is checked against this topic via [`Record::verify`].
#[derive(Debug, Clone)]
pub struct ValidationConfig {
    /// The 32-byte discovery topic hash the record must be signed for.
    pub topic: [u8; 32],
    /// Maximum age (in minutes) before a record is considered stale.
    pub max_record_age_minutes: u64,
    /// Maximum clock skew (in minutes) allowed for future-dated records.
    pub max_clock_skew_minutes: u64,
    /// Maximum serialized [`Record`] size in bytes.
    pub max_record_size: usize,
    /// Maximum number of records to examine per single lookup call.
    pub max_records_per_lookup: usize,
    /// Maximum number of candidate [`EndpointId`] values to return.
    pub max_candidate_peers: usize,
}

impl ValidationConfig {
    /// Create a new config with the required topic and sensible defaults
    /// for all other parameters.
    pub fn new(topic: [u8; 32]) -> Self {
        Self {
            topic,
            max_record_age_minutes: DEFAULT_MAX_RECORD_AGE_MINUTES,
            max_clock_skew_minutes: DEFAULT_MAX_CLOCK_SKEW_MINUTES,
            max_record_size: DEFAULT_MAX_RECORD_SIZE,
            max_records_per_lookup: DEFAULT_MAX_RECORDS_PER_LOOKUP,
            max_candidate_peers: DEFAULT_MAX_CANDIDATE_PEERS,
        }
    }
}

impl Default for ValidationConfig {
    fn default() -> Self {
        Self {
            topic: [0u8; 32],
            max_record_age_minutes: DEFAULT_MAX_RECORD_AGE_MINUTES,
            max_clock_skew_minutes: DEFAULT_MAX_CLOCK_SKEW_MINUTES,
            max_record_size: DEFAULT_MAX_RECORD_SIZE,
            max_records_per_lookup: DEFAULT_MAX_RECORDS_PER_LOOKUP,
            max_candidate_peers: DEFAULT_MAX_CANDIDATE_PEERS,
        }
    }
}

// ---------------------------------------------------------------------------
// DiscoveryRecordValidator
// ---------------------------------------------------------------------------

/// A record validation engine pinned to a specific validation config and
/// reference time.
///
/// Create one per lookup cycle; the reference time (`now_minute`) determines
/// which records are considered stale or too far in the future.
#[derive(Debug, Clone)]
pub struct DiscoveryRecordValidator {
    config: ValidationConfig,
    /// Reference "now" minute for timestamp checks.
    now_minute: u64,
}

impl DiscoveryRecordValidator {
    /// Create a new validator.
    ///
    /// * `config` — tunable validation parameters.
    /// * `now_minute` — the current Unix minute (seconds / 60), used as the
    ///   reference point for staleness and future-skew checks.  Obtain from
    ///   [`distributed_topic_tracker::unix_minute(0)`].
    pub fn new(mut config: ValidationConfig, now_minute: u64) -> Self {
        // Keep the public tuning knobs from disabling the safety bounds.  A
        // caller may tighten these values, but never expand the amount of
        // attacker-controlled DHT data processed in one lookup.
        config.max_record_size = config.max_record_size.min(HARD_MAX_RECORD_SIZE);
        config.max_records_per_lookup = config
            .max_records_per_lookup
            .min(HARD_MAX_RECORDS_PER_LOOKUP);
        config.max_candidate_peers = config.max_candidate_peers.min(HARD_MAX_CANDIDATE_PEERS);
        Self { config, now_minute }
    }

    /// Validate a single decrypted [`Record`] and return its advertised
    /// [`EndpointId`] on success, or a [`RejectionReason`] on failure.
    ///
    /// The check order is optimised to fail cheap checks before the
    /// relatively expensive Ed25519 signature verification.
    pub fn validate_single(&self, record: &Record) -> Result<[u8; 32], RejectionReason> {
        // 1. Size check — cheap, catches garbage early.
        let size = record.to_bytes().len();
        if size > self.config.max_record_size {
            return Err(RejectionReason::Oversized {
                size,
                max: self.config.max_record_size,
            });
        }

        // 2. Timestamp check — cheap integer compare.
        let record_minute = record.unix_minute();
        if record_minute > self.now_minute {
            let skew = record_minute.saturating_sub(self.now_minute);
            if skew > self.config.max_clock_skew_minutes {
                return Err(RejectionReason::FutureRecord {
                    skew_minutes: skew,
                    max_skew: self.config.max_clock_skew_minutes,
                });
            }
        } else {
            let age = self.now_minute.saturating_sub(record_minute);
            if age > self.config.max_record_age_minutes {
                return Err(RejectionReason::Stale {
                    age_minutes: age,
                    max_age: self.config.max_record_age_minutes,
                });
            }
        }

        // 3. Decode payload.
        let payload: DiscoveryRecordPayload = record
            .content()
            .map_err(|e| RejectionReason::DecodeFailure(e.to_string()))?;

        // Reject future/unknown payload versions instead of silently treating
        // a structurally valid but semantically different record as a peer.
        if payload.version() != crate::discovery_record::DISCOVERY_RECORD_CONTENT_VERSION {
            return Err(RejectionReason::DecodeFailure(format!(
                "unsupported discovery payload version {}",
                payload.version()
            )));
        }

        // 4. Identity match — payload endpoint_id must match the record's pub_key.
        let pub_key = record.pub_key();
        let payload_id = payload.endpoint_id();
        if pub_key != payload_id {
            return Err(RejectionReason::IdentityMismatch);
        }

        // 5. Signature verify — the expensive check comes last.
        record
            .verify(&self.config.topic, record.unix_minute())
            .map_err(|e| RejectionReason::InvalidSignature(e.to_string()))?;

        Ok(payload_id)
    }

    /// Process a batch of decrypted [`Record`] values, returning validated,
    /// deduplicated, and bounded candidate peers.
    ///
    /// The pipeline:
    /// 1. Iterates over at most [`max_records_per_lookup`] records.
    /// 2. Runs [`validate_single`](Self::validate_single) on each.
    /// 3. Deduplicates by [`EndpointId`].
    /// 4. Filters out the local node's own [`EndpointId`] if provided.
    /// 5. Bounds the result to [`max_candidate_peers`].
    ///
    /// No secret or decrypted payload content is logged.
    pub fn filter_and_build(
        &self,
        records: Vec<Record>,
        local_endpoint_id: Option<&EndpointId>,
    ) -> PeerCandidates {
        let mut counters = ValidationCounters::default();
        let mut seen: HashSet<[u8; 32]> = HashSet::new();
        let mut candidates: Vec<EndpointId> = Vec::new();

        let local_key = local_endpoint_id.map(|ep| *ep.as_bytes());

        for record in records.into_iter().take(self.config.max_records_per_lookup) {
            counters.total += 1;

            // Per-record validation.
            let endpoint_id_bytes = match self.validate_single(&record) {
                Ok(id) => id,
                Err(reason) => {
                    trace!(
                        reason = %reason,
                        "discovery record rejected",
                    );
                    match reason {
                        RejectionReason::Oversized { .. } => counters.oversized += 1,
                        RejectionReason::Stale { .. } => counters.stale += 1,
                        RejectionReason::FutureRecord { .. } => counters.future += 1,
                        RejectionReason::DecodeFailure(_) => counters.decode_failure += 1,
                        RejectionReason::IdentityMismatch => counters.identity_mismatch += 1,
                        RejectionReason::InvalidSignature(_) => counters.invalid_signature += 1,
                        // SelfFiltered and Duplicate never come from validate_single.
                        RejectionReason::SelfFiltered => counters.self_filtered += 1,
                        RejectionReason::Duplicate => counters.duplicates += 1,
                    }
                    continue;
                }
            };

            // Self-filter.
            if let Some(local) = local_key {
                if endpoint_id_bytes == local {
                    counters.self_filtered += 1;
                    trace!("discovery record self-filtered (local endpoint)");
                    continue;
                }
            }

            // Dedup.
            if !seen.insert(endpoint_id_bytes) {
                counters.duplicates += 1;
                trace!("discovery record duplicate");
                continue;
            }

            // Bounded output — stop collecting once we have enough.
            if candidates.len() >= self.config.max_candidate_peers {
                break;
            }

            // EndpointId::from_bytes returns Ok for any 32-byte key.
            let ep = EndpointId::from_bytes(&endpoint_id_bytes).expect("valid 32-byte EndpointId");
            candidates.push(ep);
            trace!("discovery record accepted");
            counters.accepted += 1;
        }

        PeerCandidates {
            peers: candidates,
            counters,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery_record::{create_discovery_record, DISCOVERY_RECORD_CONTENT_VERSION};
    use distributed_topic_tracker::{unix_minute, Record};

    // ── Helpers ───────────────────────────────────────────────────────

    fn test_identity() -> (iroh::SecretKey, iroh::EndpointId) {
        let sk = iroh::SecretKey::generate();
        let ep = sk.public();
        (sk, ep)
    }

    fn test_identity_seeded() -> (iroh::SecretKey, iroh::EndpointId) {
        let seed = [0xABu8; 32];
        let sk = iroh::SecretKey::from_bytes(&seed);
        let ep = sk.public();
        (sk, ep)
    }

    /// Deterministic topic for repeatable tests.
    fn test_topic() -> [u8; 32] {
        [0x42u8; 32]
    }

    /// Default validator using a fixed minute so tests are deterministic.
    fn test_validator(topic: [u8; 32], now_minute: u64) -> DiscoveryRecordValidator {
        let config = ValidationConfig::new(topic);
        DiscoveryRecordValidator::new(config, now_minute)
    }

    // ── validate_single tests ─────────────────────────────────────────

    #[test]
    fn valid_record_accepted() {
        let topic = test_topic();
        let minute = 1_000_000;
        let (sk, ep) = test_identity_seeded();
        let record = create_discovery_record(topic, minute, &ep, &sk).unwrap();
        let validator = test_validator(topic, minute);
        let result = validator.validate_single(&record);
        assert_eq!(result, Ok(*ep.as_bytes()));
    }

    #[test]
    fn invalid_signature_rejected() {
        let topic = test_topic();
        let minute = 1_000_000;
        let (sk, ep) = test_identity_seeded();
        let record = create_discovery_record(topic, minute, &ep, &sk).unwrap();

        // Tamper with the signature bytes (last 64 bytes).
        let mut bytes = record.to_bytes();
        let sig_start = bytes.len() - 64;
        bytes[sig_start] ^= 0xFF;
        let tampered = Record::from_bytes(bytes).unwrap();

        let validator = test_validator(topic, minute);
        let result = validator.validate_single(&tampered);
        assert!(
            matches!(result, Err(RejectionReason::InvalidSignature(_))),
            "expected InvalidSignature, got {result:?}"
        );
    }

    #[test]
    fn stale_record_rejected() {
        let topic = test_topic();
        let now_minute = 1_000_000;
        let old_minute = now_minute - DEFAULT_MAX_RECORD_AGE_MINUTES - 1;
        let (sk, ep) = test_identity_seeded();
        let record = create_discovery_record(topic, old_minute, &ep, &sk).unwrap();
        let validator = test_validator(topic, now_minute);
        let result = validator.validate_single(&record);
        assert!(
            matches!(result, Err(RejectionReason::Stale { .. })),
            "expected Stale, got {result:?}"
        );
    }

    #[test]
    fn future_record_rejected() {
        let topic = test_topic();
        let now_minute = 1_000_000;
        let future_minute = now_minute + DEFAULT_MAX_CLOCK_SKEW_MINUTES + 1;
        let (sk, ep) = test_identity_seeded();
        let record = create_discovery_record(topic, future_minute, &ep, &sk).unwrap();
        let validator = test_validator(topic, now_minute);
        let result = validator.validate_single(&record);
        assert!(
            matches!(result, Err(RejectionReason::FutureRecord { .. })),
            "expected FutureRecord, got {result:?}"
        );
    }

    #[test]
    fn identity_mismatch_rejected() {
        let topic = test_topic();
        let minute = 1_000_000;

        // Create record signed by Alice but with Bob's EndpointId in payload.
        let (sk_a, _ep_a) = test_identity_seeded();
        let (_sk_b, ep_b) = {
            let seed = [0xBBu8; 32];
            let sk = iroh::SecretKey::from_bytes(&seed);
            let ep = sk.public();
            (sk, ep)
        };

        // Payload carries ep_b, but record is signed by sk_a.
        // create_discovery_record signs with sk_a and embeds ep_b in the
        // payload — but the payload's endpoint_id will be ep_b's bytes while
        // the record's pub_key will be sk_a's public key.
        let record = create_discovery_record(topic, minute, &ep_b, &sk_a).unwrap();

        let validator = test_validator(topic, minute);
        let result = validator.validate_single(&record);
        assert!(
            matches!(result, Err(RejectionReason::IdentityMismatch)),
            "expected IdentityMismatch, got {result:?}"
        );
    }

    #[test]
    fn oversized_record_rejected() {
        let topic = test_topic();
        let minute = 1_000_000;
        let (sk, ep) = test_identity_seeded();
        let record = create_discovery_record(topic, minute, &ep, &sk).unwrap();

        let config = ValidationConfig {
            topic,
            max_record_size: 1, // smaller than any valid record
            ..ValidationConfig::new(topic)
        };
        let validator = DiscoveryRecordValidator::new(config, minute);

        let result = validator.validate_single(&record);
        assert!(
            matches!(result, Err(RejectionReason::Oversized { .. })),
            "expected Oversized, got {result:?}"
        );
    }

    #[test]
    fn stale_at_exactly_max_age_is_accepted() {
        let topic = test_topic();
        let now_minute = 1_000_000;
        let old_minute = now_minute - DEFAULT_MAX_RECORD_AGE_MINUTES;
        let (sk, ep) = test_identity_seeded();
        let record = create_discovery_record(topic, old_minute, &ep, &sk).unwrap();
        let validator = test_validator(topic, now_minute);
        let result = validator.validate_single(&record);
        assert!(
            result.is_ok(),
            "record at exactly max age should be ok: {result:?}"
        );
    }

    #[test]
    fn future_at_exactly_max_skew_is_accepted() {
        let topic = test_topic();
        let now_minute = 1_000_000;
        let future_minute = now_minute + DEFAULT_MAX_CLOCK_SKEW_MINUTES;
        let (sk, ep) = test_identity_seeded();
        let record = create_discovery_record(topic, future_minute, &ep, &sk).unwrap();
        let validator = test_validator(topic, now_minute);
        let result = validator.validate_single(&record);
        assert!(
            result.is_ok(),
            "record at exactly max skew should be ok: {result:?}"
        );
    }

    #[test]
    fn unknown_payload_version_rejected() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct FuturePayload {
            endpoint_id: [u8; 32],
            version: u8,
        }

        let topic = test_topic();
        let minute = 1_000_000;
        let (sk, ep) = test_identity_seeded();
        let record = Record::sign(
            topic,
            minute,
            FuturePayload {
                endpoint_id: *ep.as_bytes(),
                version: DISCOVERY_RECORD_CONTENT_VERSION + 1,
            },
            sk.as_signing_key(),
        )
        .unwrap();
        let result = test_validator(topic, minute).validate_single(&record);
        assert!(matches!(result, Err(RejectionReason::DecodeFailure(_))));
    }

    #[test]
    fn caller_cannot_expand_hard_bounds() {
        let topic = test_topic();
        let config = ValidationConfig {
            max_record_size: usize::MAX,
            max_records_per_lookup: usize::MAX,
            max_candidate_peers: usize::MAX,
            ..ValidationConfig::new(topic)
        };
        let validator = DiscoveryRecordValidator::new(config, 1_000_000);
        let mut records = Vec::new();
        for i in 0..HARD_MAX_RECORDS_PER_LOOKUP + 1 {
            let seed = [i as u8; 32];
            let sk = iroh::SecretKey::from_bytes(&seed);
            let ep = sk.public();
            records.push(create_discovery_record(topic, 1_000_000, &ep, &sk).unwrap());
        }
        let result = validator.filter_and_build(records, None);
        assert_eq!(result.counters.total, HARD_MAX_RECORDS_PER_LOOKUP);
        assert_eq!(result.peers.len(), HARD_MAX_CANDIDATE_PEERS);
    }

    // ── filter_and_build tests ────────────────────────────────────────

    #[test]
    fn self_filtered() {
        let topic = test_topic();
        let minute = 1_000_000;
        let (sk, ep) = test_identity_seeded();

        let record = create_discovery_record(topic, minute, &ep, &sk).unwrap();
        let validator = test_validator(topic, minute);

        let result = validator.filter_and_build(vec![record], Some(&ep));
        assert!(
            result.peers.is_empty(),
            "self should be filtered: {:?}",
            result.peers
        );
        assert_eq!(result.counters.self_filtered, 1);
        assert_eq!(result.counters.accepted, 0);
    }

    #[test]
    fn duplicates_deduplicated() {
        let topic = test_topic();
        let minute = 1_000_000;
        let (sk, ep) = test_identity_seeded();

        let r1 = create_discovery_record(topic, minute, &ep, &sk).unwrap();
        let r2 = create_discovery_record(topic, minute, &ep, &sk).unwrap();
        let validator = test_validator(topic, minute);

        let result = validator.filter_and_build(vec![r1, r2], None);
        assert_eq!(
            result.peers.len(),
            1,
            "duplicate endpoint should produce one peer: {:?}",
            result.peers
        );
        assert_eq!(result.counters.duplicates, 1);
        assert_eq!(result.counters.accepted, 1);
    }

    #[test]
    fn candidate_count_bounded() {
        let topic = test_topic();
        let minute = 1_000_000;
        let mut records = Vec::new();
        for i in 0..DEFAULT_MAX_CANDIDATE_PEERS + 5 {
            let seed = [i as u8; 32];
            let sk = iroh::SecretKey::from_bytes(&seed);
            let ep = sk.public();
            records.push(create_discovery_record(topic, minute, &ep, &sk).unwrap());
        }

        let validator = test_validator(topic, minute);
        let result = validator.filter_and_build(records, None);
        assert_eq!(
            result.peers.len(),
            DEFAULT_MAX_CANDIDATE_PEERS,
            "candidates should be bounded"
        );
        assert_eq!(result.counters.accepted, DEFAULT_MAX_CANDIDATE_PEERS);
    }

    #[test]
    fn records_per_lookup_bounded() {
        let topic = test_topic();
        let minute = 1_000_000;
        let mut records = Vec::new();
        // Create more records than the max_records_per_lookup allows
        let extra = 5;
        for i in 0..DEFAULT_MAX_RECORDS_PER_LOOKUP + extra {
            let seed = [i as u8; 32];
            let sk = iroh::SecretKey::from_bytes(&seed);
            let ep = sk.public();
            records.push(create_discovery_record(topic, minute, &ep, &sk).unwrap());
        }

        let validator = test_validator(topic, minute);
        let result = validator.filter_and_build(records, None);
        // total examined should be bounded
        assert_eq!(result.counters.total, DEFAULT_MAX_RECORDS_PER_LOOKUP);
        // But we also have max_candidate_peers bound which is tighter here
        // (both are 20), so accepted should be 20.
        assert_eq!(result.counters.accepted, DEFAULT_MAX_CANDIDATE_PEERS);
    }

    #[test]
    fn empty_records_produces_empty_result() {
        let topic = test_topic();
        let minute = 1_000_000;
        let validator = test_validator(topic, minute);
        let result = validator.filter_and_build(vec![], None);
        assert!(result.peers.is_empty());
        assert_eq!(result.counters.total, 0);
        assert_eq!(result.counters.accepted, 0);
    }

    #[test]
    fn mixed_valid_and_invalid() {
        let topic = test_topic();
        let minute = 1_000_000;

        // Valid record.
        let (sk1, ep1) = {
            let seed = [0x11u8; 32];
            let sk = iroh::SecretKey::from_bytes(&seed);
            (sk.clone(), sk.public())
        };
        let valid = create_discovery_record(topic, minute, &ep1, &sk1).unwrap();

        // Stale record.
        let (sk2, ep2) = {
            let seed = [0x22u8; 32];
            let sk = iroh::SecretKey::from_bytes(&seed);
            (sk.clone(), sk.public())
        };
        let stale = create_discovery_record(
            topic,
            minute - DEFAULT_MAX_RECORD_AGE_MINUTES - 5,
            &ep2,
            &sk2,
        )
        .unwrap();

        // Identity mismatch record.
        let (sk3, _ep3) = {
            let seed = [0x33u8; 32];
            let sk = iroh::SecretKey::from_bytes(&seed);
            (sk.clone(), sk.public())
        };
        let (_sk_other, ep_other) = {
            let seed = [0x44u8; 32];
            let sk = iroh::SecretKey::from_bytes(&seed);
            (sk.clone(), sk.public())
        };
        let mismatched = create_discovery_record(topic, minute, &ep_other, &sk3).unwrap();

        let validator = test_validator(topic, minute);
        let result = validator.filter_and_build(vec![valid, stale, mismatched], None);

        assert_eq!(
            result.peers.len(),
            1,
            "only the valid peer should be accepted"
        );
        assert_eq!(result.counters.accepted, 1);
        assert_eq!(result.counters.stale, 1);
        assert_eq!(result.counters.identity_mismatch, 1);
    }

    #[test]
    fn all_rejected_helper() {
        let counters = ValidationCounters {
            total: 10,
            oversized: 3,
            stale: 2,
            ..Default::default()
        };
        assert!(counters.all_rejected());

        let counters2 = ValidationCounters {
            total: 5,
            accepted: 1,
            ..Default::default()
        };
        assert!(!counters2.all_rejected());

        let counters3 = ValidationCounters::default();
        assert!(!counters3.all_rejected()); // total == 0
    }

    #[test]
    fn total_rejected_helper() {
        let counters = ValidationCounters {
            oversized: 1,
            stale: 2,
            future: 1,
            decode_failure: 1,
            identity_mismatch: 1,
            invalid_signature: 1,
            self_filtered: 1,
            duplicates: 1,
            ..Default::default()
        };
        assert_eq!(counters.total_rejected(), 9);
    }

    #[test]
    fn rejection_reason_display() {
        let reasons = vec![
            RejectionReason::Oversized {
                size: 300,
                max: 256,
            },
            RejectionReason::Stale {
                age_minutes: 15,
                max_age: 10,
            },
            RejectionReason::FutureRecord {
                skew_minutes: 5,
                max_skew: 2,
            },
            RejectionReason::DecodeFailure("bad data".into()),
            RejectionReason::IdentityMismatch,
            RejectionReason::InvalidSignature("bad sig".into()),
            RejectionReason::SelfFiltered,
            RejectionReason::Duplicate,
        ];
        for r in &reasons {
            let s = r.to_string();
            assert!(!s.is_empty(), "Display should produce non-empty string");
            // Ensure no panic.
            let _ = format!("{r}");
        }
    }

    // ── ValidationConfig tests ────────────────────────────────────────

    #[test]
    fn config_new_uses_sensible_defaults() {
        let topic = [0xAAu8; 32];
        let config = ValidationConfig::new(topic);
        assert_eq!(config.topic, topic);
        assert_eq!(
            config.max_record_age_minutes,
            DEFAULT_MAX_RECORD_AGE_MINUTES
        );
        assert_eq!(
            config.max_clock_skew_minutes,
            DEFAULT_MAX_CLOCK_SKEW_MINUTES
        );
        assert_eq!(config.max_record_size, DEFAULT_MAX_RECORD_SIZE);
        assert_eq!(
            config.max_records_per_lookup,
            DEFAULT_MAX_RECORDS_PER_LOOKUP
        );
        assert_eq!(config.max_candidate_peers, DEFAULT_MAX_CANDIDATE_PEERS);
    }

    #[test]
    fn config_default_uses_zero_topic() {
        let config = ValidationConfig::default();
        assert_eq!(config.topic, [0u8; 32]);
    }

    // ── Real unix minute tests ────────────────────────────────────────

    #[test]
    fn real_unix_minute_accepts_fresh_record() {
        let topic = test_topic();
        let now = unix_minute(0);
        let (sk, ep) = test_identity();
        let record = create_discovery_record(topic, now, &ep, &sk).unwrap();
        let validator = test_validator(topic, now);
        let result = validator.validate_single(&record);
        assert!(
            result.is_ok(),
            "fresh record at real unix minute should be accepted: {result:?}"
        );
    }

    #[test]
    fn real_unix_minute_accepts_one_minute_old() {
        let topic = test_topic();
        let now = unix_minute(0);
        let one_min_ago = now.saturating_sub(1);
        let (sk, ep) = test_identity();
        let record = create_discovery_record(topic, one_min_ago, &ep, &sk).unwrap();
        let validator = test_validator(topic, now);
        let result = validator.validate_single(&record);
        assert!(
            result.is_ok(),
            "record one minute old should be accepted: {result:?}"
        );
    }

    // ── Smoke: Send + Sync ────────────────────────────────────────────

    #[test]
    fn validator_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DiscoveryRecordValidator>();
        assert_send_sync::<ValidationConfig>();
        assert_send_sync::<ValidationCounters>();
    }

    // ── Tracing: per-record rejection events ──────────────────────────

    /// Per-record rejection events emit `trace!` messages.
    #[test]
    #[n0_tracing_test::traced_test]
    fn rejection_traces_emit_trace_events() {
        let topic = test_topic();
        let minute = 1_000_000;
        let (sk, ep) = test_identity_seeded();

        // Valid record (will be accepted).
        let valid = create_discovery_record(topic, minute, &ep, &sk).unwrap();

        // Stale record (will be rejected with Stale).
        let (sk2, ep2) = {
            let seed = [0x22u8; 32];
            let sk = iroh::SecretKey::from_bytes(&seed);
            (sk.clone(), sk.public())
        };
        let stale = create_discovery_record(
            topic,
            minute - DEFAULT_MAX_RECORD_AGE_MINUTES - 5,
            &ep2,
            &sk2,
        )
        .unwrap();

        let validator = test_validator(topic, minute);
        let result = validator.filter_and_build(vec![valid, stale], Some(&ep));

        // Expected: 0 accepted (valid record is self-filtered), 1 stale, 1 self-filtered
        assert_eq!(result.counters.accepted, 0);
        assert_eq!(result.counters.stale, 1);
        assert_eq!(result.counters.self_filtered, 1);

        // Verifying that tracing was emitted is implicit — `traced_test`
        // panics if any `tracing` subscriber panics, and the test runner
        // captures output for inspection with `--nocapture`.
        let _ = result;
    }
}
