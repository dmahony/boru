//! Announcement and presence scheduling for the internal discovery
//! subsystem.
//!
//! Extracted from [`DiscoveryService`](crate::discovery_service::DiscoveryService)
//! (BORU-DISC-005). This module owns the announcement side of discovery:
//! the minimum-interval announcement throttles, the legacy
//! [`AnnounceHandle`] / control-plane [`ControlAnnounceHandle`] (Hello /
//! Presence / capability / extension / room-advertisement announce paths),
//! and the two presence **timers** — the periodic presence-refresh loop and
//! the presence-expiry sweep — plus their runtime-tunable config.
//!
//! # Architecture
//!
//! * [`AnnounceThrottle`] is the pure scheduling state machine (owned
//!   `Mutex<AnnounceThrottleState>`): at most one announcement per minimum
//!   interval. The [`AnnounceHandle`] and [`ControlAnnounceHandle`] own one
//!   or more throttles plus the per-node event-id / per-sender sequence
//!   counters, and are cheaply shareable (`Arc`) between the service handle,
//!   the drain loop and the refresh loop so the whole announcement subsystem
//!   observes one min-interval policy per message class.
//! * `DiscoveryService` remains the facade/coordinator: it keeps the
//!   `Arc<Mutex<...>>`/handle fields and the public
//!   `announce_*` facade methods, delegating to this module's handles. It
//!   re-exports [`AnnounceOutcome`], [`AnnounceThrottle`] and the `DEFAULT_*`
//!   scheduling constants so the public paths
//!   `boru_core::discovery_service::{AnnounceOutcome, AnnounceThrottle, ...}`
//!   stay stable.
//! * The presence timers ([`presence_refresh_loop`],
//!   [`presence_expiry_loop`]) live here because their cadence/jitter/TTL
//!   is announcement scheduling; they take the shared stores they mutate as
//!   parameters and never own them.
//!
//! # Invariants enforced here
//!
//! * At most one announcement per class per minimum interval
//!   ([`AnnounceThrottle`]).
//! * The per-node event id (legacy) and per-sender sequence (control) are
//!   allocated **only when the throttle passes** — a suppressed
//!   announcement does not consume an id, so the id space tracks
//!   actually-broadcast events (BORU-DISC-17).
//! * The first announcement always passes the throttle.
//! * The presence refresh interval is deliberately well under
//!   [`DEFAULT_PRESENCE_TTL`] so a peer's presence never goes stale between
//!   refreshes, and per-cycle jitter desynchronises nodes (no synchronised
//!   bursts).

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use iroh_base::PublicKey;
use n0_error::e;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};

use crate::api::GossipSender;
use crate::control_plane::advertisement::PublicRoomAdvertisement as AdvertisementPayload;
use crate::control_plane::capabilities::CapabilitySet;
use crate::control_plane::connectivity::{ConnectivityEvent, PeerConnectivityStore};
use crate::control_plane::extensions::ExtensionsPayload;
use crate::control_plane::message::{CoarsePresence, ControlEnvelope, BORU_APP_PROTOCOL_VERSION};
use crate::control_plane::privacy::{ControlPlaneGuard, DEFAULT_PRESENCE_TTL};
use crate::control_plane::reconnect::ReconnectScheduler;
use crate::discovery::peer_registry::PeerRegistry;
use crate::discovery_message::DiscoveryMessage;
use crate::discovery_service::{DiscoveryServiceError, PeerUpdate};

/// Default minimum interval between discovery announcements (Hello /
/// Presence). Announcements are throttled to at most one per interval so a
/// join hello plus neighbour-up re-announcements cannot become an aggressive
/// broadcast loop on the discovery topic.
pub const DEFAULT_ANNOUNCE_MIN_INTERVAL: Duration = Duration::from_secs(30);

/// Default minimum interval between **control-plane** announcements
/// (HELLO / PRESENCE envelopes, BORU-CP-04). A separate throttle instance
/// from the legacy discovery announcements so the control-plane presence
/// refresh cannot be starved by legacy neighbour-up hellos (and vice
/// versa).
pub const DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL: Duration = Duration::from_secs(30);

/// Default base interval between control-plane PRESENCE refresh
/// announcements (BORU-CP-04 / PDF Task 2.1 step 3). Deliberately low
/// frequency and comfortably under [`DEFAULT_PRESENCE_TTL`] so a peer's
/// presence never goes stale between refreshes.
pub const DEFAULT_PRESENCE_REFRESH_INTERVAL: Duration = Duration::from_secs(120);

/// Default jitter added to each presence-refresh sleep. Randomising the
/// per-cycle delay desynchronises nodes so they do not announce in
/// synchronised bursts (PDF Task 2.1 step 3).
pub const DEFAULT_PRESENCE_REFRESH_JITTER: Duration = Duration::from_secs(60);

/// Announce CAPABILITIES every N-th presence-refresh tick (BORU-CP-11 /
/// PDF Task 4.2 step 2). Presence refreshes every
/// [`DEFAULT_PRESENCE_REFRESH_INTERVAL`], so this re-broadcasts the local
/// capability set roughly every 6 minutes at the default cadence — enough
/// for a peer that joined after our startup announcement to still learn the
/// current set, while remaining low-frequency (bounded resources). `0`
/// disables periodic capability refreshes entirely.
pub const DEFAULT_CAPABILITIES_REFRESH_EVERY: u32 = 3;

/// Announce EXTENSIONS every N-th presence-refresh tick (BORU-CP-16 / PDF
/// Phase 6). Mirrors [`DEFAULT_CAPABILITIES_REFRESH_EVERY`]: presence
/// refreshes every [`DEFAULT_PRESENCE_REFRESH_INTERVAL`], so this
/// re-broadcasts the local extensions advertisement roughly every 6 minutes
/// at the default cadence — enough for a peer that joined after our startup
/// announcement to still learn the current payload, while remaining
/// low-frequency (bounded resources). `0` disables periodic extensions
/// refreshes entirely.
pub const DEFAULT_EXTENSIONS_REFRESH_EVERY: u32 = 3;

/// Outcome of a throttled discovery announcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceOutcome {
    /// The announcement was broadcast to the discovery topic.
    Announced,
    /// The announcement was suppressed by the throttle (too soon since the
    /// last one); nothing was broadcast.
    Throttled,
    /// The announcement was a no-op: the payload (e.g. the local capability
    /// set) is byte-identical to the last announced one, so nothing was
    /// broadcast (BORU-CP-11 idempotence — no duplicate advertisements for
    /// an unchanged capability set).
    Unchanged,
    /// The announcement was refused by the visibility guard (BORU-DIR-04):
    /// a room advertisement for a Private or PublicUnlisted room was
    /// submitted, and only PublicDiscoverable rooms may be advertised.
    /// Nothing was broadcast.
    NotDiscoverable,
}

/// Minimum-interval throttle for discovery announcements.
///
/// At most one announcement is broadcast per [`min_interval`](Self::min_interval)
/// (default [`DEFAULT_ANNOUNCE_MIN_INTERVAL`]). The very first announcement
/// always passes; later attempts within the interval are suppressed. This
/// prevents aggressive broadcast loops (join + neighbour-up + presence must
/// not spam the discovery topic) while still guaranteeing one hello per
/// join.
///
/// The throttle is cheaply shareable (`Arc`): the service handle and the
/// drain loop use the same instance, so join-time and neighbour-up
/// announcements share one policy.
#[derive(Debug)]
pub struct AnnounceThrottle {
    state: Mutex<AnnounceThrottleState>,
}

#[derive(Debug)]
struct AnnounceThrottleState {
    /// Minimum spacing between allowed announcements.
    min_interval: Duration,
    /// When the last announcement was broadcast (`None` = never yet).
    last_announce: Option<Instant>,
}

impl AnnounceThrottle {
    /// A throttle using the default interval
    /// ([`DEFAULT_ANNOUNCE_MIN_INTERVAL`]).
    pub fn new() -> Self {
        Self::with_min_interval(DEFAULT_ANNOUNCE_MIN_INTERVAL)
    }

    /// A throttle with a custom minimum interval (tests use short intervals
    /// to exercise the throttle without sleeping).
    pub fn with_min_interval(min_interval: Duration) -> Self {
        Self {
            state: Mutex::new(AnnounceThrottleState {
                min_interval,
                last_announce: None,
            }),
        }
    }

    /// The configured minimum interval between announcements.
    pub fn min_interval(&self) -> Duration {
        self.state
            .lock()
            .expect("announce throttle lock poisoned")
            .min_interval
    }

    /// Update the minimum interval between announcements.
    ///
    /// Safe to call while the throttle is shared (the service handle and the
    /// drain loop use the same instance).
    pub fn set_min_interval(&self, min_interval: Duration) {
        self.state
            .lock()
            .expect("announce throttle lock poisoned")
            .min_interval = min_interval;
    }

    /// Whether an announcement is allowed right now.
    ///
    /// When allowed, records the announcement time; the caller must only
    /// broadcast if this returns `true`.
    pub fn try_announce(&self) -> bool {
        let mut state = self.state.lock().expect("announce throttle lock poisoned");
        let now = Instant::now();
        let allowed = match state.last_announce {
            Some(prev) => now.duration_since(prev) >= state.min_interval,
            None => true,
        };
        if allowed {
            state.last_announce = Some(now);
        }
        allowed
    }
}

impl Default for AnnounceThrottle {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Announcement handle (sender + throttle + local identity)
// ---------------------------------------------------------------------------

/// Shared announcement state: the gossip sender, the local node identity,
/// the announcement throttle, and the per-node event-id counter.
///
/// Cloned into the drain loop so neighbour-up events can re-announce
/// presence. All clones share one [`AnnounceThrottle`] via `Arc`, so
/// join-time and neighbour-up announcements observe the same minimum-interval
/// policy. The event-id counter is shared the same way (BORU-DISC-17): every
/// announcement gets a fresh, monotonically increasing id so receivers can
/// dedup by `(node_id, event_id)`.
#[derive(Clone, Debug)]
pub(crate) struct AnnounceHandle {
    pub(crate) sender: GossipSender,
    local_node: PublicKey,
    pub(crate) throttle: Arc<AnnounceThrottle>,
    next_event_id: Arc<AtomicU64>,
}

impl AnnounceHandle {
    pub(crate) fn new(sender: GossipSender, local_node: PublicKey) -> Self {
        Self {
            sender,
            local_node,
            throttle: Arc::new(AnnounceThrottle::new()),
            // BORU-CP-07: seed the event-id counter RANDOMLY so a restarted
            // process (same identity) does not reuse the pre-restart id
            // space. The gossip actor dedups by message content (blake3,
            // plumtree `MessageId`), so a byte-identical HELLO from a
            // restarted peer is dropped at the gossip layer and never
            // reaches the discovery service — silently breaking the
            // automatic-reconnection trigger. A random start makes every
            // process incarnation's announcements distinct while keeping
            // within-process monotonicity for the (node_id, event_id)
            // dedup key.
            next_event_id: Arc::new(AtomicU64::new(rand::random::<u64>())),
        }
    }

    /// Allocate the next per-node event id (monotonic, starts at 0).
    fn next_event_id(&self) -> u64 {
        self.next_event_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Raw, unthrottled publish (used by [`DiscoveryService::publish`]).
    pub(crate) async fn publish(
        &self,
        message: DiscoveryMessage,
    ) -> Result<(), DiscoveryServiceError> {
        let bytes = postcard::to_stdvec(&message)
            .map_err(|source| e!(DiscoveryServiceError::Serialize { source }))?;
        self.sender
            .broadcast(Bytes::from(bytes))
            .await
            .map_err(|source| e!(DiscoveryServiceError::Api { source }))?;
        Ok(())
    }

    /// Throttled announce of an arbitrary discovery message.
    ///
    /// The event id is allocated ONLY when the announcement passes the
    /// throttle — a suppressed announcement does not consume an id, so the
    /// id space tracks actually-broadcast events (BORU-DISC-17).
    async fn announce<F>(&self, build: F) -> Result<AnnounceOutcome, DiscoveryServiceError>
    where
        F: FnOnce(u64) -> DiscoveryMessage,
    {
        if !self.throttle.try_announce() {
            debug!("discovery: announcement throttled");
            return Ok(AnnounceOutcome::Throttled);
        }
        let event_id = self.next_event_id();
        self.publish(build(event_id)).await?;
        Ok(AnnounceOutcome::Announced)
    }

    /// Announce this node with a `Hello` carrying a fresh per-node event id.
    pub(crate) async fn announce_hello(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.announce(|event_id| DiscoveryMessage::hello_with_event(self.local_node, event_id))
            .await
    }

    /// Announce this node with a `Presence` heartbeat carrying a fresh
    /// per-node event id.
    pub(crate) async fn announce_presence(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.announce(|event_id| DiscoveryMessage::presence_with_event(self.local_node, event_id))
            .await
    }
}

/// Shared control-plane announcement state (BORU-CP-04 / BORU-CP-11): the
/// gossip sender, the local node identity, a per-sender monotonic sequence
/// counter (BORU-CP-01 dedup key), and throttles for control-plane
/// announcements.
///
/// Separate from the legacy [`AnnounceHandle`]: control-plane HELLO /
/// PRESENCE / CAPABILITIES / EXTENSIONS envelopes (magic `BC`) are a
/// different wire format with their own sequence namespace, and their
/// refresh cadence must not be starved by legacy neighbour-up hellos (or
/// vice versa).
///
/// Shares one per-sender sequence counter across all control-plane message
/// types so receivers' `(sender_node_id, sequence)` dedup stays monotonic
/// per sender. The legacy announce throttle and the control throttle are
/// separate instances so the legacy neighbour-up hellos cannot starve the
/// control-plane presence refresh (or vice versa). CAPABILITIES gets its
/// own throttle too: the join-time control HELLO fires immediately before
/// the join-time capabilities announcement, and a shared throttle would
/// suppress the second. EXTENSIONS (BORU-CP-16) follows the same pattern.
#[derive(Clone, Debug)]
pub(crate) struct ControlAnnounceHandle {
    sender: GossipSender,
    local_node: PublicKey,
    /// BORU-CP-17: the node's Ed25519 secret key, used to sign every
    /// outbound control envelope so receivers can attribute relayed
    /// envelopes to this node cryptographically. `None` (tests) keeps the
    /// legacy unsigned envelope format.
    local_secret: Option<iroh_base::SecretKey>,
    sequence: Arc<AtomicU64>,
    pub(crate) throttle: Arc<AnnounceThrottle>,
    /// Separate throttle for CAPABILITIES announcements (BORU-CP-11). The
    /// join-time HELLO and the join-time capabilities announcement fire
    /// back-to-back; sharing the control throttle would starve one of them.
    caps_throttle: Arc<AnnounceThrottle>,
    /// The last capability set actually broadcast, as its wire id list.
    /// Used to make `announce_capabilities(force = false)` a no-op for an
    /// unchanged set (idempotence — no duplicate advertisements).
    last_announced_caps: Arc<Mutex<Option<Vec<String>>>>,
    /// Separate throttle for EXTENSIONS announcements (BORU-CP-16, PDF
    /// Phase 6). The join-time HELLO + CAPABILITIES + EXTENSIONS burst
    /// fires back-to-back; sharing either throttle would starve one.
    extensions_throttle: Arc<AnnounceThrottle>,
    /// Separate throttle for PUBLIC_ROOM_ADVERTISEMENT announcements
    /// (BORU-DIR-03). Room advertisements are lower-frequency and must not
    /// be starved by (or starve) the presence/capabilities/extension
    /// cadence; Phase 3 (publish/refresh) will tune the interval per room.
    pub(crate) advert_throttle: Arc<AnnounceThrottle>,
    /// The last EXTENSIONS payload actually broadcast. Used to make
    /// `announce_extensions(force = false)` a no-op for an unchanged payload
    /// (idempotence — no duplicate advertisements).
    last_announced_extensions: Arc<Mutex<Option<ExtensionsPayload>>>,
    /// Latest resolver result shared with the endpoint watcher.
    coarse_presence: Arc<Mutex<Option<CoarsePresence>>>,
}

impl ControlAnnounceHandle {
    pub(crate) fn new(
        sender: GossipSender,
        local_node: PublicKey,
        local_secret: Option<iroh_base::SecretKey>,
    ) -> Self {
        Self {
            sender,
            local_node,
            local_secret,
            // BORU-DIR-23: seed the sequence counter with wall-clock
            // seconds (monotonic per identity across restarts). The
            // original random seed made a restarted advertiser's fresh
            // sequence space collide with the pre-restart space at the
            // receive gate (`PeerControlStateStore::record` rejects any
            // sequence `<=` the last seen for that sender), so a restarted
            // room's re-announcement was silently dropped ~50% of the
            // time (matrix scenario "Advertiser restarts — advertisement
            // returns after discovery startup"). `now_secs` both avoids
            // the gossip actor's blake3 content dedup for byte-identical
            // frames (the original rationale) and guarantees the
            // post-restart sequence is higher than anything the same
            // identity broadcast before.
            sequence: Arc::new(AtomicU64::new(unix_now_secs())),
            throttle: Arc::new(AnnounceThrottle::with_min_interval(
                DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL,
            )),
            caps_throttle: Arc::new(AnnounceThrottle::with_min_interval(
                DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL,
            )),
            last_announced_caps: Arc::new(Mutex::new(None)),
            extensions_throttle: Arc::new(AnnounceThrottle::with_min_interval(
                DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL,
            )),
            advert_throttle: Arc::new(AnnounceThrottle::with_min_interval(
                DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL,
            )),
            last_announced_extensions: Arc::new(Mutex::new(None)),
            coarse_presence: Arc::new(Mutex::new(None)),
        }
    }

    /// Allocate the next per-sender control-plane sequence (monotonic,
    /// starts at 0). Receivers dedup by `(sender_node_id, sequence)`.
    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed)
    }

    /// Update the minimum interval for CAPABILITIES announcements
    /// (BORU-CP-11). Tests use short intervals.
    pub(crate) fn set_caps_min_interval(&self, min_interval: Duration) {
        self.caps_throttle.set_min_interval(min_interval);
    }

    /// Update the minimum interval for EXTENSIONS announcements
    /// (BORU-CP-16). Tests use short intervals.
    pub(crate) fn set_extensions_min_interval(&self, min_interval: Duration) {
        self.extensions_throttle.set_min_interval(min_interval);
    }

    /// BORU-CP-17: sign `envelope` with the node's secret key when one is
    /// available. Without a key (tests) the envelope is returned unchanged
    /// (legacy unsigned format).
    pub(crate) fn signed(&self, mut envelope: ControlEnvelope) -> ControlEnvelope {
        if let Some(sk) = &self.local_secret {
            envelope.sign(sk);
        }
        envelope
    }

    /// Throttled announce of an arbitrary control-plane envelope.
    ///
    /// The sequence is allocated ONLY when the announcement passes the
    /// throttle — a suppressed announcement does not consume a sequence, so
    /// the sequence space tracks actually-broadcast envelopes.
    async fn announce<F>(&self, build: F) -> Result<AnnounceOutcome, DiscoveryServiceError>
    where
        F: FnOnce(u64) -> ControlEnvelope,
    {
        if !self.throttle.try_announce() {
            debug!("discovery: control announcement throttled");
            return Ok(AnnounceOutcome::Throttled);
        }
        let sequence = self.next_sequence();
        let bytes = self.signed(build(sequence)).encode();
        self.sender
            .broadcast(Bytes::from(bytes))
            .await
            .map_err(|source| e!(DiscoveryServiceError::Api { source }))?;
        Ok(AnnounceOutcome::Announced)
    }

    /// Announce this node with a control-plane HELLO: the stable peer
    /// identity (envelope `sender_node_id`) plus the minimum protocol
    /// metadata ([`BORU_APP_PROTOCOL_VERSION`]) — PDF Task 2.1 step 1.
    pub(crate) async fn announce_hello(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.announce(|sequence| {
            ControlEnvelope::hello(
                self.local_node,
                sequence,
                unix_now_secs(),
                BORU_APP_PROTOCOL_VERSION,
            )
        })
        .await
    }

    /// Announce a control-plane PRESENCE heartbeat suggesting our own
    /// default presence TTL (receivers clamp it to their own default —
    /// BORU-CP-03).
    pub(crate) async fn announce_presence(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        let coarse = self
            .coarse_presence
            .lock()
            .expect("coarse presence lock poisoned")
            .clone();
        self.announce(|sequence| {
            ControlEnvelope::presence_with_coarse(
                self.local_node,
                sequence,
                unix_now_secs(),
                Some(DEFAULT_PRESENCE_TTL.as_secs() as u32),
                coarse,
            )
        })
        .await
    }

    pub(crate) fn set_coarse_presence(&self, coarse: Option<CoarsePresence>) {
        *self
            .coarse_presence
            .lock()
            .expect("coarse presence lock poisoned") = coarse;
    }

    /// Announce a control-plane CAPABILITIES envelope carrying `caps`
    /// (BORU-CP-11 / PDF Task 4.2 steps 1–2).
    ///
    /// * `force = false` is the explicit startup / material-change path: an
    ///   unchanged set (byte-identical to the last broadcast) is a no-op
    ///   returning [`AnnounceOutcome::Unchanged`] — no duplicate
    ///   advertisement for a capability set that has not materially changed.
    /// * `force = true` is the periodic-refresh path: the set is
    ///   re-broadcast even when unchanged so peers that joined after the
    ///   previous announcement still learn the current set (the gossip
    ///   actor dedups byte-identical payloads for neighbours that already
    ///   have them).
    /// * `bypass_throttle = true` is the neighbour-up path: a freshly
    ///   connected peer must learn the set immediately even when the
    ///   join-time burst happened within the 30s min-interval (the
    ///   join-time announce and the mesh edge forming are often <1s apart
    ///   after a restart, so the throttle would otherwise suppress the
    ///   re-announce and the peer waits for the periodic refresh). The
    ///   throttle's broadcast-loop protection is unnecessary here because
    ///   NeighborUp is a discrete endpoint event, not a loop.
    ///
    /// Either way the CAPABILITIES throttle bounds the rate (unless
    /// bypassed), the sequence is allocated only when a broadcast actually
    /// happens, and the broadcast is a control-plane envelope — never a
    /// chat message.
    pub(crate) async fn announce_capabilities(
        &self,
        caps: &CapabilitySet,
        force: bool,
        bypass_throttle: bool,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        let wire = caps.to_wire();
        if !force {
            let last = self
                .last_announced_caps
                .lock()
                .expect("last announced caps lock poisoned");
            if last.as_deref() == Some(wire.as_slice()) {
                return Ok(AnnounceOutcome::Unchanged);
            }
        }
        if !bypass_throttle && !self.caps_throttle.try_announce() {
            debug!("discovery: capabilities announcement throttled");
            return Ok(AnnounceOutcome::Throttled);
        }
        let sequence = self.next_sequence();
        let bytes = self
            .signed(ControlEnvelope::capabilities(
                self.local_node,
                sequence,
                unix_now_secs(),
                wire.clone(),
            ))
            .encode();
        self.sender
            .broadcast(Bytes::from(bytes))
            .await
            .map_err(|source| e!(DiscoveryServiceError::Api { source }))?;
        *self
            .last_announced_caps
            .lock()
            .expect("last announced caps lock poisoned") = Some(wire);
        Ok(AnnounceOutcome::Announced)
    }

    /// Announce a control-plane EXTENSIONS envelope carrying `payload`
    /// (BORU-CP-16 / PDF Phase 6).
    ///
    /// Mirrors [`announce_capabilities`](Self::announce_capabilities):
    /// * `force = false` is the explicit startup / material-change path: an
    ///   unchanged payload (equal to the last broadcast) is a no-op
    ///   returning [`AnnounceOutcome::Unchanged`].
    /// * `force = true` is the periodic-refresh path: the payload is
    ///   re-broadcast even when unchanged so peers that joined after the
    ///   previous announcement still learn it.
    ///
    /// The EXTENSIONS throttle bounds the rate, the sequence is allocated
    /// only when a broadcast actually happens, and the broadcast is a
    /// control-plane envelope — never a chat message.
    pub(crate) async fn announce_extensions(
        &self,
        payload: &ExtensionsPayload,
        force: bool,
        bypass_throttle: bool,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        if payload.is_empty() {
            // Nothing to advertise: an all-None payload is a no-op even on
            // the forced refresh path.
            return Ok(AnnounceOutcome::Unchanged);
        }
        if !force {
            let last = self
                .last_announced_extensions
                .lock()
                .expect("last announced extensions lock poisoned");
            if last.as_ref() == Some(payload) {
                return Ok(AnnounceOutcome::Unchanged);
            }
        }
        if !bypass_throttle && !self.extensions_throttle.try_announce() {
            debug!("discovery: extensions announcement throttled");
            return Ok(AnnounceOutcome::Throttled);
        }
        let sequence = self.next_sequence();
        let bytes = self
            .signed(ControlEnvelope::extensions(
                self.local_node,
                sequence,
                unix_now_secs(),
                payload.clone(),
            ))
            .encode();
        self.sender
            .broadcast(Bytes::from(bytes))
            .await
            .map_err(|source| e!(DiscoveryServiceError::Api { source }))?;
        *self
            .last_announced_extensions
            .lock()
            .expect("last announced extensions lock poisoned") = Some(payload.clone());
        Ok(AnnounceOutcome::Announced)
    }

    /// Announce a PUBLIC_ROOM_ADVERTISEMENT control-plane envelope carrying
    /// `advert` (BORU-DIR-03, PDF Phase 1 Task 1.3).
    ///
    /// The caller is responsible for building the advertisement and signing
    /// it with its node key ([`PublicRoomAdvertisement::sign`]) — the
    /// service does not hold a secret key. An unsigned advertisement is
    /// still broadcast (receivers mark it clearly untrusted, never
    /// canonical); a signed one lets receivers attribute the payload to
    /// this node.
    ///
    /// The room-advertisement throttle bounds the rate independently of the
    /// presence/capabilities/extension cadence, and the sequence is
    /// allocated only when a broadcast actually happens. The broadcast is a
    /// control-plane envelope — never a chat message, never a join.
    pub(crate) async fn announce_room_advertisement(
        &self,
        advert: AdvertisementPayload,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        // BORU-DIR-04 (PDF 2.1): only PublicDiscoverable rooms are ever
        // advertised. Private and PublicUnlisted rooms must not emit a
        // PUBLIC_ROOM_ADVERTISEMENT — this is the emit-site guard.
        if !advert.visibility.is_discoverable() {
            debug!(
                visibility = ?advert.visibility,
                "discovery: refusing to advertise non-discoverable room",
            );
            return Ok(AnnounceOutcome::NotDiscoverable);
        }
        if !self.advert_throttle.try_announce() {
            debug!("discovery: room-advertisement announcement throttled");
            return Ok(AnnounceOutcome::Throttled);
        }
        let sequence = self.next_sequence();
        let bytes = self
            .signed(ControlEnvelope::public_room_advertisement(
                self.local_node,
                sequence,
                unix_now_secs(),
                advert,
            ))
            .encode();
        self.sender
            .broadcast(Bytes::from(bytes))
            .await
            .map_err(|source| e!(DiscoveryServiceError::Api { source }))?;
        Ok(AnnounceOutcome::Announced)
    }

    /// Announce a PUBLIC_ROOM_WITHDRAWAL control-plane envelope carrying
    /// `withdrawal` (BORU-DIR-09, PDF Phase 3 Task 3.3).
    ///
    /// The caller is responsible for building the withdrawal and signing it
    /// with its node key ([`PublicRoomWithdrawal::sign`]) — the service
    /// does not hold a secret key. An unsigned withdrawal is still
    /// broadcast, but receivers discard it (never applied); a signed one
    /// lets receivers attribute the payload to this node and apply it only
    /// when this node is the room's designated authority (`owner_peer_id`).
    ///
    /// The same room-advertisement throttle bounds the rate, and the
    /// sequence is allocated only when a broadcast actually happens. The
    /// broadcast is a control-plane envelope — never a chat message, never
    /// a join.
    pub(crate) async fn announce_room_withdrawal(
        &self,
        withdrawal: crate::control_plane::advertisement::PublicRoomWithdrawal,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        if !self.advert_throttle.try_announce() {
            debug!("discovery: room-withdrawal announcement throttled",);
            return Ok(AnnounceOutcome::Throttled);
        }
        let sequence = self.next_sequence();
        let bytes = self
            .signed(ControlEnvelope::public_room_withdrawal(
                self.local_node,
                sequence,
                unix_now_secs(),
                withdrawal,
            ))
            .encode();
        self.sender
            .broadcast(Bytes::from(bytes))
            .await
            .map_err(|source| e!(DiscoveryServiceError::Api { source }))?;
        Ok(AnnounceOutcome::Announced)
    }
}

/// Current unix epoch seconds; `0` (unknown) on clock failure, which the
/// envelope treats as "timestamp unknown".
fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Presence expiry (BORU-CP-03)
// ---------------------------------------------------------------------------

/// Runtime-tunable presence-expiry configuration shared between the
/// [`DiscoveryService`] builders and the sweep task.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PresenceExpiryConfig {
    /// Peers not heard from within this window are removed from active
    /// presence.
    pub(crate) ttl: Duration,
    /// How often the sweep runs.
    pub(crate) sweep_interval: Duration,
}

/// Background task that removes stale peers from active presence
/// (BORU-CP-03 TTL expiry).
///
/// Every `sweep_interval` it:
///
/// 1. Prunes the legacy discovery [`PeerRegistry`] of peers not heard from
///    within the configured TTL and emits [`PeerUpdate::Expired`] for each
///    (so the Discover sidebar can drop them from visible presence).
/// 2. Expires stale entries in the control-plane presence store (the
///    BORU-CP-03 hint cache).
///
/// Logs state transitions only, never message contents. The sweep interval
/// is re-read from the shared config before every sleep, so builder tuning
/// (e.g. short intervals in tests) takes effect immediately.
pub(crate) async fn presence_expiry_loop(
    config: Arc<Mutex<PresenceExpiryConfig>>,
    registry: Arc<Mutex<PeerRegistry>>,
    guard: Arc<Mutex<ControlPlaneGuard>>,
    connectivity: Arc<Mutex<PeerConnectivityStore>>,
    reconnect: Arc<Mutex<ReconnectScheduler>>,
    peer_updates_tx: broadcast::Sender<PeerUpdate>,
    cancel: CancellationToken,
) {
    loop {
        // Read the current sweep interval each cycle so the builders can
        // tune it after construction (tests use short intervals).
        let sweep = config
            .lock()
            .expect("expiry config lock poisoned")
            .sweep_interval;

        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                debug!("discovery presence expiry loop cancelled");
                break;
            }
            _ = tokio::time::sleep(sweep) => {
                let ttl = config.lock().expect("expiry config lock poisoned").ttl;
                let now = Instant::now();

                // 1. Legacy discovery registry.
                let expired_registry: Vec<PublicKey> = {
                    let mut reg = registry.lock().expect("peer registry lock poisoned");
                    reg.prune_older_than(ttl)
                };
                for node in &expired_registry {
                    info!(
                        node = %node.fmt_short(),
                        ttl_secs = ttl.as_secs(),
                        "discovery: peer expired from active presence (TTL)",
                    );
                    // BORU-CP-05: the timeout event moves the peer to
                    // OfflineStale in the connectivity state machine.
                    {
                        let mut store = connectivity.lock().expect("connectivity store lock poisoned");
                        store.apply(*node, ConnectivityEvent::Timeout, now);
                    }
                    // BORU-CP-07: the peer went offline — cancel any queued
                    // reconnect attempt. A later fresh announcement will
                    // re-queue from an immediate attempt (no residual
                    // backoff).
                    {
                        let mut scheduler = reconnect.lock().expect("reconnect scheduler lock poisoned");
                        scheduler.reset(node);
                    }
                    let _ = peer_updates_tx.send(PeerUpdate::Expired { node_id: *node });
                }

                // 2. Control-plane presence store.
                let expired_control: Vec<PublicKey> = {
                    let mut g = guard.lock().expect("control-plane guard lock poisoned");
                    g.expire_stale(now)
                };
                for node in &expired_control {
                    info!(
                        node = %node.fmt_short(),
                        ttl_secs = ttl.as_secs(),
                        "control: presence expired from active presence (TTL)",
                    );
                    // BORU-CP-05: feed the timeout into the connectivity
                    // state machine too (idempotent if already offline).
                    {
                        let mut store = connectivity.lock().expect("connectivity store lock poisoned");
                        store.apply(*node, ConnectivityEvent::Timeout, now);
                    }
                    // BORU-CP-07: offline cancels any queued reconnect.
                    {
                        let mut scheduler = reconnect.lock().expect("reconnect scheduler lock poisoned");
                        scheduler.reset(node);
                    }
                }
            }
        }
    }
    debug!("discovery presence expiry loop exited");
}

// ---------------------------------------------------------------------------
// Control-plane presence refresh (BORU-CP-04)
// ---------------------------------------------------------------------------

/// Runtime-tunable control-plane presence-refresh configuration shared
/// between the [`DiscoveryService`] builders and the refresh task.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PresenceRefreshConfig {
    /// Base delay between PRESENCE refresh announcements.
    pub(crate) interval: Duration,
    /// Jitter added to each sleep: `sleep(interval + random(0..=jitter))`.
    pub(crate) jitter: Duration,
    /// Announce CAPABILITIES every N-th refresh tick (`0` = never).
    /// Defaults to [`DEFAULT_CAPABILITIES_REFRESH_EVERY`]; each
    /// capabilities announcement uses its own throttle so the periodic
    /// presence and capability refreshes never starve each other.
    pub(crate) caps_every: u32,
    /// Announce EXTENSIONS every N-th refresh tick (`0` = never).
    /// Defaults to [`DEFAULT_EXTENSIONS_REFRESH_EVERY`]; each extensions
    /// announcement uses its own throttle (BORU-CP-16).
    pub(crate) extensions_every: u32,
}

/// Background task that keeps this node's control-plane presence alive
/// (BORU-CP-04, PDF Task 2.1 step 3) and periodically re-advertises the
/// local capability set (BORU-CP-11, PDF Task 4.2 step 2).
///
/// Every `interval + random(0..=jitter)` it broadcasts one control-plane
/// PRESENCE envelope (magic `BC`), so peers refresh this node's entry in
/// their [`PeerControlStateStore`](crate::control_plane::privacy::PeerControlStateStore).
/// The join-time HELLO covers the immediate announcement; this loop is the
/// low-frequency refresh "while running".
///
/// Every `caps_every`-th tick it additionally re-broadcasts the current
/// local capability set ([`CapabilitySet`]) — even when unchanged — so a
/// peer that joined after our startup announcement still learns the set
/// within a bounded time (the gossip actor dedups byte-identical payloads
/// for neighbours that already have them). The capabilities announcement
/// uses its own throttle, so the periodic presence and capability refreshes
/// never starve each other; an unchanged explicit announcement between
/// ticks is still a no-op ([`AnnounceOutcome::Unchanged`]).
///
/// The per-cycle jitter desynchronises nodes so a fleet of clients does not
/// announce in synchronised bursts. The interval is deliberately well under
/// [`DEFAULT_PRESENCE_TTL`] so a peer's presence never goes stale between
/// refreshes. The announcement still passes the control-plane announce
/// throttle, so an explicit announce right before a tick suppresses that
/// tick (idempotence — no duplicate bursts).
///
/// The configured interval/jitter/cadence are re-read every cycle so builder
/// tuning (e.g. short intervals in tests) takes effect immediately. Logs
/// state transitions only, never message contents.
pub(crate) async fn presence_refresh_loop(
    control_announce: ControlAnnounceHandle,
    local_caps: Arc<Mutex<CapabilitySet>>,
    local_extensions: Arc<Mutex<ExtensionsPayload>>,
    config: Arc<Mutex<PresenceRefreshConfig>>,
    cancel: CancellationToken,
) {
    let mut tick: u64 = 0;
    loop {
        let (interval, jitter, caps_every, extensions_every) = {
            let cfg = config.lock().expect("refresh config lock poisoned");
            (
                cfg.interval,
                cfg.jitter,
                cfg.caps_every,
                cfg.extensions_every,
            )
        };
        let delay = interval + random_jitter(jitter);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                debug!("discovery presence refresh loop cancelled");
                break;
            }
            _ = tokio::time::sleep(delay) => {
                tick = tick.wrapping_add(1);
                match control_announce.announce_presence().await {
                    Ok(AnnounceOutcome::Announced) => {
                        info!(
                            interval_secs = interval.as_secs(),
                            jitter_secs = jitter.as_secs(),
                            "control: presence refresh announced",
                        );
                    }
                    Ok(AnnounceOutcome::Throttled) => {
                        trace!("control: presence refresh suppressed by throttle");
                    }
                    Ok(AnnounceOutcome::Unchanged) => {}
                    Ok(_) => {}
                    Err(error) => {
                        warn!(
                            error = %error,
                            "control: presence refresh failed; continuing",
                        );
                    }
                }
                // BORU-CP-11: periodic capability refresh (force=true so an
                // unchanged set still reaches peers that joined late).
                if caps_every > 0 && tick.is_multiple_of(caps_every as u64) {
                    let caps = local_caps.lock().expect("local caps lock poisoned").clone();
                    match control_announce.announce_capabilities(&caps, true, false).await {
                        Ok(AnnounceOutcome::Announced) => {
                            info!(
                                caps_count = caps.len(),
                                "control: capabilities refresh announced",
                            );
                        }
                        Ok(AnnounceOutcome::Throttled) => {
                            trace!("control: capabilities refresh suppressed by throttle");
                        }
                        Ok(AnnounceOutcome::Unchanged) => {}
                        Ok(_) => {}
                        Err(error) => {
                            warn!(
                                error = %error,
                                "control: capabilities refresh failed; continuing",
                            );
                        }
                    }
                }
                // BORU-CP-16: periodic extensions refresh (force=true so an
                // unchanged payload still reaches peers that joined late).
                if extensions_every > 0 && tick.is_multiple_of(extensions_every as u64) {
                    let extensions = local_extensions
                        .lock()
                        .expect("local extensions lock poisoned")
                        .clone();
                    match control_announce.announce_extensions(&extensions, true, false).await {
                        Ok(AnnounceOutcome::Announced) => {
                            info!("control: extensions refresh announced");
                        }
                        Ok(AnnounceOutcome::Throttled) => {
                            trace!("control: extensions refresh suppressed by throttle");
                        }
                        Ok(AnnounceOutcome::Unchanged) => {}
                        Ok(_) => {}
                        Err(error) => {
                            warn!(
                                error = %error,
                                "control: extensions refresh failed; continuing",
                            );
                        }
                    }
                }
            }
        }
    }
    debug!("discovery presence refresh loop exited");
}

/// Random delay in `0..=jitter` (0 when `jitter` is zero, so tests get
/// deterministic timing). `rand::random` is cryptographically seeded; the
/// distribution shape does not matter here, only that nodes desynchronise.
fn random_jitter(jitter: Duration) -> Duration {
    if jitter.is_zero() {
        return Duration::ZERO;
    }
    let millis = jitter.as_millis().max(1) as u64;
    Duration::from_millis(rand::random::<u64>() % millis)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announce_throttle_first_passes_then_suppresses_then_recovers() {
        let throttle = AnnounceThrottle::with_min_interval(Duration::from_millis(50));
        assert!(throttle.try_announce());
        assert!(!throttle.try_announce());
        std::thread::sleep(Duration::from_millis(70));
        assert!(throttle.try_announce());
    }

    #[test]
    fn announce_throttle_default_interval_is_documented() {
        assert_eq!(
            AnnounceThrottle::new().min_interval(),
            DEFAULT_ANNOUNCE_MIN_INTERVAL
        );
    }
}
