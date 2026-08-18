//! Internal discovery subsystem — focused, owned-state modules extracted
//! from [`DiscoveryService`](crate::discovery_service::DiscoveryService).
//!
//! `DiscoveryService` remains the facade/coordinator (it owns the join,
//! publish, and receive-path wiring plus the `Arc<Mutex<...>>` handles into
//! the shared stores). This directory holds the cohesive, pure concern
//! modules it delegates to. Each module owns a single architectural
//! concern, exposes a narrow API, and keeps its pure logic unit-testable in
//! isolation (no network, no peers).
//!
//! This is the first module of the extraction series (BORU-DISC-004): the
//! peer registry + `(node_id, event_id)` dedup policy. Later tasks add the
//! announcement/presence, reconnect, control-plane, capabilities, and
//! room-directory concerns here.

pub mod peer_registry;
/// Announcement and presence scheduling — the announce throttles, the
/// legacy/control announce handles, and the presence refresh/expiry timers
/// (BORU-DISC-005). Net-gated: the handles/loops drive `GossipSender`
/// broadcasts, so they (and their configs) only exist with the `net`
/// feature, mirroring `discovery_service` itself.
#[cfg(feature = "net")]
pub mod presence_scheduler;
/// Capabilities / extensions advertisement — the local capability set and
/// Phase 6 extensions payload plus the update/announce + neighbour-up wiring
/// (BORU-DISC-008). Net-gated: it drives `ControlAnnounceHandle` broadcasts,
/// so it only exists with the `net` feature, mirroring `discovery_service`
/// and `presence_scheduler`.
#[cfg(feature = "net")]
pub mod caps_advertise;
