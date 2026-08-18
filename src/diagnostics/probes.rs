//! Boru diagnostics submodule (structural split BORU-CORE-002).

use super::*;

// =============================================================================
// DiagnosticProbe — wire format sent through the gossip mesh
// =============================================================================

/// A diagnostic probe that travels through the normal room gossip path.
///
/// Probes are not displayed as ordinary chat messages by default.  They
/// are recorded in the [`Diagnostics`] store on both the sending and
/// receiving side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticProbe {
    /// Unique, collision-resistant probe identifier.
    pub probe_id: String,
    /// Public key of the sender, as a hex string.
    pub sender_id: String,
    /// Room ID (hex-encoded topic).
    pub room_id: String,
    /// Unix epoch millisecond when the probe was sent.
    pub sent_at_ms: i64,
    /// Optional diagnostic payload text (inert, never executed).
    pub payload: Option<String>,
}

// =============================================================================
// ReceivedProbe — enhanced with full metadata
// =============================================================================

/// A probe received from a remote peer, with full delivery metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceivedProbe {
    /// Unique probe identifier (matches what the sender generated).
    pub probe_id: String,
    /// Room ID where the probe was received.
    pub room_id: String,
    /// Public key of the sender, as a hex string.
    pub sender_id: String,
    /// Unix epoch millisecond when the probe was sent (from sender).
    pub sent_at_ms: i64,
    /// Unix epoch millisecond when the probe was received locally.
    pub received_at_ms: i64,
    /// Calculated latency in milliseconds, or `None` if clocks differ.
    pub latency_ms: Option<i64>,
    /// Message hash (blake3 hex) computed from the wire content.
    pub message_hash: String,
    /// How many times this probe has been received (duplicate count).
    pub duplicate_count: u32,
    /// When the probe was received (wall-clock).
    pub timestamp: DateTime<Utc>,
    /// The room context.
    pub room_id_opt: Option<TopicId>,
}

// =============================================================================
// Peer diagnostic state
// =============================================================================

/// The observed state of a single diagnostic stage.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DiagnosticStageState {
    /// The stage has not been observed to start.
    NotStarted,
    /// The stage is currently in progress.
    InProgress,
    /// The stage completed successfully.
    Succeeded,
    /// The stage failed.
    Failed,
    /// The stage could not be observed in the current architecture.
    NotObserved,
}

/// The observed state of a connection to a peer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConnectionDiagnosticState {
    /// No connection attempt has been observed.
    NotStarted,
    /// A connection attempt is in progress.
    Connecting,
    /// Connection was established.
    Connected,
    /// Connection attempt failed.
    Failed,
    /// Connection was established but later disconnected.
    Disconnected,
    /// Connection state could not be observed.
    NotObserved,
}

/// Current diagnostic state for an observed peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerDiagnosticState {
    /// The peer's public key as a hex string.
    pub peer_id: String,
    /// Discovery sources that have reported this peer.
    pub discovery_sources: Vec<DiscoverySource>,
    /// Whether the peer has been discovered at all.
    pub discovered: bool,
    /// Wall-clock millisecond when the peer was first discovered.
    pub discovered_at_ms: Option<i64>,
    /// State of address lookup.
    pub address_lookup_state: DiagnosticStageState,
    /// Resolved addresses for this peer.
    pub addresses: Vec<String>,
    /// State of the connection to this peer.
    pub connection_state: ConnectionDiagnosticState,
    /// Address at which the peer is connected, if known.
    pub connected_address: Option<String>,
    /// Transport used for the connection, if known.
    pub transport: Option<String>,
    /// Whether a relay was used for the connection.
    pub used_relay: Option<bool>,
    /// State of room subscription for this peer.
    pub subscription_state: DiagnosticStageState,
    /// Whether the peer is a member of the topic.
    pub topic_member: bool,
    /// Wall-clock millisecond when the peer was last seen.
    pub last_seen_at_ms: Option<i64>,
    /// The stage at which the last error occurred, if any.
    pub last_error_stage: Option<String>,
    /// The last error message, if any.
    pub last_error: Option<String>,
}

// =============================================================================
// Peer state update logic
// =============================================================================

/// Update a [`PeerDiagnosticState`] from a [`DiagnosticEvent`].
///
/// Returns the updated state (or a new one if `current` is `None`).
/// This is deterministic — calling it twice with the same event and
/// state produces the same result.
pub fn update_peer_state(
    current: Option<PeerDiagnosticState>,
    event: &DiagnosticEvent,
) -> PeerDiagnosticState {
    let peer_id = event.peer_id.clone().unwrap_or_default();
    let mut state = current.unwrap_or(PeerDiagnosticState {
        peer_id: peer_id.clone(),
        discovery_sources: Vec::new(),
        discovered: false,
        discovered_at_ms: None,
        address_lookup_state: DiagnosticStageState::NotStarted,
        addresses: Vec::new(),
        connection_state: ConnectionDiagnosticState::NotStarted,
        connected_address: None,
        transport: None,
        used_relay: None,
        subscription_state: DiagnosticStageState::NotStarted,
        topic_member: false,
        last_seen_at_ms: None,
        last_error_stage: None,
        last_error: None,
    });

    let now_ms = event.timestamp.timestamp_millis();

    match &event.kind {
        DiagnosticEventKind::PeerDiscovered => {
            state.discovered = true;
            if state.discovered_at_ms.is_none() {
                state.discovered_at_ms = Some(now_ms);
            }
            state.last_seen_at_ms = Some(now_ms);
        }
        DiagnosticEventKind::PeerDiscoveredWithAddr { source, addresses } => {
            state.discovered = true;
            if state.discovered_at_ms.is_none() {
                state.discovered_at_ms = Some(now_ms);
            }
            if !state.discovery_sources.contains(source) {
                state.discovery_sources.push(source.clone());
            }
            for addr in addresses {
                if !state.addresses.contains(addr) {
                    state.addresses.push(addr.clone());
                }
            }
            state.last_seen_at_ms = Some(now_ms);
        }
        DiagnosticEventKind::DiscoveryStarted { source } => {
            if !state.discovery_sources.contains(source) {
                state.discovery_sources.push(source.clone());
            }
        }
        DiagnosticEventKind::AddressLookupStarted { .. } => {
            state.address_lookup_state = DiagnosticStageState::InProgress;
        }
        DiagnosticEventKind::AddressResolved { source, addresses } => {
            state.address_lookup_state = DiagnosticStageState::Succeeded;
            if !state.discovery_sources.contains(source) {
                state.discovery_sources.push(source.clone());
            }
            for addr in addresses {
                if !state.addresses.contains(addr) {
                    state.addresses.push(addr.clone());
                }
            }
        }
        DiagnosticEventKind::AddressLookupFailed { error, .. } => {
            state.address_lookup_state = DiagnosticStageState::Failed;
            state.last_error_stage = Some("address_lookup".to_string());
            state.last_error = Some(error.clone());
        }
        DiagnosticEventKind::ConnectionAttemptStarted { addresses } => {
            state.connection_state = ConnectionDiagnosticState::Connecting;
            for addr in addresses {
                if !state.addresses.contains(addr) {
                    state.addresses.push(addr.clone());
                }
            }
        }
        DiagnosticEventKind::ConnectionEstablished {
            remote_address,
            transport,
            used_relay,
        } => {
            state.connection_state = ConnectionDiagnosticState::Connected;
            state.connected_address = remote_address.clone();
            state.transport = transport.clone();
            state.used_relay = *used_relay;
            state.last_seen_at_ms = Some(now_ms);
        }
        DiagnosticEventKind::ConnectionFailed { error, .. } => {
            state.connection_state = ConnectionDiagnosticState::Failed;
            state.last_error_stage = Some("connection".to_string());
            state.last_error = Some(error.clone());
        }
        DiagnosticEventKind::RoomSubscriptionStarted => {
            state.subscription_state = DiagnosticStageState::InProgress;
        }
        DiagnosticEventKind::RoomSubscriptionJoined => {
            state.subscription_state = DiagnosticStageState::Succeeded;
        }
        DiagnosticEventKind::RoomSubscriptionFailed { error } => {
            state.subscription_state = DiagnosticStageState::Failed;
            state.last_error_stage = Some("subscription".to_string());
            state.last_error = Some(error.clone());
        }
        DiagnosticEventKind::PeerAddedToTopic => {
            state.topic_member = true;
            state.last_seen_at_ms = Some(now_ms);
        }
        DiagnosticEventKind::PeerRemovedFromTopic { .. } => {
            state.topic_member = false;
        }
        DiagnosticEventKind::ProbeReceived { sender_id, .. } => {
            if sender_id == &state.peer_id {
                state.last_seen_at_ms = Some(now_ms);
            }
        }
        DiagnosticEventKind::PeerJoinedRoom => {
            state.last_seen_at_ms = Some(now_ms);
        }
        _ => {}
    }

    state
}

// =============================================================================
// Failure classification
// =============================================================================

/// The stage at which a discovery test failed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DiscoveryFailureStage {
    /// The local room is not available (not joined or inactive).
    LocalRoomUnavailable,
    /// The expected peer was never discovered.
    Discovery,
    /// Peer was discovered but address lookup explicitly failed.
    AddressResolution,
    /// Address resolved but connection explicitly failed.
    Connection,
    /// Connection established but subscription failed.
    Subscription,
    /// Subscription joined but peer never appeared as a topic member.
    TopicMembership,
    /// Topic member present but probe could not be broadcast.
    ProbeBroadcast,
    /// Probe broadcast but not confirmed before timeout.
    ProbeDelivery,
    /// Insufficient or conflicting evidence — cannot determine the failure stage.
    Unknown,
}

/// Structured evidence collected during a discovery test.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscoveryTestEvidence {
    /// Whether the local node is joined to the room.
    pub local_room_joined: bool,
    /// Whether the expected peer was discovered.
    pub peer_discovered: bool,
    /// Whether address lookup was observed.
    pub address_lookup_observed: bool,
    /// Whether address resolution succeeded.
    pub address_resolved: bool,
    /// Whether a connection attempt was observed.
    pub connection_attempted: bool,
    /// Whether a connection was established.
    pub connection_established: bool,
    /// Whether room subscription was observed to start.
    pub subscription_started: bool,
    /// Whether room subscription completed successfully.
    pub subscription_joined: bool,
    /// Whether the peer is recorded as a topic member.
    pub peer_in_topic: bool,
    /// Whether a probe was broadcast.
    pub probe_broadcast: bool,
    /// Whether the probe was received or acknowledged.
    pub probe_received_or_acknowledged: bool,
}

/// The result of a complete discovery test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryTestResult {
    /// Whether the overall test was a success.
    pub success: bool,
    /// The room ID being tested.
    pub room_id: String,
    /// The local node ID.
    pub local_node_id: String,
    /// The expected peer ID.
    pub expected_peer_id: String,
    /// The stage at which the test failed, if any.
    pub failed_stage: Option<DiscoveryFailureStage>,
    /// Human-readable summary of the test outcome.
    pub summary: String,
    /// Structured evidence collected.
    pub evidence: DiscoveryTestEvidence,
    /// The peer's diagnostic state, if observed.
    pub peer: Option<PeerDiagnosticState>,
    /// The starting event sequence number.
    pub event_sequence_start: u64,
    /// The ending event sequence number.
    pub event_sequence_end: u64,
    /// Relevant events collected during the test.
    pub relevant_events: Vec<DiagnosticEvent>,
    /// Result of a diagnostic probe, if one was sent.
    pub probe: Option<ProbeTestResult>,
}

/// Result of a single diagnostic probe send and delivery check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeTestResult {
    /// The probe identifier.
    pub probe_id: String,
    /// Whether the probe was broadcast successfully.
    pub broadcast_accepted: bool,
    /// Whether delivery was confirmed.
    pub delivery_confirmed: bool,
    /// Latency in milliseconds, if known.
    pub latency_ms: Option<i64>,
}

// =============================================================================
// Classifier
// =============================================================================

/// Classify a discovery test from the collected evidence and peer state.
///
/// Returns a [`DiscoveryFailureStage`] and a human-readable summary.
///
/// Rules:
///   - Local room missing or inactive → `LocalRoomUnavailable`
///   - Expected peer never discovered → `Discovery`
///   - Peer discovered but lookup explicitly failed → `AddressResolution`
///   - Address resolved but connection explicitly failed → `Connection`
///   - Connection established but subscription failed → `Subscription`
///   - Subscription joined but peer never a topic member → `TopicMembership`
///   - Topic member present but probe not broadcast → `ProbeBroadcast`
///   - Probe broadcast but not confirmed → `ProbeDelivery`
///   - Insufficient or conflicting evidence → `Unknown`
///
/// A stage is NOT considered failed merely because no event was emitted
/// when that stage is not observable in the current architecture.
pub fn classify_discovery_test(
    evidence: &DiscoveryTestEvidence,
    peer: Option<&PeerDiagnosticState>,
) -> (Option<DiscoveryFailureStage>, String) {
    // Check local room first
    if !evidence.local_room_joined {
        return (
            Some(DiscoveryFailureStage::LocalRoomUnavailable),
            "Local room is not available (not joined or inactive).".to_string(),
        );
    }

    // Check discovery
    if !evidence.peer_discovered {
        return (
            Some(DiscoveryFailureStage::Discovery),
            "Expected peer was never discovered.".to_string(),
        );
    }

    // Check address lookup — only if we observed it start or fail
    if evidence.address_lookup_observed {
        if let Some(p) = peer {
            if p.address_lookup_state == DiagnosticStageState::Failed {
                return (
                    Some(DiscoveryFailureStage::AddressResolution),
                    format!(
                        "Address lookup failed: {}",
                        p.last_error.as_deref().unwrap_or("unknown error")
                    ),
                );
            }
        }
        if !evidence.address_resolved {
            // Lookup was observed but didn't succeed — that's a failure
            return (
                Some(DiscoveryFailureStage::AddressResolution),
                "Address lookup was observed but did not complete successfully.".to_string(),
            );
        }
    }

    // Check connection
    if evidence.connection_attempted {
        if let Some(p) = peer {
            if p.connection_state == ConnectionDiagnosticState::Failed {
                return (
                    Some(DiscoveryFailureStage::Connection),
                    format!(
                        "Connection attempt failed: {}",
                        p.last_error.as_deref().unwrap_or("unknown error")
                    ),
                );
            }
        }
        if !evidence.connection_established {
            return (
                Some(DiscoveryFailureStage::Connection),
                "Connection was attempted but not established.".to_string(),
            );
        }
    }

    // Check subscription
    if evidence.subscription_started {
        if let Some(p) = peer {
            if p.subscription_state == DiagnosticStageState::Failed {
                return (
                    Some(DiscoveryFailureStage::Subscription),
                    format!(
                        "Subscription failed: {}",
                        p.last_error.as_deref().unwrap_or("unknown error")
                    ),
                );
            }
        }
        if !evidence.subscription_joined {
            return (
                Some(DiscoveryFailureStage::Subscription),
                "Subscription was started but not completed.".to_string(),
            );
        }
    }

    // Check topic membership
    if evidence.subscription_joined && !evidence.peer_in_topic {
        return (
            Some(DiscoveryFailureStage::TopicMembership),
            "Peer joined subscription but is not a topic member.".to_string(),
        );
    }

    // Check probe
    if evidence.peer_in_topic && !evidence.probe_broadcast {
        return (
            Some(DiscoveryFailureStage::ProbeBroadcast),
            "Topic member present but probe was not broadcast.".to_string(),
        );
    }

    if evidence.probe_broadcast && !evidence.probe_received_or_acknowledged {
        return (
            Some(DiscoveryFailureStage::ProbeDelivery),
            "Probe was broadcast but delivery was not confirmed.".to_string(),
        );
    }

    // All stages successful
    (
        None,
        "All diagnostic stages completed successfully.".to_string(),
    )
}

// =============================================================================
// DiagnosticProbe generation
// =============================================================================

/// Generate a collision-resistant probe ID from the current timestamp
/// and a random component.
pub fn generate_probe_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Use a simple hash of time + process-level randomness
    let mut buf = [0u8; 16];
    let pid = std::process::id();
    let hash_input = format!("{now:020x}-{pid:x}");
    let hash = blake3::hash(hash_input.as_bytes());
    buf.copy_from_slice(&hash.as_bytes()[..16]);
    hex::encode(buf)
}
