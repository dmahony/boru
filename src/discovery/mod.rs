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

/// Capabilities / extensions advertisement — the local capability set and
/// Phase 6 extensions payload plus the update/announce + neighbour-up wiring
/// (BORU-DISC-008). Net-gated: it drives `ControlAnnounceHandle` broadcasts,
/// so it only exists with the `net` feature, mirroring `discovery_service`
/// and `presence_scheduler`.
#[cfg(feature = "net")]
pub mod caps_advertise;
/// Connectivity wiring — the background loop that turns discovery peer
/// updates into connectivity actions (dial every newly discovered peer into
/// the discovery gossip mesh) plus the deduplicated single dial
/// (BORU-DISC-11). Net-gated: it binds a `GossipSender` to dial, so it only
/// exists with the `net` feature, mirroring `discovery_service`. Owns no
/// shared mutable state of its own (only drives the shared connectivity /
/// reconnect handles the facade passes in).
#[cfg(feature = "net")]
pub mod connectivity;
/// Room-directory lifecycle — the bounded room-directory cache, the outbound
/// room advertisement / withdrawal announce paths, and the TTL expiry sweep
/// (BORU-DISC-009). Net-gated: it drives `ControlAnnounceHandle` broadcasts,
/// so it only exists with the `net` feature, mirroring `discovery_service`
/// and `presence_scheduler`.
#[cfg(feature = "net")]
pub mod directory_lifecycle;
/// Per-peer path classification sweep — the periodic background task that
/// asks iroh for each tracked peer's current transport addresses and records
/// the classified path (direct / relay / transitioning) via the
/// diagnostic-only path events (BORU-CP-14). Net-gated: it needs a live
/// `iroh::Endpoint`, so it only exists with the `net` feature, mirroring
/// `discovery_service`. Owns no shared mutable state of its own.
#[cfg(feature = "net")]
pub mod path_refresh;
pub mod peer_registry;
/// Announcement and presence scheduling — the announce throttles, the
/// legacy/control announce handles, and the presence refresh/expiry timers
/// (BORU-DISC-005). Net-gated: the handles/loops drive `GossipSender`
/// broadcasts, so they (and their configs) only exist with the `net`
/// feature, mirroring `discovery_service` itself.
#[cfg(feature = "net")]
pub mod presence_scheduler;
