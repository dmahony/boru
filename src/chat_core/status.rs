//! Connection status and mesh-health types.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use iroh::{Endpoint, PublicKey, RelayMode};

use crate::proto::TopicId;

// ── Status context ────────────────────────────────────────────────────────────

/// Overall mesh health summary shown in the status panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshHealth {
    /// The mesh looks healthy right now.
    Good,
    /// The mesh is connected but some peers have gone quiet.
    Degraded(String),
    /// The transport is offline.
    Offline(String),
}

/// Connection status information displayed in the status panel.
#[derive(Clone, Debug)]
pub struct StatusContext {
    /// Human-readable transport status message.
    pub transport_status: String,
    /// The gossip topic for this chat room.
    pub topic: TopicId,
    /// Relay configuration.
    pub relay_mode: RelayMode,
    /// Whether we are connected to peers.
    pub connected: bool,
    /// Number of known peers.
    pub peer_count: usize,
    /// Our display name / label.
    pub identity_label: String,
    /// A notice about the transport (shown in the status panel).
    pub transport_notice: String,
    /// Number of peers with a direct (hole-punched) connection.
    pub direct_peers: usize,
    /// Number of peers connected through a relay server.
    pub relayed_peers: usize,
    /// Set of peer PublicKeys that are currently gossip neighbors.
    pub neighbors: HashSet<PublicKey>,
    /// Cached per-peer connection type (direct vs relay).
    pub peer_connection_types: HashMap<PublicKey, ConnectionType>,
    /// Last time we saw any gossip activity from each peer.
    pub last_activity: HashMap<PublicKey, Instant>,
    /// Measured round-trip latency per peer, populated by periodic
    /// [`crate::chat_core::Message::LatencyPing`] / [`crate::chat_core::Message::LatencyPong`]
    /// probes.
    pub peer_latencies: HashMap<PublicKey, Duration>,
    /// Current mesh health summary for the UI.
    pub mesh_health: MeshHealth,
    /// Whether private-room DHT discovery is enabled.
    pub dht_enabled: bool,
    /// Number of peers discovered via DHT.
    pub dht_peer_count: usize,
}

impl StatusContext {
    /// Recompute the mesh health from the latest gossip activity and transport state.
    pub async fn recompute_mesh_health(&mut self, endpoint: &Endpoint) {
        let now = Instant::now();
        let stale_threshold = Duration::from_secs(120);
        let stale_peer = self.neighbors.iter().find_map(|peer| {
            self.last_activity.get(peer).and_then(|seen_at| {
                let age = now.saturating_duration_since(*seen_at);
                (age > stale_threshold).then_some((*peer, age))
            })
        });

        let online = tokio::time::timeout(Duration::from_millis(0), endpoint.online())
            .await
            .is_ok();

        let new_health = if !online {
            MeshHealth::Offline("iroh endpoint is offline".to_string())
        } else if let Some((peer, age)) = stale_peer {
            MeshHealth::Degraded(format!(
                "peer {} has been quiet for {}s",
                peer.fmt_short(),
                age.as_secs()
            ))
        } else {
            MeshHealth::Good
        };

        if new_health != self.mesh_health {
            match &new_health {
                MeshHealth::Good => {}
                MeshHealth::Degraded(reason) | MeshHealth::Offline(reason) => {
                    tracing::warn!("mesh health degraded: {reason}");
                }
            }
        }

        self.mesh_health = new_health;
    }

    /// Check the current mesh health against a previously observed state and
    /// return an optional user-facing notification message on transition.
    ///
    /// Returns `Some(notification)` when the mesh health has changed since
    /// `last_health` was recorded, or `None` on the first call or when the
    /// state has not changed.
    ///
    /// The caller should display the returned message to the user (e.g. as a
    /// system notification in the chat log) and persist the updated
    /// `last_health` for future calls.
    pub fn check_mesh_quiescence(&self, last_health: &mut Option<MeshHealth>) -> Option<String> {
        let current_health = &self.mesh_health;
        let notification = match (last_health.as_ref(), current_health) {
            // Good → Degraded: warn the user
            (Some(MeshHealth::Good), MeshHealth::Degraded(reason)) => {
                Some(format!("⚠ Mesh health degraded: {reason}"))
            }
            // Good → Offline: warn the user
            (Some(MeshHealth::Good), MeshHealth::Offline(reason)) => {
                Some(format!("⚠ Mesh offline: {reason}"))
            }
            // Degraded → Good: recovery
            (Some(MeshHealth::Degraded(_)), MeshHealth::Good) => {
                Some("✓ Mesh health recovered: all peers are active.".to_string())
            }
            // Offline → Good: recovery
            (Some(MeshHealth::Offline(_)), MeshHealth::Good) => {
                Some("✓ Mesh health recovered: endpoint is back online.".to_string())
            }
            // First check: don't notify
            (None, _) => None,
            // Same state or other transitions: no notification
            _ => None,
        };
        *last_health = Some(current_health.clone());
        notification
    }
}

/// Whether a peer's connection goes through a relay server or directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionType {
    /// Peer has at least one direct (IP-based) address.
    Direct,
    /// Peer is reachable only via a relay server.
    Relayed,
    /// Connection type is unknown (not a neighbor, or no info yet).
    Unknown,
}

