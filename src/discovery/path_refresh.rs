//! Per-peer path classification sweep (BORU-CP-14).
//!
//! Extracted from [`DiscoveryService`](crate::discovery_service::DiscoveryService):
//! the periodic background task that asks iroh for each tracked peer's current
//! transport addresses and records the classified path (direct / relay /
//! transitioning) in the shared [`PeerConnectivityStore`] via the
//! diagnostic-only path events.
//!
//! The module owns the pure classification policy ([`classify_path_addrs`],
//! [`PathAddrKind`], [`classify_peer_path`]) plus the sweep loop that drives
//! it. It owns no shared mutable state of its own — it only reads/writes the
//! shared connectivity-store handle the facade passes in.
//!
//! BORU-CP-14 makes the path events diagnostic-only: they update the per-peer
//! `path_kind` hint and log path changes in structured logs, but they never
//! move the state machine and never reset or duplicate conversation state
//! (PDF Task 5.2). A relay-only path is NOT a failure — a peer on a relay
//! path stays `Reachable`. Peers with no information in iroh's remote map
//! ([`PathKind::Unknown`]) are skipped entirely — a lack of information must
//! never fabricate a path label or refresh a peer's liveness (which would
//! defeat TTL expiry).
//!
//! Net-gated: the sweep needs a live [`iroh::Endpoint`], so it only exists
//! with the `net` feature, mirroring `discovery_service`.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use iroh_base::PublicKey;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::control_plane::connectivity::{ConnectivityEvent, PathKind, PeerConnectivityStore};

/// BORU-CP-14: how often the path-refresh sweep re-classifies every tracked
/// peer's current path from the iroh endpoint (seconds). Diagnostic
/// cadence; not latency-critical.
const PATH_REFRESH_INTERVAL_SECS: u64 = 15;

/// The transport kind of one address in iroh's `remote_info` snapshot,
/// reduced for classification (BORU-CP-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathAddrKind {
    /// A direct IP transport address.
    Ip,
    /// A relay server address.
    Relay,
    /// Any other (custom) transport address.
    Other,
}

/// Pure classification of an iroh `remote_info` snapshot into a path kind
/// (BORU-CP-14, PDF Task 5.2 step 1). Testable without a live endpoint.
///
/// `addrs` is `(kind, active)` for every known transport address.
/// Classification:
///
/// * any **active IP** path → [`PathKind::Direct`] (a direct path is open;
///   a relay fallback may also be open),
/// * otherwise any **active relay** path → [`PathKind::Relay`] (the peer is
///   reachable via relay right now — still reachable),
/// * otherwise known addresses but **none active** → [`PathKind::Transitioning`]
///   (path in flux: connecting / re-negotiating between direct and relay),
/// * no addresses at all → [`PathKind::Unknown`] (no reliable
///   classification — report Unknown rather than guessing).
///
/// The result is diagnostic/optimization metadata only; it never proves
/// application-level success and chat delivery never depends on it.
fn classify_path_addrs(addrs: impl IntoIterator<Item = (PathAddrKind, bool)>) -> PathKind {
    let mut has_any = false;
    let mut has_active_relay = false;
    for (kind, active) in addrs {
        has_any = true;
        if active {
            match kind {
                PathAddrKind::Ip => return PathKind::Direct,
                PathAddrKind::Relay => has_active_relay = true,
                PathAddrKind::Other => {}
            }
        }
    }
    if has_active_relay {
        PathKind::Relay
    } else if has_any {
        PathKind::Transitioning
    } else {
        PathKind::Unknown
    }
}

/// Classify one peer's current path from iroh's `remote_info` snapshot.
/// `None` (no information for the peer in the endpoint's remote map) →
/// [`PathKind::Unknown`].
async fn classify_peer_path(endpoint: &iroh::Endpoint, peer: PublicKey) -> PathKind {
    let endpoint_id: iroh_base::EndpointId = peer;
    let Some(info) = endpoint.remote_info(endpoint_id).await else {
        return PathKind::Unknown;
    };
    classify_path_addrs(info.addrs().map(|addr| {
        let kind = if addr.addr().is_ip() {
            PathAddrKind::Ip
        } else if addr.addr().is_relay() {
            PathAddrKind::Relay
        } else {
            PathAddrKind::Other
        };
        (
            kind,
            matches!(addr.usage(), iroh::endpoint::TransportAddrUsage::Active),
        )
    }))
}

/// BORU-CP-14: periodic per-peer path classification sweep.
///
/// Every [`PATH_REFRESH_INTERVAL_SECS`] the loop asks iroh for each tracked
/// peer's current transport addresses and records the classified path
/// (direct / relay / transitioning) in the connectivity store via the
/// diagnostic-only path events. Path *changes* are logged in structured
/// logs (`connectivity: peer path changed` at `info!`, from
/// [`PeerConnectivityStore::apply`]); path events never move the state
/// machine and never reset or duplicate conversation state (PDF Task 5.2).
///
/// Peers with no information in iroh's remote map ([`PathKind::Unknown`])
/// are skipped entirely — a lack of information must never fabricate a path
/// label or refresh a peer's liveness (which would defeat TTL expiry).
pub(crate) async fn path_refresh_loop(
    endpoint: iroh::Endpoint,
    connectivity: Arc<Mutex<PeerConnectivityStore>>,
    cancel: CancellationToken,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(PATH_REFRESH_INTERVAL_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await; // consume the immediate first tick
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                debug!("discovery path refresh cancelled");
                break;
            }
            _ = interval.tick() => {
                let peers: Vec<PublicKey> = {
                    let store = connectivity.lock().expect("connectivity store lock poisoned");
                    store.peers().map(|(pk, _)| *pk).collect()
                };
                for peer in peers {
                    let kind = classify_peer_path(&endpoint, peer).await;
                    let event = match kind {
                        PathKind::Direct => ConnectivityEvent::PathChangedDirect,
                        PathKind::Relay => ConnectivityEvent::PathChangedRelay,
                        PathKind::Transitioning => ConnectivityEvent::PathChangedTransitioning,
                        PathKind::Unknown => continue,
                    };
                    let mut store = connectivity.lock().expect("connectivity store lock poisoned");
                    store.apply(peer, event, Instant::now());
                }
            }
        }
    }
    debug!("discovery path refresh exited");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Any active IP path classifies Direct, even when a relay path is
    /// also open.
    #[test]
    fn classify_direct_when_any_active_ip_path() {
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Relay, true), (PathAddrKind::Ip, true),]),
            PathKind::Direct
        );
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Ip, true), (PathAddrKind::Relay, true),]),
            PathKind::Direct
        );
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Ip, true)]),
            PathKind::Direct
        );
    }

    /// No active IP path but an active relay path classifies Relay — a
    /// relay connection is still considered reachable (BORU-CP-14).
    #[test]
    fn classify_relay_when_only_active_relay_path() {
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Ip, false), (PathAddrKind::Relay, true),]),
            PathKind::Relay
        );
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Relay, true), (PathAddrKind::Other, true),]),
            PathKind::Relay,
            "custom transports never beat an active relay path"
        );
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Relay, true)]),
            PathKind::Relay
        );
    }

    /// Known addresses but none active classify Transitioning (path in
    /// flux: connecting / re-negotiating).
    #[test]
    fn classify_transitioning_when_no_active_path() {
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Ip, false)]),
            PathKind::Transitioning
        );
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Ip, false), (PathAddrKind::Relay, false),]),
            PathKind::Transitioning
        );
    }

    /// No addresses at all classify Unknown — report Unknown rather than
    /// guessing.
    #[test]
    fn classify_unknown_when_no_addresses() {
        assert_eq!(
            classify_path_addrs([] as [(PathAddrKind, bool); 0]),
            PathKind::Unknown
        );
    }
}
