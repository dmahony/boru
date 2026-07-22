//! Centralised safety and timing limits for public-room DHT discovery,
//! peer validation, and chat messaging.
//!
//! All tunable parameters that control DHT traffic rate, record validation
//! strictness, peer-count bounds, message size, and backfill limits live
//! in a single [`PublicRoomConfig`] struct so they can be overridden easily
//! in tests without touching production defaults.
//!
//! # Why defaults are conservative
//!
//! | Parameter | Conservative default | Rationale |
//! |-----------|---------------------|-----------|
//! | `publish_interval` | 5 minutes | Avoid spamming the DHT. Production DHT nodes are rate-sensitive; sub-minute publication on Mainline DHT is considered antisocial. |
//! | `discover_interval` | 30 seconds | Fast enough to notice churn within a minute, slow enough that a room with 1000 peers generates < 35 lookups/second globally. |
//! | `stale_record_age_minutes` | 10 minutes | A peer that hasn't re-published in 10 minutes is likely gone. Longer windows retain stale entries; shorter windows drop peers during transient network partitions. |
//! | `future_clock_tolerance_minutes` | 2 minutes | Small enough to reject misdated records, large enough to tolerate NTP slew on consumer hardware. |
//! | `max_record_size` | 256 bytes | A valid discovery record is ~171 B raw (~270 B encrypted). 256 B provides headroom without accepting DHT spam. |
//! | `max_records_per_lookup` | 20 | Bounds CPU time per discovery cycle. Realistic public rooms have < 10 active peers; 20 handles large rooms without unbounded work. |
//! | `max_candidate_peers_per_cycle` | 20 | Prevents a single discovery from triggering 1000 gossip joins. The caller can batch joins; 5–10 concurrent joins is typical. |
//! | `max_concurrent_joins` | 5 | Prevents connection storms when many new peers appear simultaneously. |
//! | `retry_backoff_min` | 1 second | DHT lookups rarely fail — a 1 s backoff is respectful of network resources. |
//! | `retry_backoff_max` | 60 seconds | Caps exponential growth so a flaky DHT connection doesn't idle for hours before retrying. |
//! | `message_size_limit` | 4096 bytes | Matches the gossip proto's [`DEFAULT_MAX_MESSAGE_SIZE`]. Chat messages include metadata overhead; 4 KiB accommodates long text without enabling abuse. |
//! | `nickname_length_limit` | 32 bytes | Enough for display names in any script; short enough to fit in a TUI status line. |
//! | `per_peer_message_rate` | 10.0 msgs/s | Prevents a single peer from flooding the room. Liberal enough for interactive chat; strict enough to stop a fast loop. |
//! | `blob_announcement_limit` | 5 | Prevents a peer from announcing hundreds of blobs in one burst. |
//! | `blob_download_limit` | 10 | Limits concurrent / queued downloads per peer to avoid resource exhaustion. |
//! | `backfill_request_limit` | 50 | A new joiner only needs the most recent messages to get context. 50 messages (~200 KiB at 4 KiB each) is a polite request size. |
//! | `jitter_factor` | 0.1 (10%) | Prevents thundering-herd DHT traffic when many peers start simultaneously. |
//!
//! [`DEFAULT_MAX_MESSAGE_SIZE`]: crate::proto::DEFAULT_MAX_MESSAGE_SIZE
//!
//! # Test overrides
//!
//! Tests create configs with struct-literal syntax:
//!
//! ```ignore
//! use std::time::Duration;
//! use boru_core::public_room_config::PublicRoomConfig;
//!
//! let cfg = PublicRoomConfig {
//!     discover_interval: Duration::from_millis(10),
//!     publish_interval: Duration::from_millis(10),
//!     ..Default::default()
//! };
//! ```
//!
//! This pattern lets tests use fast intervals without changing the
//! production defaults or adding CLI flags — the compiler checks that
//! every field is either explicitly set or falls through to Default.

use std::time::Duration;

use crate::discovery_validation::ValidationConfig;
use crate::public_room_continuous::ContinuousTrackerConfig;

// ---------------------------------------------------------------------------
// PublicRoomConfig
// ---------------------------------------------------------------------------

/// Master configuration for the public-room DHT discovery system.
///
/// All timing, sizing, rate, and bound limits used by the public-room
/// tracker, validation pipeline, continuous loops, and chat layer are
/// centralised here.
///
/// Create a default for production use, or override individual fields
/// for tests:
///
/// ```ignore
/// let cfg = PublicRoomConfig {
///     discover_interval: Duration::from_millis(10),
///     ..Default::default()
/// };
/// ```
///
/// See the [module-level documentation](self) for why each default is
/// conservative.
#[derive(Debug, Clone)]
pub struct PublicRoomConfig {
    // ── DHT Lookup timing ──────────────────────────────────────────
    /// Timeout for the initial DHT lookup before treating it as empty.
    ///
    /// The first discovery cycle after startup may take longer because
    /// the DHT routing table is cold.  This timeout prevents the initial
    /// lookup from hanging indefinitely.
    ///
    /// Default: 15 seconds.
    pub initial_lookup_timeout: Duration,

    /// Interval between periodic DHT discovery lookups (with jitter).
    ///
    /// Default: 30 seconds.
    pub discover_interval: Duration,

    /// Interval between periodic DHT publication refreshes (with jitter).
    ///
    /// Default: 5 minutes.
    pub publish_interval: Duration,

    // ── Record staleness ───────────────────────────────────────────
    /// Maximum age (in minutes) of a discovery record before it is
    /// considered stale and discarded.
    ///
    /// Default: 10 minutes.
    pub stale_record_age_minutes: u64,

    /// Allowed clock skew (in minutes) for future-dated discovery records.
    ///
    /// Default: 2 minutes.
    pub future_clock_tolerance_minutes: u64,

    // ── Retry / backoff ────────────────────────────────────────────
    /// Initial delay before the first retry on a DHT failure.
    ///
    /// Doubled on each subsequent failure, capped at [`retry_backoff_max`].
    ///
    /// Default: 1 second.
    pub retry_backoff_min: Duration,

    /// Maximum delay for exponential backoff on repeated DHT failures.
    ///
    /// Default: 60 seconds.
    pub retry_backoff_max: Duration,

    // ── Lookup bounds ──────────────────────────────────────────────
    /// Maximum number of raw records to examine in a single discovery
    /// lookup call.
    ///
    /// Default: 20.
    pub max_records_per_lookup: usize,

    /// Maximum serialized [`Record`] size in bytes.
    ///
    /// A valid signed discovery record is ~171 B raw (~270 B encrypted).
    /// 256 B provides headroom without accepting DHT spam.
    ///
    /// Default: 256 bytes.
    ///
    /// [`Record`]: distributed_topic_tracker::Record
    pub max_record_size: usize,

    /// Maximum number of candidate [`EndpointId`] values to return from
    /// a single discovery cycle.
    ///
    /// Default: 20.
    pub max_candidate_peers_per_cycle: usize,

    /// Maximum number of concurrent gossip join operations.
    ///
    /// This is informational / advisory — the tracker sends batches via
    /// a channel and the caller is responsible for concurrency.
    ///
    /// Default: 5.
    pub max_concurrent_joins: usize,

    /// Maximum number of candidate connection proposals in one tracker session.
    /// Default: 10.
    pub max_candidates_per_session: usize,

    /// Maximum candidate connection proposals per rate-limit window.
    /// Default: 10.
    pub connection_attempts_per_window: usize,

    /// Candidate connection-attempt rate-limit window.
    /// Default: 60 seconds.
    pub connection_attempt_window: Duration,

    // ── Chat limits ────────────────────────────────────────────────
    /// Maximum size (in bytes) of a single chat message body.
    ///
    /// Matches the gossip proto's [`DEFAULT_MAX_MESSAGE_SIZE`] (4096 B).
    /// Chat messages carry metadata overhead in addition to the body,
    /// so the wire limit is governed by the proto — this is the
    /// application-level sanity check.
    ///
    /// Default: 4096 bytes.
    ///
    /// [`DEFAULT_MAX_MESSAGE_SIZE`]: crate::proto::DEFAULT_MAX_MESSAGE_SIZE
    pub message_size_limit: usize,

    /// Maximum length (in bytes) of a user's display nickname.
    ///
    /// Enough for display names in any script (CJK characters at 3 B
    /// each → ~10 characters), short enough to fit in a TUI status line.
    ///
    /// Default: 32 bytes.
    pub nickname_length_limit: usize,

    /// Maximum message rate per peer (messages per second).
    ///
    /// A value of `0.0` means no rate limit.
    ///
    /// Default: 10.0 msgs/s.
    pub per_peer_message_rate: f64,

    // ── Blob / image limits ────────────────────────────────────────
    /// Maximum number of blobs or images a single peer may announce
    /// in one burst.
    ///
    /// Default: 5.
    pub blob_announcement_limit: usize,

    /// Maximum number of blobs or images a single peer may download
    /// concurrently or in a queue.
    ///
    /// Default: 10.
    pub blob_download_limit: usize,

    /// Maximum size of a single automatically downloaded blob in a public room.
    /// Default: 10 MiB.
    pub max_blob_size_bytes: usize,

    // ── Backfill / history ─────────────────────────────────────────
    /// Maximum number of messages to request in a single backfill
    /// (history sync) operation.
    ///
    /// Default: 50.
    pub backfill_request_limit: u32,

    // ── Jitter ─────────────────────────────────────────────────────
    /// Jitter factor applied to intervals and backoff delays.
    ///
    /// * `0.0` — no jitter (deterministic timing).
    /// * `0.1` — ±10% jitter (default).
    /// * `0.5` — ±50% jitter (maximum).
    ///
    /// Values outside [0.0, 0.5] are clamped by [`Self::sanitize`].
    ///
    /// Default: 0.1.
    pub jitter_factor: f64,
}

// ---------------------------------------------------------------------------
// Default (production-conservative)
// ---------------------------------------------------------------------------

impl Default for PublicRoomConfig {
    fn default() -> Self {
        Self {
            // DHT lookup timing
            initial_lookup_timeout: Duration::from_secs(15),
            discover_interval: Duration::from_secs(30),
            publish_interval: Duration::from_secs(300), // 5 minutes

            // Record staleness
            stale_record_age_minutes: 10,
            future_clock_tolerance_minutes: 2,

            // Retry / backoff
            retry_backoff_min: Duration::from_secs(1),
            retry_backoff_max: Duration::from_secs(60),

            // Lookup bounds
            max_records_per_lookup: 20,
            max_record_size: 256,
            max_candidate_peers_per_cycle: 20,
            max_concurrent_joins: 5,
            max_candidates_per_session: 10,
            connection_attempts_per_window: 10,
            connection_attempt_window: Duration::from_secs(60),

            // Chat limits
            message_size_limit: 4096,
            nickname_length_limit: 32,
            per_peer_message_rate: 10.0,

            // Blob / image limits
            blob_announcement_limit: 5,
            blob_download_limit: 10,
            max_blob_size_bytes: 10 * 1024 * 1024,

            // Backfill
            backfill_request_limit: 50,

            // Jitter
            jitter_factor: 0.1,
        }
    }
}

// ---------------------------------------------------------------------------
// Conversions to sub-configs
// ---------------------------------------------------------------------------

impl From<PublicRoomConfig> for ContinuousTrackerConfig {
    fn from(cfg: PublicRoomConfig) -> Self {
        Self {
            publish_interval: cfg.publish_interval,
            discover_interval: cfg.discover_interval,
            max_candidates_per_cycle: cfg.max_candidate_peers_per_cycle,
            max_concurrent_joins: cfg.max_concurrent_joins,
            max_candidates_per_session: cfg.max_candidates_per_session,
            connection_attempts_per_window: cfg.connection_attempts_per_window,
            connection_attempt_window: cfg.connection_attempt_window,
            initial_retry_delay: cfg.retry_backoff_min,
            max_retry_delay: cfg.retry_backoff_max,
            jitter_factor: cfg.jitter_factor,
            stale_peer_ttl: None,
        }
    }
}

impl From<&PublicRoomConfig> for ValidationConfig {
    fn from(cfg: &PublicRoomConfig) -> Self {
        Self {
            topic: [0u8; 32], // caller must override after conversion
            max_record_age_minutes: cfg.stale_record_age_minutes,
            max_clock_skew_minutes: cfg.future_clock_tolerance_minutes,
            max_record_size: cfg.max_record_size,
            max_records_per_lookup: cfg.max_records_per_lookup,
            max_candidate_peers: cfg.max_candidate_peers_per_cycle,
        }
    }
}

// ---------------------------------------------------------------------------
// Sanitization
// ---------------------------------------------------------------------------

impl PublicRoomConfig {
    /// Clamp floating-point tuning values to safe finite ranges and return self.
    pub fn sanitize(mut self) -> Self {
        self.jitter_factor = if self.jitter_factor.is_finite() {
            self.jitter_factor.clamp(0.0, 0.5)
        } else {
            0.1
        };
        // Keep the per-peer timestamp vector bounded even when configuration
        // originates outside the trusted process (for example a config file).
        self.per_peer_message_rate = if self.per_peer_message_rate.is_finite() {
            self.per_peer_message_rate.clamp(0.0, 1_000.0)
        } else {
            0.0
        };
        self
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Default fields ─────────────────────────────────────────────

    #[test]
    fn default_initial_lookup_timeout_is_15s() {
        assert_eq!(
            PublicRoomConfig::default().initial_lookup_timeout,
            Duration::from_secs(15)
        );
    }

    #[test]
    fn default_discover_interval_is_30s() {
        assert_eq!(
            PublicRoomConfig::default().discover_interval,
            Duration::from_secs(30)
        );
    }

    #[test]
    fn default_publish_interval_is_5min() {
        assert_eq!(
            PublicRoomConfig::default().publish_interval,
            Duration::from_secs(300)
        );
    }

    #[test]
    fn default_stale_record_age_is_10min() {
        assert_eq!(PublicRoomConfig::default().stale_record_age_minutes, 10);
    }

    #[test]
    fn default_future_clock_tolerance_is_2min() {
        assert_eq!(
            PublicRoomConfig::default().future_clock_tolerance_minutes,
            2
        );
    }

    #[test]
    fn default_retry_backoff_min_is_1s() {
        assert_eq!(
            PublicRoomConfig::default().retry_backoff_min,
            Duration::from_secs(1)
        );
    }

    #[test]
    fn default_retry_backoff_max_is_60s() {
        assert_eq!(
            PublicRoomConfig::default().retry_backoff_max,
            Duration::from_secs(60)
        );
    }

    #[test]
    fn default_max_records_per_lookup_is_20() {
        assert_eq!(PublicRoomConfig::default().max_records_per_lookup, 20);
    }

    #[test]
    fn default_max_record_size_is_256() {
        assert_eq!(PublicRoomConfig::default().max_record_size, 256);
    }

    #[test]
    fn default_max_candidate_peers_is_20() {
        assert_eq!(
            PublicRoomConfig::default().max_candidate_peers_per_cycle,
            20
        );
    }

    #[test]
    fn default_max_concurrent_joins_is_5() {
        assert_eq!(PublicRoomConfig::default().max_concurrent_joins, 5);
    }

    #[test]
    fn default_message_size_limit_is_4096() {
        assert_eq!(PublicRoomConfig::default().message_size_limit, 4096);
    }

    #[test]
    fn default_nickname_length_limit_is_32() {
        assert_eq!(PublicRoomConfig::default().nickname_length_limit, 32);
    }

    #[test]
    fn default_per_peer_message_rate_is_10() {
        assert!((PublicRoomConfig::default().per_peer_message_rate - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn default_blob_announcement_limit_is_5() {
        assert_eq!(PublicRoomConfig::default().blob_announcement_limit, 5);
    }

    #[test]
    fn default_blob_download_limit_is_10() {
        assert_eq!(PublicRoomConfig::default().blob_download_limit, 10);
    }

    #[test]
    fn default_backfill_request_limit_is_50() {
        assert_eq!(PublicRoomConfig::default().backfill_request_limit, 50);
    }

    #[test]
    fn default_jitter_factor_is_0_1() {
        assert!((PublicRoomConfig::default().jitter_factor - 0.1).abs() < f64::EPSILON);
    }

    // ── Test override pattern ──────────────────────────────────────

    /// Tests can override individual fields with struct-literal syntax.
    #[test]
    fn test_override_discover_interval() {
        let cfg = PublicRoomConfig {
            discover_interval: Duration::from_millis(10),
            ..Default::default()
        };
        assert_eq!(cfg.discover_interval, Duration::from_millis(10));
        // Other fields remain at production defaults.
        assert_eq!(cfg.publish_interval, Duration::from_secs(300));
        assert_eq!(cfg.initial_lookup_timeout, Duration::from_secs(15));
    }

    /// Tests can override multiple fields simultaneously.
    #[test]
    fn test_override_multiple_fields() {
        let cfg = PublicRoomConfig {
            discover_interval: Duration::from_millis(10),
            publish_interval: Duration::from_millis(10),
            stale_record_age_minutes: 1,
            ..Default::default()
        };
        assert_eq!(cfg.discover_interval, Duration::from_millis(10));
        assert_eq!(cfg.publish_interval, Duration::from_millis(10));
        assert_eq!(cfg.stale_record_age_minutes, 1);
        assert_eq!(cfg.future_clock_tolerance_minutes, 2); // unchanged default
    }

    // ── Sanitize ───────────────────────────────────────────────────

    #[test]
    fn sanitize_clamps_high_jitter() {
        let cfg = PublicRoomConfig {
            jitter_factor: 2.0,
            ..Default::default()
        };
        let sanitized = cfg.sanitize();
        assert!(
            sanitized.jitter_factor <= 0.5,
            "jitter_factor should be clamped to 0.5, got {}",
            sanitized.jitter_factor
        );
    }

    #[test]
    fn sanitize_clamps_negative_jitter() {
        let cfg = PublicRoomConfig {
            jitter_factor: -1.0,
            ..Default::default()
        };
        let sanitized = cfg.sanitize();
        assert!(
            sanitized.jitter_factor >= 0.0,
            "negative jitter_factor should be clamped to 0.0, got {}",
            sanitized.jitter_factor
        );
    }

    #[test]
    fn sanitize_preserves_valid_jitter() {
        let cfg = PublicRoomConfig {
            jitter_factor: 0.25,
            ..Default::default()
        };
        assert!((cfg.clone().sanitize().jitter_factor - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn sanitize_replaces_non_finite_and_caps_rate() {
        let cfg = PublicRoomConfig {
            jitter_factor: f64::NAN,
            per_peer_message_rate: f64::INFINITY,
            ..Default::default()
        }
        .sanitize();
        assert_eq!(cfg.jitter_factor, 0.1);
        assert_eq!(cfg.per_peer_message_rate, 0.0);
    }

    // ── Conversion to ContinuousTrackerConfig ───────────────────────

    #[test]
    fn into_continuous_tracker_config_maps_fields() {
        let cfg = PublicRoomConfig {
            publish_interval: Duration::from_secs(42),
            discover_interval: Duration::from_secs(7),
            max_candidate_peers_per_cycle: 10,
            max_concurrent_joins: 3,
            retry_backoff_min: Duration::from_millis(500),
            retry_backoff_max: Duration::from_secs(30),
            jitter_factor: 0.2,
            ..Default::default()
        };
        let ctc: ContinuousTrackerConfig = cfg.into();
        assert_eq!(ctc.publish_interval, Duration::from_secs(42));
        assert_eq!(ctc.discover_interval, Duration::from_secs(7));
        assert_eq!(ctc.max_candidates_per_cycle, 10);
        assert_eq!(ctc.max_concurrent_joins, 3);
        assert_eq!(ctc.initial_retry_delay, Duration::from_millis(500));
        assert_eq!(ctc.max_retry_delay, Duration::from_secs(30));
        assert!((ctc.jitter_factor - 0.2).abs() < f64::EPSILON);
    }

    // ── Conversion to ValidationConfig ──────────────────────────────

    #[test]
    fn into_validation_config_maps_fields() {
        let cfg = PublicRoomConfig {
            stale_record_age_minutes: 5,
            future_clock_tolerance_minutes: 1,
            max_record_size: 512,
            max_records_per_lookup: 15,
            max_candidate_peers_per_cycle: 10,
            ..Default::default()
        };
        let vc: ValidationConfig = (&cfg).into();
        assert_eq!(vc.topic, [0u8; 32]); // placeholder — caller overrides
        assert_eq!(vc.max_record_age_minutes, 5);
        assert_eq!(vc.max_clock_skew_minutes, 1);
        assert_eq!(vc.max_record_size, 512);
        assert_eq!(vc.max_records_per_lookup, 15);
        assert_eq!(vc.max_candidate_peers, 10);
    }

    // ── Clone + Debug ──────────────────────────────────────────────

    #[test]
    fn config_is_clone() {
        let a = PublicRoomConfig::default();
        let b = a.clone();
        assert_eq!(a.discover_interval, b.discover_interval);
        assert_eq!(a.message_size_limit, b.message_size_limit);
    }

    #[test]
    fn config_debug_does_not_panic() {
        let cfg = PublicRoomConfig::default();
        let _ = format!("{cfg:?}");
    }

    // ── Field-level sanity ─────────────────────────────────────────

    /// All production defaults are positive / non-zero.
    #[test]
    fn all_defaults_are_positive() {
        let cfg = PublicRoomConfig::default();
        assert!(cfg.initial_lookup_timeout > Duration::ZERO);
        assert!(cfg.discover_interval > Duration::ZERO);
        assert!(cfg.publish_interval > Duration::ZERO);
        assert!(cfg.stale_record_age_minutes > 0);
        assert!(cfg.future_clock_tolerance_minutes > 0);
        assert!(cfg.retry_backoff_min > Duration::ZERO);
        assert!(cfg.retry_backoff_max > Duration::ZERO);
        assert!(cfg.max_records_per_lookup > 0);
        assert!(cfg.max_record_size > 0);
        assert!(cfg.max_candidate_peers_per_cycle > 0);
        assert!(cfg.max_concurrent_joins > 0);
        assert!(cfg.message_size_limit > 0);
        assert!(cfg.nickname_length_limit > 0);
        assert!(cfg.per_peer_message_rate > 0.0);
        assert!(cfg.blob_announcement_limit > 0);
        assert!(cfg.blob_download_limit > 0);
        assert!(cfg.backfill_request_limit > 0);
        assert!(cfg.jitter_factor >= 0.0);
    }

    /// Backoff min <= backoff max (sensible floor/ceiling).
    #[test]
    fn retry_backoff_min_less_than_max() {
        let cfg = PublicRoomConfig::default();
        assert!(
            cfg.retry_backoff_min <= cfg.retry_backoff_max,
            "retry_backoff_min ({:?}) must be <= retry_backoff_max ({:?})",
            cfg.retry_backoff_min,
            cfg.retry_backoff_max
        );
    }
}
