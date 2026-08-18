//! Boru diagnostics submodule (structural split BORU-CORE-002).

use super::*;

// =============================================================================
// Room and peer diagnostic snapshots
// =============================================================================

/// A lightweight, serializable diagnostic snapshot of a single peer's state.
///
/// Derived from existing friends-store records and diagnostic events.
/// Intentionally omits secret keys, tickets, and mailbox keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerDiagnosticSnapshot {
    /// The peer's public key as a hex string.
    pub peer_id: String,
    /// Discovery sources that have reported this peer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discovery_sources: Vec<DiscoverySource>,
    /// Known network addresses for this peer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<String>,
    /// Whether the peer appears connected.
    pub connected: bool,
    /// Unix epoch millisecond when the peer was last seen, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_timestamp_ms: Option<i64>,
    /// The last error recorded for this peer, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// A serializable diagnostic snapshot of a single room's state.
///
/// Built from existing application state (friends store, diagnostics,
/// room store, subscription state) rather than from a second independent
/// model.  Contains no secret keys, tickets, or mailbox keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomDiagnosticSnapshot {
    /// The local node's public key as a hex string.
    pub node_id: String,
    /// The room's gossip topic as a hex string.
    pub room_id: String,
    /// Whether the room has been joined.
    pub joined: bool,
    /// Whether the local node is currently subscribed to the room's gossip
    /// topic (has an active gossip subscription handle).
    pub subscribed: bool,
    /// Number of peers associated with this room.
    pub peer_count: usize,
    /// Per-peer diagnostic snapshots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<PeerDiagnosticSnapshot>,
    /// Discovery sources that are enabled for this room.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discovery_sources_enabled: Vec<String>,
    /// The last error recorded for this room, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Build a [`RoomDiagnosticSnapshot`] from existing application state.
///
/// Reads from:
/// - `friends` — friend record metadata (addresses, online status, last seen)
/// - `diagnostics` — event-based peer state (discovery sources, errors)
/// - `room_store` — persisted room metadata (topic, discovery secret)
/// - `is_subscribed` — whether the local node holds an active subscription
///
/// # Security
///
/// No secret keys, tickets, or mailbox keys are included in the output.
#[cfg(feature = "net")]
pub fn build_room_snapshot(
    node_id: &iroh_base::PublicKey,
    room_topic: TopicId,
    room_store: Option<&crate::room::RoomStore>,
    friends: &crate::friends::FriendsStore,
    diagnostics: &Diagnostics,
    is_subscribed: bool,
) -> RoomDiagnosticSnapshot {
    // Determine if we've joined this room by checking if the room store
    // knows about this topic.
    let joined = room_store.map(|rs| rs.topic == room_topic).unwrap_or(false);

    // Check if the room has a discovery secret enabled (private-room DHT).
    let discovery_sources_enabled = room_store
        .and_then(|rs| rs.discovery_secret.as_ref())
        .map(|_| vec!["discovery_secret".to_string()])
        .unwrap_or_default();

    // Get peer diagnostic states from event replay for additional info.
    let diag_states = diagnostics.peer_states();

    let mut peers: Vec<PeerDiagnosticSnapshot> = Vec::new();

    for (friend_id, record) in &friends.friends {
        // Only include established friends -- skip blocked / not-friend /
        // deprecated pending variants.
        if record.relationship != crate::friends::FriendRelationship::Friends {
            continue;
        }

        let peer_id = friend_id.as_str().to_string();

        // Collect discovery sources from diagnostic event state.
        let discovery_sources = diag_states
            .get(&peer_id)
            .map(|s| s.discovery_sources.clone())
            .unwrap_or_default();

        // Collect addresses from the friend record's known addresses.
        let addresses: Vec<String> = record
            .known_addrs
            .iter()
            .map(|addr| format!("{addr:?}"))
            .collect();

        // Connected status from the friend record's online/offline status.
        let connected = record.status.online;

        // Convert u64 unix-millisecond timestamp to i64 for the snapshot.
        let last_seen_timestamp_ms = record.status.last_seen_at_unix_ms.map(|ts| ts as i64);

        // Last error from diagnostic peer state.
        let last_error = diag_states.get(&peer_id).and_then(|s| s.last_error.clone());

        peers.push(PeerDiagnosticSnapshot {
            peer_id,
            discovery_sources,
            addresses,
            connected,
            last_seen_timestamp_ms,
            last_error,
        });
    }

    // Sort peers: connected first, then alphabetically by peer_id.
    peers.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.peer_id.cmp(&b.peer_id))
    });

    let peer_count = peers.len();

    // Last room-level error — scan the most recent event for an Error variant
    // matching this room topic.
    let last_error = diagnostics
        .events_since(0, 100, Some(room_topic))
        .iter()
        .find_map(|e| {
            if let DiagnosticEventKind::Error(msg) = &e.kind {
                Some(msg.clone())
            } else {
                None
            }
        });

    RoomDiagnosticSnapshot {
        node_id: node_id.to_string(),
        room_id: hex::encode(room_topic.as_bytes()),
        joined,
        subscribed: is_subscribed,
        peer_count,
        peers,
        discovery_sources_enabled,
        last_error,
    }
}

/// A snapshot of the Iced application state relevant for diagnostics.
///
/// Built from the running `IcedChat` state.  Contains no secret keys,
/// tickets, or mailbox keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcedStateSnapshot {
    /// The local node's public key as a hex string.
    pub node_id: String,
    /// Application version string.
    pub version: String,
    /// Name of the active screen (e.g. "ChatList", "Chat", "Settings").
    pub active_screen: String,
    /// The active room topic as a hex string, if a chat is open.
    pub active_room: Option<String>,
    /// Number of live conversations (including background ones).
    pub conversation_count: usize,
    /// Number of gossip neighbors across all active rooms.
    pub neighbor_count: usize,
    /// Number of peers reachable via direct (hole-punched) connections.
    pub direct_peer_count: usize,
    /// Number of peers connected through a relay server.
    pub relayed_peer_count: usize,
    /// Summary of mesh health (e.g. "Good", "Degraded", "Poor", "Unknown").
    pub mesh_health: String,
    /// Number of friends currently marked online.
    pub online_friend_count: usize,
    /// Total number of friends in the friends list.
    pub friend_count: usize,
    /// Total number of chat entries across all conversations.
    pub total_entry_count: usize,
    /// Whether dark mode is active.
    pub dark_mode: bool,
    /// The current composer text for the active conversation, or empty string
    /// if no conversation is open or the composer is empty.
    pub composer_text: String,
    /// Whether any modal dialog (e.g. confirmation, error, help overlay) is
    /// currently open and blocking other UI interactions.
    pub dialog_open: bool,
    /// Total number of unread messages across all conversations.
    pub unread_count: usize,
    /// File Sharing dashboard data when the File Sharing screen is active.
    /// `None` when the dashboard is not open (or diagnostics are disabled).
    ///
    /// This gives automated E2E harnesses actionable per-tab data —
    /// file names, transfer progress, download states, and peer lists —
    /// without needing to parse screenshots.
    pub dashboard: Option<DashboardSnapshot>,
    /// Wall-clock timestamp of the snapshot.
    pub timestamp: DateTime<Utc>,
}

/// A compact, serializable view of the File Sharing dashboard state.
///
/// Populated by the iced frontend whenever the File Sharing screen is
/// active.  Contains only display-safe fields (file names, byte counts,
/// states, peer identifiers) — never local paths, tokens, or tickets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DashboardSnapshot {
    /// The active dashboard tab name (`files_sharing`, `downloading`,
    /// `downloaded`, `shared_with_me`, `activity`).
    pub active_tab: String,
    /// Files this node has registered for sharing (Shared by Me tab).
    pub shared_by_me_files: Vec<FileSummary>,
    /// In-progress inbound transfers (Downloading tab).
    pub downloading: Vec<TransferSummary>,
    /// Completed downloads (Downloaded tab).
    pub downloaded: Vec<DownloadSummary>,
    /// Files shared to this node by peers (Shared with Me tab).
    pub shared_with_me_files: Vec<FileSummary>,
    /// Recent activity log entries (Activity tab).
    pub activity: Vec<ActivitySummary>,
}

/// Display-safe summary of a file shown in a dashboard tab.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileSummary {
    /// Display filename (no path components).
    pub name: String,
    /// Size in bytes when known.
    pub size_bytes: Option<u64>,
}

/// Display-safe summary of an in-progress transfer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferSummary {
    /// Display filename (no path components).
    pub name: String,
    /// Remote peer public key (hex) when known.
    pub peer_id: Option<String>,
    /// Bytes transferred so far.
    pub bytes: u64,
    /// Total bytes when known.
    pub total_bytes: Option<u64>,
    /// Transfer state (`active`, `verifying`, `completed`, `failed`,
    /// `cancelled`, `disconnected`).
    pub state: String,
}

/// Display-safe summary of a completed download.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DownloadSummary {
    /// Display filename (no path components).
    pub name: String,
    /// Total size in bytes.
    pub size_bytes: u64,
    /// Source peer display label (safe; never a raw public key or path).
    pub source_peer: String,
}

/// Display-safe summary of a recent activity log entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivitySummary {
    /// Human-safe label (file/peer display label).
    pub label: String,
    /// Normalized action (Requested, Authorized, Started, Downloaded,
    /// Uploaded, Failed, Cancelled, Denied, ...).
    pub action: String,
    /// Local observation timestamp in Unix milliseconds.
    pub occurred_at_ms: u64,
}
