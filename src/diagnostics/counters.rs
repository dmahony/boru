//! Boru diagnostics submodule (structural split BORU-CORE-002).

use super::*;

// =============================================================================
// DiagnosticCounters — atomic counters (BORU-DISC-20, PDF Phase 6)
// =============================================================================

/// Atomic counters for discovery/conversation-topic diagnostics.
///
/// Complements the [`DiagnosticEvent`] ring buffer: events answer *"what
/// happened when"*, counters answer *"how many so far"* without any storage
/// pressure. The four counters required by the discovery logging step are
/// here — discovery peers seen, direct topics joined, group topics joined,
/// and malformed discovery packets — plus a separate unsupported-version
/// packet counter so the BORU-DISC-19 protocol gate is observable on its
/// own.
///
/// Instances are cheaply cloneable and share the underlying atomics, so a
/// single global ([`DIAGNOSTIC_COUNTERS`]) can be observed from multiple
/// modules (the discovery service, the iced frontend, the MCP layer) while
/// tests use isolated instances.
#[derive(Debug, Clone, Default)]
pub struct DiagnosticCounters {
    /// Peers seen on the internal discovery topic (fresh registry entries).
    discovery_peers_seen: Arc<AtomicU64>,
    /// Direct (deterministic pairwise) conversation topics joined.
    direct_topics_joined: Arc<AtomicU64>,
    /// Group/room conversation topics joined.
    group_topics_joined: Arc<AtomicU64>,
    /// Malformed (undecodable) discovery packets dropped.
    malformed_discovery_packets: Arc<AtomicU64>,
    /// Discovery packets dropped for speaking an unsupported protocol
    /// version (the BORU-DISC-19 version gate).
    unsupported_version_packets: Arc<AtomicU64>,
}

impl DiagnosticCounters {
    /// Create an isolated counter set (tests use this; production shares
    /// the global [`DIAGNOSTIC_COUNTERS`] via `Clone`).
    pub fn new() -> Self {
        Self::default()
    }

    /// A fresh peer was registered from the discovery topic.
    pub fn record_discovery_peer_seen(&self) {
        self.discovery_peers_seen.fetch_add(1, Ordering::Relaxed);
    }

    /// A direct (deterministic pairwise) conversation topic was joined.
    pub fn record_direct_topic_joined(&self) {
        self.direct_topics_joined.fetch_add(1, Ordering::Relaxed);
    }

    /// A group/room conversation topic was joined.
    pub fn record_group_topic_joined(&self) {
        self.group_topics_joined.fetch_add(1, Ordering::Relaxed);
    }

    /// A malformed (undecodable) discovery packet was dropped.
    pub fn record_malformed_discovery_packet(&self) {
        self.malformed_discovery_packets
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A discovery packet speaking an unsupported protocol version was
    /// dropped by the version gate.
    pub fn record_unsupported_version_packet(&self) {
        self.unsupported_version_packets
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Number of peers seen on the discovery topic (fresh registrations).
    pub fn discovery_peers_seen(&self) -> u64 {
        self.discovery_peers_seen.load(Ordering::Relaxed)
    }

    /// Number of direct conversation topics joined.
    pub fn direct_topics_joined(&self) -> u64 {
        self.direct_topics_joined.load(Ordering::Relaxed)
    }

    /// Number of group conversation topics joined.
    pub fn group_topics_joined(&self) -> u64 {
        self.group_topics_joined.load(Ordering::Relaxed)
    }

    /// Number of malformed (undecodable) discovery packets dropped.
    pub fn malformed_discovery_packets(&self) -> u64 {
        self.malformed_discovery_packets.load(Ordering::Relaxed)
    }

    /// Number of unsupported-version discovery packets dropped.
    pub fn unsupported_version_packets(&self) -> u64 {
        self.unsupported_version_packets.load(Ordering::Relaxed)
    }

    /// Point-in-time snapshot of all counters.
    pub fn snapshot(&self) -> DiagnosticCountersSnapshot {
        DiagnosticCountersSnapshot {
            discovery_peers_seen: self.discovery_peers_seen(),
            direct_topics_joined: self.direct_topics_joined(),
            group_topics_joined: self.group_topics_joined(),
            malformed_discovery_packets: self.malformed_discovery_packets(),
            unsupported_version_packets: self.unsupported_version_packets(),
        }
    }
}

/// Point-in-time snapshot of [`DiagnosticCounters`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiagnosticCountersSnapshot {
    /// Peers seen on the internal discovery topic (fresh registrations).
    pub discovery_peers_seen: u64,
    /// Direct (deterministic pairwise) conversation topics joined.
    pub direct_topics_joined: u64,
    /// Group/room conversation topics joined.
    pub group_topics_joined: u64,
    /// Malformed (undecodable) discovery packets dropped.
    pub malformed_discovery_packets: u64,
    /// Discovery packets dropped by the unsupported-version gate.
    pub unsupported_version_packets: u64,
}

/// Global atomic counters for discovery/conversation-topic diagnostics.
///
/// Lazily initialised on first access. Clones share the same underlying
/// atomics, so the discovery service and the frontends can all bump and read
/// the same counters without a lock.
pub static DIAGNOSTIC_COUNTERS: LazyLock<DiagnosticCounters> =
    LazyLock::new(DiagnosticCounters::new);

// =============================================================================
// DirectoryCounters — room-directory advertisement counters (BORU-DIR-22)
// =============================================================================

/// Atomic counters for the **room directory** (BORU-DIR-22, PDF Phase 8
/// Task 8.1).
///
/// These are deliberately a separate counter set from [`DiagnosticCounters`]
/// (which counts discovery/control-plane *peers* and *topics*): directory
/// diagnostics answer *"what happened to room advertisements"* and must stay
/// distinct from actual room-message diagnostics (PDF guardrail: never mix
/// directory diagnostics into room-message diagnostics). They are also
/// metadata-level only — every counter counts *advertisements* (bounded,
/// validated metadata), never chat contents, message bodies, or private room
/// history.
///
/// The seven counters required by PDF Task 8.1 step 1:
///
/// * **received** — a `PUBLIC_ROOM_ADVERTISEMENT` envelope was decoded and
///   admitted by the control-plane guard (before any auth verdict);
/// * **accepted** — the advertisement entered or refreshed the local
///   directory cache ([`AdvertiseOutcome::Added`] / `Refreshed`);
/// * **rejected** — the advertisement was dropped (signature verification
///   failed, or the guard's minimal-content policy rejected it);
/// * **expired** — a cached advertisement was evicted by TTL
///   ([`crate::room_directory::RoomDirectory::evict_expired`]);
/// * **withdrawn** — a verified, authoritative withdrawal removed a listing;
/// * **deduplicated** — a repeated/identical advertisement was collapsed
///   into the existing entry (no second card, no UI churn);
/// * **rate-limited** — an advertisement envelope was dropped by the
///   per-sender control-plane rate limiter.
///
/// With these plus the per-room diagnostics view
/// ([`crate::room_directory::RoomDirectory::diagnostics_snapshot`]), a
/// developer can tell whether a room was never advertised, rejected,
/// expired, or simply failed to join (PDF Task 8.1 acceptance criteria).
#[derive(Debug, Clone, Default)]
pub struct DirectoryCounters {
    /// Decoded + guard-admitted `PUBLIC_ROOM_ADVERTISEMENT` envelopes.
    advertisements_received: Arc<AtomicU64>,
    /// Advertisements that entered or refreshed the directory cache.
    advertisements_accepted: Arc<AtomicU64>,
    /// Advertisements dropped (auth failure / minimal-content policy).
    advertisements_rejected: Arc<AtomicU64>,
    /// Cached advertisements evicted by TTL expiry.
    advertisements_expired: Arc<AtomicU64>,
    /// Listings removed by a verified authoritative withdrawal.
    advertisements_withdrawn: Arc<AtomicU64>,
    /// Repeated/identical advertisements collapsed into an existing entry.
    advertisements_deduplicated: Arc<AtomicU64>,
    /// Advertisement envelopes dropped by the per-sender rate limiter.
    advertisements_rate_limited: Arc<AtomicU64>,
}

impl DirectoryCounters {
    /// Create an isolated counter set (tests use this; production shares
    /// the global [`DIRECTORY_COUNTERS`] via `Clone`).
    pub fn new() -> Self {
        Self::default()
    }

    /// A `PUBLIC_ROOM_ADVERTISEMENT` envelope was decoded and admitted.
    pub fn record_advertisement_received(&self) {
        self.advertisements_received.fetch_add(1, Ordering::Relaxed);
    }

    /// An advertisement entered or refreshed the directory cache.
    pub fn record_advertisement_accepted(&self) {
        self.advertisements_accepted.fetch_add(1, Ordering::Relaxed);
    }

    /// An advertisement was dropped (auth failure / minimal-content policy).
    pub fn record_advertisement_rejected(&self) {
        self.advertisements_rejected.fetch_add(1, Ordering::Relaxed);
    }

    /// A cached advertisement was evicted by TTL expiry.
    pub fn record_advertisement_expired(&self) {
        self.advertisements_expired.fetch_add(1, Ordering::Relaxed);
    }

    /// A listing was removed by a verified authoritative withdrawal.
    pub fn record_advertisement_withdrawn(&self) {
        self.advertisements_withdrawn
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A repeated/identical advertisement was collapsed into an existing
    /// entry (deduplicated, no second card).
    pub fn record_advertisement_deduplicated(&self) {
        self.advertisements_deduplicated
            .fetch_add(1, Ordering::Relaxed);
    }

    /// An advertisement envelope was dropped by the per-sender rate limiter.
    pub fn record_advertisement_rate_limited(&self) {
        self.advertisements_rate_limited
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Number of decoded + admitted room advertisements.
    pub fn advertisements_received(&self) -> u64 {
        self.advertisements_received.load(Ordering::Relaxed)
    }

    /// Number of advertisements that entered/refreshed the directory cache.
    pub fn advertisements_accepted(&self) -> u64 {
        self.advertisements_accepted.load(Ordering::Relaxed)
    }

    /// Number of advertisements dropped (auth / minimal-content policy).
    pub fn advertisements_rejected(&self) -> u64 {
        self.advertisements_rejected.load(Ordering::Relaxed)
    }

    /// Number of cached advertisements evicted by TTL expiry.
    pub fn advertisements_expired(&self) -> u64 {
        self.advertisements_expired.load(Ordering::Relaxed)
    }

    /// Number of listings removed by verified authoritative withdrawals.
    pub fn advertisements_withdrawn(&self) -> u64 {
        self.advertisements_withdrawn.load(Ordering::Relaxed)
    }

    /// Number of repeated/identical advertisements deduplicated.
    pub fn advertisements_deduplicated(&self) -> u64 {
        self.advertisements_deduplicated.load(Ordering::Relaxed)
    }

    /// Number of advertisement envelopes dropped by the rate limiter.
    pub fn advertisements_rate_limited(&self) -> u64 {
        self.advertisements_rate_limited.load(Ordering::Relaxed)
    }

    /// Point-in-time snapshot of all directory counters.
    pub fn snapshot(&self) -> DirectoryCountersSnapshot {
        DirectoryCountersSnapshot {
            advertisements_received: self.advertisements_received(),
            advertisements_accepted: self.advertisements_accepted(),
            advertisements_rejected: self.advertisements_rejected(),
            advertisements_expired: self.advertisements_expired(),
            advertisements_withdrawn: self.advertisements_withdrawn(),
            advertisements_deduplicated: self.advertisements_deduplicated(),
            advertisements_rate_limited: self.advertisements_rate_limited(),
        }
    }

    /// The atomic backing the **expired** counter, for injection into the
    /// room-directory cache (BORU-DIR-22). The cache bumps it on TTL
    /// eviction so the counter is truthful even when eviction happens
    /// inside `RoomDirectory` rather than at the service boundary.
    pub fn expired_sink(&self) -> Arc<AtomicU64> {
        self.advertisements_expired.clone()
    }
}

/// Point-in-time snapshot of [`DirectoryCounters`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirectoryCountersSnapshot {
    /// Decoded + guard-admitted `PUBLIC_ROOM_ADVERTISEMENT` envelopes.
    pub advertisements_received: u64,
    /// Advertisements that entered or refreshed the directory cache.
    pub advertisements_accepted: u64,
    /// Advertisements dropped (auth failure / minimal-content policy).
    pub advertisements_rejected: u64,
    /// Cached advertisements evicted by TTL expiry.
    pub advertisements_expired: u64,
    /// Listings removed by a verified authoritative withdrawal.
    pub advertisements_withdrawn: u64,
    /// Repeated/identical advertisements collapsed into an existing entry.
    pub advertisements_deduplicated: u64,
    /// Advertisement envelopes dropped by the per-sender rate limiter.
    pub advertisements_rate_limited: u64,
}

/// Global atomic counters for room-directory diagnostics (BORU-DIR-22).
///
/// Lazily initialised on first access. Clones share the same underlying
/// atomics, so the discovery service (bump), the frontend and the MCP layer
/// (read) all observe the same values without a lock.
pub static DIRECTORY_COUNTERS: LazyLock<DirectoryCounters> = LazyLock::new(DirectoryCounters::new);

// =============================================================================
// DhtCounters — DHT effectiveness metrics (BORU-DHT-08)
// =============================================================================

/// Disposition counts of a single DHT lookup batch (`lookup -> valid records ->
/// rejected-by-reason`).  This is a **feature-independent** mirror of
/// [`crate::discovery_validation::ValidationCounters`] (which lives in the
/// net-gated module), so the always-available diagnostics layer can record the
/// DHT pipeline without pulling in the net stack.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DhtLookupCounts {
    /// Records that passed validation (unique `EndpointId`s, `accepted`).
    pub valid: u64,
    /// Rejected because the serialized record exceeded the size limit.
    pub oversized: u64,
    /// Rejected because the record timestamp was too stale.
    pub stale: u64,
    /// Rejected because the record timestamp was too far in the future.
    pub future: u64,
    /// Rejected because the record content could not be decoded.
    pub decode: u64,
    /// Rejected because the embedded `pub_key` did not match the payload id.
    pub identity: u64,
    /// Rejected because the Ed25519 signature was invalid.
    pub signature: u64,
    /// Rejected because the record advertised the local node's own id.
    pub own: u64,
    /// Rejected because the `EndpointId` was a duplicate in the batch.
    pub duplicate: u64,
}

/// Atomic counters tracking the end-to-end DHT discovery->join pipeline
/// (`lookup -> valid records -> new candidates -> queued -> join attempts ->
/// successful neighbours`), suitable for MCP/doctor tooling.
///
/// Deliberately separate from [`DiagnosticCounters`] (control-plane peers /
/// topics) and [`DirectoryCounters`] (room advertisements): DHT effectiveness
/// answers *"is the Mainline DHT actually producing and joining peers for the
/// discovery mesh"*.  Every counter is metadata (counts, timestamps) — never
/// chat contents, secret keys, private-room secrets, or full `EndpointId`s
/// (use [`EndpointId::fmt_short`] at log sites, never full keys).
///
/// Instances are cheaply cloneable and share the underlying atomics, so a
/// single global ([`DHT_COUNTERS`]) can be bumped by the bootstrap/public/
/// private DHT trackers and the join path, and read by tests / MCP / doctor.
#[derive(Debug, Clone, Default)]
pub struct DhtCounters {
    // Lookup
    lookup_cycles: Arc<AtomicU64>,
    lookup_failures: Arc<AtomicU64>,
    // Records (received / valid / rejected-by-reason)
    records_received: Arc<AtomicU64>,
    records_valid: Arc<AtomicU64>,
    rejected_oversized: Arc<AtomicU64>,
    rejected_stale: Arc<AtomicU64>,
    rejected_future: Arc<AtomicU64>,
    rejected_decode: Arc<AtomicU64>,
    rejected_identity: Arc<AtomicU64>,
    rejected_signature: Arc<AtomicU64>,
    rejected_self: Arc<AtomicU64>,
    rejected_duplicate: Arc<AtomicU64>,
    // Candidates / queue
    unique_candidates: Arc<AtomicU64>,
    queued: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    // Join
    join_attempts: Arc<AtomicU64>,
    join_successes: Arc<AtomicU64>,
    // Degradation
    degraded_transitions: Arc<AtomicU64>,
    // Timing (wall-clock unix ms; 0 = never yet)
    first_candidate_at_ms: Arc<AtomicU64>,
    first_neighbour_at_ms: Arc<AtomicU64>,
}

/// Count the number of unix-epoch milliseconds right now.
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl DhtCounters {
    /// Create an isolated counter set (tests use this; production shares
    /// the global [`DHT_COUNTERS`] via `Clone`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one completed lookup (`encrypted` records fetched from the
    /// backend) together with the per-reason disposition of the validation
    /// pass.  Bumps `lookup_cycles`, `records_received`, `records_valid`, and
    /// each rejected-by-reason counter.
    pub fn record_lookup(&self, received_encrypted: u64, counts: DhtLookupCounts) {
        self.lookup_cycles.fetch_add(1, Ordering::Relaxed);
        self.records_received
            .fetch_add(received_encrypted, Ordering::Relaxed);
        self.records_valid
            .fetch_add(counts.valid, Ordering::Relaxed);
        self.rejected_oversized
            .fetch_add(counts.oversized, Ordering::Relaxed);
        self.rejected_stale
            .fetch_add(counts.stale, Ordering::Relaxed);
        self.rejected_future
            .fetch_add(counts.future, Ordering::Relaxed);
        self.rejected_decode
            .fetch_add(counts.decode, Ordering::Relaxed);
        self.rejected_identity
            .fetch_add(counts.identity, Ordering::Relaxed);
        self.rejected_signature
            .fetch_add(counts.signature, Ordering::Relaxed);
        self.rejected_self.fetch_add(counts.own, Ordering::Relaxed);
        self.rejected_duplicate
            .fetch_add(counts.duplicate, Ordering::Relaxed);
    }

    /// A lookup failed at the backend level (the whole cycle errored).
    pub fn record_lookup_failure(&self) {
        self.lookup_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Record `n` *new* unique candidates admitted at handoff (after rolling
    /// admission).  Sets the time-to-first-candidate on the first one.
    pub fn record_unique_candidates(&self, n: u64) {
        if n == 0 {
            return;
        }
        self.unique_candidates.fetch_add(n, Ordering::Relaxed);
        self.first_candidate_at_ms
            .compare_exchange(0, now_unix_ms(), Ordering::Relaxed, Ordering::Relaxed)
            .ok();
    }

    /// One candidate entered the bounded pending join queue.
    pub fn record_queued(&self, n: u64) {
        self.queued.fetch_add(n, Ordering::Relaxed);
    }

    /// One candidate was rejected at queue overflow and dropped.
    pub fn record_dropped(&self, n: u64) {
        self.dropped.fetch_add(n, Ordering::Relaxed);
    }

    /// One join attempt (a `join_peers` call) was issued.
    pub fn record_join_attempt(&self, n: u64) {
        self.join_attempts.fetch_add(n, Ordering::Relaxed);
    }

    /// One join attempt succeeded (peer added to the gossip mesh).
    pub fn record_join_success(&self, n: u64) {
        self.join_successes.fetch_add(n, Ordering::Relaxed);
    }

    /// A DHT loop transitioned from healthy to degraded (start of a new
    /// consecutive-failure streak).  The caller decides when a streak begins.
    pub fn record_degraded_transition(&self) {
        self.degraded_transitions.fetch_add(1, Ordering::Relaxed);
    }

    /// A peer actually became a gossip neighbour.  Sets the
    /// time-to-first-neighbour on the first one.
    pub fn record_neighbour_up(&self) {
        self.first_neighbour_at_ms
            .compare_exchange(0, now_unix_ms(), Ordering::Relaxed, Ordering::Relaxed)
            .ok();
    }

    /// Point-in-time snapshot of all DHT counters, shaped as the concise
    /// pipeline `lookup -> valid -> new -> queued -> join attempts ->
    /// successful neighbours` for MCP/doctor tooling.
    pub fn snapshot(&self) -> DhtEffectivenessSnapshot {
        DhtEffectivenessSnapshot {
            lookup_cycles: self.lookup_cycles.load(Ordering::Relaxed),
            lookup_failures: self.lookup_failures.load(Ordering::Relaxed),
            records_received: self.records_received.load(Ordering::Relaxed),
            records_valid: self.records_valid.load(Ordering::Relaxed),
            rejected_oversized: self.rejected_oversized.load(Ordering::Relaxed),
            rejected_stale: self.rejected_stale.load(Ordering::Relaxed),
            rejected_future: self.rejected_future.load(Ordering::Relaxed),
            rejected_decode: self.rejected_decode.load(Ordering::Relaxed),
            rejected_identity: self.rejected_identity.load(Ordering::Relaxed),
            rejected_signature: self.rejected_signature.load(Ordering::Relaxed),
            rejected_self: self.rejected_self.load(Ordering::Relaxed),
            rejected_duplicate: self.rejected_duplicate.load(Ordering::Relaxed),
            unique_candidates: self.unique_candidates.load(Ordering::Relaxed),
            queued: self.queued.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            join_attempts: self.join_attempts.load(Ordering::Relaxed),
            join_successes: self.join_successes.load(Ordering::Relaxed),
            degraded_transitions: self.degraded_transitions.load(Ordering::Relaxed),
            first_candidate_at_ms: self.first_candidate_at_ms.load(Ordering::Relaxed),
            first_neighbour_at_ms: self.first_neighbour_at_ms.load(Ordering::Relaxed),
            timestamp: Utc::now(),
        }
    }
}

/// Concise, serializable snapshot of the DHT effectiveness pipeline
/// (`lookup -> valid -> new -> queued -> join attempts -> successful
/// neighbours`), suitable for MCP/doctor tooling.
///
/// Contains only counts and timestamps — never chat contents, secret keys,
/// private-room secrets, or full peer identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DhtEffectivenessSnapshot {
    /// Completed DHT lookup operations (each backend lookup).
    pub lookup_cycles: u64,
    /// Lookups that errored at the backend level.
    pub lookup_failures: u64,
    /// Encrypted records returned by the backend across all lookups.
    pub records_received: u64,
    /// Records that passed validation (unique `EndpointId`s).
    pub records_valid: u64,
    /// Records rejected: oversize limit.
    pub rejected_oversized: u64,
    /// Records rejected: too stale.
    pub rejected_stale: u64,
    /// Records rejected: too far in the future.
    pub rejected_future: u64,
    /// Records rejected: decode failure.
    pub rejected_decode: u64,
    /// Records rejected: identity mismatch.
    pub rejected_identity: u64,
    /// Records rejected: invalid signature.
    pub rejected_signature: u64,
    /// Records rejected: self-filtered.
    pub rejected_self: u64,
    /// Records rejected: duplicate `EndpointId`.
    pub rejected_duplicate: u64,
    /// New unique candidates admitted at handoff (forwarded toward the queue).
    pub unique_candidates: u64,
    /// Candidates entered the bounded pending join queue.
    pub queued: u64,
    /// Candidates dropped at queue overflow.
    pub dropped: u64,
    /// Join attempts issued (`join_peers` calls).
    pub join_attempts: u64,
    /// Join attempts that succeeded (peer added to the mesh).
    pub join_successes: u64,
    /// Number of healthy->degraded transitions across DHT loops.
    pub degraded_transitions: u64,
    /// Wall-clock ms of the first admitted candidate (0 = never yet).
    pub first_candidate_at_ms: u64,
    /// Wall-clock ms of the first gossip neighbour (0 = never yet).
    pub first_neighbour_at_ms: u64,
    /// Wall-clock time this snapshot was taken.
    pub timestamp: DateTime<Utc>,
}

/// Global atomic counters for DHT effectiveness diagnostics (BORU-DHT-08).
///
/// Lazily initialised on first access. Clones share the same underlying
/// atomics, so the DHT trackers, the join path, tests, and the MCP layer all
/// observe the same values without a lock.
pub static DHT_COUNTERS: LazyLock<DhtCounters> = LazyLock::new(DhtCounters::new);

/// Convenience accessor returning the current global DHT effectiveness
/// snapshot (for MCP/doctor tooling).
pub fn dht_effectiveness_snapshot() -> DhtEffectivenessSnapshot {
    DHT_COUNTERS.snapshot()
}
