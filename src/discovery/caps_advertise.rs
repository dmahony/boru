//! Capabilities / extensions advertisement — the local capability set and
//! Phase 6 extensions payload this node advertises, plus the announce
//! wiring that broadcasts them.
//!
//! Extracted from [`DiscoveryService`](crate::discovery_service::DiscoveryService)
//! (BORU-DISC-008). This module owns the *advertisement state* for
//! capabilities (BORU-CP-11 / PDF Task 4.2) and extensions (BORU-CP-16 /
//! PDF Phase 6): the local `CapabilitySet` / `ExtensionsPayload` stores,
//! the pure update + announce logic that builds the control-plane
//! envelopes, the explicit-startup / material-change announce paths, and
//! the side-effect wiring that re-announces both to a freshly connected
//! gossip neighbour (the neighbour-up path). The periodic refresh cadence
//! itself stays in [`presence_scheduler`](crate::discovery::presence_scheduler)
//! (BORU-DISC-005) — this module just shares its advertisement stores with
//! that loop via [`CapsAdvertiser::caps_handle`] /
//! [`CapsAdvertiser::extensions_handle`].
//!
//! # Architecture
//!
//! * [`CapsAdvertiser`] is the focused, owned-state module. It creates and
//!   owns the single `local_caps` / `local_extensions` stores, the
//!   [`ControlAnnounceHandle`] clone (throttles + sequence + broadcast) it
//!   announces through, and the shared [`RoomDirectory`] it keeps in sync
//!   with the local capability set. `DiscoveryService` remains the
//!   facade/coordinator: it keeps the same-named public API
//!   (`local_capabilities`, `update_local_capabilities`,
//!   `announce_capabilities`, `local_extensions`,
//!   `update_local_extensions`, `announce_extensions`, `capability_gate`),
//!   delegating to this module — so callers and the wire format are
//!   unchanged.
//! * Exactly one mutable instance of each store exists (created here);
//!   [`caps_handle`](CapsAdvertiser::caps_handle) and
//!   [`extensions_handle`](CapsAdvertiser::extensions_handle) hand out `Arc`
//!   *clones of the same store* to the drain loop and the presence-refresh
//!   loop, so there is no duplicate mutable state (BORU-DISC-008 STOP
//!   condition).
//! * The broadcast is always a **control-plane envelope** — never a chat
//!   message, never a legacy discovery message, never a join.
//!
//! # Invariants enforced here
//!
//! * The explicit / material-change announce paths (`force = false`) are
//!   idempotent: an unchanged set or payload is a no-op
//!   ([`AnnounceOutcome::Unchanged`]) — no duplicate advertisement
//!   (BORU-CP-11/16 idempotence). The neighbour-up path (`force = true`,
//!   `bypass_throttle = true`) re-broadcasts unconditionally so a late
//!   joiner still learns the current set; the caps/extensions throttles
//!   bound the rate when not bypassed.
//! * An all-`None` extensions payload is never broadcast (nothing to
//!   advertise).
//! * The room directory's optional-feature negotiation stays in sync with
//!   the local capability set on every material change (PDF Task 6.2 step
//!   2).

use std::sync::{Arc, Mutex};

use iroh_base::PublicKey;
use tracing::{debug, info, warn};

use crate::control_plane::capabilities::{default_local_capabilities, CapabilitySet};
use crate::control_plane::extensions::{default_local_extensions, ExtensionsPayload};
use crate::discovery::presence_scheduler::{AnnounceOutcome, ControlAnnounceHandle};
use crate::discovery_service::DiscoveryServiceError;
use crate::room_directory::RoomDirectory;

/// The focused capabilities/extensions advertisement state and announce
/// logic (BORU-DISC-008).
///
/// Owns the local capability set ([`CapabilitySet`], BORU-CP-11 / PDF Task
/// 4.2) and the local Phase 6 extensions payload ([`ExtensionsPayload`],
/// BORU-CP-16 / PDF Phase 6), plus the pure update / announce methods that
/// broadcast them as control-plane envelopes. Cheaply cloneable (all fields
/// are `Arc`-backed or a cloneable announce handle), so the drain loop can
/// hold one to re-announce on neighbour-up.
///
/// `DiscoveryService` delegates its `local_capabilities` /
/// `update_local_capabilities` / `announce_capabilities` / `local_extensions`
/// / `update_local_extensions` / `announce_extensions` facades here, and the
/// `DiscoveryCapabilityGate` shares [`caps_handle`](Self::caps_handle).
#[derive(Clone, Debug)]
pub(crate) struct CapsAdvertiser {
    /// The local capability set this node advertises (BORU-CP-11 / PDF Task
    /// 4.2). Defaults to [`default_local_capabilities`]; the app replaces it
    /// via `update_local_capabilities` when locally enabled capabilities
    /// materially change. Shared with the periodic refresh loop so it always
    /// re-announces the current set.
    local_caps: Arc<Mutex<CapabilitySet>>,
    /// The local Phase 6 extensions advertisement this node advertises
    /// (BORU-CP-16 / PDF Phase 6). Defaults to [`default_local_extensions`];
    /// the app replaces it via `update_local_extensions` when the locally
    /// derived extension metadata materially changes (e.g. group reachability
    /// from known local memberships, device identity, file readiness). Shared
    /// with the periodic refresh loop so it always re-announces the current
    /// payload.
    local_extensions: Arc<Mutex<ExtensionsPayload>>,
    /// The control-plane announce handle (throttles + sequence + sender) used
    /// to broadcast every caps/extensions envelope.
    control_announce: ControlAnnounceHandle,
    /// The shared room directory, kept in sync with the local capability set
    /// on material change (PDF Task 6.2 step 2).
    room_directory: Arc<Mutex<RoomDirectory>>,
}

impl CapsAdvertiser {
    /// Create the advertiser with the default local capability set and
    /// extensions payload.
    pub(crate) fn new(
        control_announce: ControlAnnounceHandle,
        room_directory: Arc<Mutex<RoomDirectory>>,
    ) -> Self {
        Self {
            local_caps: Arc::new(Mutex::new(default_local_capabilities())),
            local_extensions: Arc::new(Mutex::new(default_local_extensions())),
            control_announce,
            room_directory,
        }
    }

    /// An `Arc` clone of the single local capability store — handed to the
    /// presence-refresh loop and the drain loop so they observe (and the
    /// refresh loop re-reads) the same mutable state without duplicating it.
    pub(crate) fn caps_handle(&self) -> Arc<Mutex<CapabilitySet>> {
        self.local_caps.clone()
    }

    /// An `Arc` clone of the single local extensions store — handed to the
    /// presence-refresh loop and the drain loop, sharing the same state.
    pub(crate) fn extensions_handle(&self) -> Arc<Mutex<ExtensionsPayload>> {
        self.local_extensions.clone()
    }

    /// The local capability set this node currently advertises.
    ///
    /// Defaults to [`default_local_capabilities`]; the app replaces it via
    /// `update_local_capabilities` when locally enabled capabilities
    /// materially change.
    pub(crate) fn local_capabilities(&self) -> CapabilitySet {
        self.local_caps
            .lock()
            .expect("local caps lock poisoned")
            .clone()
    }

    /// Replace the local capability set and announce it when it materially
    /// changed (BORU-CP-11 / PDF Task 4.2 step 2).
    ///
    /// The new set is stored; if it differs from the last announced set a
    /// CAPABILITIES envelope is broadcast on the discovery topic — a
    /// control-plane message, never a chat message. If the set is
    /// byte-identical to the last announced one, nothing is broadcast
    /// ([`AnnounceOutcome::Unchanged`]) — an idempotent no-op. The room
    /// directory's optional-feature negotiation is kept in sync (PDF Task 6.2
    /// step 2).
    pub(crate) async fn update_local_capabilities(
        &self,
        caps: CapabilitySet,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        {
            let mut local = self.local_caps.lock().expect("local caps lock poisoned");
            *local = caps.clone();
        }
        self.room_directory
            .lock()
            .expect("room directory lock poisoned")
            .set_local_capabilities(caps);
        self.announce_capabilities().await
    }

    /// Broadcast the current local capability set (startup + material-change
    /// path, BORU-CP-11 / PDF Task 4.2 step 1).
    ///
    /// Returns [`AnnounceOutcome::Unchanged`] when the set is byte-identical
    /// to the last announced one (no duplicate advertisement for an unchanged
    /// set). The periodic refresh loop re-announces the set on its own cadence
    /// so peers that joined after the previous announcement still learn it.
    pub(crate) async fn announce_capabilities(
        &self,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        let caps = self.local_capabilities();
        self.control_announce
            .announce_capabilities(&caps, false, false)
            .await
    }

    /// The local Phase 6 extensions advertisement this node currently
    /// advertises.
    ///
    /// Defaults to [`default_local_extensions`] (every capability-backed
    /// extension section this build implements). The app replaces it via
    /// `update_local_extensions` when the locally derived extension metadata
    /// materially changes.
    pub(crate) fn local_extensions(&self) -> ExtensionsPayload {
        self.local_extensions
            .lock()
            .expect("local extensions lock poisoned")
            .clone()
    }

    /// Replace the local extensions advertisement and announce it when it
    /// materially changed (BORU-CP-16 / PDF Phase 6).
    ///
    /// The new payload is stored; if it differs from the last announced one an
    /// EXTENSIONS envelope is broadcast on the discovery topic — a
    /// control-plane message, never a chat message. If the payload is
    /// identical to the last announced one, nothing is broadcast
    /// ([`AnnounceOutcome::Unchanged`]) — an idempotent no-op. An all-`None`
    /// payload is never broadcast (nothing to advertise).
    pub(crate) async fn update_local_extensions(
        &self,
        payload: ExtensionsPayload,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        {
            let mut local = self
                .local_extensions
                .lock()
                .expect("local extensions lock poisoned");
            *local = payload;
        }
        self.announce_extensions().await
    }

    /// Broadcast the current local extensions advertisement (startup +
    /// material-change path, BORU-CP-16 / PDF Phase 6).
    ///
    /// Returns [`AnnounceOutcome::Unchanged`] when the payload is identical to
    /// the last announced one (no duplicate advertisement) or empty. The
    /// periodic refresh loop re-announces the payload on its own cadence so
    /// peers that joined after the previous announcement still learn it.
    pub(crate) async fn announce_extensions(
        &self,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        let payload = self.local_extensions();
        self.control_announce
            .announce_extensions(&payload, false, false)
            .await
    }

    /// Re-announce the local capability set and extensions to a freshly
    /// connected gossip neighbour (the neighbour-up side-effect wiring,
    /// BORU-CP-11/16).
    ///
    /// A peer that connects after our join announcement must learn what we
    /// support IMMEDIATELY, not on the next periodic refresh (up to ~6-9
    /// minutes at the default cadence). `force=true` rebroadcasts even when
    /// the set/payload is unchanged so the late joiner still receives it; the
    /// caps/extensions throttles are bypassed (`bypass_throttle=true`) because
    /// NeighborUp is a discrete endpoint event, not a broadcast loop. Fire and
    /// forget: never blocks the receive drain.
    pub(crate) fn reannounce_on_neighbor_up(&self, peer: PublicKey) {
        let caps = self
            .local_caps
            .lock()
            .expect("local caps lock poisoned")
            .clone();
        let control = self.control_announce.clone();
        tokio::spawn(async move {
            match control.announce_capabilities(&caps, true, true).await {
                Ok(AnnounceOutcome::Announced) => {
                    info!(
                        peer = %peer.fmt_short(),
                        caps_count = caps.len(),
                        "discovery: re-announced capabilities after neighbor up",
                    );
                }
                Ok(AnnounceOutcome::Throttled) => {
                    debug!(
                        peer = %peer.fmt_short(),
                        "discovery: neighbor-up capabilities suppressed by throttle",
                    );
                }
                Ok(AnnounceOutcome::Unchanged) => {}
                Ok(_) => {}
                Err(error) => {
                    warn!(
                        peer = %peer.fmt_short(),
                        error = %error,
                        "discovery: neighbor-up capabilities failed",
                    );
                }
            }
        });

        let extensions = self
            .local_extensions
            .lock()
            .expect("local extensions lock poisoned")
            .clone();
        let control = self.control_announce.clone();
        tokio::spawn(async move {
            match control.announce_extensions(&extensions, true, true).await {
                Ok(AnnounceOutcome::Announced) => {
                    info!(
                        peer = %peer.fmt_short(),
                        "discovery: re-announced extensions after neighbor up",
                    );
                }
                Ok(AnnounceOutcome::Throttled) => {
                    debug!(
                        peer = %peer.fmt_short(),
                        "discovery: neighbor-up extensions suppressed by throttle",
                    );
                }
                Ok(AnnounceOutcome::Unchanged) => {}
                Ok(_) => {}
                Err(error) => {
                    warn!(
                        peer = %peer.fmt_short(),
                        error = %error,
                        "discovery: neighbor-up extensions failed",
                    );
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Command;
    use crate::control_plane::message::{
        ControlEnvelope, ControlMessageType, ControlPayload, ControlPlaneDecode,
        CONTROL_PLANE_MAGIC,
    };
    use irpc::channel::mpsc as irpc_mpsc;

    fn test_key(byte: u8) -> PublicKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed).public()
    }

    /// Build a `CapsAdvertiser` over an offline (never-fed) command channel,
    /// returning it plus a receiver for the broadcast commands it emits.
    fn test_advertiser() -> (CapsAdvertiser, irpc_mpsc::Receiver<Command>) {
        let (cmd_tx, cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let sender = crate::api::GossipSender::new(cmd_tx);
        let room_directory = Arc::new(Mutex::new(RoomDirectory::new()));
        let control_announce = ControlAnnounceHandle::new(sender, test_key(0xAA), None);
        (
            CapsAdvertiser::new(control_announce, room_directory),
            cmd_rx,
        )
    }

    /// Assert the next command is a control-plane Broadcast whose decoded
    /// envelope matches `check`.
    async fn expect_control_broadcast(
        cmd_rx: &mut irpc_mpsc::Receiver<Command>,
        what: &str,
    ) -> ControlEnvelope {
        let command = tokio::time::timeout(std::time::Duration::from_secs(5), cmd_rx.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what} broadcast"))
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command for {what}, got {command:?}");
        };
        assert!(
            bytes.starts_with(&CONTROL_PLANE_MAGIC),
            "a caps/extensions announcement must be a control-plane envelope"
        );
        assert!(
            postcard::from_bytes::<crate::discovery_message::DiscoveryMessage>(&bytes).is_err(),
            "a caps/extensions envelope must never decode as a legacy DiscoveryMessage"
        );
        match ControlEnvelope::decode(&bytes).expect("control envelope decodes") {
            ControlPlaneDecode::Message(env) => env,
            other => panic!("expected decoded envelope, got {other:?}"),
        }
    }

    #[test]
    fn new_initializes_default_local_state() {
        let (advert, _cmd_rx) = test_advertiser();
        assert_eq!(advert.local_capabilities(), default_local_capabilities());
        assert_eq!(advert.local_extensions(), default_local_extensions());
    }

    /// `announce_capabilities` broadcasts a CAPABILITIES control-plane
    /// envelope carrying the current local capability set — a control-plane
    /// message, never a chat message.
    #[tokio::test]
    async fn announce_capabilities_broadcasts_capabilities_envelope() {
        let (advert, mut cmd_rx) = test_advertiser();

        assert_eq!(
            advert.announce_capabilities().await.unwrap(),
            AnnounceOutcome::Announced
        );
        let env = expect_control_broadcast(&mut cmd_rx, "capabilities").await;
        assert_eq!(env.message_type, ControlMessageType::Capabilities);
        let ControlPayload::Capabilities(payload) = &env.payload else {
            panic!("expected Capabilities payload, got {:?}", env.payload);
        };
        let local = advert.local_capabilities();
        assert_eq!(payload.capabilities, local.to_wire());
        assert!(payload.capabilities.contains(&"files-v2".to_string()));
    }

    /// Re-announcing the SAME local capability set is an idempotent no-op:
    /// [`AnnounceOutcome::Unchanged`] and no second broadcast (BORU-CP-11
    /// idempotence — no duplicate advertisements for an unchanged set).
    #[tokio::test]
    async fn announce_capabilities_dedups_unchanged_set() {
        let (advert, mut cmd_rx) = test_advertiser();
        advert
            .control_announce
            .set_caps_min_interval(std::time::Duration::ZERO);

        assert_eq!(
            advert.announce_capabilities().await.unwrap(),
            AnnounceOutcome::Announced
        );
        expect_control_broadcast(&mut cmd_rx, "capabilities").await;

        // Same set again — no duplicate broadcast.
        assert_eq!(
            advert.announce_capabilities().await.unwrap(),
            AnnounceOutcome::Unchanged
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(80), cmd_rx.recv())
                .await
                .is_err(),
            "unchanged capability set must not be re-broadcast"
        );
    }

    /// Replacing the local capability set broadcasts the NEW set (the
    /// "locally enabled capabilities materially change" path) without any chat
    /// message; `local_capabilities()` reflects the change and the room
    /// directory's negotiation is kept in sync.
    #[tokio::test]
    async fn update_local_capabilities_stores_announces_and_syncs_directory() {
        let (advert, mut cmd_rx) = test_advertiser();
        advert
            .control_announce
            .set_caps_min_interval(std::time::Duration::ZERO);

        let shrunk = CapabilitySet::from_wire(vec!["files-v2".to_string()]);
        assert_eq!(
            advert
                .update_local_capabilities(shrunk.clone())
                .await
                .unwrap(),
            AnnounceOutcome::Announced
        );
        assert_eq!(advert.local_capabilities(), shrunk);
        // PDF Task 6.2 step 2: the room directory's optional-feature
        // negotiation follows the local capability set.
        assert_eq!(
            advert
                .room_directory
                .lock()
                .unwrap()
                .local_capabilities()
                .clone(),
            shrunk
        );

        let env = expect_control_broadcast(&mut cmd_rx, "updated capabilities").await;
        let ControlPayload::Capabilities(payload) = &env.payload else {
            panic!("expected Capabilities payload, got {:?}", env.payload);
        };
        assert_eq!(payload.capabilities, vec!["files-v2".to_string()]);

        // Re-updating to the SAME set is a no-op.
        assert_eq!(
            advert.update_local_capabilities(shrunk).await.unwrap(),
            AnnounceOutcome::Unchanged
        );
    }

    /// `announce_extensions` broadcasts an EXTENSIONS control-plane envelope
    /// carrying the current local extensions payload.
    #[tokio::test]
    async fn announce_extensions_broadcasts_extensions_envelope() {
        let (advert, mut cmd_rx) = test_advertiser();

        assert_eq!(
            advert.announce_extensions().await.unwrap(),
            AnnounceOutcome::Announced
        );
        let env = expect_control_broadcast(&mut cmd_rx, "extensions").await;
        assert_eq!(env.message_type, ControlMessageType::Extensions);
        let ControlPayload::Extensions(payload) = &env.payload else {
            panic!("expected Extensions payload, got {:?}", env.payload);
        };
        assert_eq!(payload, &advert.local_extensions());
    }

    /// Replacing the local extensions payload broadcasts the NEW payload; an
    /// all-`None` payload is never broadcast (nothing to advertise).
    #[tokio::test]
    async fn update_local_extensions_announces_material_change_and_skips_empty() {
        let (advert, mut cmd_rx) = test_advertiser();
        advert
            .control_announce
            .set_extensions_min_interval(std::time::Duration::ZERO);

        let full = default_local_extensions();
        assert_eq!(
            advert.update_local_extensions(full.clone()).await.unwrap(),
            AnnounceOutcome::Announced
        );
        assert_eq!(advert.local_extensions(), full);
        expect_control_broadcast(&mut cmd_rx, "extensions").await;

        // An all-None payload is a no-op — nothing to advertise.
        assert_eq!(
            advert
                .update_local_extensions(ExtensionsPayload::default())
                .await
                .unwrap(),
            AnnounceOutcome::Unchanged
        );
    }

    /// A freshly connected neighbour immediately receives BOTH the local
    /// capability set and the extensions payload (neighbour-up wiring,
    /// BORU-CP-11/16), even when both are unchanged from the last explicit
    /// announcement (`force=true`).
    #[tokio::test]
    async fn reannounce_on_neighbor_up_broadcasts_caps_and_extensions() {
        let (advert, mut cmd_rx) = test_advertiser();
        let peer = test_key(0xBB);

        advert.reannounce_on_neighbor_up(peer);

        // Two broadcasts: one Capabilities, one Extensions (order
        // nondeterministic — the two fire-and-forget spawns race).
        let mut saw_caps = false;
        let mut saw_ext = false;
        for _ in 0..2 {
            let env = expect_control_broadcast(&mut cmd_rx, "neighbor-up").await;
            match env.message_type {
                ControlMessageType::Capabilities => saw_caps = true,
                ControlMessageType::Extensions => saw_ext = true,
                other => panic!("unexpected neighbor-up message type {other:?}"),
            }
        }
        assert!(saw_caps, "neighbour-up must re-announce capabilities");
        assert!(saw_ext, "neighbour-up must re-announce extensions");
    }
}
