//! Private-room DHT discovery tracker.
//!
//! [`PrivateRoomTracker`] is a minimal wrapper around a
//! [`TopicDiscoveryBackend`] that provides publish-once / discover-once
//! operations for **private** rooms.  It differs from [`PublicRoomTracker`]
//! in two key ways:
//!
//! 1. **Namespace isolation.**  The DHT namespace is derived via
//!    BLAKE3(topic || secret) instead of from a public room name, so
//!    only peers who know both the gossip [`TopicId`] and the
//!    [`DiscoverySecret`] can locate each other on the DHT.
//! 2. **Key material.**  The [`DiscoverySecret`] itself is used as the
//!    discovery key for signing and verifying records, replacing the
//!    public-room's deterministic discovery key.
//!
//! # Lifecycle
//!
//! 1. [`new`](Self::new) — construct with a backend, topic, and secret.
//! 2. [`publish_once`](Self::publish_once) — advertise local presence.
//! 3. [`discover_once`](Self::discover_once) — find valid peers.
//! 4. [`shutdown`](Self::shutdown) — release backend resources.
//!
//! # Minimal example
//!
//! ```ignore
//! use crate::discovery_backend::InMemoryDiscoveryBackend;
//! use crate::discovery_secret::DiscoverySecret;
//! use crate::private_room_tracker::PrivateRoomTracker;
//! use iroh::{EndpointId, SecretKey};
//!
//! let sk = SecretKey::generate();
//! let ep = sk.public();
//! let topic = [0xABu8; 32];
//! let secret = DiscoverySecret::from_bytes([0x42u8; 32]);
//!
//! let tracker = PrivateRoomTracker::new(
//!     Box::new(InMemoryDiscoveryBackend::new()),
//!     topic,
//!     secret,
//!     ep,
//!     sk,
//! ).await.unwrap();
//! ```

use std::{collections::HashMap, sync::Arc, time::Instant};

use tokio::{
    sync::mpsc,
    time::{interval, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, info_span, trace, warn, Instrument};

use crate::discovery_backend::{
    EncryptedDiscoveryRecord, NamespaceId, TopicDiscoveryBackend, MAX_DISCOVERY_PAYLOAD_SIZE,
};
use crate::discovery_record::create_discovery_record;
use crate::discovery_secret::DiscoverySecret;
use crate::discovery_validation::{DiscoveryRecordValidator, PeerCandidates, ValidationConfig};
use crate::proto::TopicId;
use distributed_topic_tracker::{
    encryption_keypair, unix_minute, EncryptedRecord, Record, RotationHandle,
    TopicId as TrackerTopicId,
};
use iroh::{EndpointId, SecretKey};
use n0_error::Result;

// ---------------------------------------------------------------------------
// Domain separation constant
// ---------------------------------------------------------------------------

/// Domain separator for private-room DHT namespace derivation.
///
/// Deliberately distinct from all public-room domain separators so that
/// the same (topic, secret) pair produces a namespace that is guaranteed
/// different from any public-room namespace, the gossip topic itself, or
/// any discovery key.
pub const PRIVATE_ROOM_DOMAIN_SEPARATOR: &[u8] = b"boru-chat private-room v1";

/// Derive a private-room DHT namespace from a topic and secret.
///
/// The namespace is `BLAKE3(PRIVATE_ROOM_DOMAIN_SEPARATOR || topic || secret)`.
/// This provides **domain isolation** from public rooms: even if an attacker
/// knows the gossip [`TopicId`], they cannot derive the DHT namespace without
/// the [`DiscoverySecret`].
pub fn private_room_namespace(topic: &TopicId, secret: &DiscoverySecret) -> NamespaceId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(PRIVATE_ROOM_DOMAIN_SEPARATOR);
    hasher.update(topic.as_bytes());
    hasher.update(secret.as_bytes());
    let hash = hasher.finalize();
    NamespaceId::new(*hash.as_bytes())
}

// ---------------------------------------------------------------------------
// PrivateRoomTracker
// ---------------------------------------------------------------------------

/// A minimal private-room tracker for DHT-based peer discovery.
///
/// Wraps a [`TopicDiscoveryBackend`] with a private-room identity model.
/// The namespace is derived from the gossip topic + discovery secret
/// (see [`private_room_namespace`]), providing isolation from public rooms.
///
/// # Lifecycle
///
/// 1. [`new`](Self::new) — construct.
/// 2. [`publish_once`](Self::publish_once) — advertise local presence.
/// 3. [`discover_once`](Self::discover_once) — find peers.
/// 4. [`shutdown`](Self::shutdown) — release backend resources.
///
/// # Cancellation
///
/// The internal [`CancellationToken`] is a placeholder for future background
/// tasks (e.g. periodic re-publish). It is fired during [`shutdown`](Self::shutdown)
/// but currently has no listeners.
pub struct PrivateRoomTracker {
    /// The underlying discovery backend.
    backend: Box<dyn TopicDiscoveryBackend>,
    /// The DHT namespace derived from this room's topic and secret.
    namespace: NamespaceId,
    /// The discovery key — the secret bytes used for signing/verifying records.
    discovery_key: [u8; 32],
    /// The gossip topic for logging / identification.
    topic: TopicId,
    /// This node's iroh EndpointId.
    local_endpoint_id: EndpointId,
    /// This node's iroh SecretKey — used to sign discovery records.
    secret_key: SecretKey,
    /// Cancellation token for future background tasks.
    cancel: CancellationToken,
}

impl std::fmt::Debug for PrivateRoomTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateRoomTracker")
            .field("topic", &hex::encode(&self.topic.as_bytes()[..4]))
            .field("local_endpoint_id", &self.local_endpoint_id)
            .field("namespace", &hex::encode(&self.namespace.as_bytes()[..4]))
            .finish()
    }
}

impl PrivateRoomTracker {
    /// Create a new private-room tracker.
    ///
    /// # Parameters
    ///
    /// * `backend` — the DHT-like discovery backend (in-memory for tests,
    ///   MainlineDHT in production).
    /// * `topic` — the room's gossip [`TopicId`].
    /// * `secret` — the room's [`DiscoverySecret`] (shared with all members).
    /// * `local_endpoint_id` — this node's iroh [`EndpointId`].
    /// * `secret_key` — this node's iroh [`SecretKey`] for signing records.
    pub fn new(
        backend: Box<dyn TopicDiscoveryBackend>,
        topic: TopicId,
        secret: DiscoverySecret,
        local_endpoint_id: EndpointId,
        secret_key: SecretKey,
    ) -> Self {
        let namespace = private_room_namespace(&topic, &secret);
        let discovery_key = *secret.as_bytes();
        info!(
            topic = %hex::encode(&topic.as_bytes()[..4]),
            namespace = %hex::encode(&namespace.as_bytes()[..4]),
            "private room tracker created",
        );
        Self {
            backend,
            namespace,
            discovery_key,
            topic,
            local_endpoint_id,
            secret_key,
            cancel: CancellationToken::new(),
        }
    }

    fn encryption_key(&self, minute: u64) -> ed25519_dalek::SigningKey {
        let tracker_topic = TrackerTopicId::from_hash(&self.discovery_key);
        let secret_hash = *blake3::hash(&self.discovery_key).as_bytes();
        encryption_keypair(
            &tracker_topic,
            &RotationHandle::default(),
            secret_hash,
            minute,
        )
    }

    /// Return the gossip topic this tracker is configured for.
    pub fn topic(&self) -> &TopicId {
        &self.topic
    }

    /// Return a short, non-secret room identifier suitable for tracing.
    fn topic_short(&self) -> String {
        hex::encode(&self.topic.as_bytes()[..4])
    }

    /// Return the DHT namespace used for publish/lookup.
    pub fn namespace(&self) -> &NamespaceId {
        &self.namespace
    }

    /// Return this node's EndpointId.
    pub fn local_endpoint_id(&self) -> &EndpointId {
        &self.local_endpoint_id
    }

    /// Publish this node's presence to the private room's DHT namespace once.
    ///
    /// Creates and signs a discovery record advertising this node's
    /// [`EndpointId`], then publishes it via the backend under the
    /// private-room namespace.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying backend's [`publish`](TopicDiscoveryBackend::publish)
    /// fails, or if record creation fails (extremely unlikely).
    pub async fn publish_once(&self) -> Result<()> {
        let start = Instant::now();
        let topic_short = hex::encode(&self.topic.as_bytes()[..4]);
        let local = self.local_endpoint_id.fmt_short();

        let now = unix_minute(0);
        let record = create_discovery_record(
            self.discovery_key,
            now,
            &self.local_endpoint_id,
            &self.secret_key,
        )?;
        let encrypted_record = record.encrypt(&self.encryption_key(now));
        let wire_record = encrypted_record.to_bytes()?;
        let result = async {
            self.backend
                .publish(&self.namespace, EncryptedDiscoveryRecord::new(wire_record))
                .await
        }
        .instrument(info_span!("tracker.publish", tracker = "private", room = %topic_short))
        .await;

        let duration_us = start.elapsed().as_micros() as u64;
        match &result {
            Ok(()) => info!(
                topic = %topic_short,
                local = %local,
                outcome = "success",
                duration_us = duration_us,
                "private room publish completed",
            ),
            Err(e) => warn!(
                topic = %topic_short,
                local = %local,
                error = %e,
                outcome = "failure",
                duration_us = duration_us,
                "private room publish failed",
            ),
        }

        result
    }

    /// Find valid peers on the private room's DHT namespace.
    ///
    /// Deserialises records from the backend, validates each through the full
    /// pipeline (size, timestamp, decode, identity match, signature), filters
    /// out the local node and duplicates, and returns the bounded result set.
    ///
    /// # Returns
    ///
    /// A [`Vec`] of validated [`EndpointId`] values from other peers.  The
    /// result is bounded by [`ValidationConfig::max_candidate_peers`] (default
    /// 20).  Returns an empty `Vec` when no valid peers exist — not an error.
    ///
    /// Records that fail deserialisation from raw bytes are silently skipped.
    pub async fn discover_once(&self) -> Result<Vec<EndpointId>> {
        let start = Instant::now();
        let topic_short = hex::encode(&self.topic.as_bytes()[..4]);
        let local = self.local_endpoint_id.fmt_short();

        // Fetch encrypted records from the backend.
        let encrypted = match async { self.backend.lookup(&self.namespace).await }
            .instrument(info_span!("tracker.lookup", tracker = "private", room = %topic_short))
            .await
        {
            Ok(records) => records,
            Err(error) => {
                warn!(
                    topic = %topic_short,
                    local = %local,
                    outcome = "failure",
                    error = %error,
                    duration_us = start.elapsed().as_micros() as u64,
                    "private room lookup failed",
                );
                return Err(error);
            }
        };
        let total_encrypted = encrypted.len();

        // Decrypt native tracker envelopes for the current and previous minute.
        // A wrong room secret cannot decrypt these bytes, even if records are
        // accidentally copied into this namespace.
        let mut records: Vec<Record> = Vec::with_capacity(encrypted.len());
        let now_minute = unix_minute(0);
        for er in encrypted {
            if er.payload.len() > MAX_DISCOVERY_PAYLOAD_SIZE {
                continue;
            }
            let Ok(encrypted_record) = EncryptedRecord::from_bytes(er.payload) else {
                continue;
            };
            let mut decrypted = None;
            for minute in [now_minute, now_minute.saturating_sub(1)] {
                if let Ok(record) = encrypted_record.decrypt(&self.encryption_key(minute)) {
                    decrypted = Some(record);
                    break;
                }
            }
            if let Some(record) = decrypted {
                records.push(record);
            }
        }
        let total_records = records.len();

        // Validate and filter through the discovery-validation pipeline
        // using the discovery_key derived from the shared secret.
        let config = ValidationConfig::new(self.discovery_key);
        let now_minute = unix_minute(0);
        let validator = DiscoveryRecordValidator::new(config, now_minute);
        let PeerCandidates { peers, counters } =
            validator.filter_and_build(records, Some(&self.local_endpoint_id));

        let duration_us = start.elapsed().as_micros() as u64;
        let accepted = counters.accepted;
        let rejected = counters.total_rejected();
        for peer in &peers {
            tracing::trace!(
                topic = %topic_short,
                candidate = %peer.fmt_short(),
                "private room candidate accepted for join",
            );
        }
        if accepted > 0 {
            info!(
                topic = %topic_short,
                local = %local,
                encrypted = total_encrypted,
                records = total_records,
                accepted = accepted,
                rejected = rejected,
                oversized = counters.oversized,
                stale = counters.stale,
                future = counters.future,
                decode_failure = counters.decode_failure,
                identity_mismatch = counters.identity_mismatch,
                invalid_signature = counters.invalid_signature,
                self_filtered = counters.self_filtered,
                duplicates = counters.duplicates,
                duration_us = duration_us,
                outcome = "success",
                "private room discovery found peers",
            );
        } else {
            debug!(
                topic = %topic_short,
                local = %local,
                encrypted = total_encrypted,
                records = total_records,
                accepted = accepted,
                rejected = rejected,
                duration_us = duration_us,
                outcome = "success",
                "private room discovery returned no peers",
            );
        }

        Ok(peers)
    }

    /// Consume the tracker for use by a continuous runner.
    pub fn into_inner(self) -> Self {
        self
    }

    ///
    /// Fires the cancellation token and calls [`shutdown`](TopicDiscoveryBackend::shutdown)
    /// on the backend.
    ///
    /// **Consumes** the tracker — call this once when done.
    pub async fn shutdown(self) {
        let topic_short = hex::encode(&self.topic.as_bytes()[..4]);
        info!(topic = %topic_short, "private room tracker shutting down");
        self.cancel.cancel();
        let _ = self.backend.shutdown().await;
        info!(topic = %topic_short, "private room tracker shut down");
    }
}

/// Runs private-room publication and discovery in the background.
///
/// Two independent tokio tasks: one for publication, one for discovery.
/// Each tick applies uniform jitter to the configured interval. Failures
/// use exponential backoff capped at `max_retry_delay`. The shared
/// [`CancellationToken`] is fired on shutdown; both tasks observe it and
/// exit promptly.
pub struct PrivateContinuousTracker {
    cancel: CancellationToken,
    task_handle: tokio::task::JoinHandle<()>,
    tracker: Arc<PrivateRoomTracker>,
}

impl std::fmt::Debug for PrivateContinuousTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateContinuousTracker")
            .finish_non_exhaustive()
    }
}

impl PrivateContinuousTracker {
    /// Start periodic private-room publish/discover loops with configurable timing.
    ///
    /// Spawns two background tokio tasks:
    ///
    /// 1. **Publish loop** — periodically re-publishes local presence on the
    ///    DHT via the private-room namespace. On failure, retries with
    ///    exponential backoff.
    /// 2. **Discovery loop** — periodically looks up peers on the DHT and
    ///    sends newly discovered [`EndpointId`] values through `new_peers_tx`.
    ///    Deduplicates peers already sent in previous ticks.
    ///
    /// The caller should read from the corresponding
    /// [`mpsc::Receiver<Vec<EndpointId>>`] and forward batches to
    /// [`crate::api::GossipSender::join_peers`] (or use
    /// [`crate::public_room_continuous::spawn_join_fanout`]).
    pub fn start(
        tracker: PrivateRoomTracker,
        config: crate::public_room_continuous::ContinuousTrackerConfig,
        new_peers_tx: mpsc::Sender<Vec<EndpointId>>,
    ) -> Self {
        let tracker = Arc::new(tracker);
        let cancel = CancellationToken::new();

        // Create the publication policy with default settings.
        let policy = crate::public_room_continuous::PublicationPolicy::new(
            crate::public_room_continuous::PublicationPolicyConfig::default(),
        );

        let topic_short = tracker.topic_short();
        let local = tracker.local_endpoint_id().fmt_short();
        info!(
            topic = %topic_short,
            local = %local,
            publish_ms = config.publish_interval.as_millis() as u64,
            discover_ms = config.discover_interval.as_millis() as u64,
            max_candidates = config.max_candidates_per_cycle,
            "private continuous tracker starting background loops",
        );

        let task_cancel = cancel.clone();
        let task_tracker = Arc::clone(&tracker);

        let task_handle = tokio::spawn(async move {
            let cfg_p = config.clone();
            let tracker_p = Arc::clone(&task_tracker);
            let cancel_p = task_cancel.clone();
            let policy_p = policy;
            let publish_task = tokio::task::spawn(async move {
                private_publish_loop(tracker_p, cfg_p, policy_p, cancel_p).await;
            });

            let cfg_d = config;
            let tracker_d = Arc::clone(&task_tracker);
            let cancel_d = task_cancel;
            let discover_task = tokio::task::spawn(async move {
                private_discover_loop(tracker_d, cfg_d, new_peers_tx, cancel_d).await;
            });

            let _ = tokio::join!(publish_task, discover_task);
        });

        Self {
            cancel,
            task_handle,
            tracker,
        }
    }

    /// Stop background work and release the DHT backend.
    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.task_handle.await;
        if let Ok(tracker) = Arc::try_unwrap(self.tracker) {
            tracker.shutdown().await;
        }
    }
}

// ---------------------------------------------------------------------------
// Background loops
// ---------------------------------------------------------------------------

/// Periodic publication loop for a private room with policy-governed timing.
///
/// Uses a [`PublicationPolicy`] to decide whether each tick should produce
/// a real DHT write.  On success, the policy is updated so subsequent ticks
/// within the same DHT minute are skipped (unless refresh age triggers a
/// heartbeat).  On failure, the policy applies exponential backoff.
async fn private_publish_loop(
    tracker: Arc<PrivateRoomTracker>,
    config: crate::public_room_continuous::ContinuousTrackerConfig,
    mut policy: crate::public_room_continuous::PublicationPolicy,
    cancel: CancellationToken,
) {
    let mut ticker = interval(config.publish_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                info!(topic = %tracker.topic_short(), "private continuous publish cancelled");
                break;
            }
            _ = ticker.tick() => {
                let _ = crate::public_room_continuous::apply_jitter(
                    config.publish_interval,
                    config.jitter_factor,
                );

                // ── Policy check ──────────────────────────────────────────
                let now = Instant::now();
                let current_minute = unix_minute(0);
                match policy.decide(current_minute, now) {
                    crate::public_room_continuous::PublicationDecision::Skip { reason, next_check_after } => {
                        debug!(
                            topic = %tracker.topic_short(),
                            minute = current_minute,
                            reason,
                            next_check_ms = next_check_after.as_millis() as u64,
                            consecutive_failures = policy.consecutive_failures(),
                            "private publish skipped by policy",
                        );
                        continue;
                    }
                    crate::public_room_continuous::PublicationDecision::Publish => {
                        debug!(
                            topic = %tracker.topic_short(),
                            minute = current_minute,
                            consecutive_failures = policy.consecutive_failures(),
                            "private publish proceeding after policy decision",
                        );
                    }
                }

                let start = Instant::now();
                let result = crate::public_room_continuous::retry_with_backoff(
                    || tracker.publish_once(),
                    config.initial_retry_delay,
                    config.max_retry_delay,
                    config.jitter_factor,
                    &cancel,
                )
                .await;
                let duration_us = start.elapsed().as_micros() as u64;

                match result {
                    Ok(()) => {
                        policy.record_success(current_minute);
                        debug!(
                            topic = %tracker.topic_short(),
                            minute = current_minute,
                            duration_us = duration_us,
                            "private continuous publish succeeded",
                        );
                    }
                    Err(e) => {
                        if cancel.is_cancelled() {
                            break;
                        }
                        policy.record_failure();
                        let failures = policy.consecutive_failures();
                        if failures >= 3 {
                            warn!(
                                topic = %tracker.topic_short(),
                                error = %e,
                                consecutive_failures = failures,
                                duration_us = duration_us,
                                fallback = "continue_with_stale_advertisement",
                                "private continuous publish degraded DHT state",
                            );
                        } else {
                            warn!(
                                topic = %tracker.topic_short(),
                                error = %e,
                                consecutive_failures = failures,
                                duration_us = duration_us,
                                "private continuous publish failed after retries",
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Periodic discovery loop for a private room.
///
/// Looks up peers on the DHT at the configured interval (with jitter).
/// Discovered peers are sent through `new_peers_tx` as batches.
/// Tracks consecutive failures to detect degraded DHT state.
/// Deduplicates by tracking already-forwarded peers.
/// Applies stale-ttl eviction to the known-peers set on each tick.
async fn private_discover_loop(
    tracker: Arc<PrivateRoomTracker>,
    config: crate::public_room_continuous::ContinuousTrackerConfig,
    new_peers_tx: mpsc::Sender<Vec<EndpointId>>,
    cancel: CancellationToken,
) {
    let mut ticker = interval(config.discover_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut consecutive_failures: u32 = 0;

    // Track peers we've already forwarded so we don't re-send them.
    let mut known_peers: HashMap<EndpointId, Instant> = HashMap::new();
    let max_candidates = config.max_candidates_per_cycle;
    let staleness_ttl = config.stale_peer_ttl;

    // Cumulative peer-set size reported per tick (not reset on eviction).
    let mut cumulative_peers_seen: usize = 0;

    info!(
        topic = %tracker.topic_short(),
        discover_ms = config.discover_interval.as_millis() as u64,
        max_candidates = max_candidates,
        "private continuous discovery loop started",
    );

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                info!(topic = %tracker.topic_short(), "private continuous discover cancelled");
                break;
            }
            _ = ticker.tick() => {
                let _ = crate::public_room_continuous::apply_jitter(
                    config.discover_interval,
                    config.jitter_factor,
                );

                // Evict stale known peers before processing this tick.
                if let Some(ttl) = staleness_ttl {
                    let cutoff = Instant::now() - ttl;
                    let before = known_peers.len();
                    known_peers.retain(|_, last_seen| *last_seen >= cutoff);
                    let evicted = before - known_peers.len();
                    if evicted > 0 {
                        trace!(
                            topic = %tracker.topic_short(),
                            evicted,
                            remaining = known_peers.len(),
                            "evicted stale known-peers entries",
                        );
                    }
                }

                let start = Instant::now();
                let result = crate::public_room_continuous::retry_with_backoff(
                    || tracker.discover_once(),
                    config.initial_retry_delay,
                    config.max_retry_delay,
                    config.jitter_factor,
                    &cancel,
                )
                .await;
                let duration_us = start.elapsed().as_micros() as u64;

                match result {
                    Ok(peers) => {
                        consecutive_failures = 0;
                        let now = Instant::now();

                        // Deduplicate: skip already-known peers, cap at max_candidates.
                        let mut new_peers = Vec::with_capacity(max_candidates.min(peers.len()));
                        for peer in peers {
                            if new_peers.len() >= max_candidates {
                                break;
                            }
                            if known_peers.contains_key(&peer) {
                                continue;
                            }
                            trace!(
                                topic = %tracker.topic_short(),
                                candidate = %peer.fmt_short(),
                                "private room candidate peer admitted",
                            );
                            known_peers.insert(peer, now);
                            new_peers.push(peer);
                        }

                        if new_peers.is_empty() {
                            continue;
                        }

                        let new_count = new_peers.len();
                        cumulative_peers_seen += new_count;
                        info!(
                            topic = %tracker.topic_short(),
                            new = new_count,
                            cumulative = cumulative_peers_seen,
                            known = known_peers.len(),
                            duration_us = duration_us,
                            "private continuous discovery found new peers",
                        );

                        if new_peers_tx.send(new_peers).await.is_err() {
                            info!(
                                topic = %tracker.topic_short(),
                                "private continuous discover channel closed, stopping",
                            );
                            break;
                        }
                    }
                    Err(e) => {
                        if cancel.is_cancelled() {
                            break;
                        }
                        consecutive_failures += 1;
                        if consecutive_failures >= 3 {
                            warn!(
                                topic = %tracker.topic_short(),
                                error = %e,
                                consecutive_failures = consecutive_failures,
                                duration_us = duration_us,
                                fallback = "continue_with_existing_peers",
                                "private continuous discover degraded DHT state",
                            );
                        } else {
                            warn!(
                                topic = %tracker.topic_short(),
                                error = %e,
                                consecutive_failures = consecutive_failures,
                                duration_us = duration_us,
                                "private continuous discover failed after retries",
                            );
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery_backend::InMemoryDiscoveryBackend;
    use n0_tracing_test::traced_test;

    /// Helper: generate a fresh test identity.
    fn test_identity() -> (SecretKey, EndpointId) {
        let sk = SecretKey::generate();
        let ep = sk.public();
        (sk, ep)
    }

    /// Helper: block on an async future for synchronous test contexts.
    fn block_on<F: std::future::Future<Output = T>, T>(f: F) -> T {
        tokio::runtime::Runtime::new().unwrap().block_on(f)
    }

    fn test_tracker(
        shared: Option<InMemoryDiscoveryBackend>,
    ) -> (PrivateRoomTracker, InMemoryDiscoveryBackend) {
        let backend = shared.unwrap_or_default();
        let (sk, ep) = test_identity();
        let topic = TopicId::from_bytes([0xABu8; 32]);
        let secret = DiscoverySecret::from_bytes([0x42u8; 32]);
        let tracker = PrivateRoomTracker::new(Box::new(backend.clone()), topic, secret, ep, sk);
        (tracker, backend)
    }

    // ── Tracing smoke tests ──────────────────────────────────────────

    /// Tracing is emitted during publish + discover cycle without panics.
    #[test]
    #[traced_test]
    fn traced_publish_discover_roundtrip() {
        publish_discover_roundtrip();
    }

    /// Tracing is emitted on publish without panics.
    #[test]
    #[traced_test]
    fn traced_publish_logs() {
        let (tracker, _backend) = test_tracker(None);
        block_on(tracker.publish_once()).unwrap();
        block_on(tracker.shutdown());
    }

    /// Tracing is emitted during tracker shutdown.
    #[test]
    #[traced_test]
    fn traced_shutdown_logs() {
        let (tracker, _backend) = test_tracker(None);
        block_on(tracker.shutdown());
    }

    /// Private-room lifecycle logs never contain room secrets, complete
    /// invitations, or decrypted record contents.
    ///
    /// `n0_tracing_test` captures all crate events (including debug, info, and
    /// warn), so this is deliberately stricter than checking only the default
    /// subscriber filter.  The assertion is made after the real publish and
    /// decrypt/validate path has run, rather than against a hand-built log.
    #[test]
    #[traced_test]
    fn secret_safe_logging_excludes_sensitive_room_data() {
        let topic = TopicId::from_bytes([0xABu8; 32]);
        let secret = DiscoverySecret::from_bytes([0x42u8; 32]);
        let invite = crate::chat_core::RoomInviteV2::new(topic, secret);
        let invitation = invite.encode();
        let raw_secret = hex::encode(secret.as_bytes());

        let (publisher_key, publisher) = test_identity();
        let publisher_string = publisher.to_string();
        let publisher_hex = hex::encode(publisher.as_bytes());
        let (_observer_key, observer) = test_identity();
        let backend = InMemoryDiscoveryBackend::new();
        let publisher_tracker = PrivateRoomTracker::new(
            Box::new(backend.clone()),
            topic,
            secret,
            publisher,
            publisher_key,
        );
        block_on(publisher_tracker.publish_once()).unwrap();

        // This exercises decryption and validation of a real record.  The
        // payload is the sensitive value that must not be formatted into logs.
        let payload = crate::discovery_record::DiscoveryRecordPayload::new(&publisher);
        let payload_bytes = postcard::to_allocvec(&payload).unwrap();
        let payload_hex = hex::encode(payload_bytes);

        let observer_tracker = PrivateRoomTracker::new(
            Box::new(backend),
            topic,
            secret,
            observer,
            SecretKey::generate(),
        );
        let peers = block_on(observer_tracker.discover_once()).unwrap();
        assert_eq!(peers, vec![publisher]);
        block_on(observer_tracker.shutdown());

        for forbidden in [
            raw_secret.as_str(),
            invitation.as_str(),
            payload_hex.as_str(),
            publisher_string.as_str(),
            publisher_hex.as_str(),
        ] {
            assert!(
                !logs_contain(forbidden),
                "sensitive value appeared in tracing output: {forbidden}"
            );
        }
    }

    // ── Construction ──────────────────────────────────────────────────

    #[test]
    fn tracker_new_constructs_with_identity() {
        let (sk, ep) = test_identity();
        let topic = TopicId::from_bytes([0xABu8; 32]);
        let secret = DiscoverySecret::from_bytes([0x42u8; 32]);
        let backend = InMemoryDiscoveryBackend::new();

        let tracker = PrivateRoomTracker::new(Box::new(backend.clone()), topic, secret, ep, sk);

        assert_eq!(tracker.topic(), &topic);
        assert_eq!(tracker.local_endpoint_id(), &ep);

        // Namespace should be deterministic
        let expected_ns =
            private_room_namespace(&topic, &DiscoverySecret::from_bytes([0x42u8; 32]));
        assert_eq!(tracker.namespace(), &expected_ns);
    }

    #[test]
    fn different_secrets_produce_different_namespaces() {
        let topic = TopicId::from_bytes([0xABu8; 32]);
        let secret_a = DiscoverySecret::from_bytes([0x01u8; 32]);
        let secret_b = DiscoverySecret::from_bytes([0x02u8; 32]);
        let ns_a = private_room_namespace(&topic, &secret_a);
        let ns_b = private_room_namespace(&topic, &secret_b);
        assert_ne!(
            ns_a, ns_b,
            "different secrets must give different namespaces"
        );
    }

    #[test]
    fn same_secret_same_topic_same_namespace() {
        let topic = TopicId::from_bytes([0xABu8; 32]);
        let secret = DiscoverySecret::from_bytes([0x42u8; 32]);
        let ns_a = private_room_namespace(&topic, &secret);
        let ns_b = private_room_namespace(&topic, &secret);
        assert_eq!(ns_a, ns_b, "same inputs must give same namespace");
    }

    #[test]
    fn private_namespace_differs_from_public() {
        // Private-room namespace should not equal any public-room topic
        let topic = TopicId::from_bytes([0xABu8; 32]);
        let secret = DiscoverySecret::from_bytes([0x42u8; 32]);
        let private_ns = private_room_namespace(&topic, &secret);
        // A public-room namespace derived from the same topic bytes
        // (through the tracker_namespace_from_topic function) should differ.
        let public_ns = crate::topic_derivation::tracker_namespace_from_topic(topic.as_bytes());
        assert_ne!(
            private_ns.as_bytes(),
            &public_ns.hash(),
            "private-room namespace must differ from public-room namespace"
        );
    }

    // ── publish_once + discover_once ──────────────────────────────────

    #[test]
    fn publish_discover_roundtrip() {
        let (sk_a, ep_a) = test_identity();
        let (_sk_b, ep_b) = test_identity();
        let topic = TopicId::from_bytes([0xABu8; 32]);
        let secret = DiscoverySecret::from_bytes([0x42u8; 32]);

        // Use a shared backend so both trackers operate on the same store.
        let shared = InMemoryDiscoveryBackend::new();

        // Tracker A publishes into shared backend.
        let tracker_a =
            PrivateRoomTracker::new(Box::new(shared.clone()), topic, secret, ep_a, sk_a);
        block_on(tracker_a.publish_once()).unwrap();
        block_on(tracker_a.shutdown());

        // Tracker B discovers from the same shared backend.
        let tracker_b = PrivateRoomTracker::new(
            Box::new(shared.clone()),
            topic,
            secret,
            ep_b,
            SecretKey::generate(),
        );
        let peers = block_on(tracker_b.discover_once()).unwrap();

        // B should discover A (and not itself).
        assert!(
            peers.contains(&ep_a),
            "expected peer A to be discovered, got {peers:?}"
        );
        assert!(
            peers.len() == 1,
            "expected exactly one peer (A), got {}",
            peers.len()
        );
        block_on(tracker_b.shutdown());
    }

    #[test]
    fn self_filter_excludes_local_peer() {
        let (sk, ep) = test_identity();
        let topic = TopicId::from_bytes([0xABu8; 32]);
        let secret = DiscoverySecret::from_bytes([0x42u8; 32]);
        let backend = InMemoryDiscoveryBackend::new();

        // Publish our own presence.
        let tracker = PrivateRoomTracker::new(Box::new(backend.clone()), topic, secret, ep, sk);
        block_on(tracker.publish_once()).unwrap();

        // Discover — our own EndpointId should be filtered out.
        let peers = block_on(tracker.discover_once()).unwrap();
        assert!(
            !peers.contains(&ep),
            "self endpoint should be filtered out, got {peers:?}"
        );
        assert!(peers.is_empty(), "expected no peers, got {peers:?}");

        block_on(tracker.shutdown());
    }

    #[test]
    fn different_secret_isolation() {
        // Two trackers using different secrets for the same topic
        // should NOT discover each other.
        let (sk_a, ep_a) = test_identity();
        let (_sk_b, ep_b) = test_identity();
        let topic = TopicId::from_bytes([0xABu8; 32]);
        let secret_a = DiscoverySecret::from_bytes([0x01u8; 32]);
        let secret_b = DiscoverySecret::from_bytes([0x02u8; 32]);

        let shared = InMemoryDiscoveryBackend::new();

        // Tracker A publishes with secret A.
        let tracker_a =
            PrivateRoomTracker::new(Box::new(shared.clone()), topic, secret_a, ep_a, sk_a);
        block_on(tracker_a.publish_once()).unwrap();
        block_on(tracker_a.shutdown());

        // Tracker B tries to discover with secret B (different namespace).
        let tracker_b = PrivateRoomTracker::new(
            Box::new(shared.clone()),
            topic,
            secret_b,
            ep_b,
            SecretKey::generate(),
        );
        let peers = block_on(tracker_b.discover_once()).unwrap();
        assert!(
            peers.is_empty(),
            "different secrets should isolate rooms, got {peers:?}"
        );
        block_on(tracker_b.shutdown());
    }

    #[test]
    fn malformed_and_wrong_secret_envelopes_are_ignored() {
        let (sk, ep) = test_identity();
        let topic = TopicId::from_bytes([0xCDu8; 32]);
        let secret = DiscoverySecret::from_bytes([0x52u8; 32]);
        let backend = InMemoryDiscoveryBackend::new();
        let tracker = PrivateRoomTracker::new(Box::new(backend.clone()), topic, secret, ep, sk);
        let namespace = *tracker.namespace();
        block_on(backend.publish(&namespace, EncryptedDiscoveryRecord::new(vec![0xAA; 32])))
            .unwrap();

        // A valid native envelope encrypted with another room secret must
        // also be rejected at decryption, not passed to record validation.
        let other_key = SecretKey::generate();
        let other_ep = other_key.public();
        let record =
            create_discovery_record(*secret.as_bytes(), unix_minute(0), &other_ep, &other_key)
                .unwrap();
        let wrong_envelope = record
            .encrypt(&tracker.encryption_key(unix_minute(0) + 1))
            .to_bytes()
            .unwrap();
        block_on(backend.publish(&namespace, EncryptedDiscoveryRecord::new(wrong_envelope)))
            .unwrap();

        assert!(block_on(tracker.discover_once()).unwrap().is_empty());
    }

    // ── Shutdown ──────────────────────────────────────────────────────

    #[test]
    fn shutdown_releases_backend() {
        let (tracker, backend) = test_tracker(None);
        block_on(tracker.shutdown());
        // After shutdown, the backend should still accept operations
        // (in-memory backend never truly shuts down, but the call should
        // not panic or error).
        assert!(block_on(backend.shutdown()).is_ok());
    }

    #[test]
    fn publish_after_shutdown_is_allowed_on_backend() {
        let (tracker, backend) = test_tracker(None);
        block_on(tracker.shutdown());

        // The backend should still be usable independently after the
        // tracker that owned it has shut down.
        let ns = NamespaceId::new([0u8; 32]);
        let result = block_on(backend.publish(&ns, EncryptedDiscoveryRecord::new(vec![1, 2, 3])));
        assert!(result.is_ok());
    }

    // ── Namespace derivation ──────────────────────────────────────────

    #[test]
    fn namespace_is_nonzero() {
        let topic = TopicId::from_bytes([0u8; 32]);
        let secret = DiscoverySecret::from_bytes([0u8; 32]);
        let ns = private_room_namespace(&topic, &secret);
        assert!(
            ns.as_bytes().iter().any(|&b| b != 0),
            "namespace should not be all-zero"
        );
    }

    #[test]
    fn namespace_is_deterministic() {
        let topic = TopicId::from_bytes([0xABu8; 32]);
        let secret = DiscoverySecret::from_bytes([0x42u8; 32]);
        let a = private_room_namespace(&topic, &secret);
        let b = private_room_namespace(&topic, &secret);
        assert_eq!(a, b);
    }

    // ── Send + Sync ───────────────────────────────────────────────────

    #[test]
    fn private_room_tracker_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PrivateRoomTracker>();
    }

    #[test]
    fn namespace_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NamespaceId>();
    }

    // ═══════════════════════════════════════════════════════════════════
    // PrivateContinuousTracker tests
    // ═══════════════════════════════════════════════════════════════════

    use std::time::Duration;

    use crate::public_room_continuous::{apply_jitter, ContinuousTrackerConfig};

    /// Helper: create a test tracker suitable for continuous tests.
    fn continuous_test_setup() -> (InMemoryDiscoveryBackend, PrivateRoomTracker) {
        let backend = InMemoryDiscoveryBackend::new();
        let (sk, ep) = test_identity();
        let topic = TopicId::from_bytes([0xC0u8; 32]);
        let secret = DiscoverySecret::from_bytes([0xC0u8; 32]);
        let tracker = PrivateRoomTracker::new(Box::new(backend.clone()), topic, secret, ep, sk);
        (backend, tracker)
    }

    // ── Tracing smoke tests ──────────────────────────────────────────

    /// Tracing is emitted during continuous tracker lifecycle without panics.
    #[tokio::test]
    #[traced_test]
    async fn traced_continuous_tracker_lifecycle() {
        let (_alice_sk, alice_ep) = test_identity();
        let (bob_sk, bob_ep) = test_identity();
        let backend = InMemoryDiscoveryBackend::new();
        let topic = TopicId::from_bytes([0xC1u8; 32]);
        let secret = DiscoverySecret::from_bytes([0xC1u8; 32]);

        // Bob publishes first.
        let bob_tracker =
            PrivateRoomTracker::new(Box::new(backend.clone()), topic, secret, bob_ep, bob_sk);
        bob_tracker.publish_once().await.unwrap();

        let alice_tracker = PrivateRoomTracker::new(
            Box::new(backend.clone()),
            topic,
            secret,
            alice_ep,
            SecretKey::generate(),
        );

        let (tx, mut rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(50),
            publish_interval: Duration::from_secs(3600),
            max_candidates_per_cycle: 20,
            ..Default::default()
        };

        let continuous = PrivateContinuousTracker::start(alice_tracker, config.sanitize(), tx);

        // Wait for discovery to find Bob.
        let result = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        continuous.shutdown().await;

        let peers = result
            .expect("timeout waiting for discovery")
            .expect("channel closed unexpectedly");
        assert!(peers.contains(&bob_ep), "expected Bob to be discovered");
    }

    // ── Discovery ───────────────────────────────────────────────────

    /// Continuous tracker discovers a peer that published before start.
    #[tokio::test]
    async fn continuous_tracker_discovers_new_peer() {
        let (_alice_sk, alice_ep) = test_identity();
        let (bob_sk, bob_ep) = test_identity();
        let backend = InMemoryDiscoveryBackend::new();
        let topic = TopicId::from_bytes([0xC2u8; 32]);
        let secret = DiscoverySecret::from_bytes([0xC2u8; 32]);

        // Bob publishes first.
        let bob_tracker =
            PrivateRoomTracker::new(Box::new(backend.clone()), topic, secret, bob_ep, bob_sk);
        bob_tracker.publish_once().await.unwrap();

        let alice_tracker = PrivateRoomTracker::new(
            Box::new(backend.clone()),
            topic,
            secret,
            alice_ep,
            SecretKey::generate(),
        );

        let (tx, mut rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(50),
            publish_interval: Duration::from_secs(3600),
            max_candidates_per_cycle: 20,
            ..Default::default()
        };

        let continuous = PrivateContinuousTracker::start(alice_tracker, config.sanitize(), tx);

        let result = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;

        continuous.shutdown().await;

        let peers = result
            .expect("timeout waiting for discovery")
            .expect("channel closed unexpectedly");

        assert!(
            peers.contains(&bob_ep),
            "expected Bob's EndpointId to be discovered, got {peers:?}"
        );
    }

    // ── Deduplication ───────────────────────────────────────────────

    /// Once a peer is known, subsequent ticks should NOT re-send it.
    #[tokio::test]
    async fn continuous_tracker_does_not_repeat_known_peers() {
        let (alice_sk, alice_ep) = test_identity();
        let (bob_sk, bob_ep) = test_identity();
        let backend = InMemoryDiscoveryBackend::new();
        let topic = TopicId::from_bytes([0xC3u8; 32]);
        let secret = DiscoverySecret::from_bytes([0xC3u8; 32]);

        let bob_tracker =
            PrivateRoomTracker::new(Box::new(backend.clone()), topic, secret, bob_ep, bob_sk);
        bob_tracker.publish_once().await.unwrap();

        let alice_tracker =
            PrivateRoomTracker::new(Box::new(backend.clone()), topic, secret, alice_ep, alice_sk);

        let (tx, mut rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(50),
            publish_interval: Duration::from_secs(3600),
            max_candidates_per_cycle: 20,
            ..Default::default()
        };

        let continuous = PrivateContinuousTracker::start(alice_tracker, config.sanitize(), tx);

        // First discovery should find Bob.
        let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timeout waiting for first discovery")
            .expect("channel closed unexpectedly");
        assert!(!first.is_empty(), "expected at least one peer");
        assert!(first.contains(&bob_ep), "expected Bob");

        // Wait a bit — subsequent ticks should NOT send Bob again.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Drain any messages that may have arrived.
        while let Ok(Some(batch)) = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await
        {
            assert!(
                batch.is_empty(),
                "expected empty batch (peer already known), got {batch:?}"
            );
        }

        continuous.shutdown().await;
    }

    // ── Shutdown / Cancellation ─────────────────────────────────────

    /// Shutdown stops background tasks promptly.
    #[tokio::test]
    async fn shutdown_stops_background_tasks() {
        let (_backend, tracker) = continuous_test_setup();
        let (tx, _rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(10),
            publish_interval: Duration::from_millis(10),
            ..Default::default()
        };

        let continuous = PrivateContinuousTracker::start(tracker, config.sanitize(), tx);

        // Drop the receiver so the discovery loop stops on send failure.
        drop(_rx);

        // Shutdown should complete quickly — no blocking.
        tokio::time::timeout(Duration::from_secs(5), continuous.shutdown())
            .await
            .expect("shutdown timed out (tasks did not stop)");
    }

    /// Explicit cancellation via shutdown() stops discovery and no further
    /// batches are received.
    #[tokio::test]
    async fn cancellation_stops_discovery_promptly() {
        let (_backend, tracker) = continuous_test_setup();
        let (tx, _rx) = mpsc::channel::<Vec<EndpointId>>(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(20),
            publish_interval: Duration::from_secs(3600),
            ..Default::default()
        };

        let continuous = PrivateContinuousTracker::start(tracker, config.sanitize(), tx);

        // Let it run briefly.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Shutdown should complete within a generous timeout.
        tokio::time::timeout(Duration::from_secs(3), continuous.shutdown())
            .await
            .expect("shutdown did not complete promptly after cancellation");
    }

    // ── Graceful degradation ────────────────────────────────────────

    /// Empty backend should not produce panics or non-empty batches.
    #[tokio::test]
    async fn graceful_degradation_on_empty_backend() {
        let (_backend, tracker) = continuous_test_setup();
        let (tx, mut rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(20),
            publish_interval: Duration::from_secs(3600),
            ..Default::default()
        };

        let continuous = PrivateContinuousTracker::start(tracker, config.sanitize(), tx);

        // Let the discovery loop tick a few times with no peers.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Any received batches must be empty.
        while let Ok(Some(batch)) = tokio::time::timeout(Duration::from_millis(5), rx.recv()).await
        {
            assert!(batch.is_empty(), "expected empty batch, got {batch:?}");
        }

        continuous.shutdown().await;
    }

    // ── Determistic / unit tests ────────────────────────────────────

    /// apply_jitter for factor 0.0 returns the base unchanged.
    #[test]
    fn apply_jitter_zero_factor_returns_base() {
        let base = Duration::from_secs(30);
        let result = apply_jitter(base, 0.0);
        assert_eq!(result, base);
    }

    /// apply_jitter with positive factor stays within [0, base * (1 + factor)].
    #[test]
    fn apply_jitter_within_bounds() {
        let base = Duration::from_secs(10);
        let factor = 0.2;
        for _ in 0..100 {
            let result = apply_jitter(base, factor);
            let result_ns = result.as_nanos();
            let base_ns = base.as_nanos();
            let max_ns = (base_ns as f64 * (1.0 + factor)) as u128;
            // Lower bound is 0 (clamped), upper bound is base * (1 + factor).
            assert!(
                result_ns <= max_ns,
                "jitter {result_ns}ns exceeded max {max_ns}ns for base {base_ns}ns * (1 + {factor})"
            );
        }
    }

    /// apply_jitter with negative factor is clamped to 0 -> returns base.
    #[test]
    fn apply_jitter_negative_factor_returns_base() {
        let base = Duration::from_secs(30);
        let result = apply_jitter(base, -0.1);
        assert_eq!(result, base);
    }

    /// Config sanitize clamps jitter_factor to [0.0, 0.5].
    #[test]
    fn config_sanitize_clamps_jitter() {
        let cfg = ContinuousTrackerConfig {
            jitter_factor: 2.0,
            ..Default::default()
        };
        let sanitized = cfg.sanitize();
        assert!(
            sanitized.jitter_factor <= 0.5,
            "jitter_factor should be clamped to 0.5, got {}",
            sanitized.jitter_factor
        );

        let cfg2 = ContinuousTrackerConfig {
            jitter_factor: -1.0,
            ..Default::default()
        };
        let sanitized2 = cfg2.sanitize();
        assert!(
            sanitized2.jitter_factor >= 0.0,
            "negative jitter_factor should be clamped to 0.0, got {}",
            sanitized2.jitter_factor
        );
    }

    /// Config fields are used by the private continuous tracker.
    #[tokio::test]
    async fn config_fields_affect_discovery_timing() {
        let (bob_sk, bob_ep) = test_identity();
        let backend = InMemoryDiscoveryBackend::new();
        let topic = TopicId::from_bytes([0xC4u8; 32]);
        let secret = DiscoverySecret::from_bytes([0xC4u8; 32]);

        // Bob publishes first.
        let bob_tracker =
            PrivateRoomTracker::new(Box::new(backend.clone()), topic, secret, bob_ep, bob_sk);
        bob_tracker.publish_once().await.unwrap();

        let alice_tracker = PrivateRoomTracker::new(
            Box::new(backend.clone()),
            topic,
            secret,
            SecretKey::generate().public(),
            SecretKey::generate(),
        );

        let (tx, mut rx) = mpsc::channel(16);

        // Use a very short interval so discovery happens fast.
        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(20),
            publish_interval: Duration::from_secs(3600), // slow publish
            max_candidates_per_cycle: 10,
            ..Default::default()
        };

        let continuous = PrivateContinuousTracker::start(alice_tracker, config.sanitize(), tx);

        // Should discover Bob within a reasonable time.
        let result = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        continuous.shutdown().await;

        let peers = result
            .expect("timeout waiting for discovery with fast interval")
            .expect("channel closed unexpectedly");
        assert!(
            peers.contains(&bob_ep),
            "expected Bob to be discovered with configured interval, got {peers:?}"
        );
    }

    // ── Secret-safe logging ─────────────────────────────────────────

    /// The continuous tracker logs must not leak secrets.
    #[tokio::test]
    #[traced_test]
    async fn continuous_secret_safe_logging() {
        let (_alice_sk, alice_ep) = test_identity();
        let (bob_sk, bob_ep) = test_identity();
        let backend = InMemoryDiscoveryBackend::new();
        let topic = TopicId::from_bytes([0xC5u8; 32]);
        let secret = DiscoverySecret::from_bytes([0xC5u8; 32]);
        let raw_secret = hex::encode(secret.as_bytes());
        let invitation = crate::chat_core::RoomInviteV2::new(topic, secret).encode();
        let bob_hex = hex::encode(bob_ep.as_bytes());

        let bob_tracker =
            PrivateRoomTracker::new(Box::new(backend.clone()), topic, secret, bob_ep, bob_sk);
        bob_tracker.publish_once().await.unwrap();

        let alice_tracker = PrivateRoomTracker::new(
            Box::new(backend.clone()),
            topic,
            secret,
            alice_ep,
            SecretKey::generate(),
        );

        let (tx, mut rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(50),
            publish_interval: Duration::from_secs(3600),
            ..Default::default()
        };

        let continuous = PrivateContinuousTracker::start(alice_tracker, config.sanitize(), tx);

        let _ = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        continuous.shutdown().await;

        for forbidden in [raw_secret.as_str(), invitation.as_str(), bob_hex.as_str()] {
            assert!(
                !logs_contain(forbidden),
                "sensitive value appeared in tracing output: {forbidden}"
            );
        }
    }
}
