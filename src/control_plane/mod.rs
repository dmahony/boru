//! Boru hidden-discovery control plane (BORU-CP).
//!
//! The control plane carries **metadata about reachability, state,
//! capabilities, and protocol compatibility** on the internal discovery
//! topic. It must never carry user chat text, attachment data, file bytes,
//! group history, tunnel payloads, or call media — those stay on their own
//! authenticated data-plane channels.
//!
//! | Plane | Purpose |
//! |-------|---------|
//! | Control plane | discover / negotiate / diagnose |
//! | Data plane    | direct chat / groups / files / tunnels / calls / screen sharing |
//!
//! Module layout:
//! * [`message`] — the versioned, typed control-plane message envelope
//!   ([`ControlEnvelope`](message::ControlEnvelope)) plus its strict
//!   forward-compatible wire decoder.
//! * [`privacy`] — the BORU-CP-03 privacy/abuse layer: minimal-advertisement
//!   whitelist policy, per-sender rate limiting, `(sender, sequence)` dedup,
//!   TTL-based presence expiry, and sender attribution ([`ControlPlaneGuard`]).
//! * [`connectivity`] — the BORU-CP-05 explicit peer connectivity state
//!   machine: states (Unknown / Discovered / Connecting / Reachable /
//!   DirectTopicReady / Degraded / OfflineStale), the deterministic
//!   transition table, and a bounded per-peer transition trail
//!   ([`PeerConnectivityStore`]).
//!
//! Later tasks in the BORU-CP chain add capability negotiation (CP-05 is
//! the state machine; capabilities are Phase 4) and diagnostics (Phase 5)
//! as siblings of [`message`].

pub mod connectivity;
pub mod message;
pub mod privacy;

pub use connectivity::*;
pub use message::*;
pub use privacy::*;
