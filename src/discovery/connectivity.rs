//! Connectivity wiring for the discovery facade (BORU-DISC-11).
//!
//! Extracted from [`DiscoveryService`](crate::discovery_service::DiscoveryService):
//! the background loop that turns discovery peer updates into connectivity
//! actions (dial every newly discovered peer into the discovery gossip mesh),
//! plus the single, deduplicated `join_peers` dial (`maybe_dial`) it drives.
//!
//! This is the Phase-4 "use discovery only to improve connectivity" wiring —
//! the same mechanism the mDNS handler in `main.rs` and
//! [`DynamicPeerJoiner`](crate::dynamic_joiner::DynamicPeerJoiner) use for
//! mDNS/DHT results. Dialing a peer improves the mesh/address book but never
//! grants friendship, group membership, or a conversation — no
//! [`FriendsStore`](crate::friends::FriendsStore), no
//! [`ConversationStore`](crate::conversations::ConversationStore), and no
//! chat payload ever crosses the discovery topic.
//!
//! The module owns **no** shared mutable state of its own: it only drives the
//! shared [`PeerConnectivityStore`] and [`ReconnectScheduler`] handles that
//! the facade creates and passes in, and its only task-local state is the
//! `dialed: HashSet<EndpointId>` one-per-service-lifetime dial-dedup set.
//! Net-gated: it binds a [`GossipSender`] to dial, so it only exists with the
//! `net` feature, mirroring `discovery_service`.
//!
//! Invariants preserved from the facade (see
//! [`crate::discovery_service`] module docs, § 7.3):
//! * each peer is dialed at most once per service lifetime (dedup by endpoint
//!   id); a `PeerUpdate::Seen` refresh or repeat advertisement never re-dials;
//! * the local node is never dialed;
//! * a dial feeds the connectivity state machine with the real
//!   [`ConnectivityEvent`]s (`EndpointConnecting` before, `EndpointConnected`
//!   / `EndpointFailed` after), and a successful dial clears any queued
//!   reconnect backoff — but discovery traffic alone never fabricates a
//!   transition and never clears backoff on its own.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use iroh_base::PublicKey;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};

use crate::api::GossipSender;
use crate::control_plane::connectivity::{ConnectivityEvent, PeerConnectivityStore};
use crate::control_plane::reconnect::ReconnectScheduler;
use crate::discovery_service::PeerUpdate;

/// Background task that turns discovery peer updates into connectivity
/// actions: every newly discovered peer is dialed into the discovery gossip
/// mesh via [`GossipSender::join_peers`].
///
/// Deduplication: each peer is dialed at most once per service lifetime
/// (tracked by endpoint id). A `PeerUpdate::Seen` refresh or repeat
/// advertisement does not re-dial. The local node is never dialed.
///
/// See the module docs for the structural invariants (connectivity only,
/// never friendship/group/conversation; no chat payload on the discovery
/// topic; real connection events feed the state machine).
pub(crate) async fn connectivity_loop(
    sender: GossipSender,
    mut updates: broadcast::Receiver<PeerUpdate>,
    connectivity: Arc<Mutex<PeerConnectivityStore>>,
    reconnect: Arc<Mutex<ReconnectScheduler>>,
    local_node: PublicKey,
    cancel: CancellationToken,
) {
    let mut dialed: HashSet<iroh_base::EndpointId> = HashSet::new();
    // BORU-CP-13: a slow periodic debug dump of the per-peer diagnostic
    // snapshots, so `RUST_LOG=debug` shows the full stage timeline
    // (discovery / endpoint / path / topic / gossip / decode / delivery)
    // without any extra tooling. Guarded by `tracing::enabled!` so the
    // render cost is zero when debug logging is off.
    let mut dump_interval = tokio::time::interval(Duration::from_secs(60));
    dump_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    dump_interval.tick().await; // consume the immediate first tick
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                debug!("discovery connectivity loop cancelled");
                break;
            }
            _ = dump_interval.tick() => {
                if tracing::enabled!(tracing::Level::DEBUG) {
                    let store = connectivity.lock().expect("connectivity store lock poisoned");
                    let snapshots =
                        crate::control_plane::diagnostics::snapshots_for(&store, &local_node, Instant::now());
                    for snap in &snapshots {
                        debug!(%snap, "diagnostics: per-peer snapshot");
                    }
                }
            }
            update = updates.recv() => {
                match update {
                    Ok(PeerUpdate::Seen { node_id, .. }) => {
                        maybe_dial(
                            &sender,
                            &connectivity,
                            &reconnect,
                            &mut dialed,
                            local_node,
                            node_id,
                        )
                        .await;
                    }
                    Ok(PeerUpdate::Advertised { advertised, .. }) => {
                        maybe_dial(
                            &sender,
                            &connectivity,
                            &reconnect,
                            &mut dialed,
                            local_node,
                            advertised,
                        )
                        .await;
                    }
                    Ok(PeerUpdate::Expired { .. }) => {
                        // The peer went stale (BORU-CP-03 TTL expiry). No
                        // dial action: it was already dialed when first
                        // seen, and expiry does not revoke connectivity —
                        // it only removes it from active presence.
                        trace!("discovery: expired peer ignored by connectivity loop");
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        debug!("discovery connectivity loop lagged");
                    }
                    Err(_) => break,
                }
            }
        }
    }
    debug!("discovery connectivity loop exited");
}

/// Dial `peer` into the discovery gossip mesh once (deduplicated).
///
/// Connectivity only: `join_peers` makes the gossip actor establish a mesh
/// edge / resolve the peer's address book entry through the existing
/// mechanisms — it never creates friends, groups, or conversations.
///
/// BORU-CP-05: feeds the peer connectivity state machine with the dial
/// result — [`ConnectivityEvent::EndpointConnecting`] before the dial and
/// [`ConnectivityEvent::EndpointConnected`] / [`ConnectivityEvent::EndpointFailed`]
/// afterwards. Duplicate dials are filtered by `dialed`, and a duplicate
/// `EndpointConnecting` is an idempotent no-op in the state machine, so a
/// flood of announcements can never cause a connection loop.
async fn maybe_dial(
    sender: &GossipSender,
    connectivity: &Arc<Mutex<PeerConnectivityStore>>,
    reconnect: &Arc<Mutex<ReconnectScheduler>>,
    dialed: &mut HashSet<iroh_base::EndpointId>,
    local_node: PublicKey,
    peer: PublicKey,
) {
    if peer == local_node {
        trace!(peer = %peer.fmt_short(), "discovery: not dialing self");
        return;
    }
    let endpoint: iroh_base::EndpointId = peer.into();
    if !dialed.insert(endpoint) {
        trace!(peer = %peer.fmt_short(), "discovery: peer already dialed");
        return;
    }
    {
        let mut store = connectivity
            .lock()
            .expect("connectivity store lock poisoned");
        store.apply(peer, ConnectivityEvent::EndpointConnecting, Instant::now());
    }
    match sender.join_peers(vec![endpoint]).await {
        Ok(()) => {
            info!(peer = %peer.fmt_short(), "discovery: dialed discovered peer for connectivity");
            {
                let mut store = connectivity
                    .lock()
                    .expect("connectivity store lock poisoned");
                store.apply(peer, ConnectivityEvent::EndpointConnected, Instant::now());
            }
            // BORU-CP-07: the endpoint dial succeeded — a real connection
            // event. Cancel any queued reconnect attempt for this peer so
            // the reconnect loop does not dial again redundantly.
            {
                let mut scheduler = reconnect.lock().expect("reconnect scheduler lock poisoned");
                scheduler.reset(&peer);
            }
        }
        Err(error) => {
            warn!(
                peer = %peer.fmt_short(),
                error = %error,
                "discovery: join_peers failed",
            );
            {
                let mut store = connectivity
                    .lock()
                    .expect("connectivity store lock poisoned");
                store.apply_with_error(
                    peer,
                    ConnectivityEvent::EndpointFailed,
                    Some(error.to_string()),
                    Instant::now(),
                );
            }
        }
    }
}
