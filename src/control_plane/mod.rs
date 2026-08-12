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
//!
//! Later tasks in the BORU-CP chain add the service boundary (CP-02),
//! presence state (CP-04), capability negotiation (CP-05), and diagnostics
//! (CP-06) as siblings of [`message`].

pub mod message;
pub mod privacy;

pub use message::*;
pub use privacy::*;
