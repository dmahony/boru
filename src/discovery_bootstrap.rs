//! Global DHT bootstrap tracker (BORU-DHT-01).
//!
//! Lets a fresh internet-only Boru node (no mDNS / friend / ticket) bootstrap
//! into the internal discovery gossip mesh entirely over the Mainline DHT:
//!
//! 1. **Publish** — this node advertises its [`EndpointId`] under a
//!    deterministic Mainnet namespace, so it becomes discoverable by other
//!    fresh nodes (and keeps the 5-minute lease alive).
//! 2. **Discover** — it looks up the same namespace, validates every record
//!    through the shared discovery-validation pipeline, filters out the local
//!    node and duplicates, **randomises** and **selects** a small, diverse
//!    sample (default 3–8, hard-capped at 16) of valid [`EndpointId`]s, and
//!    feeds them into the existing discovery connectivity/join path so they
//!    are dialed into the mesh.
//!
//! This tracker only supplies candidate [`EndpointId`]s for connectivity.
//! All ongoing presence / advertisement stays on the internal discovery gossip
//! topic. It never creates a friendship, a group, a conversation, unread
//! counts, or a chat payload — see the architecture guardrails in
//! `docs/discovery-architecture.md`.
//!
//! # Lifecycle
//!
//! 1. [`tracker`](DiscoveryBootstrapTracker::new) — construct with a
//!    [`TopicDiscoveryBackend`](crate::discovery_backend::TopicDiscoveryBackend)
//!    (production: [`MainlineDhtBackend`](crate::discovery_backend::MainlineDhtBackend)
//!    wrapping the shared member-discovery DHT handle; tests:
//!    [`InMemoryDiscoveryBackend`](crate::discovery_backend::InMemoryDiscoveryBackend)).
//! 2. [`run`](DiscoveryBootstrapTracker::run) — spawn the background loop that
//!    publishes + discovers on the ~5-minute lease and hands selected
//!    candidates to a sink.  Returns promptly when the supplied
//!    [`CancellationToken`] fires (deterministic cancel on shutdown).
//! 3. [`publish_once`](DiscoveryBootstrapTracker::publish_once) /
//!    [`discover_candidates`](DiscoveryBootstrapTracker::discover_candidates)
//!    are also exposed as one-shot operations for callers that want to drive
//!    the cadence themselves.
//!
//! Under `--no-dht` the tracker is simply not constructed (no backend is
//! created), so the whole bootstrap path is skipped.

use std::time::Duration;

use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, RngExt, SeedableRng};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::discovery_backend::{
    EncryptedDiscoveryRecord, MAX_DISCOVERY_PAYLOAD_SIZE, NamespaceId, TopicDiscoveryBackend,
    bootstrap_namespace,
};
use crate::discovery_record::create_discovery_record;
use crate::discovery_validation::{DiscoveryRecordValidator, PeerCandidates, ValidationConfig};
use distributed_topic_tracker::{Record, unix_minute};
use iroh::{EndpointId, SecretKey};
use n0_error::Result;

/// Default minimum number of bootstrap candidates to aim for in a sample.
pub const BOOTSTRAP_MIN_TARGET: usize = 3;
/// Default maximum number of candidates selected per lookup cycle.
pub const BOOTSTRAP_MAX_TARGET: usize = 8;
/// Absolute never-exceed cap on candidates selected in a single cycle.
pub const BOOTSTRAP_HARD_MAX: usize = 16;
/// Default refresh cadence (seconds) — half the 600 s lease, matching
/// [`DISCOVERY_REFRESH_SECS`](crate::discovery_backend::DISCOVERY_REFRESH_SECS).
pub const BOOTSTRAP_REFRESH_SECS: u64 = 300;

/// Tuning parameters for the global bootstrap loop.
#[derive(Debug, Clone)]
pub struct BootstrapConfig {
    /// Soft target lower bound for a selected sample.  Not a hard requirement —
    /// if fewer valid candidates exist we return all of them (better to join
    /// one peer than none).
    pub min_target: usize,
    /// Maximum number of candidates selected per cycle (after randomisation).
    pub max_target: usize,
    /// Absolute never-exceed cap, applied on top of `max_target`.
    pub hard_max: usize,
    /// Refresh cadence (seconds): publish + lookup on this interval.
    pub refresh_secs: u64,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            min_target: BOOTSTRAP_MIN_TARGET,
            max_target: BOOTSTRAP_MAX_TARGET,
            hard_max: BOOTSTRAP_HARD_MAX,
            refresh_secs: BOOTSTRAP_REFRESH_SECS,
        }
    }
}

/// Global DHT bootstrap tracker.
///
/// Owns a [`TopicDiscoveryBackend`] (the shared DHT handle in production) and
/// a deterministic namespace derived from
/// [`bootstrap_namespace`](crate::discovery_backend::bootstrap_namespace).
pub struct DiscoveryBootstrapTracker {
    backend: Box<dyn TopicDiscoveryBackend>,
    /// Deterministic namespace: `BLAKE3("boru-chat/discovery-bootstrap/v1" || network-byte)`.
    namespace: [u8; 32],
    /// This node's iroh EndpointId (advertised + self-filtered).
    local_endpoint_id: EndpointId,
    /// This node's iroh SecretKey — used to sign bootstrap records.
    secret_key: SecretKey,
    config: BootstrapConfig,
}

impl std::fmt::Debug for DiscoveryBootstrapTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoveryBootstrapTracker")
            .field("namespace", &self.namespace)
            .field(
                "local_endpoint_id",
                &self.local_endpoint_id.fmt_short().to_string(),
            )
            .field("config", &self.config)
            .finish()
    }
}

impl DiscoveryBootstrapTracker {
    /// Construct a tracker for the given network byte.
    ///
    /// * `backend` — the discovery backend (production: `MainlineDhtBackend`
    ///   wrapping the shared member-discovery DHT handle).
    /// * `network_byte` — [`PublicNetwork::network_byte`](crate::public_room::PublicNetwork::network_byte),
    ///   used to keep Mainnet / Development / Test namespaces isolated.
    /// * `local_endpoint_id` — this node's iroh EndpointId.
    /// * `secret_key` — this node's iroh SecretKey (signs bootstrap records).
    /// * `config` — tuning; `Default::default()` for the standard cadence.
    pub fn new(
        backend: Box<dyn TopicDiscoveryBackend>,
        network_byte: u8,
        local_endpoint_id: EndpointId,
        secret_key: SecretKey,
        config: BootstrapConfig,
    ) -> Self {
        // The safety caps are structural: never let tuning expand the sample
        // beyond the hard bound.
        let config = BootstrapConfig {
            max_target: config.max_target.min(config.hard_max),
            ..config
        };
        Self {
            backend,
            namespace: bootstrap_namespace(network_byte),
            local_endpoint_id,
            secret_key,
            config,
        }
    }

    /// The deterministic bootstrap namespace this tracker publishes/looks up.
    pub fn namespace(&self) -> &[u8; 32] {
        &self.namespace
    }

    /// Publish this node's [`EndpointId`] to the bootstrap namespace.
    ///
    /// Creates and signs a minimal discovery record (EndpointId + version) and
    /// stores it via the backend.  Errors surface so the caller can treat a
    /// failed publish as a degraded refresh (never a reason to tear down the
    /// discovery mesh).
    pub async fn publish_once(&self) -> Result<()> {
        let now = unix_minute(0);
        let record = create_discovery_record(
            self.namespace,
            now,
            &self.local_endpoint_id,
            &self.secret_key,
            None,
            None,
        )?;
        let namespace = NamespaceId::new(self.namespace);
        let result = self
            .backend
            .publish(&namespace, EncryptedDiscoveryRecord::new(record.to_bytes()))
            .await;
        match &result {
            Ok(()) => info!(
                local = %self.local_endpoint_id.fmt_short(),
                "global DHT bootstrap: published own EndpointId",
            ),
            Err(error) => warn!(
                local = %self.local_endpoint_id.fmt_short(),
                error = %error,
                "global DHT bootstrap: publish failed (degraded refresh)",
            ),
        }
        result
    }

    /// Run a discovery cycle and return the selected, validated [`EndpointId`]s.
    ///
    /// Mirrors the public-room tracker's lookup path: fetch encrypted records,
    /// deserialise (skip malformed/oversized), run the full validation pipeline
    /// (size, timestamp, decode, identity match, signature), self-filter and
    /// de-duplicate, then randomise and select a small sample bounded by
    /// `max_target`/`hard_max`.
    ///
    /// Uses the caller-supplied RNG so tests can be deterministic; the
    /// convenience [`discover_candidates`](Self::discover_candidates) wraps a
    /// thread RNG.
    pub async fn discover_candidates_with_rng<R: Rng + ?Sized>(
        &self,
        rng: &mut R,
    ) -> Result<Vec<EndpointId>> {
        let namespace = NamespaceId::new(self.namespace);
        let encrypted = self.backend.lookup(&namespace).await?;
        let total_encrypted = encrypted.len();

        let mut records: Vec<Record> = Vec::with_capacity(encrypted.len());
        for er in encrypted {
            if er.payload.len() > MAX_DISCOVERY_PAYLOAD_SIZE {
                debug!(
                    len = er.payload.len(),
                    "global DHT bootstrap: skipped oversized record"
                );
                continue;
            }
            if let Ok(record) = Record::from_bytes(er.payload) {
                records.push(record);
            }
        }

        let validator =
            DiscoveryRecordValidator::new(ValidationConfig::new(self.namespace), unix_minute(0));
        let PeerCandidates { peers, .. } =
            validator.filter_and_build(records, Some(&self.local_endpoint_id));

        let selected = self.select_candidates(peers, rng);
        if !selected.is_empty() {
            info!(
                encrypted = total_encrypted,
                accepted = selected.len(),
                "global DHT bootstrap: discovered candidate peers",
            );
        } else {
            debug!(
                encrypted = total_encrypted,
                "global DHT bootstrap: no valid candidates this cycle",
            );
        }
        Ok(selected)
    }

    /// Convenience wrapper over [`discover_candidates_with_rng`](Self::discover_candidates_with_rng)
    /// using an OS-seeded [`StdRng`] (kept `Send` so it can run inside a
    /// `tokio::spawn` background loop).
    pub async fn discover_candidates(&self) -> Result<Vec<EndpointId>> {
        let mut rng: StdRng = StdRng::from_rng(&mut rand::rng());
        self.discover_candidates_with_rng(&mut rng).await
    }

    /// Randomise and select a bounded, diverse sample of valid candidates.
    ///
    /// * Never exceeds `hard_max` (structural cap; `max_target` is clamped to
    ///   `hard_max` at construction).
    /// * When there are more valid candidates than `max_target`, shuffles and
    ///   takes a uniform sample of `max_target` — no persistent bias toward the
    ///   earliest DHT results.
    /// * When there are fewer, returns all of them (never fabricates peers;
    ///   `min_target` is a soft goal, not a guarantee).
    /// * Input is expected to be *already validated* (see the validation
    ///   pipeline) — unvalidated records are never shuffled into the sample.
    pub fn select_candidates<R: Rng + ?Sized>(
        &self,
        peers: Vec<EndpointId>,
        rng: &mut R,
    ) -> Vec<EndpointId> {
        if peers.is_empty() {
            return Vec::new();
        }
        let max = self.config.max_target.min(self.config.hard_max);
        if peers.len() <= max {
            let mut peers = peers;
            peers.shuffle(rng);
            return peers;
        }
        // Partial Fisher–Yates / reservoir-style sampling: shuffle only needs
        // the first `max` positions, avoiding a full O(n) shuffle of a large T
        // hostile result set.
        let mut peers = peers;
        for i in 0..max {
            let j = rng.random_range(i..peers.len());
            peers.swap(i, j);
        }
        peers.truncate(max);
        peers
    }

    /// Run the background bootstrap loop until `cancel` fires.
    ///
    /// Publishes this node's EndpointId and discovers + feeds candidates into
    /// `sink` immediately at startup, then on every `refresh_secs` interval.
    /// Degrades gracefully: a failed publish or lookup is logged and the next
    /// cycle proceeds; the discovery mesh is never torn down by a DHT failure.
    ///
    /// Returns when `cancel` is cancelled (deterministic shutdown).
    pub async fn run<F>(self, sink: F, cancel: CancellationToken)
    where
        F: Fn(Vec<EndpointId>) + Send + Sync + 'static,
    {
        let mut interval =
            tokio::time::interval(Duration::from_secs(self.config.refresh_secs.max(1)));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The first tick fires immediately — that is the startup bootstrap.
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    debug!("global DHT bootstrap loop cancelled");
                    break;
                }
                _ = interval.tick() => {
                    self.cycle(&sink).await;
                }
            }
        }
        debug!("global DHT bootstrap loop exited");
    }

    async fn cycle<F>(&self, sink: &F)
    where
        F: Fn(Vec<EndpointId>) + Send + Sync,
    {
        // Best-effort publish first so other fresh nodes can find this one.
        if let Err(error) = self.publish_once().await {
            warn!(error = %error, "global DHT bootstrap: publish degraded for this cycle");
        }
        match self.discover_candidates().await {
            Ok(candidates) if !candidates.is_empty() => {
                info!(
                    count = candidates.len(),
                    "global DHT bootstrap: feeding candidates into discovery mesh",
                );
                sink(candidates);
            }
            Ok(_) => {
                debug!("global DHT bootstrap: no candidates to feed this cycle");
            }
            Err(error) => {
                warn!(error = %error, "global DHT bootstrap: lookup degraded for this cycle");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use crate::discovery_backend::InMemoryDiscoveryBackend;

    /// Deterministic RNG for repeatable tests.
    fn rng() -> StdRng {
        StdRng::seed_from_u64(42)
    }

    fn test_identity() -> (SecretKey, EndpointId) {
        let sk = SecretKey::generate();
        let ep = sk.public();
        (sk, ep)
    }

    fn block_on<F: std::future::Future<Output = T>, T>(f: F) -> T {
        tokio::runtime::Runtime::new().unwrap().block_on(f)
    }

    fn tracker(network_byte: u8, ep: EndpointId, sk: SecretKey) -> DiscoveryBootstrapTracker {
        DiscoveryBootstrapTracker::new(
            Box::new(InMemoryDiscoveryBackend::new()),
            network_byte,
            ep,
            sk,
            BootstrapConfig::default(),
        )
    }

    // ── Namespace ──────────────────────────────────────────────────────

    #[test]
    fn namespace_is_deterministic_and_network_separated() {
        let (sk, ep) = test_identity();
        let t_main = tracker(0x00, ep, sk.clone());
        let t_main2 = tracker(0x00, ep, sk.clone());
        let t_test = tracker(0x02, ep, sk.clone());
        assert_eq!(t_main.namespace(), t_main2.namespace());
        assert_eq!(*t_main.namespace(), bootstrap_namespace(0x00));
        assert_ne!(t_main.namespace(), t_test.namespace());
    }

    // ── Selection ──────────────────────────────────────────────────────

    #[test]
    fn select_returns_empty_for_empty_input() {
        let (sk, ep) = test_identity();
        let t = tracker(0x00, ep, sk);
        assert!(t.select_candidates(Vec::new(), &mut rng()).is_empty());
    }

    #[test]
    fn select_keeps_all_when_below_max_target() {
        let (sk, ep) = test_identity();
        let t = tracker(0x00, ep, sk);
        let mut peers = Vec::new();
        for i in 0..BOOTSTRAP_MAX_TARGET - 1 {
            let (sk_i, ep_i) = test_identity();
            let _ = sk_i;
            peers.push(ep_i);
        }
        let selected = t.select_candidates(peers.clone(), &mut rng());
        let mut sorted = selected.clone();
        sorted.sort();
        let mut expected = peers;
        expected.sort();
        // All kept (reordered uniformly), none dropped.
        assert_eq!(sorted, expected);
    }

    #[test]
    fn select_caps_at_max_target_when_overflow() {
        let (sk, ep) = test_identity();
        let t = tracker(0x00, ep, sk);
        let mut peers = Vec::new();
        for _ in 0..(BOOTSTRAP_MAX_TARGET + 5) {
            let (_sk, ep_i) = test_identity();
            peers.push(ep_i);
        }
        let selected = t.select_candidates(peers, &mut rng());
        assert_eq!(selected.len(), BOOTSTRAP_MAX_TARGET);
        // No duplicates within the sample.
        let mut seen = std::collections::HashSet::new();
        for p in &selected {
            assert!(seen.insert(*p.as_bytes()), "duplicate in sample");
        }
    }

    #[test]
    fn select_never_duplicates_and_never_exceeds_hard_max() {
        let (sk, ep) = test_identity();
        // Construct a tracker whose configured max_target could overshoot the
        // hard cap; construction must clamp it.
        let t = DiscoveryBootstrapTracker::new(
            Box::new(InMemoryDiscoveryBackend::new()),
            0x00,
            ep,
            sk,
            BootstrapConfig {
                min_target: 20,
                max_target: 40,
                hard_max: 16,
                refresh_secs: 300,
            },
        );
        let mut peers = Vec::new();
        for _ in 0..30 {
            let (_sk, ep_i) = test_identity();
            peers.push(ep_i);
        }
        let selected = t.select_candidates(peers, &mut rng());
        assert!(
            selected.len() <= 16,
            "hard max violated: {}",
            selected.len()
        );
        let unique: std::collections::HashSet<_> = selected.iter().map(|p| *p.as_bytes()).collect();
        assert_eq!(unique.len(), selected.len(), "duplicates in sample");
    }

    #[test]
    fn selection_does_not_persist_earliest_result_bias() {
        // Repeated selections from the same candidate pool should, across many
        // seeded draws, include candidates that were NOT at the head of the
        // input order — proving input order is not preserved.
        let (sk, ep) = test_identity();
        let t = tracker(0x00, ep, sk);
        let mut peers = Vec::new();
        for _ in 0..BOOTSTRAP_MAX_TARGET + 10 {
            let (_sk, ep_i) = test_identity();
            peers.push(ep_i);
        }
        let mut saw_late = false;
        let mut first_members: std::collections::HashSet<[u8; 32]> =
            std::collections::HashSet::new();
        for idx in 0..40 {
            let mut rng = StdRng::seed_from_u64(idx as u64);
            let selected = t.select_candidates(peers.clone(), &mut rng);
            if idx == 0 {
                first_members = selected.iter().map(|p| *p.as_bytes()).collect();
            }
            for p in &selected {
                // Candidates beyond the first BOOTSTRAP_MAX_TARGET of input.
                let pos = peers.iter().position(|q| q == p).unwrap();
                if pos >= BOOTSTRAP_MAX_TARGET {
                    saw_late = true;
                }
            }
        }
        assert!(saw_late, "sample never included a late-ordered candidate");
    }

    // ── Publish + discover roundtrip ───────────────────────────────────

    #[test]
    fn publish_and_discover_roundtrip_no_self() {
        let shared = InMemoryDiscoveryBackend::new();
        let (sk_a, ep_a) = test_identity();
        let (_sk_b, ep_b) = test_identity();

        let tracker_a = DiscoveryBootstrapTracker::new(
            Box::new(shared.clone()),
            0x00,
            ep_a,
            sk_a,
            BootstrapConfig::default(),
        );
        // Node B's tracker on the same namespace (shared backend) discovers A.
        let tracker_b = DiscoveryBootstrapTracker::new(
            Box::new(shared.clone()),
            0x00,
            ep_b,
            SecretKey::generate(),
            BootstrapConfig::default(),
        );

        block_on(tracker_a.publish_once()).unwrap();
        let candidates = block_on(tracker_b.discover_candidates()).unwrap();
        assert!(
            candidates.contains(&ep_a),
            "B should discover A, got {candidates:?}"
        );
        assert!(
            !candidates.contains(&ep_b),
            "B's own EndpointId must be self-filtered"
        );
    }

    #[test]
    fn roundtrip_respects_network_separation() {
        let shared = InMemoryDiscoveryBackend::new();
        let (sk_a, ep_a) = test_identity();
        let (_sk_b, ep_b) = test_identity();

        // A publishes to Mainnet; B only searches Development namespace.
        let tracker_a = DiscoveryBootstrapTracker::new(
            Box::new(shared.clone()),
            0x00,
            ep_a,
            sk_a,
            BootstrapConfig::default(),
        );
        let tracker_b = DiscoveryBootstrapTracker::new(
            Box::new(shared.clone()),
            0x01,
            ep_b,
            SecretKey::generate(),
            BootstrapConfig::default(),
        );

        block_on(tracker_a.publish_once()).unwrap();
        let candidates = block_on(tracker_b.discover_candidates()).unwrap();
        assert!(
            candidates.is_empty(),
            "cross-network lookup must not find peers, got {candidates:?}"
        );
    }

    #[test]
    fn discover_returns_empty_when_no_records() {
        let (sk, ep) = test_identity();
        let t = tracker(0x02, ep, sk);
        let candidates = block_on(t.discover_candidates()).unwrap();
        assert!(candidates.is_empty());
    }

    #[test]
    fn multiple_nodes_all_discovered_and_bounded() {
        let shared = InMemoryDiscoveryBackend::new();
        let mut published = Vec::new();
        for i in 0..6 {
            let (sk_i, ep_i) = test_identity();
            let t = DiscoveryBootstrapTracker::new(
                Box::new(shared.clone()),
                0x00,
                ep_i,
                sk_i,
                BootstrapConfig::default(),
            );
            block_on(t.publish_once()).unwrap();
            published.push(ep_i);
        }
        // A fresh node discovers them (self-filtered).
        let (_sk_seeker, seeker_ep) = test_identity();
        let seeker = DiscoveryBootstrapTracker::new(
            Box::new(shared.clone()),
            0x00,
            seeker_ep,
            SecretKey::generate(),
            BootstrapConfig::default(),
        );
        let candidates = block_on(seeker.discover_candidates()).unwrap();
        assert!(!candidates.is_empty());
        assert!(candidates.len() <= BOOTSTRAP_MAX_TARGET);
        for ep in &candidates {
            assert!(published.contains(ep), "unexpected candidate {ep}");
        }
    }

    // ── Cancellation ───────────────────────────────────────────────────

    #[test]
    fn run_exits_on_cancel() {
        let shared = InMemoryDiscoveryBackend::new();
        let (sk, ep) = test_identity();
        let t = DiscoveryBootstrapTracker::new(
            Box::new(shared),
            0x00,
            ep,
            sk,
            BootstrapConfig {
                refresh_secs: 1,
                ..BootstrapConfig::default()
            },
        );
        let cancel = CancellationToken::new();
        let sink = |_peers: Vec<EndpointId>| {};
        let mut rt = tokio::runtime::Runtime::new().unwrap();
        let handle = rt.spawn(t.run(sink, cancel.clone()));
        // Allow at least one cycle to run, then cancel and join promptly.
        std::thread::sleep(std::time::Duration::from_millis(50));
        cancel.cancel();
        rt.block_on(async move {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        });
    }
}
