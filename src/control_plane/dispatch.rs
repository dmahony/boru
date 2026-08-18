//! BORU-CP-02: control-plane receive dispatch (BORU-DISC-007).
//!
//! Owns the **control envelope decode → validate → event emission**
//! pipeline for control-plane envelopes received on the discovery topic.
//! This is the explicit event-callback boundary (PDF Task 1.2) that keeps
//! control-plane messages out of chat-message handlers: valid envelopes are
//! delivered to [`ControlEvent`](crate::discovery_service::ControlEvent)
//! subscribers, never to peer-registry, conversation, unread-count, or
//! renderer state.
//!
//! The decoder itself ([`ControlEnvelope::decode`]) and the privacy/abuse
//! guard ([`ControlPlaneGuard`]) live in their own focused modules
//! (`message` and `privacy`). This module is the *dispatcher* that owns the
//! receive-side state and the decode→validate→emit orchestration:
//!
//! ```text
//!   wire bytes
//!      │  ControlEnvelope::decode      (forward-compatible, bounded)
//!      ▼
//!   decode verdict  (Message / UnknownType / UnsupportedVersion / Err)
//!      │  version gate (already applied by decode)
//!      │  self-filter   (sender_node_id != local_node)
//!      ▼
//!   ControlPlaneGuard::admit  (rate limit → attribution → advert policy
//!      │                        → (sender, sequence) dedup → presence)
//!      ▼
//!   Accept:
//!      ├─ connectivity[DiscoverySeen]       (BORU-CP-05)
//!      ├─ PUBLIC_ROOM_ADVERTISEMENT → auth → directory → RoomAdvertisement event
//!      ├─ PUBLIC_ROOM_WITHDRAWAL   → auth → authority → directory → RoomWithdrawal event
//!      └─ otherwise                → generic ControlEvent::Received
//! ```
//!
//! The peer registry is deliberately **not** touched: control-plane traffic
//! is the service boundary's own event stream and never becomes
//! conversation/peer-registry state (PDF Task 1.2). Every rejected frame is
//! dropped (logged, counted) without panicking or affecting chat handling.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use iroh_base::PublicKey;
use tokio::sync::broadcast;
use tracing::{debug, info, trace, warn};

use crate::control_plane::advertisement::AdvertisementAuth;
use crate::control_plane::connectivity::{ConnectivityEvent, PeerConnectivityStore};
use crate::control_plane::message::{ControlEnvelope, ControlPayload, ControlPlaneDecode};
use crate::control_plane::privacy::{ControlPlaneGuard, GuardRejectReason, GuardVerdict};
use crate::diagnostics::{DiagnosticCounters, DirectoryCounters};
use crate::discovery_service::{
    ControlEvent, IncomingOutcome, RoomAdvertisementEvent, RoomWithdrawalEvent,
};
use crate::room_directory::{AdvertiseOutcome, RoomDirectory};

/// Explicit owned state of the control-plane receive dispatch pipeline.
///
/// `DiscoveryService` (via its [`ReceiveCore`](crate::discovery_service))
/// keeps the shared guards/registries as `Arc<Mutex<…>>` handles and
/// constructs one of these dispatchers as a facade, delegating every
/// received control-plane frame to [`ControlPlaneDispatcher::handle_incoming`].
///
/// All `Arc`/atomic handles stored here are **shared** with the owning
/// service (clones of the same underlying mutex/atomics/`broadcast` sender),
/// so there is exactly one mutable control-plane state — no duplication.
#[derive(Clone, Debug)]
pub struct ControlPlaneDispatcher {
    /// Local node identity — used to ignore self-originated envelopes.
    local_node: PublicKey,
    /// The BORU-CP-03 privacy/abuse guard: per-sender rate limiting,
    /// `(sender_node_id, sequence)` dedup, minimal-advertisement policy,
    /// sender attribution, and the TTL-expiring control-plane presence
    /// store. Shared with the presence-expiry sweep.
    guard: Arc<Mutex<ControlPlaneGuard>>,
    /// The BORU-CP-05 explicit peer connectivity state machine: per-peer
    /// connectivity state + deterministic transition trail, fed a real
    /// discovery event only here and from the registry receive path.
    connectivity: Arc<Mutex<PeerConnectivityStore>>,
    /// Bounded local room-directory cache (BORU-DIR-10). Pure cached
    /// discovery metadata — never creates conversations or subscribes.
    room_directory: Arc<Mutex<RoomDirectory>>,
    /// Atomic discovery counters (BORU-DISC-20): malformed / unsupported /
    /// peer-seen accounting. Shared atomics, not duplicated counts.
    counters: DiagnosticCounters,
    /// Atomic room-directory advertisement counters (BORU-DIR-22). Shared
    /// atomics (also fed by the directory cache's expiry sink).
    directory_counters: DirectoryCounters,
    /// Broadcast channel of control-plane events for callers (BORU-CP-02).
    control_events_tx: broadcast::Sender<ControlEvent>,
}

impl ControlPlaneDispatcher {
    /// Construct the dispatcher from the owning service's shared state.
    ///
    /// All handles are clones of the service's own `Arc`/atomic/broadcast
    /// state — the dispatcher shares the same underlying mutable state; it
    /// does not create a duplicate copy.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        local_node: PublicKey,
        guard: Arc<Mutex<ControlPlaneGuard>>,
        connectivity: Arc<Mutex<PeerConnectivityStore>>,
        room_directory: Arc<Mutex<RoomDirectory>>,
        counters: DiagnosticCounters,
        directory_counters: DirectoryCounters,
        control_events_tx: broadcast::Sender<ControlEvent>,
    ) -> Self {
        Self {
            local_node,
            guard,
            connectivity,
            room_directory,
            counters,
            directory_counters,
            control_events_tx,
        }
    }

    /// Deserialise + dispatch one received control-plane envelope (magic
    /// `BC`, BORU-CP-01 wire format).
    ///
    /// The control-plane gate order is: decode → protocol-version check →
    /// self-filter → dedup by `(sender_node_id, sequence)` → emit
    /// [`ControlEvent::Received`]. The peer registry is deliberately NOT
    /// touched: control-plane traffic is the service boundary's own event
    /// stream, never conversation/peer-registry state (PDF Task 1.2). A
    /// malformed, unknown-type, or unsupported-version frame is dropped
    /// (logged, counted) without panicking or affecting chat handling.
    pub fn handle_incoming(&self, content: &[u8], delivered_from: PublicKey) -> IncomingOutcome {
        let envelope = match ControlEnvelope::decode(content) {
            Ok(ControlPlaneDecode::Message(envelope)) => envelope,
            Ok(ControlPlaneDecode::UnknownType { message_type, .. }) => {
                debug!(
                    delivered_from = %delivered_from.fmt_short(),
                    message_type,
                    "discovery: unknown control message type ignored",
                );
                return IncomingOutcome::UnknownControlType { message_type };
            }
            Ok(ControlPlaneDecode::UnsupportedVersion { found, expected }) => {
                self.counters.record_unsupported_version_packet();
                warn!(
                    delivered_from = %delivered_from.fmt_short(),
                    found,
                    expected,
                    "discovery: unsupported control-plane protocol version dropped",
                );
                return IncomingOutcome::UnsupportedVersion { found, expected };
            }
            Err(error) => {
                self.counters.record_malformed_discovery_packet();
                debug!(
                    delivered_from = %delivered_from.fmt_short(),
                    error = %error,
                    "discovery: malformed control-plane envelope dropped",
                );
                return IncomingOutcome::Undecodable;
            }
        };

        if envelope.sender_node_id == self.local_node {
            trace!(node = %envelope.sender_node_id.fmt_short(), "discovery: ignoring self control message");
            return IncomingOutcome::SelfMessage;
        }

        // BORU-CP-03 privacy/abuse gates: rate limit (by the authenticated
        // delivery source) → attribution → minimal-advertisement policy →
        // dedup by (sender_node_id, sequence) → presence state update.
        let verdict = {
            let mut guard = self
                .guard
                .lock()
                .expect("control-plane guard lock poisoned");
            guard.admit(&envelope, delivered_from, Instant::now())
        };
        match verdict {
            GuardVerdict::Accept => {
                info!(
                    sender = %envelope.sender_node_id.fmt_short(),
                    message_type = ?envelope.message_type,
                    sequence = envelope.sequence,
                    "discovery: control-plane message received",
                );
                // BORU-CP-05: a real discovery event — feed the peer
                // connectivity state machine. The guard already deduplicated
                // by (sender, sequence), so a duplicate delivery is an
                // idempotent no-op here (never a connection loop).
                {
                    let mut connectivity = self
                        .connectivity
                        .lock()
                        .expect("connectivity store lock poisoned");
                    connectivity.apply(
                        envelope.sender_node_id,
                        ConnectivityEvent::DiscoverySeen,
                        Instant::now(),
                    );
                }
                // BORU-DIR-01: decode room advertisements ONLY here — at the
                // discovery/control-plane service boundary. A
                // PUBLIC_ROOM_ADVERTISEMENT envelope is interpreted into its
                // typed payload and emitted as the dedicated
                // `ControlEvent::RoomAdvertisement` event — never as a
                // generic `Received` envelope, never into peer-presence,
                // conversation, or chat handling. Malformed/oversized
                // advertisements are already rejected by decode + guard
                // above, so reaching this point means the advertisement is
                // well-formed, bounded, and attributed to its real sender
                // (the transport attribution gate bound the envelope's
                // `sender_node_id` to the authenticated gossip delivery
                // source).
                // BORU-DIR-03 (PDF Task 1.3): the advertisement must ALSO
                // carry a valid publisher signature before it may enter the
                // trusted directory view. Verification is against the
                // envelope's `sender_node_id` — the claimed publisher.
                // * Invalid signature → forged/tampered payload: DISCARD.
                // * Missing signature → clearly untrusted: emitted with
                //   [`AdvertisementAuth::MissingSignature`] so the directory
                //   can list it as unverified but never as canonical.
                // * Verified → emitted with [`AdvertisementAuth::Verified`];
                //   whether the publisher is the room authority (canonical
                //   metadata) is decided by
                //   [`PublicRoomAdvertisement::is_authoritative_publisher`].
                if let ControlPayload::PublicRoomAdvertisement(advert) = &envelope.payload {
                    // BORU-DIR-22 (PDF Task 8.1): a decoded, guard-admitted
                    // room advertisement was received. Count it before the
                    // auth verdict so "received" includes both accepted and
                    // rejected advertisements (the developer can tell a
                    // room was *seen* even when it never entered the cache).
                    self.directory_counters.record_advertisement_received();
                    let auth = advert.verify_signed(&envelope.sender_node_id);
                    match auth {
                        AdvertisementAuth::InvalidSignature => {
                            self.counters.record_malformed_discovery_packet();
                            // BORU-DIR-22: auth-failed advertisement counted
                            // as rejected (distinct from expired / withdrawn /
                            // never-advertised).
                            self.directory_counters.record_advertisement_rejected();
                            warn!(
                                sender = %envelope.sender_node_id.fmt_short(),
                                sequence = envelope.sequence,
                                "discovery: room advertisement signature verification failed; dropped",
                            );
                            return IncomingOutcome::AdvertisementAuthRejected;
                        }
                        AdvertisementAuth::Verified { .. }
                        | AdvertisementAuth::MissingSignature => {
                            info!(
                                sender = %envelope.sender_node_id.fmt_short(),
                                sequence = envelope.sequence,
                                advert_version = advert.advert_version,
                                auth = ?auth,
                                "discovery: public-room advertisement received",
                            );
                            // BORU-DIR-10 (PDF Phase 4, Task 4.1): maintain
                            // the bounded local room-directory cache at the
                            // discovery/control-plane service boundary — the
                            // same place advertisements are decoded. The
                            // cache is keyed by stable room_id, stores the
                            // latest valid advertisement plus provenance
                            // (publisher, auth verdict, first/last seen,
                            // expiry, compatibility, local join state), is
                            // bounded (entry count + metadata bytes), and
                            // merges duplicate/refresh advertisements
                            // deterministically. It NEVER creates a
                            // Conversation record, subscribes to a room
                            // topic, downloads history, or grants permission
                            // (PDF Core rule) — pure cached discovery
                            // metadata.
                            // BORU-DIR-11 (PDF Task 4.2): the directory
                            // deduplicates identical advertisements and
                            // detects conflicting metadata. Only a real
                            // cache change (Added/Refreshed) emits the
                            // typed UI event — repeated gossip and
                            // deterministic no-ops must not churn
                            // subscribers. Conflicts are logged at debug
                            // level (short identities only), never surfaced
                            // as normal UI events.
                            let outcome = self
                                .room_directory
                                .lock()
                                .expect("room directory lock poisoned")
                                .apply_advertisement(
                                    advert.clone(),
                                    envelope.sender_node_id,
                                    auth,
                                    envelope.sequence,
                                    envelope.timestamp_secs,
                                );
                            match outcome {
                                AdvertiseOutcome::Added | AdvertiseOutcome::Refreshed => {
                                    // BORU-DIR-22: the advertisement entered
                                    // or refreshed the directory cache.
                                    self.directory_counters.record_advertisement_accepted();
                                    let _ = self.control_events_tx.send(
                                        ControlEvent::RoomAdvertisement(RoomAdvertisementEvent {
                                            sender_node_id: envelope.sender_node_id,
                                            sequence: envelope.sequence,
                                            timestamp_secs: envelope.timestamp_secs,
                                            auth,
                                            advert: advert.clone(),
                                        }),
                                    );
                                }
                                AdvertiseOutcome::Duplicate => {
                                    // BORU-DIR-22: a repeated/identical
                                    // advertisement was collapsed into the
                                    // existing entry (no second card).
                                    self.directory_counters.record_advertisement_deduplicated();
                                    trace!(
                                        sender = %envelope.sender_node_id.fmt_short(),
                                        sequence = envelope.sequence,
                                        "discovery: duplicate room advertisement deduplicated; no UI churn",
                                    );
                                }
                                AdvertiseOutcome::Conflict => {
                                    debug!(
                                        sender = %envelope.sender_node_id.fmt_short(),
                                        sequence = envelope.sequence,
                                        room = %advert.room_id,
                                        "discovery: conflicting room advertisement; deterministic winner retained, entry marked conflicted",
                                    );
                                }
                                AdvertiseOutcome::Unchanged => {
                                    trace!(
                                        sender = %envelope.sender_node_id.fmt_short(),
                                        sequence = envelope.sequence,
                                        "discovery: room advertisement was a deterministic no-op",
                                    );
                                }
                            }
                            return IncomingOutcome::ControlMessage;
                        }
                    }
                }
                // BORU-DIR-09 (PDF Task 3.3): a PUBLIC_ROOM_WITHDRAWAL
                // envelope is interpreted into its typed payload here — at
                // the discovery/control-plane service boundary — and
                // emitted as the dedicated `ControlEvent::RoomWithdrawal`
                // event, never as a generic `Received` envelope.
                //
                // The same authoritative identity rules as advertisements
                // (BORU-DIR-03) apply before a withdrawal may be applied:
                // * Invalid or missing signature → forged/tampered/untrusted:
                //   DISCARD. It can never remove an advertisement.
                // * Verified but NOT signed by the room's designated
                //   authority (`owner_peer_id`) → verified-but-spoofed
                //   withdrawal attempt: DISCARD.
                // * Verified AND authoritative → emitted as
                //   `ControlEvent::RoomWithdrawal`; directory clients
                //   remove the matching advertisement immediately. TTL
                //   expiry remains the safety net if it is missed.
                if let ControlPayload::PublicRoomWithdrawal(withdrawal) = &envelope.payload {
                    let auth = withdrawal.verify_signed(&envelope.sender_node_id);
                    match auth {
                        AdvertisementAuth::InvalidSignature
                        | AdvertisementAuth::MissingSignature => {
                            self.counters.record_malformed_discovery_packet();
                            warn!(
                                sender = %envelope.sender_node_id.fmt_short(),
                                sequence = envelope.sequence,
                                "discovery: room withdrawal signature verification failed; dropped",
                            );
                            return IncomingOutcome::WithdrawalAuthRejected;
                        }
                        AdvertisementAuth::Verified { .. } => {
                            if !withdrawal.is_authoritative_publisher(&envelope.sender_node_id) {
                                warn!(
                                    sender = %envelope.sender_node_id.fmt_short(),
                                    sequence = envelope.sequence,
                                    "discovery: room withdrawal signed by non-authority publisher; dropped",
                                );
                                return IncomingOutcome::WithdrawalNotAuthoritative;
                            }
                            info!(
                                sender = %envelope.sender_node_id.fmt_short(),
                                sequence = envelope.sequence,
                                room = %withdrawal.room_id,
                                "discovery: public-room withdrawal received and verified",
                            );
                            // BORU-DIR-10: apply the verified, authoritative
                            // withdrawal to the bounded directory cache
                            // immediately — the directory removes the room's
                            // entry when the withdrawing authority matches
                            // the stored owner. TTL expiry remains the
                            // safety net if a withdrawal is missed.
                            let removed = self
                                .room_directory
                                .lock()
                                .expect("room directory lock poisoned")
                                .apply_withdrawal(withdrawal.room_id, withdrawal.owner_peer_id);
                            // BORU-DIR-22 (PDF Task 8.1): a listing removed
                            // by a verified authoritative withdrawal is
                            // counted as withdrawn (distinct from expired /
                            // rejected / never-advertised).
                            if removed {
                                self.directory_counters.record_advertisement_withdrawn();
                            }
                            let _ = self.control_events_tx.send(ControlEvent::RoomWithdrawal(
                                RoomWithdrawalEvent {
                                    sender_node_id: envelope.sender_node_id,
                                    sequence: envelope.sequence,
                                    timestamp_secs: envelope.timestamp_secs,
                                    withdrawal: withdrawal.clone(),
                                },
                            ));
                            return IncomingOutcome::ControlMessage;
                        }
                    }
                }

                let _ = self
                    .control_events_tx
                    .send(ControlEvent::Received(envelope));
                IncomingOutcome::ControlMessage
            }
            GuardVerdict::Reject(reason) => {
                // Log the state transition, never the message contents.
                // Each rejection is bounded by the rate limiter, so a
                // malicious peer cannot cause unbounded log spam.
                match reason {
                    GuardRejectReason::SpoofedSender => {
                        self.counters.record_malformed_discovery_packet();
                        warn!(
                            claimed = %envelope.sender_node_id.fmt_short(),
                            delivered_from = %delivered_from.fmt_short(),
                            "discovery: control envelope sender mismatch dropped",
                        );
                        IncomingOutcome::SpoofedSender
                    }
                    GuardRejectReason::RateLimited => {
                        // BORU-DIR-22 (PDF Task 8.1): count advertisement
                        // envelopes dropped by the per-sender rate limiter
                        // (distinct from rejected-by-auth advertisements —
                        // the rate limiter fires before decode/policy).
                        if matches!(
                            &envelope.payload,
                            ControlPayload::PublicRoomAdvertisement(_)
                        ) {
                            self.directory_counters.record_advertisement_rate_limited();
                        }
                        warn!(
                            sender = %delivered_from.fmt_short(),
                            "discovery: control-plane rate limit exceeded",
                        );
                        IncomingOutcome::RateLimited
                    }
                    GuardRejectReason::Duplicate => {
                        trace!(
                            sender = %envelope.sender_node_id.fmt_short(),
                            sequence = envelope.sequence,
                            "discovery: duplicate control envelope ignored",
                        );
                        IncomingOutcome::Duplicate
                    }
                    GuardRejectReason::AdvertViolation(violation) => {
                        debug!(
                            sender = %envelope.sender_node_id.fmt_short(),
                            violation = ?violation,
                            "discovery: control advertisement rejected by minimal-content policy",
                        );
                        IncomingOutcome::AdvertViolation(violation)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::message::{CONTROL_PLANE_MAGIC, CONTROL_PLANE_PROTOCOL_VERSION};

    /// Deterministic test identity: a `SecretKey` seeded from a single byte
    /// produces a valid Ed25519 public key.
    fn test_key(byte: u8) -> PublicKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed).public()
    }

    /// A test `HELLO` envelope for `sender` (app protocol version 1).
    fn hello(sender: PublicKey, sequence: u64) -> ControlEnvelope {
        ControlEnvelope::hello(sender, sequence, 1_700_000_000, 1)
    }

    /// Build an isolated dispatcher with a fresh guard/connectivity/directory
    /// and an isolated counter set; returns the control-event receiver too.
    fn test_dispatcher(
        local: PublicKey,
    ) -> (ControlPlaneDispatcher, broadcast::Receiver<ControlEvent>) {
        let (control_events_tx, rx) = broadcast::channel(64);
        let dispatcher = ControlPlaneDispatcher::new(
            local,
            Arc::new(Mutex::new(ControlPlaneGuard::new())),
            Arc::new(Mutex::new(PeerConnectivityStore::new())),
            Arc::new(Mutex::new(RoomDirectory::new())),
            DiagnosticCounters::new(),
            DirectoryCounters::new(),
            control_events_tx,
        );
        (dispatcher, rx)
    }

    /// `ControlPlaneDispatcher::new` clones the shared handles, so the
    /// dispatcher and the owning service see the SAME guard/connectivity/
    /// directory state (no duplicate mutable state).
    #[test]
    fn dispatcher_shares_underlying_state_not_duplicates_it() {
        let local = test_key(0xAA);
        let guard = Arc::new(Mutex::new(ControlPlaneGuard::new()));
        let connectivity = Arc::new(Mutex::new(PeerConnectivityStore::new()));
        let directory = Arc::new(Mutex::new(RoomDirectory::new()));
        let (tx, _rx) = broadcast::channel(16);
        let dispatcher = ControlPlaneDispatcher::new(
            local,
            guard.clone(),
            connectivity.clone(),
            directory.clone(),
            DiagnosticCounters::new(),
            DirectoryCounters::new(),
            tx,
        );

        // The dispatcher routes to the exact same guard/directory instances.
        let peer = test_key(0xBB);
        let env = hello(peer, 1);
        assert_eq!(
            dispatcher.handle_incoming(&env.encode(), peer),
            IncomingOutcome::ControlMessage
        );
        let presence_count = guard.lock().unwrap().presence_count();
        assert_eq!(presence_count, 1, "guard state shared with the service");
        let _ = &connectivity;
        let _ = &directory;
    }

    /// A malformed (undecodable) control-plane frame is dropped without
    /// panicking and counted as a malformed packet.
    #[test]
    fn malformed_control_frame_is_undecodable() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let (dispatcher, _rx) = test_dispatcher(local);

        // Magic + supported version + garbage that cannot parse as a header.
        let mut bytes = vec![
            CONTROL_PLANE_MAGIC[0],
            CONTROL_PLANE_MAGIC[1],
            CONTROL_PLANE_PROTOCOL_VERSION,
        ];
        bytes.extend_from_slice(b"garbage-not-a-control-header");
        assert_eq!(
            dispatcher.handle_incoming(&bytes, peer),
            IncomingOutcome::Undecodable
        );
    }

    /// An unknown (future) message type is ignored safely — forward
    /// compatibility, fail closed for that feature.
    #[test]
    fn unknown_control_type_is_ignored() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let (dispatcher, _rx) = test_dispatcher(local);

        let mut bytes = hello(peer, 9).encode();
        bytes[3] = 0x7F; // header message_type → unknown tag
        assert_eq!(
            dispatcher.handle_incoming(&bytes, peer),
            IncomingOutcome::UnknownControlType { message_type: 0x7F }
        );
    }

    /// An unsupported protocol version fails closed for that feature.
    #[test]
    fn unsupported_control_version_fails_closed() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let (dispatcher, _rx) = test_dispatcher(local);

        let mut bytes = hello(peer, 1).encode();
        bytes[2] = CONTROL_PLANE_PROTOCOL_VERSION + 1; // bump the version byte
        assert_eq!(
            dispatcher.handle_incoming(&bytes, peer),
            IncomingOutcome::UnsupportedVersion {
                found: CONTROL_PLANE_PROTOCOL_VERSION + 1,
                expected: CONTROL_PLANE_PROTOCOL_VERSION,
            }
        );
    }

    /// A self-originated control envelope is ignored.
    #[test]
    fn self_control_message_is_ignored() {
        let local = test_key(0xAA);
        let (dispatcher, _rx) = test_dispatcher(local);
        let bytes = hello(local, 1).encode();
        assert_eq!(
            dispatcher.handle_incoming(&bytes, local),
            IncomingOutcome::SelfMessage
        );
    }

    /// A valid, guard-accepted HELLO emits `ControlEvent::Received` — the
    /// explicit event-callback boundary (never peer-registry state).
    #[test]
    fn accepted_hello_emits_received_event() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let (dispatcher, mut rx) = test_dispatcher(local);

        let env = hello(peer, 1);
        assert_eq!(
            dispatcher.handle_incoming(&env.encode(), peer),
            IncomingOutcome::ControlMessage
        );

        let event = rx
            .try_recv()
            .expect("control event was emitted to the broadcast channel");
        assert_eq!(event, ControlEvent::Received(env));
    }

    /// An envelope whose claimed sender differs from the authenticated
    /// delivery source is rejected as a spoofing attempt.
    #[test]
    fn spoofed_sender_is_rejected() {
        let local = test_key(0xAA);
        let real_author = test_key(0xBB);
        let relay = test_key(0xCC);
        let (dispatcher, _rx) = test_dispatcher(local);

        let bytes = hello(real_author, 1).encode();
        // The unsigned envelope claims `real_author` but arrives via `relay`.
        assert_eq!(
            dispatcher.handle_incoming(&bytes, relay),
            IncomingOutcome::SpoofedSender
        );
    }

    /// A duplicate `(sender_node_id, sequence)` frame is dropped by the guard.
    #[test]
    fn duplicate_sender_sequence_is_rejected() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let (dispatcher, _rx) = test_dispatcher(local);

        let bytes = hello(peer, 7).encode();
        assert_eq!(
            dispatcher.handle_incoming(&bytes, peer),
            IncomingOutcome::ControlMessage
        );
        assert_eq!(
            dispatcher.handle_incoming(&bytes, peer),
            IncomingOutcome::Duplicate
        );
    }

    /// A per-sender rate limit is enforced by the guard (BORU-CP-03).
    #[test]
    fn rate_limited_sender_is_rejected() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let (dispatcher, _rx) = test_dispatcher(local);

        // First CONTROL_RATE_LIMIT_MAX_FRAMES distinct sequences pass; the
        // next one within the window is rate-limited.
        let max = crate::control_plane::privacy::CONTROL_RATE_LIMIT_MAX_FRAMES;
        for seq in 1..=max {
            assert_eq!(
                dispatcher.handle_incoming(&hello(peer, seq as u64).encode(), peer),
                IncomingOutcome::ControlMessage,
                "frame {seq} within the rate limit must be accepted"
            );
        }
        assert_eq!(
            dispatcher.handle_incoming(&hello(peer, (max as u64) + 1).encode(), peer),
            IncomingOutcome::RateLimited
        );
    }

    /// The peer connectivity state machine is fed a real discovery event on
    /// an accepted control envelope (BORU-CP-05) — confirmed via the shared
    /// connectivity store.
    #[test]
    fn accepted_control_feeds_connectivity_state() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let connectivity = Arc::new(Mutex::new(PeerConnectivityStore::new()));
        let (tx, _rx) = broadcast::channel(16);
        let dispatcher = ControlPlaneDispatcher::new(
            local,
            Arc::new(Mutex::new(ControlPlaneGuard::new())),
            connectivity.clone(),
            Arc::new(Mutex::new(RoomDirectory::new())),
            DiagnosticCounters::new(),
            DirectoryCounters::new(),
            tx,
        );

        assert_eq!(
            dispatcher.handle_incoming(&hello(peer, 3).encode(), peer),
            IncomingOutcome::ControlMessage
        );
        let state = connectivity.lock().unwrap().state(&peer);
        assert_ne!(
            state,
            crate::control_plane::connectivity::PeerConnectivityState::Unknown,
            "accepted control traffic must mark the peer as discovered"
        );
    }
}
