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
