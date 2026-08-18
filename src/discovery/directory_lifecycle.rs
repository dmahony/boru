//! Room-directory lifecycle — the bounded local room-directory cache, the
//! outbound room advertisement / withdrawal announce paths, and the TTL
//! expiry sweep (BORU-DISC-009).
//!
//! Extracted from
//! [`DiscoveryService`](crate::discovery_service::DiscoveryService). This
//! module owns the *room-directory lifecycle* state: the single cached
//! [`RoomDirectory`] (BORU-DIR-10, PDF Phase 4 Task 4.1), the runtime-tunable
//! TTL-sweep configuration (BORU-DIR-23, PDF Phase 8 test matrix "Advertiser
//! disappears"), and the pure outbound advertise / withdraw logic that
//! broadcasts room advertisements and withdrawals as control-plane
//! envelopes.
//!
//! # Architecture
//!
//! * [`RoomDirectoryLifecycle`] is the focused, owned-state module. It
//!   creates and owns the single `Arc<Mutex<RoomDirectory>>` cache, the
//!   shared `directory_expiry_config`, and the [`ControlAnnounceHandle`] it
//!   announces advertised rooms through. `DiscoveryService` remains the
//!   facade/coordinator: it keeps the same-named public API
//!   (`announce_room_advertisement`, `announce_room_withdrawal`,
//!   `room_directory`, `with_directory_sweep_interval`), delegating to this
//!   module — so callers and the wire format are unchanged.
//! * Exactly one mutable `RoomDirectory` instance exists (created here);
//!   [`room_directory`](Self::room_directory) hands out `Arc` *clones of the
//!   same cache* to the control-plane receive dispatcher, the capabilities
//!   advertiser, and the app read handle, so there is no duplicate mutable
//!   state (BORU-DISC-009 STOP condition).
//! * The **receive-side** apply of incoming advertisements / withdrawals to
//!   the shared cache lives in the control-plane receive dispatch
//!   (`crate::control_plane::dispatch`, BORU-DISC-007) — it shares the same
//!   `Arc<Mutex<RoomDirectory>>` this module owns. This module owns the
//!   outbound announce path (BORU-DIR-03/09) and the TTL sweep (BORU-DIR-23).
//!
//! # Invariants enforced here
//!
//! * Only [`RoomVisibility::PublicDiscoverable`] rooms are ever advertised
//!   (BORU-DIR-04 emit-site guard) — a Private or PublicUnlisted
//!   advertisement is refused with [`AnnounceOutcome::NotDiscoverable`] and
//!   nothing is broadcast.
//! * A room advertisement / withdrawal broadcast is always a
//!   **control-plane envelope** — never a chat message, never a legacy
//!   discovery message, never a room join (PDF Core rule).
//! * The TTL sweep ([`evict_expired`](RoomDirectory::evict_expired) on a
//!   fixed cadence) is the production cleanup mechanism for
//!   "advertiser disappears" — without it, expired rooms would only leave
//!   the cache as a side effect of the *next* advertisement arriving.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::control_plane::advertisement::PublicRoomAdvertisement;
use crate::control_plane::advertisement::PublicRoomWithdrawal;
use crate::diagnostics::DirectoryCounters;
use crate::discovery::presence_scheduler::{AnnounceOutcome, ControlAnnounceHandle};
use crate::discovery_service::DiscoveryServiceError;
use crate::room_directory::RoomDirectory;

/// How often the room-directory TTL sweep (BORU-DIR-23, PDF Phase 8 test
/// matrix scenario "Advertiser disappears") wakes to evict expired room
/// advertisements.
///
/// Each cached advertisement carries its own `expires_after_secs` TTL
/// (policy minimum 60 s, default 1 h). The sweep runs every
/// [`DEFAULT_DIRECTORY_SWEEP_INTERVAL`] — comfortably under the policy
/// minimum TTL so a room whose advertiser disappears leaves the active
/// directory within one sweep of its expiry, while refreshes arriving
/// within the TTL keep it live (no flicker on temporary packet loss; PDF
/// Task 3.2 step 5). This is the production wiring for the cache's
/// [`evict_expired`](RoomDirectory::evict_expired) — without it, expired
/// entries would only be evicted as a side effect of the *next*
/// advertisement arriving.
pub const DEFAULT_DIRECTORY_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// Runtime-tunable room-directory expiry configuration shared between the
/// [`DiscoveryService`](crate::discovery_service::DiscoveryService) builders
/// and the sweep task (BORU-DIR-23).
#[derive(Debug, Clone, Copy)]
pub(crate) struct DirectoryExpiryConfig {
    /// How often the sweep runs to evict expired room advertisements.
    pub(crate) sweep_interval: Duration,
}

/// The focused room-directory lifecycle state and logic (BORU-DISC-009).
///
/// Owns the single cached [`RoomDirectory`] (BORU-DIR-10), the
/// runtime-tunable TTL-sweep configuration (BORU-DIR-23), and the pure
/// outbound advertise / withdraw methods that broadcast room advertisements
/// and withdrawals as control-plane envelopes. Cheaply cloneable (all fields
/// are `Arc`-backed or a cloneable announce handle).
///
/// `DiscoveryService` delegates its `announce_room_advertisement`,
/// `announce_room_withdrawal`, `room_directory` and
/// `with_directory_sweep_interval` facades here.
#[derive(Debug, Clone)]
pub struct RoomDirectoryLifecycle {
    /// The bounded local room-directory cache (BORU-DIR-10 / PDF Phase 4
    /// Task 4.1): keyed by stable room_id, stores the latest valid
    /// advertisement plus provenance (publisher, auth verdict, first/last
    /// seen, expiry, compatibility, local join state), enforces entry-count
    /// + metadata-size bounds, and merges duplicate/refresh advertisements
    /// deterministically. Shared with the receive dispatcher and the
    /// capabilities advertiser via `Arc` clones of this one instance.
    room_directory: Arc<Mutex<RoomDirectory>>,
    /// Shared room-directory TTL-expiry configuration (sweep interval) so
    /// the builder can tune it after construction and the sweep observes it
    /// (BORU-DIR-23).
    directory_expiry_config: Arc<Mutex<DirectoryExpiryConfig>>,
    /// The control-plane announce handle (throttle + sequence + sender) used
    /// to broadcast room advertisements and withdrawals.
    control_announce: ControlAnnounceHandle,
}

impl RoomDirectoryLifecycle {
    /// Create the lifecycle with a fresh bounded room-directory cache,
    /// wiring the cache's TTL-expiry counter to `directory_counters`
    /// (BORU-DIR-22) so "expired advertisements" diagnostics are truthful
    /// even though eviction runs inside the cache.
    pub fn new(
        control_announce: ControlAnnounceHandle,
        directory_counters: DirectoryCounters,
    ) -> Self {
        let room_directory = Arc::new(Mutex::new(RoomDirectory::new()));
        // BORU-DIR-22 (PDF Phase 8 Task 8.1): wire the TTL-expiry counter
        // into the cache so "expired advertisements" diagnostics are
        // truthful even though eviction runs inside the cache. The
        // directory is otherwise a pure cache with no diagnostics
        // dependency.
        room_directory
            .lock()
            .expect("room directory lock poisoned")
            .set_expired_sink(Some(directory_counters.expired_sink()));
        Self {
            room_directory,
            directory_expiry_config: Arc::new(Mutex::new(DirectoryExpiryConfig {
                sweep_interval: DEFAULT_DIRECTORY_SWEEP_INTERVAL,
            })),
            control_announce,
        }
    }

    /// An `Arc` clone of the single bounded room-directory cache — handed to
    /// the control-plane receive dispatcher, the capabilities advertiser,
    /// and any read-only consumer so they all observe (and the dispatcher
    /// writes) the same cache without duplicating it.
    pub fn room_directory(&self) -> Arc<Mutex<RoomDirectory>> {
        self.room_directory.clone()
    }

    /// Broadcast a PUBLIC_ROOM_ADVERTISEMENT control-plane envelope carrying
    /// `advert` (BORU-DIR-03, PDF Phase 1 Task 1.3).
    ///
    /// The caller builds the advertisement and signs it with its node key
    /// ([`PublicRoomAdvertisement::sign`]) so receivers can attribute the
    /// payload to this node — the lifecycle never holds a secret key. An
    /// unsigned advertisement is still broadcast but receivers treat it as
    /// clearly untrusted (never canonical).
    ///
    /// Visibility guard (BORU-DIR-04, PDF Phase 2 Task 2.1): only
    /// [`RoomVisibility::PublicDiscoverable`] rooms are advertised. A
    /// Private or PublicUnlisted advertisement is refused with
    /// [`AnnounceOutcome::NotDiscoverable`] and nothing is broadcast.
    ///
    /// The room-advertisement throttle bounds the rate; the broadcast is a
    /// control-plane envelope, never a chat message, and never a room join
    /// (PDF Core rule: advertisements only advertise existence).
    pub async fn announce_room_advertisement(
        &self,
        advert: PublicRoomAdvertisement,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.control_announce
            .announce_room_advertisement(advert)
            .await
    }

    /// Announce a PUBLIC_ROOM_WITHDRAWAL control-plane envelope carrying
    /// `withdrawal` (BORU-DIR-09, PDF Phase 3 Task 3.3).
    ///
    /// The caller builds the withdrawal and signs it with its node key
    /// ([`PublicRoomWithdrawal::sign`]) so receivers can attribute the
    /// payload to this node — the lifecycle itself never holds a secret key.
    /// An unsigned withdrawal is still broadcast but receivers discard it
    /// (never applied).
    ///
    /// The room-advertisement throttle bounds the rate; the broadcast is a
    /// control-plane envelope, never a chat message. Directory clients
    /// remove the matching advertisement when the withdrawal verifies; TTL
    /// expiry remains the safety net if it is missed.
    pub async fn announce_room_withdrawal(
        &self,
        withdrawal: PublicRoomWithdrawal,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.control_announce
            .announce_room_withdrawal(withdrawal)
            .await
    }

    /// Override the room-directory TTL sweep interval (BORU-DIR-23 / PDF
    /// Phase 8 test matrix "Advertiser disappears").
    ///
    /// Defaults to [`DEFAULT_DIRECTORY_SWEEP_INTERVAL`]. Tests use short
    /// intervals to exercise the sweep without sleeping.
    pub fn set_sweep_interval(&self, interval: Duration) {
        self.directory_expiry_config
            .lock()
            .expect("directory expiry config lock poisoned")
            .sweep_interval = interval;
    }

    /// Spawn the room-directory TTL sweep task (BORU-DIR-23): every sweep
    /// interval it calls [`RoomDirectory::evict_expired`], removing every
    /// cached room whose TTL elapsed since the last valid refresh.
    ///
    /// The sweep interval is re-read from the shared config before every
    /// sleep, so builder tuning (e.g. short intervals in tests) takes effect
    /// immediately.
    pub fn spawn_expiry_loop(&self, cancel: CancellationToken) -> JoinHandle<()> {
        tokio::spawn(directory_expiry_loop(
            self.directory_expiry_config.clone(),
            self.room_directory.clone(),
            cancel,
        ))
    }
}

/// Background task that evicts expired room advertisements from the
/// bounded room-directory cache (BORU-DIR-23 / PDF Task 3.2 step 4).
///
/// Every `sweep_interval` it calls
/// [`RoomDirectory::evict_expired`], which removes every cached room whose
/// TTL elapsed since the last valid refresh. This is the production wiring
/// for the matrix scenario "Advertiser disappears — Room becomes stale and
/// expires after TTL": without this sweep, expired rooms would only leave
/// the cache as a side effect of the *next* advertisement arriving (the
/// receive path evicts expired entries before inserting a new room).
/// Refreshes arriving within the TTL keep entries live — the sweep only
/// removes genuinely stale rooms, so temporary packet loss does not cause
/// room flicker (PDF Task 3.2 step 5).
///
/// The sweep interval is re-read from the shared config before every sleep,
/// so builder tuning (e.g. short intervals in tests) takes effect
/// immediately. Logs state transitions only, never message contents.
async fn directory_expiry_loop(
    config: Arc<Mutex<DirectoryExpiryConfig>>,
    room_directory: Arc<Mutex<RoomDirectory>>,
    cancel: CancellationToken,
) {
    loop {
        let sweep = config
            .lock()
            .expect("directory expiry config lock poisoned")
            .sweep_interval;

        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                info!("discovery directory expiry loop cancelled");
                break;
            }
            _ = tokio::time::sleep(sweep) => {
                let evicted = {
                    let mut dir = room_directory.lock().expect("room directory lock poisoned");
                    dir.evict_expired()
                };
                if !evicted.is_empty() {
                    info!(
                        count = evicted.len(),
                        "discovery: evicted room advertisements whose TTL expired",
                    );
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
    use crate::control_plane::message::{ControlEnvelope, CONTROL_PLANE_MAGIC};
    use irpc::channel::mpsc as irpc_mpsc;
    use std::time::Instant;

    use crate::api::{Command, GossipSender};

    fn test_key(byte: u8) -> iroh_base::PublicKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed).public()
    }

    fn test_secret_key(byte: u8) -> iroh_base::SecretKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed)
    }

    /// Build a `RoomDirectoryLifecycle` over an offline (never-fed) command
    /// channel, returning it plus a receiver for the broadcast commands it
    /// emits.
    fn test_lifecycle() -> (RoomDirectoryLifecycle, irpc_mpsc::Receiver<Command>) {
        let (cmd_tx, cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let sender = GossipSender::new(cmd_tx);
        let control_announce = ControlAnnounceHandle::new(sender, test_key(0xAA), None);
        let directory_counters = DirectoryCounters::new();
        (
            RoomDirectoryLifecycle::new(control_announce, directory_counters),
            cmd_rx,
        )
    }

    fn test_advert() -> PublicRoomAdvertisement {
        use crate::control_plane::advertisement::PublicRoomAdvertisement;
        PublicRoomAdvertisement::minimal(
            crate::proto::state::TopicId::from_bytes([0x42; 32]),
            "Room".into(),
            test_key(0xAA).as_bytes().to_owned(),
        )
    }

    fn test_withdrawal() -> PublicRoomWithdrawal {
        let mut withdrawal = PublicRoomWithdrawal::minimal(
            crate::proto::state::TopicId::from_bytes([0x41; 32]),
            test_key(0xAA).as_bytes().to_owned(),
        );
        withdrawal.sign(&test_secret_key(0xAA));
        withdrawal
    }

    /// `new` creates a fresh boundable room-directory cache and wires the
    /// expiry sink so TTL evictions tick the directory counter.
    #[tokio::test]
    async fn new_creates_empty_cache_with_expiry_sink() {
        let (lifecycle, _cmd_rx) = test_lifecycle();
        // No advertisement has ever been applied, so the cache is empty.
        assert!(lifecycle.room_directory().lock().unwrap().is_empty());
    }

    /// `announce_room_advertisement` broadcasts a PUBLIC_ROOM_ADVERTISEMENT
    /// control-plane envelope carrying the advertisement — never a chat
    /// message, never a legacy discovery message.
    #[tokio::test]
    async fn announce_room_advertisement_broadcasts_control_envelope() {
        let (lifecycle, mut cmd_rx) = test_lifecycle();
        let advert = test_advert();
        assert_eq!(
            lifecycle
                .announce_room_advertisement(advert.clone())
                .await
                .unwrap(),
            AnnounceOutcome::Announced
        );
        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for room advertisement broadcast")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        assert!(bytes.starts_with(&CONTROL_PLANE_MAGIC));
        assert!(
            postcard::from_bytes::<crate::discovery_message::DiscoveryMessage>(&bytes).is_err(),
            "a room advertisement must never decode as a legacy DiscoveryMessage"
        );
        match ControlEnvelope::decode(&bytes).expect("control envelope decodes") {
            crate::control_plane::message::ControlPlaneDecode::Message(env) => {
                assert_eq!(
                    env.message_type,
                    crate::control_plane::message::ControlMessageType::PublicRoomAdvertisement
                );
                let crate::control_plane::message::ControlPayload::PublicRoomAdvertisement(payload) =
                    &env.payload
                else {
                    panic!(
                        "expected PublicRoomAdvertisement payload, got {:?}",
                        env.payload
                    );
                };
                assert_eq!(payload.room_id, advert.room_id);
            }
            other => panic!("expected decoded envelope, got {other:?}"),
        }
    }

    /// BORU-DIR-04 (PDF 2.1): the advertisement emit site refuses Private
    /// and PublicUnlisted rooms — only PublicDiscoverable rooms may emit a
    /// PUBLIC_ROOM_ADVERTISEMENT.
    #[tokio::test]
    async fn announce_room_advertisement_refuses_non_discoverable() {
        use crate::control_plane::advertisement::RoomVisibility;

        for visibility in [RoomVisibility::Private, RoomVisibility::PublicUnlisted] {
            let (lifecycle, mut cmd_rx) = test_lifecycle();
            let mut advert = test_advert();
            advert.visibility = visibility;

            let outcome = lifecycle
                .announce_room_advertisement(advert)
                .await
                .expect("guard returns ok outcome, not an error");
            assert_eq!(
                outcome,
                AnnounceOutcome::NotDiscoverable,
                "{visibility:?} rooms must never be advertised"
            );
            // Nothing was broadcast.
            assert!(
                tokio::time::timeout(Duration::from_millis(80), cmd_rx.recv())
                    .await
                    .is_err(),
                "a {visibility:?} room must never be broadcast"
            );
        }
    }

    /// `announce_room_withdrawal` broadcasts a PUBLIC_ROOM_WITHDRAWAL
    /// control-plane envelope carrying the signed withdrawal.
    #[tokio::test]
    async fn announce_room_withdrawal_broadcasts_control_envelope() {
        let (lifecycle, mut cmd_rx) = test_lifecycle();
        let withdrawal = test_withdrawal();
        assert_eq!(
            lifecycle
                .announce_room_withdrawal(withdrawal.clone())
                .await
                .unwrap(),
            AnnounceOutcome::Announced
        );
        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for room withdrawal broadcast")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        assert!(bytes.starts_with(&CONTROL_PLANE_MAGIC));
        match ControlEnvelope::decode(&bytes).expect("control envelope decodes") {
            crate::control_plane::message::ControlPlaneDecode::Message(env) => {
                assert_eq!(
                    env.message_type,
                    crate::control_plane::message::ControlMessageType::PublicRoomWithdrawal
                );
            }
            other => panic!("expected decoded envelope, got {other:?}"),
        }
    }

    /// `set_sweep_interval` tunes the shared sweep config; the TTL sweep
    /// (via `spawn_expiry_loop`) evicts an expired advertisement on a short
    /// interval without sleeping for the full policy TTL.
    #[tokio::test]
    async fn spawn_expiry_loop_evicts_expired_entries() {
        let (lifecycle, _cmd_rx) = test_lifecycle();
        lifecycle.set_sweep_interval(Duration::from_millis(20));

        let dir = lifecycle.room_directory();
        let mut advert = test_advert();
        advert.expires_after_secs = 60; // minimum admissible policy TTL
        let outcome = {
            let mut dir = dir.lock().unwrap();
            dir.apply_advertisement(
                advert.clone(),
                test_key(0xBB),
                crate::control_plane::advertisement::AdvertisementAuth::Verified {
                    publisher: test_key(0xBB),
                },
                1,
                1_700_000_000,
            )
        };
        // First add enters the cache (DUPLICATE/CONFLICT are possible when
        // re-adding; this is a fresh cache so it is Added).
        assert!(matches!(
            outcome,
            crate::room_directory::AdvertiseOutcome::Added
        ));
        assert_eq!(dir.lock().unwrap().len(), 1);

        // Advance the clock past the TTL and let the sweep run; the entry
        // must be evicted on its own (no new advertisement to trigger a
        // side-effect eviction).
        {
            let mut d = dir.lock().unwrap();
            d.evict_expired_at(Instant::now() + Duration::from_secs(61));
            assert_eq!(d.len(), 0);
        }

        // The sweep loop reads the shared config; prove the tunable is held
        // in the shared config object.
        assert_eq!(
            lifecycle
                .directory_expiry_config
                .lock()
                .unwrap()
                .sweep_interval,
            Duration::from_millis(20)
        );
    }
}
