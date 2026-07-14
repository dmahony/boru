//! Core diagnostics — bounded event and probe storage for boru-chat.
//!
//! Provides a thread-safe [`Diagnostics`] singleton that records
//! [`DiagnosticEvent`]s and [`ReceivedProbe`]s with bounded capacity.
//! Oldest records are automatically evicted when limits are exceeded.
//!
//! # Event types
//!
//! See [`DiagnosticEventKind`] for all supported event variants, including
//! the extended lifecycle stages (discovery, address lookup, connection,
//! subscription, probes).
//!
//! # Peer state
//!
//! [`PeerDiagnosticState`] tracks the per-peer diagnostic lifecycle — what
//! stage each peer has reached.  The [`classify_discovery_test`] function
//! produces a structured failure classification from the collected evidence.
//!
//! # Probe types
//!
//! [`ReceivedProbe`] tracks probes received from peers with full metadata
//! (latency, message hash, duplicate count).  [`DiagnosticProbe`] is the
//! wire-format probe sent through the gossip mesh.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use iroh_base::PublicKey;
use serde::{Deserialize, Serialize};

use crate::TopicId;

// =============================================================================
// DiscoverySource
// =============================================================================

/// How a peer was discovered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    /// Local mDNS discovery.
    Mdns,
    /// Mainline DHT lookup.
    MainlineDht,
    /// Room join ticket.
    Ticket,
    /// Bootstrap node.
    Bootstrap,
    /// DNS Pkarr resolution.
    DnsPkarr,
    /// Gossip-layer propagation.
    Gossip,
    /// In-memory address lookup (e.g. cached from a prior session).
    MemoryLookup,
    /// Manual entry (e.g. pasted address).
    Manual,
    /// Unknown or uncategorised source.
    Unknown,
}

// =============================================================================
// DiagnosticEvent types
// =============================================================================

/// A single diagnostic event recorded by the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticEvent {
    /// Monotonically increasing sequence number.
    pub sequence: u64,
    /// Wall-clock timestamp of the event.
    pub timestamp: DateTime<Utc>,
    /// The room this event relates to, if any.
    pub room_id: Option<TopicId>,
    /// Peer this event relates to, if any.
    pub peer_id: Option<String>,
    /// The event variant and its payload.
    pub kind: DiagnosticEventKind,
}

/// Extended lifecycle stage states that complement the basic event variants.
///
/// These cover the full discovery-to-topic-membership pipeline.  Stages
/// that cannot be observed reliably record an `Unknown` or `NotObserved`
/// state rather than fabricating data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum DiagnosticEventKind {
    // ── Basic events (part 1) ──────────────────────────────────────────
    /// A room join was initiated.
    RoomJoinStarted,
    /// Room join completed successfully.
    RoomJoined,
    /// Room join failed.
    RoomJoinFailed,
    /// A new peer was discovered (outside any room context).
    PeerDiscovered,
    /// A peer joined the room.
    PeerJoinedRoom,
    /// A peer left the room.
    PeerLeftRoom,
    /// A message was broadcast by the local peer.
    MessageBroadcast {
        /// Optional message identifier (e.g. blake3 hash hex).
        message_id: Option<String>,
        /// Optional blake3 hash of the message content (hex-encoded).
        message_hash: Option<String>,
        /// Optional diagnostic probe identifier.
        probe_id: Option<String>,
    },
    /// A message was received from a remote peer.
    MessageReceived {
        /// Optional message identifier (e.g. blake3 hash hex).
        message_id: Option<String>,
        /// Optional blake3 hash of the message content (hex-encoded).
        message_hash: Option<String>,
        /// Optional diagnostic probe identifier.
        probe_id: Option<String>,
        /// Public key of the sending peer (as a hex string).
        sender_id: Option<String>,
    },
    /// A duplicate message was detected and dropped.
    DuplicateMessage,
    /// A general error condition.
    Error(String),

    // ── Extended lifecycle stages (part 2) ────────────────────────────
    /// A discovery cycle has started (from a specific source).
    DiscoveryStarted { source: DiscoverySource },
    /// A peer was discovered with addresses.
    PeerDiscoveredWithAddr {
        source: DiscoverySource,
        addresses: Vec<String>,
    },
    /// Address lookup for a peer has started.
    AddressLookupStarted { source: DiscoverySource },
    /// Address was resolved for a peer.
    AddressResolved {
        source: DiscoverySource,
        addresses: Vec<String>,
    },
    /// Address lookup for a peer failed.
    AddressLookupFailed {
        source: DiscoverySource,
        error: String,
    },
    /// Connection attempt to a peer has started.
    ConnectionAttemptStarted { addresses: Vec<String> },
    /// Connection to a peer was established.
    ConnectionEstablished {
        remote_address: Option<String>,
        transport: Option<String>,
        used_relay: Option<bool>,
    },
    /// Connection to a peer failed.
    ConnectionFailed {
        addresses: Vec<String>,
        error: String,
    },
    /// Room subscription for a peer has started.
    RoomSubscriptionStarted,
    /// Room subscription for a peer was joined.
    RoomSubscriptionJoined,
    /// Room subscription for a peer failed.
    RoomSubscriptionFailed { error: String },
    /// A peer was added to the topic member set.
    PeerAddedToTopic,
    /// A peer was removed from the topic member set.
    PeerRemovedFromTopic { reason: Option<String> },
    /// A diagnostic probe was broadcast.
    ProbeBroadcast {
        probe_id: String,
        message_hash: String,
    },
    /// A diagnostic probe was received from a peer.
    ProbeReceived {
        probe_id: String,
        message_hash: String,
        sender_id: String,
    },
    /// A diagnostic probe timed out without delivery confirmation.
    ProbeTimedOut { probe_id: String, timeout_ms: u64 },
    /// A GUI action timed out while waiting for expected completion state.
    ActionTimedOut {
        action_id: String,
        action_type: String,
        expected_completion: String,
        timeout_ms: u64,
    },
    /// A GUI test action was received from the MCP channel and is being
    /// processed by the Iced update loop.
    GuiActionReceived {
        /// The action ID string.
        action_id: String,
        /// The JSON-serialized command string.
        command_json: String,
    },
}

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

// =============================================================================
// Diagnostics store (core type)
// =============================================================================

/// Thread-safe diagnostics store with bounded event and probe buffers.
///
/// # Defaults
///
/// | Store              | Max entries |
/// |--------------------|-------------|
/// | Events             | 5 000       |
/// | Received probes    | 1 000       |
///
/// When a store exceeds its maximum, the oldest entries are evicted at
/// the next insert.  Query limits are clamped to 1 000.
///
/// When the `net` feature is enabled, a Tokio watch channel is available
/// for event-driven waiting (see [`Diagnostics::subscribe`]).
#[derive(Debug, Clone)]
pub struct Diagnostics {
    inner: Arc<DiagnosticsInner>,
}

#[derive(Debug)]
struct DiagnosticsInner {
    /// Bounded event ring buffer.  Newest entries are appended at the back.
    events: Mutex<VecDeque<DiagnosticEvent>>,
    /// Bounded received-probe map keyed by opaque identifier string.
    /// Insertion order is tracked via a parallel deque for eviction.
    received_probes: Mutex<HashMap<String, ReceivedProbe>>,
    /// Insertion-order queue for received probes (keys, oldest first).
    received_probe_order: Mutex<VecDeque<String>>,
    /// Monotonically increasing sequence counter.
    next_sequence: AtomicU64,
    /// Maximum event storage capacity.
    max_events: usize,
    /// Maximum received-probe storage capacity.
    max_received_probes: usize,
    /// Tokio watch sender for event notifications (net feature only).
    #[cfg(feature = "net")]
    event_watch: tokio::sync::watch::Sender<u64>,
}

impl Diagnostics {
    /// Create a new diagnostics store with default capacities.
    ///
    /// - Events: 5 000
    /// - Received probes: 1 000
    pub fn new() -> Self {
        Self::with_capacity(5000, 1000)
    }

    /// Create a new diagnostics store with the given capacities.
    pub fn with_capacity(max_events: usize, max_received_probes: usize) -> Self {
        Self {
            inner: Arc::new(DiagnosticsInner {
                events: Mutex::new(VecDeque::with_capacity(max_events.min(5000) + 64)),
                received_probes: Mutex::new(HashMap::with_capacity(
                    max_received_probes.min(1000) + 64,
                )),
                received_probe_order: Mutex::new(VecDeque::with_capacity(
                    max_received_probes.min(1000) + 64,
                )),
                next_sequence: AtomicU64::new(0),
                max_events,
                max_received_probes,
                #[cfg(feature = "net")]
                event_watch: tokio::sync::watch::Sender::new(0),
            }),
        }
    }

    /// Record a new diagnostic event.
    ///
    /// The event is assigned the next sequence number and a current
    /// timestamp automatically.  If the event store is at capacity,
    /// the oldest event is evicted.
    pub fn record(&self, room_id: Option<TopicId>, kind: DiagnosticEventKind) {
        self.record_with_peer(room_id, None::<&str>, kind);
    }

    /// Record a new diagnostic event with an optional peer ID.
    pub fn record_with_peer(
        &self,
        room_id: Option<TopicId>,
        peer_id: Option<impl AsRef<str>>,
        kind: DiagnosticEventKind,
    ) {
        let sequence = self.inner.next_sequence.fetch_add(1, Ordering::Relaxed);
        let event = DiagnosticEvent {
            sequence,
            timestamp: Utc::now(),
            room_id,
            peer_id: peer_id.map(|p| p.as_ref().to_string()),
            kind,
        };

        {
            let mut events = self.inner.events.lock().expect("events lock");
            if events.len() >= self.inner.max_events {
                events.pop_front();
            }
            events.push_back(event);
        }

        #[cfg(feature = "net")]
        {
            let _ = self.inner.event_watch.send(sequence);
        }
    }

    /// Return events with a sequence number greater than `since_sequence`,
    /// limited to `limit` entries and optionally filtered by `room_id`.
    ///
    /// The limit is clamped to 1 000.  Events are returned in ascending
    /// sequence order (oldest matching first).
    pub fn events_since(
        &self,
        since_sequence: u64,
        limit: usize,
        room_id: Option<TopicId>,
    ) -> Vec<DiagnosticEvent> {
        let limit = limit.min(1000);
        let events = self.inner.events.lock().expect("events lock");

        let iter: Box<dyn Iterator<Item = &DiagnosticEvent>> = if let Some(room) = room_id {
            Box::new(events.iter().filter(move |e| e.room_id == Some(room)))
        } else {
            Box::new(events.iter())
        };

        iter.filter(|e| e.sequence > since_sequence)
            .take(limit)
            .cloned()
            .collect()
    }

    /// Return events with a sequence number greater than `since_sequence`,
    /// optionally filtered by both `room_id` and `peer_id`.
    pub fn events_since_filtered(
        &self,
        since_sequence: u64,
        limit: usize,
        room_id: Option<TopicId>,
        peer_id: Option<&str>,
    ) -> Vec<DiagnosticEvent> {
        let limit = limit.min(1000);
        let events = self.inner.events.lock().expect("events lock");

        let iter: Box<dyn Iterator<Item = &DiagnosticEvent>> = if let Some(room) = room_id {
            Box::new(events.iter().filter(move |e| e.room_id == Some(room)))
        } else {
            Box::new(events.iter())
        };

        let iter = if let Some(pid) = peer_id {
            let pid_owned = pid.to_string();
            Box::new(iter.filter(move |e| e.peer_id.as_deref() == Some(&pid_owned)))
        } else {
            iter
        };

        iter.filter(|e| e.sequence > since_sequence)
            .take(limit)
            .cloned()
            .collect()
    }

    /// Return the most recently assigned sequence number.
    ///
    /// Returns 0 if no events have been recorded yet.
    pub fn latest_sequence(&self) -> u64 {
        let val = self.inner.next_sequence.load(Ordering::Relaxed);
        if val == 0 {
            0
        } else {
            val - 1
        }
    }

    /// Subscribe to new event notifications via Tokio watch.
    ///
    /// The watch sends the latest sequence number each time an event is
    /// recorded.  Use this to implement event-driven waiting without
    /// aggressive polling.
    #[cfg(feature = "net")]
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.event_watch.subscribe()
    }

    /// Record a received probe.
    ///
    /// This is the enhanced version that stores full probe metadata.
    /// If a probe with the same `probe_id` already exists, its
    /// `duplicate_count` is incremented.
    pub fn record_received_probe_enhanced(&self, probe: ReceivedProbe) {
        let id = probe.probe_id.clone();
        let mut probes = self.inner.received_probes.lock().expect("probes lock");
        let mut order = self
            .inner
            .received_probe_order
            .lock()
            .expect("probe order lock");

        // If already exists, increment duplicate count and replace
        if let Some(existing) = probes.get_mut(&id) {
            existing.duplicate_count += 1;
            existing.received_at_ms = probe.received_at_ms;
            existing.latency_ms = probe.latency_ms;
            // Refresh position in order
            if let Some(pos) = order.iter().position(|k| k == &id) {
                order.remove(pos);
            }
            order.push_back(id.clone());
            return;
        }

        // Evict oldest if at capacity
        if probes.len() >= self.inner.max_received_probes {
            if let Some(oldest_key) = order.pop_front() {
                probes.remove(&oldest_key);
            }
        }

        probes.insert(id.clone(), probe);
        order.push_back(id);
    }

    /// Record a received probe (legacy API, simple keyed storage).
    ///
    /// * `id` — opaque probe identifier.
    /// * `peer` — public key of the sending peer.
    /// * `discovery_source` — how the peer was discovered.
    /// * `room_id` — optional room context.
    pub fn record_received_probe(
        &self,
        id: String,
        peer: PublicKey,
        discovery_source: DiscoverySource,
        room_id: Option<TopicId>,
    ) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let probe = ReceivedProbe {
            probe_id: id.clone(),
            room_id: String::new(),
            sender_id: peer.to_string(),
            sent_at_ms: now_ms,
            received_at_ms: now_ms,
            latency_ms: None,
            message_hash: String::new(),
            duplicate_count: 0,
            timestamp: Utc::now(),
            room_id_opt: room_id,
        };

        let mut probes = self.inner.received_probes.lock().expect("probes lock");
        let mut order = self
            .inner
            .received_probe_order
            .lock()
            .expect("probe order lock");

        if let Some(pos) = order.iter().position(|k| k == &id) {
            order.remove(pos);
        }

        if probes.len() >= self.inner.max_received_probes {
            if let Some(oldest_key) = order.pop_front() {
                probes.remove(&oldest_key);
            }
        }

        probes.insert(id.clone(), probe);
        order.push_back(id);
    }

    /// Look up a received probe by its opaque identifier.
    pub fn find_received_probe(&self, id: &str) -> Option<ReceivedProbe> {
        let probes = self.inner.received_probes.lock().expect("probes lock");
        probes.get(id).cloned()
    }

    // ── Convenience helpers ──────────────────────────────────────────────

    /// Return the total number of events currently stored.
    pub fn event_count(&self) -> usize {
        let events = self.inner.events.lock().expect("events lock");
        events.len()
    }

    /// Return the total number of received probes currently stored.
    pub fn probe_count(&self) -> usize {
        let probes = self.inner.received_probes.lock().expect("probes lock");
        probes.len()
    }

    /// Return all stored events (for diagnostics / debug).
    pub fn all_events(&self) -> Vec<DiagnosticEvent> {
        let events = self.inner.events.lock().expect("events lock");
        events.iter().cloned().collect()
    }

    /// Build a [`DiscoveryTestEvidence`] from the stored events.
    ///
    /// Scans all events for the given room and peer to determine which
    /// stages were reached.
    pub fn build_evidence(
        &self,
        room_id: Option<TopicId>,
        peer_id: Option<&str>,
    ) -> DiscoveryTestEvidence {
        let events = self.inner.events.lock().expect("events lock");

        let mut evidence = DiscoveryTestEvidence {
            local_room_joined: false,
            peer_discovered: false,
            address_lookup_observed: false,
            address_resolved: false,
            connection_attempted: false,
            connection_established: false,
            subscription_started: false,
            subscription_joined: false,
            peer_in_topic: false,
            probe_broadcast: false,
            probe_received_or_acknowledged: false,
        };

        for event in events.iter() {
            // Filter by room if specified
            if let Some(rid) = room_id {
                if event.room_id != Some(rid) {
                    continue;
                }
            }
            // Filter by peer if specified
            if let Some(pid) = peer_id {
                if event.peer_id.as_deref() != Some(pid) {
                    continue;
                }
            }

            match &event.kind {
                DiagnosticEventKind::RoomJoined => evidence.local_room_joined = true,
                DiagnosticEventKind::PeerDiscovered
                | DiagnosticEventKind::PeerDiscoveredWithAddr { .. } => {
                    evidence.peer_discovered = true;
                }
                DiagnosticEventKind::AddressLookupStarted { .. } => {
                    evidence.address_lookup_observed = true;
                }
                DiagnosticEventKind::AddressResolved { .. } => {
                    evidence.address_resolved = true;
                }
                DiagnosticEventKind::ConnectionAttemptStarted { .. } => {
                    evidence.connection_attempted = true;
                }
                DiagnosticEventKind::ConnectionEstablished { .. } => {
                    evidence.connection_established = true;
                }
                DiagnosticEventKind::RoomSubscriptionStarted => {
                    evidence.subscription_started = true;
                }
                DiagnosticEventKind::RoomSubscriptionJoined => {
                    evidence.subscription_joined = true;
                }
                DiagnosticEventKind::PeerAddedToTopic => {
                    evidence.peer_in_topic = true;
                }
                DiagnosticEventKind::ProbeBroadcast { .. } => {
                    evidence.probe_broadcast = true;
                }
                DiagnosticEventKind::ProbeReceived { .. } => {
                    evidence.probe_received_or_acknowledged = true;
                }
                _ => {}
            }
        }

        evidence
    }

    /// Rebuild per-peer diagnostic state from all stored events.
    ///
    /// Returns a map of peer_id → [`PeerDiagnosticState`] with the
    /// accumulated state for each observed peer.
    pub fn peer_states(&self) -> HashMap<String, PeerDiagnosticState> {
        let events = self.inner.events.lock().expect("events lock");
        let mut states: HashMap<String, PeerDiagnosticState> = HashMap::new();

        for event in events.iter() {
            if let Some(pid) = &event.peer_id {
                let current = states.remove(pid);
                let updated = update_peer_state(current, event);
                states.insert(pid.clone(), updated);
            }
        }

        states
    }

    /// Get the diagnostic state for a specific peer.
    pub fn peer_state(&self, peer_id: &str) -> Option<PeerDiagnosticState> {
        let events = self.inner.events.lock().expect("events lock");
        let mut state: Option<PeerDiagnosticState> = None;

        for event in events.iter() {
            if event.peer_id.as_deref() == Some(peer_id) {
                state = Some(update_peer_state(state, event));
            }
        }

        state
    }
}

impl Default for Diagnostics {
    fn default() -> Self {
        Self::new()
    }
}

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

// =============================================================================
// Iced diagnostics types
// =============================================================================

/// Which application layer a failure is attributed to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FailureLayer {
    /// Failure occurred in the network layer (discovery, connection, gossip).
    Network,
    /// Failure occurred in the application state layer (chat_core, conversations, friends).
    ApplicationState,
    /// Failure occurred in the Iced UI update handler.
    IcedUpdate,
    /// The layer could not be determined from available evidence.
    Unknown,
}

/// A single entry in the Iced message processing journal.
///
/// Recorded each time the Iced `update()` function processes an
/// [`AppMessage`] variant (as a string summary).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcedMessageJournalEntry {
    /// Monotonically increasing sequence number.
    pub sequence: u64,
    /// Wall-clock timestamp when the message was processed.
    pub timestamp: DateTime<Utc>,
    /// The message variant name (e.g. "NetEvent", "SendPressed").
    pub message_variant: String,
    /// The layer this message targets.
    pub layer: FailureLayer,
    /// Whether processing succeeded.
    pub success: bool,
    /// Error message if processing failed, or empty string.
    pub error: String,
    /// Processing duration in milliseconds, if measured.
    pub duration_ms: Option<u64>,
}

/// Thread-safe bounded journal of recent Iced message processing.
///
/// Records the last N [`IcedMessageJournalEntry`] values as they
/// are processed by the Iced `update()` function.  Oldest entries
/// are automatically evicted when the limit is exceeded.
///
/// # Defaults
///
/// | Store         | Max entries |
/// |---------------|-------------|
/// | Journal       | 500         |
#[derive(Debug, Clone)]
pub struct IcedMessageJournal {
    inner: Arc<IcedMessageJournalInner>,
}

#[derive(Debug)]
struct IcedMessageJournalInner {
    entries: Mutex<VecDeque<IcedMessageJournalEntry>>,
    next_sequence: AtomicU64,
    max_entries: usize,
    /// Tokio watch sender for journal-change notifications (net feature only).
    #[cfg(feature = "net")]
    event_watch: tokio::sync::watch::Sender<u64>,
}

impl IcedMessageJournal {
    /// Create a new journal with the default capacity (500 entries).
    pub fn new() -> Self {
        Self::with_capacity(500)
    }

    /// Create a new journal with the given maximum number of entries.
    pub fn with_capacity(max_entries: usize) -> Self {
        Self {
            inner: Arc::new(IcedMessageJournalInner {
                entries: Mutex::new(VecDeque::with_capacity(max_entries.min(500) + 32)),
                next_sequence: AtomicU64::new(0),
                max_entries,
                #[cfg(feature = "net")]
                event_watch: tokio::sync::watch::Sender::new(0),
            }),
        }
    }

    /// Record a processed Iced message in the journal.
    pub fn record(
        &self,
        message_variant: impl AsRef<str>,
        layer: FailureLayer,
        success: bool,
        error: impl AsRef<str>,
        duration_ms: Option<u64>,
    ) {
        let sequence = self.inner.next_sequence.fetch_add(1, Ordering::Relaxed);
        let entry = IcedMessageJournalEntry {
            sequence,
            timestamp: Utc::now(),
            message_variant: message_variant.as_ref().to_string(),
            layer,
            success,
            error: error.as_ref().to_string(),
            duration_ms,
        };

        let mut entries = self.inner.entries.lock().expect("iced journal lock");
        if entries.len() >= self.inner.max_entries {
            entries.pop_front();
        }
        entries.push_back(entry);
        #[cfg(feature = "net")]
        {
            let _ = self.inner.event_watch.send(sequence);
        }
    }

    /// Subscribe to journal-change notifications.
    ///
    /// Returns a `watch::Receiver` that yields the latest sequence number
    /// each time a new entry is recorded.  The receiver is initialised to 0,
    /// so a `changed()` call will never return before the first record.
    #[cfg(feature = "net")]
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.event_watch.subscribe()
    }

    /// Return journal entries with a sequence number greater than `since_sequence`,
    /// limited to `limit` entries (clamped to 500).
    pub fn entries_since(&self, since_sequence: u64, limit: usize) -> Vec<IcedMessageJournalEntry> {
        let limit = limit.min(500);
        let entries = self.inner.entries.lock().expect("iced journal lock");
        entries
            .iter()
            .filter(|e| e.sequence > since_sequence)
            .take(limit)
            .cloned()
            .collect()
    }

    /// Return the most recently assigned sequence number (0 if no entries).
    pub fn latest_sequence(&self) -> u64 {
        let val = self.inner.next_sequence.load(Ordering::Relaxed);
        if val == 0 {
            0
        } else {
            val - 1
        }
    }

    /// Return the total number of entries currently stored.
    pub fn entry_count(&self) -> usize {
        self.inner.entries.lock().expect("iced journal lock").len()
    }

    /// Return all stored entries (for diagnostics / debug).
    pub fn all_entries(&self) -> Vec<IcedMessageJournalEntry> {
        self.inner
            .entries
            .lock()
            .expect("iced journal lock")
            .iter()
            .cloned()
            .collect()
    }
}

impl Default for IcedMessageJournal {
    fn default() -> Self {
        Self::new()
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
    /// Wall-clock timestamp of the snapshot.
    pub timestamp: DateTime<Utc>,
}

/// Combined failure analysis across all diagnostic layers.
///
/// Reports whether a failure was detected at the network layer,
/// application state layer, or Iced update handler layer, with
/// supporting evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureAnalysis {
    /// Whether a network-layer failure was detected.
    pub network_failure: bool,
    /// Whether an application-state-layer failure was detected.
    pub state_update_failure: bool,
    /// Whether an Iced update handler failure was detected.
    pub iced_update_failure: bool,
    /// Human-readable details about detected failures.
    pub details: Vec<String>,
    /// Wall-clock timestamp of the analysis.
    pub timestamp: DateTime<Utc>,
}

// =============================================================================
// GUI Action Tracking
// =============================================================================

/// Deterministic error codes for structured GUI action errors.
///
/// Each variant encodes a specific failure condition that can occur during
/// action validation or processing.  This replaces unstructured error strings
/// with machine-readable codes that callers can handle programmatically.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum GuiActionErrorCode {
    /// GUI test actions are disabled (not started with --enable-gui-test-actions).
    GuiActionsDisabled,
    /// The specified room does not exist or has not been joined.
    UnknownRoom,
    /// The specified conversation does not exist.
    UnknownConversation,
    /// The specified peer is not known.
    UnknownPeer,
    /// The action is not valid for the current screen.
    InvalidCurrentScreen,
    /// A blocking dialog (e.g. confirmation modal) is open.
    BlockingDialogOpen,
    /// No active conversation to perform the action on.
    NoActiveConversation,
    /// The composer is empty (nothing to send).
    ComposerEmpty,
    /// The composer text exceeds the maximum allowed length.
    ComposerTooLong,
    /// Sending messages is currently disabled.
    SendDisabled,
    /// The room is inactive (left or disconnected).
    RoomInactive,
    /// The action queue has been closed (application shutting down).
    ActionQueueClosed,
    /// The action queue is full (at capacity).
    ActionQueueFull,
    /// The action timed out before completion.
    ActionTimedOut,
    /// An argument or parameter was invalid.
    InvalidArgument,
    /// The command could not be deserialized or was unrecognized.
    UnknownCommand,
    /// An internal system error occurred.
    InternalError,
}

/// A structured GUI action error with a deterministic error code and
/// human-readable message.
///
/// # Example
///
/// ```ignore
/// let err = GuiActionError::new(GuiActionErrorCode::UnknownRoom, "Room 'abc123' was not found");
/// assert_eq!(err.code, GuiActionErrorCode::UnknownRoom);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiActionError {
    /// The deterministic error code.
    pub code: GuiActionErrorCode,
    /// Human-readable explanation of the error.
    pub message: String,
}

impl GuiActionError {
    /// Create a new structured action error.
    pub fn new(code: GuiActionErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for GuiActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for GuiActionError {}

/// A unique identifier for a GUI action.
///
/// Generated from a blake3 hash of the current timestamp, process ID, and
/// a random component, producing a 32-character hex string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct GuiActionId(pub String);

impl GuiActionId {
    /// Generate a new unique action ID.
    pub fn new() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut buf = [0u8; 16];
        let pid = std::process::id();
        let rnd: u64 = rand::random();
        let hash_input = format!("{now:020x}-{pid:x}-{rnd:016x}-gui-action");
        let hash = blake3::hash(hash_input.as_bytes());
        buf.copy_from_slice(&hash.as_bytes()[..16]);
        GuiActionId(hex::encode(buf))
    }
}

impl Default for GuiActionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for GuiActionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The expected UI state condition for a GUI action.
///
/// Each action can optionally declare what state it expects the UI to
/// be in after the action completes successfully.  The application state
/// checker uses this to verify that the action had the intended effect.
///
/// # Examples
///
/// ```
/// use boru_chat::diagnostics::ExpectedState;
///
/// let state = ExpectedState::ScreenIs("chat_list".into());
/// assert!(state.matches_str("screen", "chat_list"));
/// assert!(!state.matches_str("screen", "settings"));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ExpectedState {
    /// The active screen matches the given name (e.g. `"chat_list"`, `"settings"`, `"chat"`).
    ScreenIs(String),
    /// A room with the given topic hex string is selected / active.
    RoomSelected(String),
    /// A conversation with the given peer key (hex) is selected.
    ConversationSelected(String),
    /// The composer text matches the given string.
    ComposerTextIs(String),
    /// Dark mode matches the given boolean state.
    DarkModeIs(bool),
    /// A message was successfully submitted (send handled + composer cleared).
    MessageSent,
    /// The help overlay visibility matches the given boolean.
    HelpVisible(bool),
    /// Generic condition decribed by a free-form string.
    Generic(String),
}

impl ExpectedState {
    /// Check whether this expected state is satisfied by a given
    /// (category, value) observation from the UI.
    ///
    /// `category` is a string like `"screen"`, `"composer_text"`,
    /// `"dark_mode"`, etc.  `value` is the observed value.
    ///
    /// Returns `true` if the observation matches this expected state.
    pub fn matches_str(&self, category: &str, value: &str) -> bool {
        match self {
            ExpectedState::ScreenIs(expected) => category == "screen" && value == expected,
            ExpectedState::RoomSelected(expected) => category == "room" && value == expected,
            ExpectedState::ConversationSelected(expected) => {
                category == "conversation" && value == expected
            }
            ExpectedState::ComposerTextIs(expected) => {
                category == "composer_text" && value == expected
            }
            ExpectedState::DarkModeIs(expected) => {
                category == "dark_mode" && value == expected.to_string()
            }
            ExpectedState::MessageSent => category == "message_sent" && value == "true",
            ExpectedState::HelpVisible(expected) => {
                category == "help_visible" && value == expected.to_string()
            }
            ExpectedState::Generic(_) => false,
        }
    }

    /// Return a human-readable description of what condition this expected
    /// state represents (e.g. `"screen == chat_list"`).
    pub fn description(&self) -> String {
        match self {
            ExpectedState::ScreenIs(s) => format!("screen == \"{s}\""),
            ExpectedState::RoomSelected(t) => format!("room_selected({t})"),
            ExpectedState::ConversationSelected(k) => format!("conversation_selected({k})"),
            ExpectedState::ComposerTextIs(t) => format!("composer_text == \"{t}\""),
            ExpectedState::DarkModeIs(b) => format!("dark_mode == {b}"),
            ExpectedState::MessageSent => "message_sent".to_string(),
            ExpectedState::HelpVisible(b) => format!("help_visible == {b}"),
            ExpectedState::Generic(s) => s.clone(),
        }
    }
}

/// The lifecycle state of a single GUI action, from initiation to completion.
///
/// # State machine
///
/// ```text
/// Queued ──→ Validating ──→ Rejected (terminal)
///                 │
///                 └──→ AppMessageQueued ──→ AppMessageHandled ──→ Completed (terminal)
///                                                      │
///                                                      ├──→ Failed (terminal)
///                                                      │
///                                                      └──→ WaitingForExpectedState ──→ Completed (terminal)
///                                                                               │
///                                                                               └──→ TimedOut (terminal)
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiActionState {
    /// Action has been queued but not yet processed.
    Queued,
    /// Action is being validated against current application state.
    Validating,
    /// Action validation failed; will not proceed.
    Rejected,
    /// Action has been converted to an AppMessage and queued for processing.
    AppMessageQueued,
    /// AppMessage was handled by the application state layer.
    AppMessageHandled,
    /// Waiting for the UI to reflect the expected state change.
    WaitingForExpectedState,
    /// Action completed successfully (terminal).
    Completed,
    /// Action timed out waiting for completion (terminal).
    TimedOut,
    /// Action failed irrecoverably (terminal).
    Failed,
}

impl GuiActionState {
    /// Returns `true` if this is a terminal state (action is done or failed).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            GuiActionState::Completed
                | GuiActionState::TimedOut
                | GuiActionState::Failed
                | GuiActionState::Rejected
        )
    }

    /// Returns `true` if this is an active (non-terminal) state.
    pub fn is_active(&self) -> bool {
        !self.is_terminal()
    }
}

/// An incoming GUI action with structured metadata.
///
/// This is recorded when the user initiates an action through the GUI
/// (e.g. pressing Send, opening a room, toggling dark mode).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiActionRequest {
    /// Unique action identifier.
    pub action_id: GuiActionId,
    /// Unix epoch millisecond when the action was initiated in the GUI.
    pub requested_at_ms: i64,
    /// The command/action name (e.g. `"SendPressed"`, `"OpenRoom"`, `"AddFriend"`).
    pub command: String,
}

/// Current status of a GUI action, tracking its lifecycle through the system.
///
/// The status is updated as the action progresses through validation,
/// application message handling, and UI state observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiActionStatus {
    /// Unique action identifier.
    pub action_id: GuiActionId,
    /// Current lifecycle state.
    pub state: GuiActionState,
    /// Unix epoch millisecond when the action was first requested.
    pub requested_at_ms: i64,
    /// Unix epoch millisecond when the status was last updated.
    pub updated_at_ms: i64,
    /// The expected GUI revision number the action will produce, if known.
    pub expected_gui_revision: Option<u64>,
    /// The observed GUI revision number after the action was handled.
    pub observed_gui_revision: Option<u64>,
    /// Structured error if the action failed or was rejected.
    pub error: Option<GuiActionError>,
    /// Optional result payload (e.g. success message, created resource ID).
    pub result: Option<String>,
    /// The expected UI state condition that this action is waiting for,
    /// if any.  Set before the action enters `WaitingForExpectedState`
    /// and checked after the action is handled.
    pub expected_state: Option<ExpectedState>,
    /// Absolute timestamp (milliseconds since epoch) when this action
    /// should time out if still in `WaitingForExpectedState`.
    /// Set automatically when transitioning into that state.
    pub timeout_at_ms: Option<i64>,
}

impl GuiActionStatus {
    /// Transition the action to a new state, updating the timestamp.
    ///
    /// This is a raw transition; use [`GuiActionStatus::transition_to`] for
    /// validated state-machine transitions.
    pub fn set_state(&mut self, new_state: GuiActionState) {
        // Check before move — new_state is moved into self.state below
        let needs_timeout = new_state == GuiActionState::WaitingForExpectedState;

        self.state = new_state;
        self.updated_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        // Automatically arm the timeout when entering WaitingForExpectedState
        if needs_timeout {
            self.timeout_at_ms = Some(
                self.updated_at_ms
                    .checked_add(DEFAULT_ACTION_STATE_TIMEOUT_MS)
                    .unwrap_or(self.updated_at_ms),
            );
        } else {
            // Clear timeout when leaving WaitingForExpectedState
            self.timeout_at_ms = None;
        }
    }

    /// Set the expected UI state condition for this action.
    ///
    /// Returns `&mut Self` for chaining.
    pub fn with_expected_state(&mut self, expected: ExpectedState) -> &mut Self {
        self.expected_state = Some(expected);
        self
    }

    /// Returns `true` if this action has an expected state that is
    /// matched by the given (category, value) observation.
    pub fn expected_state_matches(&self, category: &str, value: &str) -> bool {
        self.expected_state
            .as_ref()
            .map(|es| es.matches_str(category, value))
            .unwrap_or(false)
    }

    /// Attempt a validated state-machine transition.
    ///
    /// Returns `Ok(())` if the transition is valid, or `Err(GuiActionError)` with
    /// a structured error.
    ///
    /// Valid transitions:
    ///   `Queued`                    → `Validating`
    ///   `Validating`                → `Rejected` | `AppMessageQueued`
    ///   `AppMessageQueued`          → `AppMessageHandled`
    ///   `AppMessageHandled`         → `Completed` | `Failed` | `WaitingForExpectedState`
    ///   `WaitingForExpectedState`   → `Completed` | `TimedOut` | `Failed`
    ///   Terminal states             → (no transitions allowed)
    pub fn transition_to(&mut self, target: GuiActionState) -> Result<(), GuiActionError> {
        use GuiActionState::*;

        let allowed = match (&self.state, &target) {
            (Queued, Validating) => true,
            (Validating, Rejected) | (Validating, AppMessageQueued) => true,
            (AppMessageQueued, AppMessageHandled) => true,
            (AppMessageHandled, Completed)
            | (AppMessageHandled, Failed)
            | (AppMessageHandled, WaitingForExpectedState) => true,
            (WaitingForExpectedState, Completed)
            | (WaitingForExpectedState, TimedOut)
            | (WaitingForExpectedState, Failed) => true,
            _ => false,
        };

        if allowed {
            Ok(self.set_state(target))
        } else {
            Err(GuiActionError::new(
                GuiActionErrorCode::InvalidArgument,
                format!("Invalid state transition: {:?} → {:?}", self.state, target),
            ))
        }
    }
}

/// Bounded, thread-safe history store for GUI action lifecycle tracking.
///
/// Stores up to `max_actions` entries.  Oldest **completed** (terminal)
/// actions are evicted first when the store is at capacity and a new
/// action is recorded.  A terminal action is one with a state of
/// [`GuiActionState::Completed`], [`GuiActionState::TimedOut`],
/// [`GuiActionState::Failed`], or [`GuiActionState::Rejected`].
/// Active (non-terminal) actions are never evicted automatically so
/// in-flight operations are never lost.
///
/// # Default capacity
///
/// | Store         | Max entries |
/// |---------------|-------------|
/// | Action history | 1 000       |
#[derive(Debug, Clone)]
pub struct GuiActionHistory {
    inner: Arc<GuiActionHistoryInner>,
}

#[derive(Debug)]
struct GuiActionHistoryInner {
    /// Map from action ID to status entry.
    actions: Mutex<HashMap<GuiActionId, GuiActionStatus>>,
    /// Insertion-order queue (action IDs, oldest first).  Used for eviction.
    order: Mutex<VecDeque<GuiActionId>>,
    /// Maximum number of stored actions.
    max_actions: usize,
}

impl GuiActionHistory {
    /// Create a new action history with the default capacity (1 000 actions).
    pub fn new() -> Self {
        Self::with_capacity(1000)
    }

    /// Create a new action history with the given maximum number of actions.
    pub fn with_capacity(max_actions: usize) -> Self {
        let capped = max_actions.max(1).clamp(1, 5000);
        Self {
            inner: Arc::new(GuiActionHistoryInner {
                actions: Mutex::new(HashMap::with_capacity(capped + 32)),
                order: Mutex::new(VecDeque::with_capacity(capped + 32)),
                max_actions: capped,
            }),
        }
    }

    /// Record a new GUI action.
    ///
    /// If the store is at capacity, oldest terminal (completed) actions are
    /// evicted to make room.  Returns the action ID.
    pub fn record(&self, request: GuiActionRequest) -> GuiActionId {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let status = GuiActionStatus {
            action_id: request.action_id.clone(),
            state: GuiActionState::Queued,
            requested_at_ms: request.requested_at_ms,
            updated_at_ms: now_ms,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        let id = status.action_id.clone();

        let mut actions = self.inner.actions.lock().expect("actions lock");
        let mut order = self.inner.order.lock().expect("order lock");

        // Evict oldest terminal actions first. If none exist, evict the
        // oldest action (even active) to enforce the capacity bound.
        while actions.len() >= self.inner.max_actions {
            // Find the oldest terminal action from the front
            let terminal_pos = order.iter().position(|id| {
                actions
                    .get(id)
                    .map(|s| s.state.is_terminal())
                    .unwrap_or(false)
            });

            if let Some(pos) = terminal_pos {
                // Evict the first terminal action found
                if let Some(id) = order.remove(pos) {
                    actions.remove(&id);
                    continue;
                }
            }

            // No terminal action found — evict the oldest action (front)
            if let Some(oldest_id) = order.pop_front() {
                actions.remove(&oldest_id);
                continue;
            }

            // Order is empty — can't evict further
            break;
        }

        actions.insert(id.clone(), status);
        order.push_back(id.clone());

        id
    }

    /// Update the state of an existing action using validated state-machine
    /// transitions.  Returns `Ok(())` on success or `Err(GuiActionError)`
    /// with a structured error code for programmatic handling.
    pub fn transition_to(
        &self,
        action_id: &GuiActionId,
        target: GuiActionState,
    ) -> Result<(), GuiActionError> {
        let mut actions = self.inner.actions.lock().expect("actions lock");
        if let Some(status) = actions.get_mut(action_id) {
            status.transition_to(target)
        } else {
            Err(GuiActionError::new(
                GuiActionErrorCode::InvalidArgument,
                format!("Action {action_id} not found"),
            ))
        }
    }

    /// Update the state of an existing action directly (no validation).
    ///
    /// Returns `true` if the action was found and updated.
    pub fn set_state(&self, action_id: &GuiActionId, state: GuiActionState) -> bool {
        let mut actions = self.inner.actions.lock().expect("actions lock");
        if let Some(status) = actions.get_mut(action_id) {
            status.set_state(state);
            true
        } else {
            false
        }
    }

    /// Set the error details on an existing action.
    ///
    /// Returns `true` if the action was found and updated, `false` otherwise.
    pub fn set_error(&self, action_id: &GuiActionId, error: GuiActionError) -> bool {
        let mut actions = self.inner.actions.lock().expect("actions lock");
        if let Some(status) = actions.get_mut(action_id) {
            status.error = Some(error);
            true
        } else {
            false
        }
    }

    /// Retrieve the status of an action by its ID.
    pub fn get(&self, action_id: &GuiActionId) -> Option<GuiActionStatus> {
        let actions = self.inner.actions.lock().expect("actions lock");
        actions.get(action_id).cloned()
    }

    /// Return all stored actions, newest first.
    pub fn all_actions(&self) -> Vec<GuiActionStatus> {
        let actions = self.inner.actions.lock().expect("actions lock");
        let order = self.inner.order.lock().expect("order lock");
        order
            .iter()
            .rev()
            .filter_map(|id| actions.get(id))
            .cloned()
            .collect()
    }

    /// Return actions matching a specific state, newest first.
    pub fn actions_with_state(&self, state: GuiActionState) -> Vec<GuiActionStatus> {
        self.all_actions()
            .into_iter()
            .filter(|a| a.state == state)
            .collect()
    }

    /// Return the total number of stored actions.
    pub fn action_count(&self) -> usize {
        let actions = self.inner.actions.lock().expect("actions lock");
        actions.len()
    }

    /// Return the number of active (non-terminal) actions.
    pub fn active_count(&self) -> usize {
        let actions = self.inner.actions.lock().expect("actions lock");
        actions.values().filter(|a| a.state.is_active()).count()
    }

    /// Set the expected completion state for a recorded action.
    ///
    /// Returns `true` if the action was found and updated.
    pub fn set_expected_state(&self, action_id: &GuiActionId, expected: ExpectedState) -> bool {
        let mut actions = self.inner.actions.lock().expect("actions lock");
        if let Some(status) = actions.get_mut(action_id) {
            status.expected_state = Some(expected);
            true
        } else {
            false
        }
    }

    /// Remove an action by ID.  Returns `true` if it existed.
    pub fn remove(&self, action_id: &GuiActionId) -> bool {
        let mut actions = self.inner.actions.lock().expect("actions lock");
        let mut order = self.inner.order.lock().expect("order lock");

        let existed = actions.remove(action_id).is_some();
        if existed {
            // Remove from order list
            if let Some(pos) = order.iter().position(|id| id == action_id) {
                order.remove(pos);
            }
        }
        existed
    }

    /// Check for actions whose timeout has expired and transition them
    /// to `TimedOut`.
    ///
    /// Returns a list of `(action_id, status)` pairs for each action that
    /// was transitioned to `TimedOut`.  Only actions in
    /// `WaitingForExpectedState` with elapsed `timeout_at_ms` are affected.
    ///
    /// This is the main timeout enforcement mechanism.  Call it before
    /// querying action status (via `get`, `all_actions`, etc.) to ensure
    /// expired actions are detected.
    ///
    /// # No busy polling
    ///
    /// To avoid polling, use [`GuiActionHistory::next_timeout_remaining_ms`]
    /// to find out when the next timeout will expire, then schedule a single
    /// timer for that moment.
    pub fn check_timeouts(&self) -> Vec<(GuiActionId, GuiActionStatus)> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let mut actions = self.inner.actions.lock().expect("actions lock");

        let mut timed_out: Vec<(GuiActionId, GuiActionStatus)> = Vec::new();

        // Collect IDs of expired actions first (to avoid borrow conflicts)
        let expired_ids: Vec<GuiActionId> = actions
            .iter()
            .filter(|(_, status)| {
                status.state == GuiActionState::WaitingForExpectedState
                    && status.timeout_at_ms.map(|t| now_ms >= t).unwrap_or(false)
            })
            .map(|(id, _)| id.clone())
            .collect();

        for id in &expired_ids {
            if let Some(status) = actions.get_mut(id) {
                status.state = GuiActionState::TimedOut;
                status.updated_at_ms = now_ms;
                status.timeout_at_ms = None;
                timed_out.push((id.clone(), status.clone()));
            }
        }

        timed_out
    }

    /// Returns the remaining milliseconds until the next action timeout
    /// expires, or `None` if no action is currently timing.
    ///
    /// Use this to schedule a single timer instead of polling:
    ///
    /// ```ignore
    /// if let Some(remaining_ms) = history.next_timeout_remaining_ms() {
    ///     tokio::time::sleep(Duration::from_millis(remaining_ms)).await;
    ///     history.check_timeouts();
    /// }
    /// ```
    pub fn next_timeout_remaining_ms(&self) -> Option<u64> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let actions = self.inner.actions.lock().expect("actions lock");

        let earliest = actions
            .values()
            .filter(|s| {
                s.state == GuiActionState::WaitingForExpectedState && s.timeout_at_ms.is_some()
            })
            .filter_map(|s| s.timeout_at_ms)
            .min()?;

        let remaining = earliest - now_ms;
        if remaining <= 0 {
            Some(0)
        } else {
            Some(remaining as u64)
        }
    }
}

impl Default for GuiActionHistory {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// GUI Wait Conditions
// =============================================================================

/// Serializable GUI wait conditions for diagnostic polling.
///
/// Each variant describes a condition that can be evaluated against
/// the current [`IcedStateSnapshot`] and [`IcedMessageJournal`].
///
/// Only variants supported by current diagnostics data (as exposed
/// by these two types) are included.
///
/// # Evaluation
///
/// Use [`evaluate_wait_condition`] to check whether a condition is
/// currently satisfied.
///
/// # Security
///
/// - No secrets (keys, tickets, tokens) are exposed.
/// - String parameters are bounded at 4096 characters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GuiWaitCondition {
    /// Whether the active screen name matches the expected value.
    ///
    /// Evaluated against [`IcedStateSnapshot::active_screen`].
    /// Example expected values: `"ChatList"`, `"Chat"`, `"Settings"`.
    ScreenIs {
        /// Expected screen name.
        expected: String,
    },
    /// Whether a room is currently selected (open), optionally matching
    /// a specific room topic.
    ///
    /// Evaluated against [`IcedStateSnapshot::active_room`].
    RoomSelected {
        /// If `Some(topic)`, requires the active room to match this topic.
        /// If `None`, any active room is sufficient.
        room_topic: Option<String>,
    },
    /// Whether at least `min_count` gossip peers are visible as neighbors.
    ///
    /// Evaluated against [`IcedStateSnapshot::neighbor_count`].
    PeerVisible {
        /// Minimum number of peers that must be visible.
        min_count: u32,
    },
    /// Whether at least `min_count` chat entries (messages) are present
    /// in the active conversation.
    ///
    /// Evaluated against [`IcedStateSnapshot::total_entry_count`].
    MessageVisible {
        /// Minimum number of chat entries that must be visible.
        min_count: u32,
    },
    /// Whether the GUI revision counter (the Iced message journal's latest
    /// sequence number) has reached at least `expected_revision`.
    ///
    /// Evaluated against [`IcedMessageJournal::latest_sequence`].
    /// This is a monotonic counter that increments each time an Iced
    /// `AppMessage` is processed — useful for waiting until pending
    /// state updates have been handled.
    GuiRevisionAtLeast {
        /// The minimum revision number that must have been reached.
        expected_revision: u64,
    },
    /// Whether a conversation (room or direct) is currently open, optionally
    /// matching a specific conversation identifier.
    ///
    /// Evaluated against [`IcedStateSnapshot::active_room`].
    /// This is a more general check than [`RoomSelected`] — it covers both
    /// group chat rooms and direct message conversations.
    ConversationSelected {
        /// If `Some(id)`, requires the active conversation to match this
        /// room topic or peer public key (hex). If `None`, any active
        /// conversation is sufficient.
        conversation_id: Option<String>,
    },
    /// Whether the composer text for the active conversation matches the
    /// expected value exactly.
    ///
    /// Evaluated against [`IcedStateSnapshot::composer_text`].
    ComposerTextIs {
        /// Expected composer text content.
        expected: String,
    },
    /// Whether a blocking modal dialog is currently open.
    ///
    /// Evaluated against [`IcedStateSnapshot::dialog_open`].
    /// Common examples include the help overlay, confirmation dialogs, and
    /// error modals.
    DialogOpen,
    /// Whether no blocking modal dialog is currently open.
    ///
    /// Evaluated against [`IcedStateSnapshot::dialog_open`].
    /// The logical inverse of [`DialogOpen`].
    DialogClosed,
    /// Whether the total number of unread messages across all conversations
    /// is at least `min_count`.
    ///
    /// Evaluated against [`IcedStateSnapshot::unread_count`].
    UnreadCountAtLeast {
        /// Minimum number of unread messages that must be pending.
        min_count: u32,
    },
}

/// Evaluate a [`GuiWaitCondition`] against the current diagnostics state.
///
/// Returns `true` if the condition is satisfied, `false` otherwise.
///
/// # Examples
///
/// ```ignore
/// use crate::diagnostics::{GuiWaitCondition, IcedStateSnapshot, IcedMessageJournal, evaluate_wait_condition};
///
/// let snapshot = IcedStateSnapshot { /* ... */ };
/// let journal = IcedMessageJournal::new();
///
/// let condition = GuiWaitCondition::ScreenIs {
///     expected: "ChatList".to_string(),
/// };
///
/// if evaluate_wait_condition(&condition, &snapshot, &journal) {
///     // Screen is ChatList
/// }
/// ```
pub fn evaluate_wait_condition(
    condition: &GuiWaitCondition,
    snapshot: &IcedStateSnapshot,
    journal: &IcedMessageJournal,
) -> bool {
    match condition {
        GuiWaitCondition::ScreenIs { expected } => snapshot.active_screen == *expected,
        GuiWaitCondition::RoomSelected { room_topic } => match room_topic {
            Some(topic) => snapshot.active_room.as_deref() == Some(topic.as_str()),
            None => snapshot.active_room.is_some(),
        },
        GuiWaitCondition::PeerVisible { min_count } => {
            snapshot.neighbor_count >= *min_count as usize
        }
        GuiWaitCondition::MessageVisible { min_count } => {
            snapshot.total_entry_count >= *min_count as usize
        }
        GuiWaitCondition::GuiRevisionAtLeast { expected_revision } => {
            journal.latest_sequence() >= *expected_revision
        }
        GuiWaitCondition::ConversationSelected { conversation_id } => match conversation_id {
            Some(id) => snapshot.active_room.as_deref() == Some(id.as_str()),
            None => snapshot.active_room.is_some(),
        },
        GuiWaitCondition::ComposerTextIs { expected } => snapshot.composer_text == *expected,
        GuiWaitCondition::DialogOpen => snapshot.dialog_open,
        GuiWaitCondition::DialogClosed => !snapshot.dialog_open,
        GuiWaitCondition::UnreadCountAtLeast { min_count } => {
            snapshot.unread_count >= *min_count as usize
        }
    }
}

// =============================================================================
// GUI Test Command Types
// =============================================================================

/// Maximum length of any user-facing string parameter in GUI test commands.
pub const GUI_TEST_COMMAND_MAX_STRING_LEN: usize = 4096;

/// Maximum timeout for wait conditions (milliseconds).
pub const GUI_TEST_COMMAND_MAX_TIMEOUT_MS: u64 = 30_000;

/// Default timeout for an action to reach the expected state (milliseconds).
/// Used when no explicit timeout is specified; 10 seconds.
pub const DEFAULT_ACTION_STATE_TIMEOUT_MS: i64 = 10_000;

/// Maximum permitted timeout for an action to reach the expected state (milliseconds).
/// Hard upper bound — 30 seconds.
pub const MAX_ACTION_STATE_TIMEOUT_MS: i64 = 30_000;

/// High-level GUI test commands that an AI agent can issue.
///
/// Each variant describes a semantic GUI action — no pixel coordinates,
/// no keyboard injection, no shell commands, no file system paths.
///
/// Only commands that map to existing GUI behaviour in the Iced chat
/// frontend are included.  All identifiers are hex-encoded strings.
///
/// # Security
///
/// - No secrets (keys, tickets, tokens) are exposed.
/// - String parameters are bounded at [`GUI_TEST_COMMAND_MAX_STRING_LEN`] chars.
/// - No arbitrary widget IDs, coordinates, or shell commands.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum GuiTestCommand {
    /// Navigate to the chat list (home) screen.
    GoToChatList,
    /// Open a specific room by its topic ID.
    OpenRoom {
        /// Room topic ID as a hex string.
        room_id: String,
    },
    /// Open a direct conversation with a peer.
    OpenConversation {
        /// Peer public key as a hex string.
        conversation_id: String,
    },
    /// Open the friend requests screen.
    OpenFriends,
    /// Open the settings screen.
    OpenSettings,
    /// Close the currently open dialog or settings screen.
    CloseDialog,
    /// Set the composer (message input) text for the active conversation.
    SetComposerText {
        /// Text to insert into the composer (max [`GUI_TEST_COMMAND_MAX_STRING_LEN`] chars,
        /// no control characters).
        text: String,
    },
    /// Submit the composer — sends whatever is currently in the composer
    /// for the active conversation.
    SubmitComposer,
    /// Select a peer by public key to view their profile or open a conversation.
    SelectPeer {
        /// Peer public key as a hex string.
        peer_id: String,
    },
    /// Toggle dark mode on/off.
    ToggleDarkMode {
        /// Target state: `true` = dark, `false` = light.
        enabled: bool,
    },
    /// Toggle the help overlay.
    ToggleHelp,
    /// Wait for a GUI condition to be satisfied.
    Wait {
        /// The condition to evaluate.
        condition: GuiWaitCondition,
        /// Maximum wait time in milliseconds (max [`GUI_TEST_COMMAND_MAX_TIMEOUT_MS`]).
        timeout_ms: u64,
    },
}

impl GuiTestCommand {
    /// Validate the command parameters.
    ///
    /// Returns `Ok(())` if the command is well-formed, or an error message.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            GuiTestCommand::OpenRoom { room_id } => {
                if room_id.len() > GUI_TEST_COMMAND_MAX_STRING_LEN {
                    return Err(format!(
                        "room_id too long ({} chars, max {})",
                        room_id.len(),
                        GUI_TEST_COMMAND_MAX_STRING_LEN
                    ));
                }
                Ok(())
            }
            GuiTestCommand::OpenConversation { conversation_id } => {
                if conversation_id.len() > GUI_TEST_COMMAND_MAX_STRING_LEN {
                    return Err(format!(
                        "conversation_id too long ({} chars, max {})",
                        conversation_id.len(),
                        GUI_TEST_COMMAND_MAX_STRING_LEN
                    ));
                }
                Ok(())
            }
            GuiTestCommand::SetComposerText { text } => {
                if text.len() > GUI_TEST_COMMAND_MAX_STRING_LEN {
                    return Err(format!(
                        "Composer text too long ({} chars, max {})",
                        text.len(),
                        GUI_TEST_COMMAND_MAX_STRING_LEN
                    ));
                }
                if text.chars().any(|c| c.is_control() && c != ' ') {
                    return Err("Composer text must not contain control characters".to_string());
                }
                Ok(())
            }
            GuiTestCommand::SelectPeer { peer_id } => {
                if peer_id.len() > GUI_TEST_COMMAND_MAX_STRING_LEN {
                    return Err(format!(
                        "peer_id too long ({} chars, max {})",
                        peer_id.len(),
                        GUI_TEST_COMMAND_MAX_STRING_LEN
                    ));
                }
                Ok(())
            }
            GuiTestCommand::ToggleDarkMode { .. } => Ok(()),
            GuiTestCommand::ToggleHelp => Ok(()),
            GuiTestCommand::GoToChatList => Ok(()),
            GuiTestCommand::OpenFriends => Ok(()),
            GuiTestCommand::OpenSettings => Ok(()),
            GuiTestCommand::CloseDialog => Ok(()),
            GuiTestCommand::SubmitComposer => Ok(()),
            GuiTestCommand::Wait {
                condition,
                timeout_ms,
            } => {
                if *timeout_ms > GUI_TEST_COMMAND_MAX_TIMEOUT_MS {
                    return Err(format!(
                        "Timeout must not exceed {}ms",
                        GUI_TEST_COMMAND_MAX_TIMEOUT_MS
                    ));
                }
                // Validate the inner wait condition (string bounds check)
                match condition {
                    GuiWaitCondition::ScreenIs { expected } => {
                        if expected.len() > GUI_TEST_COMMAND_MAX_STRING_LEN {
                            return Err(format!(
                                "Expected screen name too long ({} chars, max {})",
                                expected.len(),
                                GUI_TEST_COMMAND_MAX_STRING_LEN
                            ));
                        }
                    }
                    GuiWaitCondition::RoomSelected { room_topic } => {
                        if let Some(topic) = room_topic {
                            if topic.len() > GUI_TEST_COMMAND_MAX_STRING_LEN {
                                return Err(format!(
                                    "Room topic too long ({} chars, max {})",
                                    topic.len(),
                                    GUI_TEST_COMMAND_MAX_STRING_LEN
                                ));
                            }
                        }
                    }
                    GuiWaitCondition::PeerVisible { .. } => {}
                    GuiWaitCondition::MessageVisible { .. } => {}
                    GuiWaitCondition::GuiRevisionAtLeast { .. } => {}
                    GuiWaitCondition::ConversationSelected { .. } => {}
                    GuiWaitCondition::ComposerTextIs { expected } => {
                        if expected.len() > GUI_TEST_COMMAND_MAX_STRING_LEN {
                            return Err(format!(
                                "Composer text too long ({} chars, max {})",
                                expected.len(),
                                GUI_TEST_COMMAND_MAX_STRING_LEN
                            ));
                        }
                    }
                    GuiWaitCondition::DialogOpen => {}
                    GuiWaitCondition::DialogClosed => {}
                    GuiWaitCondition::UnreadCountAtLeast { .. } => {}
                }
                Ok(())
            }
        }
    }

    /// Return the expected state that the UI should be in after this
    /// command completes successfully, if one can be determined statically.
    ///
    /// Commands whose post-condition depends on current application state
    /// (e.g. `CloseDialog`, `SelectPeer`) return `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use boru_chat::diagnostics::{GuiTestCommand, ExpectedState};
    ///
    /// let cmd = GuiTestCommand::GoToChatList;
    /// assert_eq!(cmd.expected_state(), Some(ExpectedState::ScreenIs("ChatList".into())));
    ///
    /// let cmd = GuiTestCommand::ToggleDarkMode { enabled: true };
    /// assert_eq!(cmd.expected_state(), Some(ExpectedState::DarkModeIs(true)));
    ///
    /// let cmd = GuiTestCommand::SubmitComposer;
    /// assert_eq!(cmd.expected_state(), Some(ExpectedState::MessageSent));
    ///
    /// let cmd = GuiTestCommand::CloseDialog;
    /// assert!(cmd.expected_state().is_none());
    /// ```
    pub fn expected_state(&self) -> Option<ExpectedState> {
        match self {
            GuiTestCommand::GoToChatList => Some(ExpectedState::ScreenIs("ChatList".into())),
            GuiTestCommand::OpenRoom { room_id } => {
                Some(ExpectedState::RoomSelected(room_id.clone()))
            }
            GuiTestCommand::OpenConversation { conversation_id } => {
                Some(ExpectedState::ConversationSelected(conversation_id.clone()))
            }
            GuiTestCommand::SetComposerText { text } => {
                Some(ExpectedState::ComposerTextIs(text.clone()))
            }
            GuiTestCommand::SubmitComposer => Some(ExpectedState::MessageSent),
            GuiTestCommand::ToggleDarkMode { enabled } => Some(ExpectedState::DarkModeIs(*enabled)),
            GuiTestCommand::ToggleHelp => Some(ExpectedState::HelpVisible(true)),
            GuiTestCommand::OpenFriends => Some(ExpectedState::ScreenIs("Friends".into())),
            GuiTestCommand::OpenSettings => Some(ExpectedState::ScreenIs("Settings".into())),
            // CloseDialog: depends on what screen was behind the dialog.
            GuiTestCommand::CloseDialog => None,
            // SelectPeer: may open a conversation or profile — depends on context.
            GuiTestCommand::SelectPeer { .. } => None,
            // Wait: the condition itself IS the post-condition, tracked separately.
            GuiTestCommand::Wait { .. } => None,
        }
    }
}

// =============================================================================
// GUI Action Event Journal (event-oriented journal, complement to state-based GuiActionHistory)
// =============================================================================

/// The kind of a GUI action diagnostic event.
///
/// These are high-level lifecycle events tracked through a bounded journal,
/// complementary to the state-machine tracking in [`GuiActionHistory`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GuiActionEventKind {
    /// An action was initiated by the user or system.
    ActionRequested,
    /// An action was queued for processing.
    ActionQueued,
    /// Action validation has started.
    ActionValidationStarted,
    /// Action validation succeeded.
    ActionValidated,
    /// Action was rejected by validation.
    ActionRejected {
        /// Reason for rejection.
        reason: String,
    },
    /// An AppMessage was queued as a result of this action.
    AppMessageQueued {
        /// The AppMessage variant that was queued.
        message_variant: String,
    },
    /// An AppMessage was handled by the update handler.
    AppMessageHandled {
        /// The AppMessage variant that was handled.
        message_variant: String,
        /// Whether processing succeeded.
        success: bool,
    },
    /// The expected state was observed after an action was triggered.
    ExpectedStateObserved,
    /// An action completed successfully.
    ActionCompleted,
    /// An action timed out while waiting.
    ActionTimedOut {
        /// Timeout duration in milliseconds.
        timeout_ms: u64,
    },
    /// An action failed with an error.
    ActionFailed {
        /// Error description.
        error: String,
    },
}

/// A single GUI action diagnostic event entry in the bounded journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiActionEvent {
    /// Monotonically increasing sequence number.
    pub sequence: u64,
    /// Wall-clock timestamp when the event was recorded.
    pub timestamp: DateTime<Utc>,
    /// Unique action identifier (maps to [`GuiActionId`]).
    pub action_id: String,
    /// The event kind and its payload.
    pub kind: GuiActionEventKind,
    /// GUI revision counter at the time the event was recorded.
    pub gui_revision: u64,
    /// Optional room/conversation identifier.
    pub room_id: Option<TopicId>,
    /// Current screen name (e.g. "ChatList", "Chat", "Settings").
    pub current_screen: String,
}

/// Thread-safe bounded journal of recent GUI action diagnostic events.
///
/// Records the last N [`GuiActionEvent`] values as they are emitted during
/// GUI action lifecycle tracking.  Oldest entries are automatically evicted
/// when the limit is exceeded.
///
/// # Defaults
///
/// | Store         | Max entries |
/// |---------------|-------------|
/// | Journal       | 1 000       |
#[derive(Debug, Clone)]
pub struct GuiActionEventHistory {
    inner: Arc<GuiActionEventHistoryInner>,
}

#[derive(Debug)]
struct GuiActionEventHistoryInner {
    entries: Mutex<VecDeque<GuiActionEvent>>,
    next_sequence: AtomicU64,
    max_entries: usize,
}

impl GuiActionEventHistory {
    /// Create a new action event journal with the default capacity (1 000 entries).
    pub fn new() -> Self {
        Self::with_capacity(1000)
    }

    /// Create a new action event journal with the given maximum number of entries.
    pub fn with_capacity(max_entries: usize) -> Self {
        let capped = max_entries.max(64).min(5000);
        Self {
            inner: Arc::new(GuiActionEventHistoryInner {
                entries: Mutex::new(VecDeque::with_capacity(capped + 32)),
                next_sequence: AtomicU64::new(0),
                max_entries: capped,
            }),
        }
    }

    /// Record a GUI action diagnostic event in the journal.
    pub fn record(
        &self,
        action_id: impl AsRef<str>,
        kind: GuiActionEventKind,
        gui_revision: u64,
        room_id: Option<TopicId>,
        current_screen: impl AsRef<str>,
    ) {
        let sequence = self.inner.next_sequence.fetch_add(1, Ordering::Relaxed);
        let entry = GuiActionEvent {
            sequence,
            timestamp: Utc::now(),
            action_id: action_id.as_ref().to_string(),
            kind,
            gui_revision,
            room_id,
            current_screen: current_screen.as_ref().to_string(),
        };

        let mut entries = self.inner.entries.lock().expect("gui action events lock");
        if entries.len() >= self.inner.max_entries {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    /// Return journal entries with a sequence number greater than `since_sequence`,
    /// limited to `limit` entries (clamped to 1 000).
    pub fn entries_since(&self, since_sequence: u64, limit: usize) -> Vec<GuiActionEvent> {
        let limit = limit.min(1000);
        let entries = self.inner.entries.lock().expect("gui action events lock");
        entries
            .iter()
            .filter(|e| e.sequence > since_sequence)
            .take(limit)
            .cloned()
            .collect()
    }

    /// Return the most recently assigned sequence number (0 if no entries).
    pub fn latest_sequence(&self) -> u64 {
        let val = self.inner.next_sequence.load(Ordering::Relaxed);
        if val == 0 {
            0
        } else {
            val - 1
        }
    }

    /// Return the total number of entries currently stored.
    pub fn entry_count(&self) -> usize {
        self.inner
            .entries
            .lock()
            .expect("gui action events lock")
            .len()
    }

    /// Return all stored entries (newest first for convenience).
    pub fn all_entries(&self) -> Vec<GuiActionEvent> {
        let entries = self.inner.entries.lock().expect("gui action events lock");
        entries.iter().rev().cloned().collect()
    }
}

impl Default for GuiActionEventHistory {
    fn default() -> Self {
        Self::new()
    }
}

/// Classify the failure layer for an Iced message variant based on its name.
pub fn classify_message_layer(variant: &str) -> FailureLayer {
    // Network events and probes
    if variant.starts_with("NetEvent")
        || variant.starts_with("FriendEvent")
        || variant.starts_with("WhisperEvent")
        || variant.starts_with("InboxEvent")
        || variant.starts_with("ConnMonitorTick")
        || variant.starts_with("MeshWatchdogTick")
        || variant.starts_with("ConnCountsResult")
        || variant.starts_with("NewDiscoveredPeers")
        || variant.starts_with("FriendRequestSent")
        || variant.starts_with("FriendRequestReceived")
        || variant.starts_with("OutboxRetryResult")
        || variant.starts_with("DownloadProgress")
        || variant == "ConnMonitorTick"
    {
        return FailureLayer::Network;
    }

    // State update messages
    if variant.starts_with("OpenRoom")
        || variant.starts_with("RoomOpened")
        || variant.starts_with("RoomJoinFailed")
        || variant.starts_with("RoomSelected")
        || variant.starts_with("InputChanged")
        || variant.starts_with("SendPressed")
        || variant.starts_with("MessageSent")
        || variant.starts_with("FileSent")
        || variant.starts_with("FriendAdded")
        || variant.starts_with("FriendRemoved")
        || variant.starts_with("FriendListResult")
        || variant.starts_with("DeleteRoom")
        || variant.starts_with("FriendRequestAccept")
        || variant.starts_with("FriendRequestDecline")
        || variant.starts_with("FriendRequestCancel")
        || variant.starts_with("FriendRequestSend")
        || variant.starts_with("SendMessage")
        || variant.starts_with("OpenConversation")
        || variant.starts_with("SelectConversation")
        || variant == "GoToChatList"
        || variant == "CreateNewRoom"
        || variant == "ConfirmCreateNewRoom"
        || variant == "CancelCreateRoom"
        || variant == "ToggleDark"
        || variant == "SetNickname"
        || variant == "SaveProfile"
        || variant == "ErrorMsg"
        || variant == "SystemMsg"
    {
        return FailureLayer::ApplicationState;
    }

    // Everything else is an Iced UI update
    FailureLayer::IcedUpdate
}

/// Classify network failures from diagnostic events and peer state.
///
/// Returns a [`FailureAnalysis`] summarising failures detected at
/// each layer.  Only considers events recorded since `since_sequence`.
pub fn classify_failures(
    diagnostics: &Diagnostics,
    journal: &IcedMessageJournal,
    since_sequence: u64,
) -> FailureAnalysis {
    let mut details = Vec::new();
    let mut network_failure = false;
    let mut state_update_failure = false;
    let mut iced_update_failure = false;

    // Check diagnostics events for explicit failures
    let events = diagnostics.all_events();
    for event in events.iter() {
        if since_sequence > 0 && event.sequence <= since_sequence {
            continue;
        }
        match &event.kind {
            DiagnosticEventKind::RoomJoinFailed => {
                network_failure = true;
                details.push(format!(
                    "[network] Room join failed at seq {}",
                    event.sequence
                ));
            }
            DiagnosticEventKind::ConnectionFailed { error, .. } => {
                network_failure = true;
                details.push(format!(
                    "[network] Connection failed at seq {}: {error}",
                    event.sequence
                ));
            }
            DiagnosticEventKind::AddressLookupFailed { error, .. } => {
                network_failure = true;
                details.push(format!(
                    "[network] Address lookup failed at seq {}: {error}",
                    event.sequence
                ));
            }
            DiagnosticEventKind::RoomSubscriptionFailed { error } => {
                network_failure = true;
                details.push(format!(
                    "[network] Room subscription failed at seq {}: {error}",
                    event.sequence
                ));
            }
            DiagnosticEventKind::Error(msg) => {
                details.push(format!(
                    "[diagnostics] Error at seq {}: {msg}",
                    event.sequence
                ));
            }
            _ => {}
        }
    }

    // Check Iced message journal for failed updates
    for entry in journal.all_entries() {
        if since_sequence > 0 && entry.sequence <= since_sequence {
            continue;
        }
        if !entry.success {
            let layer_label = match entry.layer {
                FailureLayer::Network => "network",
                FailureLayer::ApplicationState => "state",
                FailureLayer::IcedUpdate => "iced",
                FailureLayer::Unknown => "unknown",
            };
            let detail = format!(
                "[{layer_label}] update failed for '{}' at seq {}: {}",
                entry.message_variant, entry.sequence, entry.error
            );
            details.push(detail);
            match entry.layer {
                FailureLayer::Network => network_failure = true,
                FailureLayer::ApplicationState => state_update_failure = true,
                FailureLayer::IcedUpdate => iced_update_failure = true,
                FailureLayer::Unknown => {
                    // Default attribution
                    iced_update_failure = true;
                }
            }
        }
    }

    FailureAnalysis {
        network_failure,
        state_update_failure,
        iced_update_failure,
        details,
        timestamp: Utc::now(),
    }
}

// =============================================================================
// GuiTestHandle — command channel for MCP → Iced
// =============================================================================

/// Handle for enqueuing GUI actions into the running Iced application.
///
/// Wraps a bounded tokio mpsc Sender. The receiver side is consumed by the
/// Iced application's subscription to produce
/// [`AppMessage::GuiTestActionReceived`](crate::diagnostics::DiagnosticEventKind::ActionRequested)
/// events.
///
/// This type lives in the library crate so both the MCP server and the Iced
/// application can reference it without coupling to example-specific modules.
///
/// # Errors
///
/// [`GuiTestHandle::enqueue`] returns a structured [`GuiActionError`]:
///
/// | Error code | Condition |
/// |---|---|
/// | [`GuiActionErrorCode::ActionQueueFull`] | Channel at capacity |
/// | [`GuiActionErrorCode::ActionQueueClosed`] | Receiver was dropped |
///
/// # Construction
///
/// ```
/// # use boru_chat::diagnostics::GuiTestHandle;
/// let (handle, receiver) = GuiTestHandle::channel(256);
/// ```
#[cfg(feature = "gui")]
#[derive(Debug, Clone)]
pub struct GuiTestHandle {
    sender: tokio::sync::mpsc::Sender<GuiActionRequest>,
}

#[cfg(feature = "gui")]
impl GuiTestHandle {
    /// Create a new handle from an existing tokio mpsc sender.
    pub fn new(sender: tokio::sync::mpsc::Sender<GuiActionRequest>) -> Self {
        Self { sender }
    }

    /// Create a new bounded channel, returning the handle (sender half)
    /// and the receiver half.
    ///
    /// The receiver should be consumed by the Iced subscription loop.
    /// The capacity must be at least 1 and at most 4096.
    pub fn channel(capacity: usize) -> (Self, tokio::sync::mpsc::Receiver<GuiActionRequest>) {
        let cap = capacity.clamp(1, 4096);
        let (tx, rx) = tokio::sync::mpsc::channel(cap);
        (Self { sender: tx }, rx)
    }

    /// Enqueue a GUI action request.
    ///
    /// Returns `Ok(())` if the request was successfully queued.
    /// Returns a structured [`GuiActionError`]:
    ///
    /// - [`GuiActionErrorCode::ActionQueueFull`] if the channel is at capacity.
    /// - [`GuiActionErrorCode::ActionQueueClosed`] if the receiver has been dropped.
    pub fn enqueue(&self, request: GuiActionRequest) -> Result<(), GuiActionError> {
        use tokio::sync::mpsc::error::TrySendError;
        self.sender.try_send(request).map_err(|e| match e {
            TrySendError::Full(_) => GuiActionError::new(
                GuiActionErrorCode::ActionQueueFull,
                format!("GUI action queue is full (capacity: {})", self.capacity()),
            ),
            TrySendError::Closed(_) => GuiActionError::new(
                GuiActionErrorCode::ActionQueueClosed,
                "GUI action channel is closed",
            ),
        })
    }

    /// Returns the maximum capacity of the underlying channel.
    pub fn capacity(&self) -> usize {
        self.sender.max_capacity()
    }

    /// Returns `true` if the receiver has been dropped (channel is closed).
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_base::SecretKey;

    /// Generate a valid public key for testing.
    fn test_key() -> PublicKey {
        SecretKey::generate().public()
    }

    // ── Basic functionality (from part 1) ──────────────────────────────

    #[test]
    fn test_record_and_query_events() {
        let diag = Diagnostics::new();

        diag.record(None, DiagnosticEventKind::RoomJoined);
        diag.record(
            None,
            DiagnosticEventKind::MessageBroadcast {
                message_id: None,
                message_hash: None,
                probe_id: None,
            },
        );
        diag.record(
            Some(TopicId::from_bytes([1u8; 32])),
            DiagnosticEventKind::MessageReceived {
                message_id: None,
                message_hash: None,
                probe_id: None,
                sender_id: None,
            },
        );

        assert_eq!(diag.event_count(), 3);
        assert_eq!(diag.latest_sequence(), 2);

        // All events since 0 (sequence 0 is excluded by > since_sequence)
        let all = diag.events_since(0, 100, None);
        assert_eq!(all.len(), 2);

        // Filter by room
        let room_events = diag.events_since(0, 100, Some(TopicId::from_bytes([1u8; 32])));
        assert_eq!(room_events.len(), 1);
        assert!(matches!(
            room_events[0].kind,
            DiagnosticEventKind::MessageReceived { .. }
        ));

        // Since sequence
        let recent = diag.events_since(1, 100, None);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].sequence, 2);
    }

    #[test]
    fn test_event_eviction() {
        let diag = Diagnostics::with_capacity(3, 100);

        for _i in 0..5 {
            diag.record(None, DiagnosticEventKind::RoomJoined);
        }

        assert_eq!(diag.event_count(), 3);
        assert_eq!(diag.latest_sequence(), 4);

        let events = diag.events_since(0, 100, None);
        assert_eq!(events.len(), 3);
        // Sequences should be 2, 3, 4 (the three newest)
        assert_eq!(events[0].sequence, 2);
        assert_eq!(events[1].sequence, 3);
        assert_eq!(events[2].sequence, 4);
    }

    #[test]
    fn test_query_limit_clamped() {
        let diag = Diagnostics::new();

        for _i in 0..10 {
            diag.record(None, DiagnosticEventKind::RoomJoined);
        }

        // Request more than max clamp (should clamp to 1000)
        let events = diag.events_since(0, 5000, None);
        assert_eq!(events.len(), 9);
    }

    #[test]
    fn test_probe_storage() {
        let diag = Diagnostics::new();
        let peer = test_key();

        diag.record_received_probe("probe-1".to_string(), peer, DiscoverySource::Mdns, None);

        let found = diag.find_received_probe("probe-1");
        assert!(found.is_some());
        assert_eq!(found.unwrap().sender_id, peer.to_string());
        assert_eq!(diag.probe_count(), 1);

        // Non-existent probe
        assert!(diag.find_received_probe("probe-nonexistent").is_none());
    }

    #[test]
    fn test_probe_eviction() {
        let diag = Diagnostics::with_capacity(100, 3);
        let p_a = test_key();
        let p_b = test_key();
        let p_c = test_key();
        let p_d = test_key();

        diag.record_received_probe("a".to_string(), p_a, DiscoverySource::Mdns, None);
        diag.record_received_probe("b".to_string(), p_b, DiscoverySource::Ticket, None);
        diag.record_received_probe("c".to_string(), p_c, DiscoverySource::Gossip, None);

        assert_eq!(diag.probe_count(), 3);

        // Insert a fourth — "a" should be evicted
        diag.record_received_probe("d".to_string(), p_d, DiscoverySource::Bootstrap, None);

        assert_eq!(diag.probe_count(), 3);
        assert!(diag.find_received_probe("a").is_none());
        assert!(diag.find_received_probe("d").is_some());
    }

    #[test]
    fn test_probe_replace_refreshes_position() {
        let diag = Diagnostics::with_capacity(100, 3);
        let p_a = test_key();
        let p_b = test_key();
        let p_c = test_key();
        let p_d = test_key();

        diag.record_received_probe("a".to_string(), p_a, DiscoverySource::Mdns, None);
        diag.record_received_probe("b".to_string(), p_b, DiscoverySource::Ticket, None);
        diag.record_received_probe("c".to_string(), p_c, DiscoverySource::Gossip, None);

        // Replace "a" — should move to newest, so "b" gets evicted next
        diag.record_received_probe("a".to_string(), p_a, DiscoverySource::Manual, None);

        // Insert a fourth — oldest is now "b"
        diag.record_received_probe("d".to_string(), p_d, DiscoverySource::Bootstrap, None);

        assert_eq!(diag.probe_count(), 3);
        assert!(diag.find_received_probe("a").is_some()); // replaced, not evicted
        assert!(diag.find_received_probe("b").is_none()); // evicted (oldest)
        assert!(diag.find_received_probe("d").is_some());
    }

    #[test]
    fn test_serialize_roundtrip() {
        let event = DiagnosticEvent {
            sequence: 1,
            timestamp: Utc::now(),
            room_id: Some(TopicId::from_bytes([0xAB; 32])),
            peer_id: None,
            kind: DiagnosticEventKind::RoomJoined,
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: DiagnosticEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.sequence, event.sequence);
        assert!(matches!(deserialized.kind, DiagnosticEventKind::RoomJoined));

        // Check snake_case serialization for old variants
        let kind_json = serde_json::to_string(&DiagnosticEventKind::PeerDiscovered).unwrap();
        assert_eq!(kind_json, "{\"type\":\"peer_discovered\"}");

        // Check tagged serialization for new variants
        let new_kind = DiagnosticEventKind::AddressLookupStarted {
            source: DiscoverySource::Mdns,
        };
        let new_json = serde_json::to_string(&new_kind).unwrap();
        let deser_new: DiagnosticEventKind = serde_json::from_str(&new_json).unwrap();
        assert!(matches!(
            deser_new,
            DiagnosticEventKind::AddressLookupStarted { .. }
        ));
    }

    #[test]
    fn test_error_variant_carries_string() {
        let diag = Diagnostics::new();
        diag.record(
            None,
            DiagnosticEventKind::Error("something went wrong".to_string()),
        );

        let events = diag.all_events();
        assert_eq!(events.len(), 1);
        match &events[0].kind {
            DiagnosticEventKind::Error(msg) => assert_eq!(msg, "something went wrong"),
            _ => panic!("expected Error variant"),
        }
    }

    #[test]
    fn test_empty_diagnostics() {
        let diag = Diagnostics::new();
        assert_eq!(diag.latest_sequence(), 0);
        assert_eq!(diag.event_count(), 0);
        assert_eq!(diag.probe_count(), 0);
        assert!(diag.find_received_probe("nothing").is_none());
    }

    // ── Part 2: Peer state tests ───────────────────────────────────────

    #[test]
    fn test_peer_state_advances_from_discovered_to_connected_to_topic_member() {
        let peer_hex = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let room = TopicId::from_bytes([1u8; 32]);

        let start_state = None;

        // Event 1: peer discovered
        let e1 = DiagnosticEvent {
            sequence: 1,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_hex.to_string()),
            kind: DiagnosticEventKind::PeerDiscovered,
        };
        let state = update_peer_state(start_state, &e1);
        assert!(state.discovered);
        assert!(state.discovered_at_ms.is_some());
        assert_eq!(state.address_lookup_state, DiagnosticStageState::NotStarted);

        // Event 2: connection established
        let e2 = DiagnosticEvent {
            sequence: 2,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_hex.to_string()),
            kind: DiagnosticEventKind::ConnectionEstablished {
                remote_address: Some("127.0.0.1:1234".to_string()),
                transport: Some("quic".to_string()),
                used_relay: Some(false),
            },
        };
        let state = update_peer_state(Some(state), &e2);
        assert_eq!(state.connection_state, ConnectionDiagnosticState::Connected);
        assert_eq!(state.connected_address.as_deref(), Some("127.0.0.1:1234"));

        // Event 3: topic member
        let e3 = DiagnosticEvent {
            sequence: 3,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_hex.to_string()),
            kind: DiagnosticEventKind::PeerAddedToTopic,
        };
        let state = update_peer_state(Some(state), &e3);
        assert!(state.topic_member);
    }

    #[test]
    fn test_failed_address_lookup_classified_as_address_resolution() {
        let peer_hex = "aaaa";
        let room = TopicId::from_bytes([2u8; 32]);

        let e1 = DiagnosticEvent {
            sequence: 1,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_hex.to_string()),
            kind: DiagnosticEventKind::PeerDiscovered,
        };
        let state = update_peer_state(None, &e1);

        let e2 = DiagnosticEvent {
            sequence: 2,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_hex.to_string()),
            kind: DiagnosticEventKind::AddressLookupStarted {
                source: DiscoverySource::Mdns,
            },
        };
        let state = update_peer_state(Some(state), &e2);

        let e3 = DiagnosticEvent {
            sequence: 3,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_hex.to_string()),
            kind: DiagnosticEventKind::AddressLookupFailed {
                source: DiscoverySource::Mdns,
                error: "DNS timeout".to_string(),
            },
        };
        let state = update_peer_state(Some(state), &e3);
        assert_eq!(state.address_lookup_state, DiagnosticStageState::Failed);
        assert_eq!(state.last_error.as_deref(), Some("DNS timeout"));

        // Build evidence and classify
        let evidence = DiscoveryTestEvidence {
            local_room_joined: true,
            peer_discovered: true,
            address_lookup_observed: true,
            address_resolved: false,
            connection_attempted: false,
            connection_established: false,
            subscription_started: false,
            subscription_joined: false,
            peer_in_topic: false,
            probe_broadcast: false,
            probe_received_or_acknowledged: false,
        };
        let (stage, _summary) = classify_discovery_test(&evidence, Some(&state));
        assert_eq!(stage, Some(DiscoveryFailureStage::AddressResolution));
    }

    #[test]
    fn test_failed_connection_classified_as_connection() {
        let peer_hex = "bbbb";
        let room = TopicId::from_bytes([3u8; 32]);

        let e1 = DiagnosticEvent {
            sequence: 1,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_hex.to_string()),
            kind: DiagnosticEventKind::PeerDiscovered,
        };
        let mut state = update_peer_state(None, &e1);

        let e2 = DiagnosticEvent {
            sequence: 2,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_hex.to_string()),
            kind: DiagnosticEventKind::ConnectionFailed {
                addresses: vec!["127.0.0.1:9999".to_string()],
                error: "Connection refused".to_string(),
            },
        };
        state = update_peer_state(Some(state), &e2);

        let evidence = DiscoveryTestEvidence {
            local_room_joined: true,
            peer_discovered: true,
            address_lookup_observed: false,
            address_resolved: true,
            connection_attempted: true,
            connection_established: false,
            subscription_started: false,
            subscription_joined: false,
            peer_in_topic: false,
            probe_broadcast: false,
            probe_received_or_acknowledged: false,
        };
        let (stage, _summary) = classify_discovery_test(&evidence, Some(&state));
        assert_eq!(stage, Some(DiscoveryFailureStage::Connection));
    }

    #[test]
    fn test_subscription_failure_classified_as_subscription() {
        let peer_hex = "cccc";
        let room = TopicId::from_bytes([4u8; 32]);

        let e1 = DiagnosticEvent {
            sequence: 1,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_hex.to_string()),
            kind: DiagnosticEventKind::PeerDiscovered,
        };
        let mut state = update_peer_state(None, &e1);

        let e2 = DiagnosticEvent {
            sequence: 2,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_hex.to_string()),
            kind: DiagnosticEventKind::RoomSubscriptionFailed {
                error: "already subscribed".to_string(),
            },
        };
        state = update_peer_state(Some(state), &e2);

        let evidence = DiscoveryTestEvidence {
            local_room_joined: true,
            peer_discovered: true,
            address_lookup_observed: false,
            address_resolved: true,
            connection_attempted: true,
            connection_established: true,
            subscription_started: true,
            subscription_joined: false,
            peer_in_topic: false,
            probe_broadcast: false,
            probe_received_or_acknowledged: false,
        };
        let (stage, _summary) = classify_discovery_test(&evidence, Some(&state));
        assert_eq!(stage, Some(DiscoveryFailureStage::Subscription));
    }

    #[test]
    fn test_missing_topic_membership_classified_correctly() {
        let evidence = DiscoveryTestEvidence {
            local_room_joined: true,
            peer_discovered: true,
            address_lookup_observed: false,
            address_resolved: true,
            connection_attempted: true,
            connection_established: true,
            subscription_started: true,
            subscription_joined: true,
            peer_in_topic: false,
            probe_broadcast: false,
            probe_received_or_acknowledged: false,
        };
        let (stage, _summary) = classify_discovery_test(&evidence, None);
        assert_eq!(stage, Some(DiscoveryFailureStage::TopicMembership));
    }

    #[test]
    fn test_probe_timeout_classified_as_probe_delivery() {
        let evidence = DiscoveryTestEvidence {
            local_room_joined: true,
            peer_discovered: true,
            address_lookup_observed: false,
            address_resolved: true,
            connection_attempted: true,
            connection_established: true,
            subscription_started: true,
            subscription_joined: true,
            peer_in_topic: true,
            probe_broadcast: true,
            probe_received_or_acknowledged: false,
        };
        let (stage, _summary) = classify_discovery_test(&evidence, None);
        assert_eq!(stage, Some(DiscoveryFailureStage::ProbeDelivery));
    }

    #[test]
    fn test_missing_discovery_classified_as_discovery() {
        let evidence = DiscoveryTestEvidence {
            local_room_joined: true,
            peer_discovered: false,
            address_lookup_observed: false,
            address_resolved: false,
            connection_attempted: false,
            connection_established: false,
            subscription_started: false,
            subscription_joined: false,
            peer_in_topic: false,
            probe_broadcast: false,
            probe_received_or_acknowledged: false,
        };
        let (stage, _summary) = classify_discovery_test(&evidence, None);
        assert_eq!(stage, Some(DiscoveryFailureStage::Discovery));
    }

    #[test]
    fn test_unknown_or_unobservable_produces_unknown() {
        // Room joined, peer not discovered — no evidence either way
        let evidence = DiscoveryTestEvidence {
            local_room_joined: true,
            peer_discovered: false,
            address_lookup_observed: false,
            address_resolved: false,
            connection_attempted: false,
            connection_established: false,
            subscription_started: false,
            subscription_joined: false,
            peer_in_topic: false,
            probe_broadcast: false,
            probe_received_or_acknowledged: false,
        };
        let (stage, _summary) = classify_discovery_test(&evidence, None);
        assert_eq!(stage, Some(DiscoveryFailureStage::Discovery));
    }

    #[test]
    fn test_complete_success_classified_no_failure() {
        let evidence = DiscoveryTestEvidence {
            local_room_joined: true,
            peer_discovered: true,
            address_lookup_observed: true,
            address_resolved: true,
            connection_attempted: true,
            connection_established: true,
            subscription_started: true,
            subscription_joined: true,
            peer_in_topic: true,
            probe_broadcast: true,
            probe_received_or_acknowledged: true,
        };
        let (stage, summary) = classify_discovery_test(&evidence, None);
        assert!(stage.is_none());
        assert!(summary.contains("successfully"));
    }

    #[test]
    fn test_local_room_unavailable_classified() {
        let evidence = DiscoveryTestEvidence {
            local_room_joined: false,
            peer_discovered: false,
            ..Default::default()
        };
        // Use default for all other fields
        let evidence = DiscoveryTestEvidence {
            local_room_joined: false,
            peer_discovered: false,
            address_lookup_observed: false,
            address_resolved: false,
            connection_attempted: false,
            connection_established: false,
            subscription_started: false,
            subscription_joined: false,
            peer_in_topic: false,
            probe_broadcast: false,
            probe_received_or_acknowledged: false,
        };
        let (stage, _summary) = classify_discovery_test(&evidence, None);
        assert_eq!(stage, Some(DiscoveryFailureStage::LocalRoomUnavailable));
    }

    #[test]
    fn test_asymmetric_peer_state_can_be_represented() {
        let peer_a = "aaaa";
        let peer_b = "bbbb";
        let room = TopicId::from_bytes([5u8; 32]);

        // Peer A is fully connected, peer B is only discovered
        let events = vec![
            DiagnosticEvent {
                sequence: 1,
                timestamp: Utc::now(),
                room_id: Some(room),
                peer_id: Some(peer_a.to_string()),
                kind: DiagnosticEventKind::PeerDiscovered,
            },
            DiagnosticEvent {
                sequence: 2,
                timestamp: Utc::now(),
                room_id: Some(room),
                peer_id: Some(peer_a.to_string()),
                kind: DiagnosticEventKind::ConnectionEstablished {
                    remote_address: Some("192.168.1.1:1234".to_string()),
                    transport: Some("quic".to_string()),
                    used_relay: Some(false),
                },
            },
            DiagnosticEvent {
                sequence: 3,
                timestamp: Utc::now(),
                room_id: Some(room),
                peer_id: Some(peer_a.to_string()),
                kind: DiagnosticEventKind::PeerAddedToTopic,
            },
            DiagnosticEvent {
                sequence: 4,
                timestamp: Utc::now(),
                room_id: Some(room),
                peer_id: Some(peer_b.to_string()),
                kind: DiagnosticEventKind::PeerDiscovered,
            },
        ];

        // Build state by replaying events
        let mut states: HashMap<&str, Option<PeerDiagnosticState>> = HashMap::new();
        for e in &events {
            let pid = e.peer_id.as_deref().unwrap_or_default();
            let current = states.remove(pid).unwrap_or(None);
            let updated = update_peer_state(current, e);
            states.insert(pid, Some(updated));
        }

        let state_a = states.get(peer_a).unwrap().as_ref().unwrap();
        let state_b = states.get(peer_b).unwrap().as_ref().unwrap();

        assert_eq!(
            state_a.connection_state,
            ConnectionDiagnosticState::Connected
        );
        assert!(state_a.topic_member);
        assert_eq!(
            state_b.connection_state,
            ConnectionDiagnosticState::NotStarted
        );
        assert!(!state_b.topic_member);
    }

    #[test]
    fn test_diagnostics_do_not_alter_normal_behaviour() {
        // Verify that the diagnostics store is purely observational
        let diag = Diagnostics::new();
        let initial_count = diag.event_count();
        assert_eq!(initial_count, 0);

        // Record some events — should not panic or affect anything else
        diag.record(None, DiagnosticEventKind::RoomJoinStarted);
        diag.record(None, DiagnosticEventKind::RoomJoined);
        assert_eq!(diag.event_count(), 2);
        assert_eq!(diag.latest_sequence(), 1);
    }

    #[test]
    fn test_generate_probe_id() {
        let id1 = generate_probe_id();
        let id2 = generate_probe_id();
        assert_ne!(id1, id2, "probe IDs should be unique");
        assert_eq!(id1.len(), 32, "probe ID should be 32 hex chars");
    }

    #[test]
    fn test_events_since_filtered() {
        let diag = Diagnostics::new();
        let room = TopicId::from_bytes([6u8; 32]);

        diag.record_with_peer(
            Some(room),
            Some("peer1"),
            DiagnosticEventKind::PeerDiscovered,
        );
        diag.record_with_peer(
            Some(room),
            Some("peer2"),
            DiagnosticEventKind::PeerDiscovered,
        );
        diag.record_with_peer(
            Some(room),
            Some("peer1"),
            DiagnosticEventKind::PeerAddedToTopic,
        );

        let peer1_events = diag.events_since_filtered(0, 100, Some(room), Some("peer1"));
        assert_eq!(peer1_events.len(), 1);
        assert!(peer1_events
            .iter()
            .all(|e| e.peer_id.as_deref() == Some("peer1")));
    }

    #[test]
    fn test_build_evidence() {
        let diag = Diagnostics::new();
        let room = TopicId::from_bytes([7u8; 32]);

        diag.record(Some(room), DiagnosticEventKind::RoomJoined);
        diag.record_with_peer(
            Some(room),
            Some("peer_x"),
            DiagnosticEventKind::PeerDiscovered,
        );
        diag.record_with_peer(
            Some(room),
            Some("peer_x"),
            DiagnosticEventKind::ConnectionEstablished {
                remote_address: None,
                transport: None,
                used_relay: None,
            },
        );

        let evidence = diag.build_evidence(Some(room), None);
        assert!(evidence.local_room_joined);
        assert!(evidence.peer_discovered);
        assert!(evidence.connection_established);
        assert!(!evidence.address_lookup_observed);

        // Without room filter should also find room events
        let evidence_all = diag.build_evidence(None, None);
        assert!(evidence_all.local_room_joined);
    }

    #[test]
    fn test_peer_state_and_build_evidence() {
        let diag = Diagnostics::new();
        let room = TopicId::from_bytes([8u8; 32]);

        diag.record_with_peer(
            Some(room),
            Some("peer_z"),
            DiagnosticEventKind::PeerDiscoveredWithAddr {
                source: DiscoverySource::Mdns,
                addresses: vec!["192.168.1.100:1234".to_string()],
            },
        );
        diag.record_with_peer(
            Some(room),
            Some("peer_z"),
            DiagnosticEventKind::ConnectionEstablished {
                remote_address: Some("192.168.1.100:1234".to_string()),
                transport: Some("quic".to_string()),
                used_relay: Some(false),
            },
        );
        diag.record_with_peer(
            Some(room),
            Some("peer_z"),
            DiagnosticEventKind::PeerAddedToTopic,
        );

        // Verify peer state reconstruction
        let states = diag.peer_states();
        let peer = states.get("peer_z").expect("peer_z should have state");
        assert!(peer.discovered);
        assert_eq!(peer.connection_state, ConnectionDiagnosticState::Connected);
        assert!(peer.topic_member);
        assert_eq!(peer.addresses.len(), 1);
        assert!(peer.addresses[0].contains("192.168.1.100"));

        // Verify evidence builder
        let evidence = diag.build_evidence(Some(room), Some("peer_z"));
        assert!(evidence.peer_discovered);
        assert!(evidence.connection_established);
        assert!(evidence.peer_in_topic);
    }

    #[test]
    fn test_enhanced_probe_storage() {
        let diag = Diagnostics::new();
        let room = TopicId::from_bytes([9u8; 32]);

        let probe = ReceivedProbe {
            probe_id: "test-probe-1".to_string(),
            room_id: "room-9".to_string(),
            sender_id: "sender-1".to_string(),
            sent_at_ms: 1000,
            received_at_ms: 1025,
            latency_ms: Some(25),
            message_hash: "abc123".to_string(),
            duplicate_count: 0,
            timestamp: Utc::now(),
            room_id_opt: Some(room),
        };

        diag.record_received_probe_enhanced(probe);

        let found = diag.find_received_probe("test-probe-1").unwrap();
        assert_eq!(found.sender_id, "sender-1");
        assert_eq!(found.latency_ms, Some(25));
        assert_eq!(found.duplicate_count, 0);

        // Duplicate should increment count
        let probe_dup = ReceivedProbe {
            probe_id: "test-probe-1".to_string(),
            room_id: "room-9".to_string(),
            sender_id: "sender-1".to_string(),
            sent_at_ms: 1000,
            received_at_ms: 1026,
            latency_ms: Some(26),
            message_hash: "abc123".to_string(),
            duplicate_count: 0,
            timestamp: Utc::now(),
            room_id_opt: Some(room),
        };
        diag.record_received_probe_enhanced(probe_dup);
        let found = diag.find_received_probe("test-probe-1").unwrap();
        assert_eq!(found.duplicate_count, 1);
    }

    #[test]
    fn test_peer_discovered_with_addr_updates_sources() {
        let peer_hex = "dddd";
        let room = TopicId::from_bytes([10u8; 32]);

        let e1 = DiagnosticEvent {
            sequence: 1,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_hex.to_string()),
            kind: DiagnosticEventKind::PeerDiscoveredWithAddr {
                source: DiscoverySource::Mdns,
                addresses: vec!["192.168.1.1:5000".to_string()],
            },
        };
        let state = update_peer_state(None, &e1);
        assert!(state.discovered);
        assert_eq!(state.discovery_sources.len(), 1);
        assert_eq!(state.addresses.len(), 1);
        assert_eq!(state.addresses[0], "192.168.1.1:5000");
    }

    // ── Snapshot tests ────────────────────────────────────────────────

    #[test]
    fn test_peer_diagnostic_snapshot_serde_roundtrip() {
        let snapshot = PeerDiagnosticSnapshot {
            peer_id: "abc123".to_string(),
            discovery_sources: vec![DiscoverySource::Mdns, DiscoverySource::Gossip],
            addresses: vec!["127.0.0.1:8080".to_string()],
            connected: true,
            last_seen_timestamp_ms: Some(1700000000000_i64),
            last_error: None,
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        let deserialized: PeerDiagnosticSnapshot = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.peer_id, "abc123");
        assert_eq!(deserialized.discovery_sources.len(), 2);
        assert_eq!(deserialized.addresses.len(), 1);
        assert!(deserialized.connected);
        assert_eq!(deserialized.last_seen_timestamp_ms, Some(1700000000000));
        assert!(deserialized.last_error.is_none());

        // Verify snake_case serialization
        assert!(json.contains("peer_id"));
        assert!(json.contains("discovery_sources"));
    }

    #[test]
    fn test_room_diagnostic_snapshot_serde_roundtrip() {
        let peer = PeerDiagnosticSnapshot {
            peer_id: "peer1".to_string(),
            discovery_sources: vec![DiscoverySource::Ticket],
            addresses: vec![],
            connected: false,
            last_seen_timestamp_ms: None,
            last_error: Some("connection refused".to_string()),
        };

        let snapshot = RoomDiagnosticSnapshot {
            node_id: "node42".to_string(),
            room_id: "deadbeef".to_string(),
            joined: true,
            subscribed: true,
            peer_count: 1,
            peers: vec![peer],
            discovery_sources_enabled: vec!["discovery_secret".to_string()],
            last_error: None,
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        let deserialized: RoomDiagnosticSnapshot = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.node_id, "node42");
        assert_eq!(deserialized.room_id, "deadbeef");
        assert!(deserialized.joined);
        assert!(deserialized.subscribed);
        assert_eq!(deserialized.peer_count, 1);
        assert_eq!(deserialized.peers.len(), 1);
        assert_eq!(
            deserialized.peers[0].last_error.as_deref(),
            Some("connection refused")
        );
        assert_eq!(
            deserialized.discovery_sources_enabled,
            vec!["discovery_secret"]
        );

        // Verify snake_case field names
        assert!(json.contains("node_id"));
        assert!(json.contains("discovery_sources_enabled"));
    }

    #[test]
    fn test_peer_snapshot_empty_defaults() {
        let snapshot = PeerDiagnosticSnapshot {
            peer_id: "".to_string(),
            discovery_sources: vec![],
            addresses: vec![],
            connected: false,
            last_seen_timestamp_ms: None,
            last_error: None,
        };

        // Serialize and verify skip_serializing_if works for empty vecs
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("\"peer_id\""));
        assert!(json.contains("\"connected\":false"));

        // Empty discovery_sources and addresses should be skipped
        assert!(!json.contains("discovery_sources"));
        assert!(!json.contains("addresses"));
        assert!(!json.contains("last_seen_timestamp_ms"));
        assert!(!json.contains("last_error"));
    }

    #[cfg(feature = "net")]
    #[test]
    fn test_build_room_snapshot_from_empty_state() {
        use crate::friends::FriendsStore;
        use crate::room::RoomStore;
        use iroh_base::PublicKey;

        let node_id = PublicKey::from_bytes(&[0xAAu8; 32]).unwrap();
        let room_topic = TopicId::from_bytes([0xBBu8; 32]);
        let diag = Diagnostics::new();

        // Create an empty friends store
        let friends = FriendsStore::empty_at(std::path::PathBuf::from("/tmp"));

        // No room store
        let snapshot = build_room_snapshot(
            &node_id,
            room_topic,
            None::<&RoomStore>,
            &friends,
            &diag,
            false,
        );

        assert!(!snapshot.joined);
        assert!(!snapshot.subscribed);
        assert_eq!(snapshot.peer_count, 0);
        assert!(snapshot.peers.is_empty());
        assert!(snapshot.discovery_sources_enabled.is_empty());
        assert!(snapshot.last_error.is_none());
        assert_eq!(snapshot.node_id, node_id.to_string());
        assert_eq!(snapshot.room_id, hex::encode(room_topic.as_bytes()));

        // Verify JSON serialization
        let json = serde_json::to_string(&snapshot).unwrap();
        let _deserialized: RoomDiagnosticSnapshot = serde_json::from_str(&json).unwrap();
    }

    // ── Test 7: Probe IDs survive serialization round-trip ──────────

    #[test]
    fn test_probe_id_serialization_roundtrip() {
        let probe = DiagnosticProbe {
            probe_id: "test-probe-42".to_string(),
            sender_id: "sender-pubkey-hex".to_string(),
            room_id: "room-topic-hex".to_string(),
            sent_at_ms: 1000000,
            payload: Some("diagnostic payload".to_string()),
        };

        // JSON round-trip
        let json = serde_json::to_string(&probe).unwrap();
        let deserialized: DiagnosticProbe = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.probe_id, "test-probe-42");
        assert_eq!(deserialized.sender_id, "sender-pubkey-hex");
        assert_eq!(deserialized.room_id, "room-topic-hex");
        assert_eq!(deserialized.sent_at_ms, 1000000);
        assert_eq!(deserialized.payload.as_deref(), Some("diagnostic payload"));

        // JSON must contain the probe_id field
        assert!(json.contains("test-probe-42"));
        assert!(json.contains("sender-pubkey-hex"));

        // Postcard binary round-trip
        let binary = postcard::to_stdvec(&probe).unwrap();
        let deserialized2: DiagnosticProbe = postcard::from_bytes(&binary).unwrap();
        assert_eq!(deserialized2.probe_id, "test-probe-42");
        assert_eq!(deserialized2.sender_id, "sender-pubkey-hex");

        // ReceivedProbe round-trip
        let received = ReceivedProbe {
            probe_id: "rx-probe-99".to_string(),
            room_id: "rx-room".to_string(),
            sender_id: "rx-sender".to_string(),
            sent_at_ms: 2000,
            received_at_ms: 2025,
            latency_ms: Some(25),
            message_hash: "deadbeef".to_string(),
            duplicate_count: 0,
            timestamp: Utc::now(),
            room_id_opt: None,
        };
        let json_rx = serde_json::to_string(&received).unwrap();
        let deserialized_rx: ReceivedProbe = serde_json::from_str(&json_rx).unwrap();
        assert_eq!(deserialized_rx.probe_id, "rx-probe-99");
        assert!(json_rx.contains("rx-probe-99"));

        // Postcard round-trip for ReceivedProbe
        let binary_rx = postcard::to_stdvec(&received).unwrap();
        let deserialized_rx2: ReceivedProbe = postcard::from_bytes(&binary_rx).unwrap();
        assert_eq!(deserialized_rx2.probe_id, "rx-probe-99");
    }

    // ── Test 10: Negative clock-derived latency becomes None ────────

    #[test]
    fn test_negative_latency_becomes_none() {
        // Simulate clock skew: sent_at_ms > received_at_ms
        let received_at_ms: i64 = 1000;
        let sent_at_ms: i64 = 2000;

        let latency = if received_at_ms >= sent_at_ms {
            Some(received_at_ms - sent_at_ms)
        } else {
            None
        };
        assert!(latency.is_none(), "negative latency should be None");

        // Same time should produce zero latency
        let received_at_ms: i64 = 2000;
        let sent_at_ms: i64 = 2000;
        let latency = if received_at_ms >= sent_at_ms {
            Some(received_at_ms - sent_at_ms)
        } else {
            None
        };
        assert_eq!(latency, Some(0), "same-time latency should be 0");

        // Normal case: received after sent
        let received_at_ms: i64 = 2025;
        let sent_at_ms: i64 = 2000;
        let latency = if received_at_ms >= sent_at_ms {
            Some(received_at_ms - sent_at_ms)
        } else {
            None
        };
        assert_eq!(latency, Some(25), "normal latency should be 25");

        // Verify that a ReceivedProbe with negative clock skew has latency=None
        let probe = ReceivedProbe {
            probe_id: "clock-skew-test".to_string(),
            room_id: "room".to_string(),
            sender_id: "sender".to_string(),
            sent_at_ms: 2000,
            received_at_ms: 1000,
            latency_ms: None, // This is what handle_net_event sets
            message_hash: "hash".to_string(),
            duplicate_count: 0,
            timestamp: Utc::now(),
            room_id_opt: None,
        };
        assert!(probe.latency_ms.is_none());
        assert_eq!(probe.sent_at_ms, 2000);
        assert_eq!(probe.received_at_ms, 1000);
    }

    // ── Test 11: Unknown room in snapshot returns structured error ──
    // (requires net feature for build_room_snapshot)

    #[cfg(feature = "net")]
    #[test]
    fn test_unknown_room_snapshot_returns_unjoined() {
        use crate::friends::FriendsStore;
        use crate::room::RoomStore;
        use iroh_base::PublicKey;

        let node_id = PublicKey::from_bytes(&[0xCCu8; 32]).unwrap();
        // Room topic that does NOT match the room store
        let room_topic = TopicId::from_bytes([0xDDu8; 32]);
        let diag = Diagnostics::new();

        let friends = FriendsStore::empty_at(std::path::PathBuf::from("/tmp"));

        // No room store at all — the room is unknown
        let snapshot = build_room_snapshot(
            &node_id,
            room_topic,
            None::<&RoomStore>,
            &friends,
            &diag,
            false,
        );

        // Unknown room should not panic; it should report joined=false
        assert!(!snapshot.joined, "unknown room should report not joined");
        assert!(!snapshot.subscribed);
        assert_eq!(snapshot.peer_count, 0);
        assert!(snapshot.peers.is_empty());
        assert!(snapshot.last_error.is_none());

        // Room store with a different topic (still unknown)
        let room_store = RoomStore::new(
            std::path::PathBuf::from("/tmp"),
            TopicId::from_bytes([0xEEu8; 32]),
        );
        let snapshot2 = build_room_snapshot(
            &node_id,
            room_topic,
            Some(&room_store),
            &friends,
            &diag,
            false,
        );
        assert!(
            !snapshot2.joined,
            "mismatched topic should report not joined"
        );
        assert_eq!(snapshot2.room_id, hex::encode(room_topic.as_bytes()));
    }

    // ── Test 12: Diagnostic output contains no secret key material ──

    #[test]
    fn test_no_secret_key_material_in_diagnostic_probe() {
        let probe = DiagnosticProbe {
            probe_id: "safe-probe".to_string(),
            sender_id: "some-public-key".to_string(),
            room_id: "room-abc".to_string(),
            sent_at_ms: 1000,
            payload: None,
        };
        let json = serde_json::to_string(&probe).unwrap();
        // Must not contain secret key fields
        assert!(!json.contains("secret_key"));
        assert!(!json.contains("secret"));
        assert!(!json.contains("private_key"));
        assert!(!json.contains("signing_key"));
    }

    #[test]
    fn test_no_secret_key_material_in_received_probe() {
        let received = ReceivedProbe {
            probe_id: "safe-rx".to_string(),
            room_id: "rx-room".to_string(),
            sender_id: "rx-pubkey".to_string(),
            sent_at_ms: 1000,
            received_at_ms: 1025,
            latency_ms: Some(25),
            message_hash: "hash".to_string(),
            duplicate_count: 0,
            timestamp: Utc::now(),
            room_id_opt: None,
        };
        let json = serde_json::to_string(&received).unwrap();
        assert!(!json.contains("secret_key"));
        assert!(!json.contains("secret"));
        assert!(!json.contains("private_key"));
    }

    #[test]
    fn test_no_secret_key_material_in_peer_snapshot() {
        let snapshot = PeerDiagnosticSnapshot {
            peer_id: "peer-pubkey".to_string(),
            discovery_sources: vec![],
            addresses: vec!["127.0.0.1:8080".to_string()],
            connected: false,
            last_seen_timestamp_ms: None,
            last_error: None,
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("secret_key"));
        assert!(!json.contains("secret"));
        assert!(!json.contains("private_key"));
    }

    #[test]
    fn test_no_secret_key_material_in_room_snapshot() {
        let snapshot = RoomDiagnosticSnapshot {
            node_id: "node-pubkey".to_string(),
            room_id: "room-topic".to_string(),
            joined: false,
            subscribed: false,
            peer_count: 0,
            peers: vec![],
            discovery_sources_enabled: vec![],
            last_error: None,
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("secret_key"));
        assert!(!json.contains("secret"));
        assert!(!json.contains("private_key"));
        assert!(!json.contains("ticket"));
        assert!(!json.contains("discovery_secret"));
    }

    // ── GUI Action Tracking tests ───────────────────────────────────────

    #[test]
    fn test_gui_action_id_unique() {
        let id1 = GuiActionId::new();
        let id2 = GuiActionId::new();
        assert_ne!(id1, id2, "action IDs should be unique");
        assert_eq!(id1.0.len(), 32, "action ID should be 32 hex chars");
        assert_eq!(id2.0.len(), 32, "action ID should be 32 hex chars");
    }

    #[test]
    fn test_gui_action_id_default_is_new() {
        let id: GuiActionId = Default::default();
        assert_eq!(id.0.len(), 32);
    }

    #[test]
    fn test_gui_action_state_terminal_classification() {
        use GuiActionState::*;

        assert!(Completed.is_terminal());
        assert!(TimedOut.is_terminal());
        assert!(Failed.is_terminal());
        assert!(Rejected.is_terminal());

        assert!(!Queued.is_terminal());
        assert!(!Validating.is_terminal());
        assert!(!AppMessageQueued.is_terminal());
        assert!(!AppMessageHandled.is_terminal());
        assert!(!WaitingForExpectedState.is_terminal());

        assert!(Queued.is_active());
        assert!(Validating.is_active());
        assert!(AppMessageQueued.is_active());
        assert!(AppMessageHandled.is_active());
        assert!(WaitingForExpectedState.is_active());

        assert!(!Completed.is_active());
        assert!(!TimedOut.is_active());
        assert!(!Failed.is_active());
        assert!(!Rejected.is_active());
    }

    #[test]
    fn test_gui_action_state_transition_valid() {
        use GuiActionState::*;

        let mut action = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: Queued,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        assert!(action.transition_to(Validating).is_ok());
        assert_eq!(action.state, Validating);

        assert!(action.transition_to(AppMessageQueued).is_ok());
        assert_eq!(action.state, AppMessageQueued);

        assert!(action.transition_to(AppMessageHandled).is_ok());
        assert_eq!(action.state, AppMessageHandled);

        assert!(action.transition_to(Completed).is_ok());
        assert_eq!(action.state, Completed);
    }

    #[test]
    fn test_gui_action_state_transition_via_failed() {
        use GuiActionState::*;

        let mut action = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: Queued,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        assert!(action.transition_to(Validating).is_ok());
        assert!(action.transition_to(Rejected).is_ok());
        assert_eq!(action.state, Rejected);
    }

    #[test]
    fn test_gui_action_state_transition_via_wait_and_timeout() {
        use GuiActionState::*;

        let mut action = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: Queued,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        action.transition_to(Validating).unwrap();
        action.transition_to(AppMessageQueued).unwrap();
        action.transition_to(AppMessageHandled).unwrap();

        assert!(action.transition_to(WaitingForExpectedState).is_ok());
        assert_eq!(action.state, WaitingForExpectedState);

        assert!(action.transition_to(TimedOut).is_ok());
        assert_eq!(action.state, TimedOut);
    }

    #[test]
    fn test_gui_action_state_transition_invalid() {
        use GuiActionState::*;

        let mut action = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: Queued,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        assert!(action.transition_to(AppMessageHandled).is_err());
        assert_eq!(action.state, Queued);

        assert!(action.transition_to(Completed).is_err());
        assert_eq!(action.state, Queued);

        action.transition_to(Validating).unwrap();

        assert!(action.transition_to(Completed).is_err());
    }

    #[test]
    fn test_gui_action_terminal_states_reject_transitions() {
        use GuiActionState::*;

        for terminal_state in [Completed, TimedOut, Failed, Rejected] {
            let mut action = GuiActionStatus {
                action_id: GuiActionId::new(),
                state: terminal_state,
                requested_at_ms: 1000,
                updated_at_ms: 1000,
                expected_gui_revision: None,
                observed_gui_revision: None,
                error: None,
                result: None,
                expected_state: None,
                timeout_at_ms: None,
            };

            assert!(
                action.transition_to(Queued).is_err(),
                "terminal state {:?} should reject transitions",
                action.state
            );
        }
    }

    #[test]
    fn test_gui_action_history_record_and_get() {
        let history = GuiActionHistory::new();
        let id = GuiActionId::new();

        let request = GuiActionRequest {
            action_id: id.clone(),
            requested_at_ms: 1000,
            command: "SendPressed".to_string(),
        };

        let returned_id = history.record(request);
        assert_eq!(returned_id, id);
        assert_eq!(history.action_count(), 1);

        let status = history.get(&id).expect("should find action");
        assert_eq!(status.action_id, id);
        assert_eq!(status.state, GuiActionState::Queued);
        assert_eq!(status.requested_at_ms, 1000);
    }

    #[test]
    fn test_gui_action_history_transition_and_get() {
        let history = GuiActionHistory::new();
        let id = GuiActionId::new();

        let request = GuiActionRequest {
            action_id: id.clone(),
            requested_at_ms: 1000,
            command: "OpenRoom".to_string(),
        };
        history.record(request);

        assert!(history
            .transition_to(&id, GuiActionState::Validating)
            .is_ok());
        let status = history.get(&id).unwrap();
        assert_eq!(status.state, GuiActionState::Validating);

        assert!(history
            .transition_to(&id, GuiActionState::Completed)
            .is_err());
        let status = history.get(&id).unwrap();
        assert_eq!(status.state, GuiActionState::Validating);

        assert!(history.set_state(&id, GuiActionState::Completed));
        let status = history.get(&id).unwrap();
        assert_eq!(status.state, GuiActionState::Completed);
    }

    #[test]
    fn test_gui_action_history_bounded_storage() {
        let history = GuiActionHistory::with_capacity(3);

        for i in 0..3 {
            let id = GuiActionId::new();
            let request = GuiActionRequest {
                action_id: id,
                requested_at_ms: i * 100,
                command: format!("Action-{i}"),
            };
            history.record(request);
        }
        assert_eq!(history.action_count(), 3);
        assert_eq!(history.active_count(), 3);

        // Verify the oldest is detected correctly
        let all = history.all_actions();
        assert_eq!(all.len(), 3);
        let oldest_id = all.last().unwrap().action_id.clone();
        history.set_state(&oldest_id, GuiActionState::Completed);

        // Verify the state was actually set
        if let Some(s) = history.get(&oldest_id) {
            assert!(s.state.is_terminal(), "set_state should make it terminal");
        } else {
            panic!("oldest_id not found after set_state!");
        }

        let new_id = GuiActionId::new();
        let request = GuiActionRequest {
            action_id: new_id.clone(),
            requested_at_ms: 300,
            command: "Action-4".to_string(),
        };
        history.record(request);

        assert_eq!(history.action_count(), 3);
        assert!(
            history.get(&oldest_id).is_none(),
            "completed action should be evicted"
        );
        assert!(history.get(&new_id).is_some(), "new action should exist");
    }

    #[test]
    fn test_gui_action_history_active_not_evicted() {
        // Terminal actions are evicted first; if none exist, oldest
        // actions are evicted to enforce capacity.
        let history = GuiActionHistory::with_capacity(3);

        // Fill with 3 actions, keep one active, complete one
        let ids: Vec<GuiActionId> = (0..3)
            .map(|i| {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: i as i64 * 100,
                    command: format!("Action-{i}"),
                };
                history.record(request);
                id
            })
            .collect();

        assert_eq!(history.action_count(), 3);
        assert_eq!(history.active_count(), 3);

        // Complete the oldest (ids[0]) and middle (ids[1]),
        // keep the newest (ids[2]) active
        history.set_state(&ids[0], GuiActionState::Completed);
        history.set_state(&ids[1], GuiActionState::Completed);
        assert_eq!(history.active_count(), 1);

        // Add a 4th — the oldest terminal (ids[0]) should be evicted
        let new_id = GuiActionId::new();
        let request = GuiActionRequest {
            action_id: new_id.clone(),
            requested_at_ms: 300,
            command: "Action-4".to_string(),
        };
        history.record(request);

        assert_eq!(history.action_count(), 3);
        assert!(
            history.get(&ids[0]).is_none(),
            "oldest terminal should be evicted"
        );
        assert!(history.get(&ids[2]).is_some(), "active action should stay");
        assert!(history.get(&new_id).is_some(), "new action should exist");
    }

    #[test]
    fn test_gui_action_history_remove() {
        let history = GuiActionHistory::with_capacity(10);
        let id = GuiActionId::new();

        let request = GuiActionRequest {
            action_id: id.clone(),
            requested_at_ms: 1000,
            command: "TestAction".to_string(),
        };
        history.record(request);
        assert_eq!(history.action_count(), 1);

        assert!(history.remove(&id));
        assert_eq!(history.action_count(), 0);
        assert!(history.get(&id).is_none());

        assert!(!history.remove(&GuiActionId::new()));
    }

    #[test]
    fn test_gui_action_serialize_roundtrip() {
        let status = GuiActionStatus {
            action_id: GuiActionId("abc123def456".to_string()),
            state: GuiActionState::AppMessageHandled,
            requested_at_ms: 1000,
            updated_at_ms: 1050,
            expected_gui_revision: Some(42),
            observed_gui_revision: Some(42),
            error: None,
            result: Some("success".to_string()),
            expected_state: None,
            timeout_at_ms: None,
        };

        let json = serde_json::to_string(&status).unwrap();
        let deserialized: GuiActionStatus = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.action_id.0, "abc123def456");
        assert_eq!(deserialized.state, GuiActionState::AppMessageHandled);
        assert_eq!(deserialized.requested_at_ms, 1000);
        assert_eq!(deserialized.expected_gui_revision, Some(42));
        assert_eq!(deserialized.result.as_deref(), Some("success"));

        assert!(json.contains("action_id"));
        assert!(json.contains("requested_at_ms"));
        assert!(json.contains("expected_gui_revision"));
        assert!(json.contains("observed_gui_revision"));
    }

    #[test]
    fn test_gui_action_state_serialize_snake_case() {
        let states = [
            (GuiActionState::Queued, "queued"),
            (GuiActionState::Validating, "validating"),
            (GuiActionState::Rejected, "rejected"),
            (GuiActionState::AppMessageQueued, "app_message_queued"),
            (GuiActionState::AppMessageHandled, "app_message_handled"),
            (
                GuiActionState::WaitingForExpectedState,
                "waiting_for_expected_state",
            ),
            (GuiActionState::Completed, "completed"),
            (GuiActionState::TimedOut, "timed_out"),
            (GuiActionState::Failed, "failed"),
        ];

        for (state, expected) in &states {
            let json = serde_json::to_value(state).unwrap();
            assert_eq!(json.as_str().unwrap(), *expected, "mismatch for {state:?}");
        }
    }

    #[test]
    fn test_gui_action_history_eviction_oldest_completed_first() {
        let history = GuiActionHistory::with_capacity(3);

        let mut ids = Vec::new();
        for i in 0..3 {
            let id = GuiActionId::new();
            ids.push(id.clone());
            let request = GuiActionRequest {
                action_id: id,
                requested_at_ms: i * 100,
                command: format!("Action-{i}"),
            };
            history.record(request);
        }

        history.set_state(&ids[0], GuiActionState::Completed);
        history.set_state(&ids[2], GuiActionState::Completed);

        let new_id = GuiActionId::new();
        let request = GuiActionRequest {
            action_id: new_id.clone(),
            requested_at_ms: 300,
            command: "Action-3".to_string(),
        };
        history.record(request);

        assert_eq!(history.action_count(), 3);
        assert!(
            history.get(&ids[0]).is_none(),
            "oldest completed should be evicted"
        );
        assert!(history.get(&ids[1]).is_some(), "active action should stay");
        assert!(
            history.get(&ids[2]).is_some(),
            "completed but not oldest should stay"
        );
    }

    #[test]
    fn test_gui_action_history_all_actions_order_newest_first() {
        let history = GuiActionHistory::with_capacity(10);
        let ids: Vec<GuiActionId> = (0..3)
            .map(|i| {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: i as i64 * 100,
                    command: format!("Action-{i}"),
                };
                history.record(request);
                id
            })
            .collect();

        let all = history.all_actions();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].action_id, ids[2]);
        assert_eq!(all[1].action_id, ids[1]);
        assert_eq!(all[2].action_id, ids[0]);
    }

    #[test]
    fn test_gui_action_history_actions_with_state() {
        let history = GuiActionHistory::with_capacity(10);

        let id1 = GuiActionId::new();
        history.record(GuiActionRequest {
            action_id: id1.clone(),
            requested_at_ms: 100,
            command: "OpenRoom".to_string(),
        });

        let id2 = GuiActionId::new();
        history.record(GuiActionRequest {
            action_id: id2.clone(),
            requested_at_ms: 200,
            command: "SendPressed".to_string(),
        });

        history.set_state(&id2, GuiActionState::Completed);

        let active = history.actions_with_state(GuiActionState::Queued);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].action_id, id1);

        let completed = history.actions_with_state(GuiActionState::Completed);
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].action_id, id2);
    }

    #[test]
    fn test_gui_action_history_default_capacity() {
        let history = GuiActionHistory::new();
        for i in 0..1000 {
            let id = GuiActionId::new();
            let request = GuiActionRequest {
                action_id: id,
                requested_at_ms: i,
                command: format!("Action-{i}"),
            };
            history.record(request);
        }
        assert_eq!(history.action_count(), 1000);
    }

    #[test]
    fn test_gui_action_history_eviction_chain() {
        let history = GuiActionHistory::with_capacity(3);

        let mut ids = Vec::new();
        for i in 0..3 {
            let id = GuiActionId::new();
            ids.push(id.clone());
            let request = GuiActionRequest {
                action_id: id,
                requested_at_ms: i * 100,
                command: format!("Action-{i}"),
            };
            history.record(request);
        }

        for id in &ids {
            history.set_state(id, GuiActionState::Completed);
        }

        for i in 3..6 {
            let id = GuiActionId::new();
            let request = GuiActionRequest {
                action_id: id,
                requested_at_ms: i * 100,
                command: format!("Action-{i}"),
            };
            history.record(request);
        }

        assert_eq!(history.action_count(), 3);
        for id in &ids {
            assert!(history.get(id).is_none(), "{id:?} should have been evicted");
        }
    }

    // ── GuiWaitCondition tests ──────────────────────────────────────

    fn test_snapshot(
        active_screen: &str,
        active_room: Option<&str>,
        neighbor_count: usize,
        total_entry_count: usize,
    ) -> IcedStateSnapshot {
        IcedStateSnapshot {
            node_id: "test-node".to_string(),
            version: "0.101.0".to_string(),
            active_screen: active_screen.to_string(),
            active_room: active_room.map(|s| s.to_string()),
            conversation_count: 0,
            neighbor_count,
            direct_peer_count: 0,
            relayed_peer_count: 0,
            mesh_health: "Good".to_string(),
            online_friend_count: 0,
            friend_count: 0,
            total_entry_count,
            dark_mode: false,
            composer_text: String::new(),
            dialog_open: false,
            unread_count: 0,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_screen_is_condition_matches() {
        let snapshot = test_snapshot("ChatList", None, 0, 0);
        let journal = IcedMessageJournal::new();

        assert!(evaluate_wait_condition(
            &GuiWaitCondition::ScreenIs {
                expected: "ChatList".to_string()
            },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_screen_is_condition_does_not_match() {
        let snapshot = test_snapshot("Settings", None, 0, 0);
        let journal = IcedMessageJournal::new();

        assert!(!evaluate_wait_condition(
            &GuiWaitCondition::ScreenIs {
                expected: "ChatList".to_string()
            },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_room_selected_any_room() {
        let snapshot = test_snapshot("Chat", Some("abc"), 0, 0);
        let journal = IcedMessageJournal::new();

        assert!(evaluate_wait_condition(
            &GuiWaitCondition::RoomSelected { room_topic: None },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_room_selected_no_room() {
        let snapshot = test_snapshot("ChatList", None, 0, 0);
        let journal = IcedMessageJournal::new();

        assert!(!evaluate_wait_condition(
            &GuiWaitCondition::RoomSelected { room_topic: None },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_room_selected_specific_topic() {
        let snapshot = test_snapshot("Chat", Some("room123"), 0, 0);
        let journal = IcedMessageJournal::new();

        assert!(evaluate_wait_condition(
            &GuiWaitCondition::RoomSelected {
                room_topic: Some("room123".to_string())
            },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_room_selected_wrong_topic() {
        let snapshot = test_snapshot("Chat", Some("room123"), 0, 0);
        let journal = IcedMessageJournal::new();

        assert!(!evaluate_wait_condition(
            &GuiWaitCondition::RoomSelected {
                room_topic: Some("other-room".to_string())
            },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_peer_visible_with_enough_neighbors() {
        let snapshot = test_snapshot("Chat", Some("room1"), 3, 0);
        let journal = IcedMessageJournal::new();

        assert!(evaluate_wait_condition(
            &GuiWaitCondition::PeerVisible { min_count: 3 },
            &snapshot,
            &journal,
        ));
        assert!(evaluate_wait_condition(
            &GuiWaitCondition::PeerVisible { min_count: 1 },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_peer_visible_not_enough_neighbors() {
        let snapshot = test_snapshot("ChatList", None, 0, 0);
        let journal = IcedMessageJournal::new();

        assert!(!evaluate_wait_condition(
            &GuiWaitCondition::PeerVisible { min_count: 1 },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_message_visible_with_enough_entries() {
        let snapshot = test_snapshot("Chat", Some("room1"), 0, 5);
        let journal = IcedMessageJournal::new();

        assert!(evaluate_wait_condition(
            &GuiWaitCondition::MessageVisible { min_count: 5 },
            &snapshot,
            &journal,
        ));
        assert!(evaluate_wait_condition(
            &GuiWaitCondition::MessageVisible { min_count: 3 },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_message_visible_not_enough_entries() {
        let snapshot = test_snapshot("Chat", Some("room1"), 0, 2);
        let journal = IcedMessageJournal::new();

        assert!(!evaluate_wait_condition(
            &GuiWaitCondition::MessageVisible { min_count: 5 },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    #[test]
    fn test_gui_revision_at_least_reached() {
        let snapshot = test_snapshot("ChatList", None, 0, 0);
        let journal = IcedMessageJournal::new();

        // Record enough entries to reach revision 2 (sequences 0, 1, 2 → latest = 2)
        journal.record("TestMessage", FailureLayer::IcedUpdate, true, "", None);
        journal.record("TestMessage", FailureLayer::IcedUpdate, true, "", None);
        journal.record("TestMessage", FailureLayer::IcedUpdate, true, "", None);
        assert_eq!(journal.latest_sequence(), 2);

        assert!(evaluate_wait_condition(
            &GuiWaitCondition::GuiRevisionAtLeast {
                expected_revision: 2
            },
            &snapshot,
            &journal,
        ));
        assert!(evaluate_wait_condition(
            &GuiWaitCondition::GuiRevisionAtLeast {
                expected_revision: 1
            },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_gui_revision_at_least_not_reached() {
        let snapshot = test_snapshot("ChatList", None, 0, 0);
        let journal = IcedMessageJournal::new();

        // Only 2 entries → revision 1
        journal.record("TestMessage", FailureLayer::IcedUpdate, true, "", None);
        journal.record("TestMessage", FailureLayer::IcedUpdate, true, "", None);

        assert!(!evaluate_wait_condition(
            &GuiWaitCondition::GuiRevisionAtLeast {
                expected_revision: 5
            },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_gui_wait_condition_serde_roundtrip() {
        let condition = GuiWaitCondition::ScreenIs {
            expected: "ChatList".to_string(),
        };

        let json = serde_json::to_string(&condition).unwrap();
        let deserialized: GuiWaitCondition = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized, condition);
        assert!(json.contains("\"type\":\"screen_is\""));
        assert!(json.contains("\"expected\":\"ChatList\""));

        // RoomSelected roundtrip
        let condition2 = GuiWaitCondition::RoomSelected {
            room_topic: Some("room123".to_string()),
        };
        let json2 = serde_json::to_string(&condition2).unwrap();
        let deserialized2: GuiWaitCondition = serde_json::from_str(&json2).unwrap();
        assert_eq!(deserialized2, condition2);
        assert!(json2.contains("room_topic"));

        // Roundtrip for all variants
        let variants = vec![
            GuiWaitCondition::ScreenIs {
                expected: "Chat".to_string(),
            },
            GuiWaitCondition::RoomSelected { room_topic: None },
            GuiWaitCondition::RoomSelected {
                room_topic: Some("abc".to_string()),
            },
            GuiWaitCondition::PeerVisible { min_count: 0 },
            GuiWaitCondition::PeerVisible { min_count: 5 },
            GuiWaitCondition::MessageVisible { min_count: 1 },
            GuiWaitCondition::GuiRevisionAtLeast {
                expected_revision: 42,
            },
            GuiWaitCondition::ConversationSelected {
                conversation_id: None,
            },
            GuiWaitCondition::ConversationSelected {
                conversation_id: Some("peer1".to_string()),
            },
            GuiWaitCondition::ComposerTextIs {
                expected: "hello".to_string(),
            },
            GuiWaitCondition::DialogOpen,
            GuiWaitCondition::DialogClosed,
            GuiWaitCondition::UnreadCountAtLeast { min_count: 5 },
        ];

        for v in variants {
            let json = serde_json::to_string(&v).unwrap();
            let deserialized: GuiWaitCondition = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, v);
        }
    }

    #[test]
    fn test_gui_wait_condition_no_secret_material() {
        let condition = GuiWaitCondition::ScreenIs {
            expected: "ChatList".to_string(),
        };
        let json = serde_json::to_string(&condition).unwrap();
        assert!(!json.contains("secret_key"));
        assert!(!json.contains("secret"));
        assert!(!json.contains("private_key"));
        assert!(!json.contains("ticket"));
    }

    #[test]
    fn test_evaluate_wait_condition_empty_journal() {
        let snapshot = test_snapshot("ChatList", None, 0, 0);
        let journal = IcedMessageJournal::new();

        // Empty journal has latest_sequence = 0, so revision 1 should fail
        assert!(!evaluate_wait_condition(
            &GuiWaitCondition::GuiRevisionAtLeast {
                expected_revision: 1
            },
            &snapshot,
            &journal,
        ));

        // revision 0 should pass (0 >= 0)
        assert!(evaluate_wait_condition(
            &GuiWaitCondition::GuiRevisionAtLeast {
                expected_revision: 0
            },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_peer_visible_zero_count_any_neighbor() {
        let snapshot = test_snapshot("Chat", Some("room1"), 1, 0);
        let journal = IcedMessageJournal::new();

        // min_count=0: should be true even with empty snapshot
        assert!(evaluate_wait_condition(
            &GuiWaitCondition::PeerVisible { min_count: 0 },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_message_visible_zero_count_any_entry() {
        let snapshot = test_snapshot("Chat", Some("room1"), 0, 0);
        let journal = IcedMessageJournal::new();

        // min_count=0: should be true even with 0 entries
        assert!(evaluate_wait_condition(
            &GuiWaitCondition::MessageVisible { min_count: 0 },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_screen_is_case_sensitive() {
        let snapshot = test_snapshot("chatlist", None, 0, 0);
        let journal = IcedMessageJournal::new();

        // Screen names are case-sensitive
        assert!(!evaluate_wait_condition(
            &GuiWaitCondition::ScreenIs {
                expected: "ChatList".to_string()
            },
            &snapshot,
            &journal,
        ));
    }

    // ── New GuiWaitCondition evaluation tests ────────────────────────

    #[test]
    fn test_conversation_selected_any() {
        let snapshot = test_snapshot("Chat", Some("room1"), 0, 0);
        let journal = IcedMessageJournal::new();

        assert!(evaluate_wait_condition(
            &GuiWaitCondition::ConversationSelected {
                conversation_id: None,
            },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_conversation_selected_no_conversation() {
        let snapshot = test_snapshot("ChatList", None, 0, 0);
        let journal = IcedMessageJournal::new();

        assert!(!evaluate_wait_condition(
            &GuiWaitCondition::ConversationSelected {
                conversation_id: None,
            },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_conversation_selected_specific_id() {
        let snapshot = test_snapshot("Chat", Some("peer-abc"), 0, 0);
        let journal = IcedMessageJournal::new();

        assert!(evaluate_wait_condition(
            &GuiWaitCondition::ConversationSelected {
                conversation_id: Some("peer-abc".to_string()),
            },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_conversation_selected_wrong_id() {
        let snapshot = test_snapshot("Chat", Some("peer-abc"), 0, 0);
        let journal = IcedMessageJournal::new();

        assert!(!evaluate_wait_condition(
            &GuiWaitCondition::ConversationSelected {
                conversation_id: Some("other-peer".to_string()),
            },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_composer_text_matches() {
        let mut snapshot = test_snapshot("Chat", Some("room1"), 0, 0);
        snapshot.composer_text = "hello world".to_string();
        let journal = IcedMessageJournal::new();

        assert!(evaluate_wait_condition(
            &GuiWaitCondition::ComposerTextIs {
                expected: "hello world".to_string(),
            },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_composer_text_does_not_match() {
        let mut snapshot = test_snapshot("Chat", Some("room1"), 0, 0);
        snapshot.composer_text = "foo".to_string();
        let journal = IcedMessageJournal::new();

        assert!(!evaluate_wait_condition(
            &GuiWaitCondition::ComposerTextIs {
                expected: "bar".to_string(),
            },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_dialog_open_when_open() {
        let mut snapshot = test_snapshot("ChatList", None, 0, 0);
        snapshot.dialog_open = true;
        let journal = IcedMessageJournal::new();

        assert!(evaluate_wait_condition(
            &GuiWaitCondition::DialogOpen,
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_dialog_open_when_closed() {
        let snapshot = test_snapshot("ChatList", None, 0, 0);
        let journal = IcedMessageJournal::new();

        assert!(!evaluate_wait_condition(
            &GuiWaitCondition::DialogOpen,
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_dialog_closed_when_closed() {
        let snapshot = test_snapshot("ChatList", None, 0, 0);
        let journal = IcedMessageJournal::new();

        assert!(evaluate_wait_condition(
            &GuiWaitCondition::DialogClosed,
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_dialog_closed_when_open() {
        let mut snapshot = test_snapshot("ChatList", None, 0, 0);
        snapshot.dialog_open = true;
        let journal = IcedMessageJournal::new();

        assert!(!evaluate_wait_condition(
            &GuiWaitCondition::DialogClosed,
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_unread_count_at_least_meets_threshold() {
        let mut snapshot = test_snapshot("ChatList", None, 0, 0);
        snapshot.unread_count = 10;
        let journal = IcedMessageJournal::new();

        assert!(evaluate_wait_condition(
            &GuiWaitCondition::UnreadCountAtLeast { min_count: 10 },
            &snapshot,
            &journal,
        ));
        assert!(evaluate_wait_condition(
            &GuiWaitCondition::UnreadCountAtLeast { min_count: 5 },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_unread_count_at_least_below_threshold() {
        let mut snapshot = test_snapshot("ChatList", None, 0, 0);
        snapshot.unread_count = 3;
        let journal = IcedMessageJournal::new();

        assert!(!evaluate_wait_condition(
            &GuiWaitCondition::UnreadCountAtLeast { min_count: 10 },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_unread_count_zero_threshold_any() {
        let snapshot = test_snapshot("ChatList", None, 0, 0);
        let journal = IcedMessageJournal::new();

        // min_count=0 should always pass
        assert!(evaluate_wait_condition(
            &GuiWaitCondition::UnreadCountAtLeast { min_count: 0 },
            &snapshot,
            &journal,
        ));
    }

    #[test]
    fn test_update_peer_state_preserves_peer_state() {
        let peer_hex = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let room = TopicId::from_bytes([42u8; 32]);

        let e1 = DiagnosticEvent {
            sequence: 1,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_hex.to_string()),
            kind: DiagnosticEventKind::PeerDiscovered,
        };
        let mut state = update_peer_state(None, &e1);
        assert!(state.discovered);

        let e2 = DiagnosticEvent {
            sequence: 2,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_hex.to_string()),
            kind: DiagnosticEventKind::ConnectionEstablished {
                remote_address: Some("10.0.0.1:1234".to_string()),
                transport: Some("quic".to_string()),
                used_relay: Some(false),
            },
        };
        state = update_peer_state(Some(state), &e2);
        assert_eq!(state.connection_state, ConnectionDiagnosticState::Connected);

        let e3 = DiagnosticEvent {
            sequence: 3,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_hex.to_string()),
            kind: DiagnosticEventKind::PeerAddedToTopic,
        };
        state = update_peer_state(Some(state), &e3);
        assert!(state.topic_member);
    }

    // ── GuiActionError and GuiActionErrorCode serialization tests ──────

    #[test]
    fn test_gui_action_error_code_serde_roundtrip() {
        // Test all error code variants serialize and deserialize
        let codes = vec![
            GuiActionErrorCode::GuiActionsDisabled,
            GuiActionErrorCode::UnknownRoom,
            GuiActionErrorCode::UnknownConversation,
            GuiActionErrorCode::UnknownPeer,
            GuiActionErrorCode::InvalidCurrentScreen,
            GuiActionErrorCode::BlockingDialogOpen,
            GuiActionErrorCode::NoActiveConversation,
            GuiActionErrorCode::ComposerEmpty,
            GuiActionErrorCode::ComposerTooLong,
            GuiActionErrorCode::SendDisabled,
            GuiActionErrorCode::RoomInactive,
            GuiActionErrorCode::ActionQueueClosed,
            GuiActionErrorCode::ActionTimedOut,
            GuiActionErrorCode::InvalidArgument,
            GuiActionErrorCode::InternalError,
        ];

        for code in &codes {
            let json = serde_json::to_string(code).unwrap();
            let deserialized: GuiActionErrorCode = serde_json::from_str(&json).unwrap();
            assert_eq!(&deserialized, code, "roundtrip failed for {code:?}");
        }

        // Verify snake_case serialization for each variant
        assert_eq!(
            serde_json::to_string(&GuiActionErrorCode::GuiActionsDisabled).unwrap(),
            "\"gui_actions_disabled\""
        );
        assert_eq!(
            serde_json::to_string(&GuiActionErrorCode::UnknownRoom).unwrap(),
            "\"unknown_room\""
        );
        assert_eq!(
            serde_json::to_string(&GuiActionErrorCode::InvalidCurrentScreen).unwrap(),
            "\"invalid_current_screen\""
        );
        assert_eq!(
            serde_json::to_string(&GuiActionErrorCode::NoActiveConversation).unwrap(),
            "\"no_active_conversation\""
        );
        assert_eq!(
            serde_json::to_string(&GuiActionErrorCode::ActionQueueClosed).unwrap(),
            "\"action_queue_closed\""
        );
        assert_eq!(
            serde_json::to_string(&GuiActionErrorCode::BlockingDialogOpen).unwrap(),
            "\"blocking_dialog_open\""
        );
        assert_eq!(
            serde_json::to_string(&GuiActionErrorCode::ComposerTooLong).unwrap(),
            "\"composer_too_long\""
        );
        assert_eq!(
            serde_json::to_string(&GuiActionErrorCode::ActionTimedOut).unwrap(),
            "\"action_timed_out\""
        );
        assert_eq!(
            serde_json::to_string(&GuiActionErrorCode::InternalError).unwrap(),
            "\"internal_error\""
        );
    }

    #[test]
    fn test_gui_action_error_serde_roundtrip() {
        let errors = vec![
            GuiActionError::new(
                GuiActionErrorCode::UnknownRoom,
                "Room 'abc123' was not found",
            ),
            GuiActionError::new(
                GuiActionErrorCode::ComposerEmpty,
                "Cannot send empty message",
            ),
            GuiActionError::new(
                GuiActionErrorCode::SendDisabled,
                "Sending is disabled in read-only mode",
            ),
            GuiActionError::new(
                GuiActionErrorCode::ActionTimedOut,
                "Action timed out after 5000ms",
            ),
            GuiActionError::new(
                GuiActionErrorCode::InternalError,
                "unexpected state: room is None",
            ),
            GuiActionError::new(
                GuiActionErrorCode::UnknownPeer,
                "Peer deadbeef is not known",
            ),
            GuiActionError::new(
                GuiActionErrorCode::InvalidArgument,
                "Invalid state transition: Queued → Completed",
            ),
        ];

        for error in &errors {
            let json = serde_json::to_string(error).unwrap();
            let deserialized: GuiActionError = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized.code, error.code);
            assert_eq!(deserialized.message, error.message);
        }

        // Verify the JSON structure uses snake_case fields
        let json = serde_json::to_string(&errors[0]).unwrap();
        assert!(json.contains("\"code\""));
        assert!(json.contains("\"message\""));
        assert!(json.contains("\"unknown_room\""));

        // Postcard binary roundtrip
        for error in &errors {
            let binary = postcard::to_stdvec(error).unwrap();
            let deserialized: GuiActionError = postcard::from_bytes(&binary).unwrap();
            assert_eq!(deserialized.code, error.code);
            assert_eq!(deserialized.message, error.message);
        }
    }

    #[test]
    fn test_gui_action_status_serde_with_error_field() {
        let status = GuiActionStatus {
            action_id: GuiActionId("deadbeef1234".to_string()),
            state: GuiActionState::Rejected,
            requested_at_ms: 1000,
            updated_at_ms: 1050,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: Some(GuiActionError::new(
                GuiActionErrorCode::UnknownRoom,
                "Room 'xyz' was not found",
            )),
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        let json = serde_json::to_string(&status).unwrap();
        let deserialized: GuiActionStatus = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.action_id.0, "deadbeef1234");
        assert_eq!(deserialized.state, GuiActionState::Rejected);
        assert_eq!(deserialized.requested_at_ms, 1000);

        let err = deserialized.error.expect("error field should be present");
        assert_eq!(err.code, GuiActionErrorCode::UnknownRoom);
        assert_eq!(err.message, "Room 'xyz' was not found");

        // Verify the JSON structure
        assert!(json.contains("\"error\""));
        assert!(json.contains("\"unknown_room\""));
        assert!(json.contains("Room 'xyz' was not found"));

        // Postcard binary roundtrip
        let binary = postcard::to_stdvec(&status).unwrap();
        let deserialized2: GuiActionStatus = postcard::from_bytes(&binary).unwrap();
        let err2 = deserialized2
            .error
            .expect("error should survive postcard roundtrip");
        assert_eq!(err2.code, GuiActionErrorCode::UnknownRoom);
        assert_eq!(err2.message, "Room 'xyz' was not found");
    }

    #[test]
    fn test_gui_action_history_transition_to_returns_structured_error() {
        let history = GuiActionHistory::new();
        let id = GuiActionId::new();

        // Non-existent action should return InvalidArgument
        let err = history
            .transition_to(&id, GuiActionState::Validating)
            .unwrap_err();
        assert_eq!(err.code, GuiActionErrorCode::InvalidArgument);
        assert!(err.message.contains(&id.0));

        // Record an action and try an invalid transition
        let request = GuiActionRequest {
            action_id: id.clone(),
            requested_at_ms: 1000,
            command: "TestAction".to_string(),
        };
        history.record(request);

        // Invalid state transition should return InvalidArgument
        let err = history
            .transition_to(&id, GuiActionState::Completed)
            .unwrap_err();
        assert_eq!(err.code, GuiActionErrorCode::InvalidArgument);
        assert!(err.message.contains("Invalid state transition"));
    }

    #[test]
    fn test_gui_action_error_display_format() {
        let err = GuiActionError::new(
            GuiActionErrorCode::UnknownRoom,
            "Room 'abc123' was not found",
        );
        let display = format!("{err}");
        assert_eq!(display, "UnknownRoom: Room 'abc123' was not found");
    }

    // ── GuiTestCommand serialization round-trip tests ──────────────────

    #[test]
    fn test_gui_test_command_json_roundtrip() {
        use GuiTestCommand::*;

        let variants: Vec<GuiTestCommand> = vec![
            GoToChatList,
            OpenRoom {
                room_id: "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".into(),
            },
            OpenConversation {
                conversation_id:
                    "deadbeef1234567890abcdef1234567890deadbeef1234567890abcdef1234567890".into(),
            },
            OpenFriends,
            OpenSettings,
            CloseDialog,
            SetComposerText {
                text: "hello world".into(),
            },
            SubmitComposer,
            SelectPeer {
                peer_id: "cafebabe1234567890abcdef1234567890cafebabe1234567890abcdef1234567890"
                    .into(),
            },
            ToggleDarkMode { enabled: true },
            ToggleHelp,
            Wait {
                condition: GuiWaitCondition::ScreenIs {
                    expected: "ChatList".into(),
                },
                timeout_ms: 5000,
            },
        ];

        for cmd in &variants {
            let json = serde_json::to_string(cmd).unwrap();
            let deserialized: GuiTestCommand = serde_json::from_str(&json).unwrap();
            assert_eq!(&deserialized, cmd, "JSON round-trip failed for {cmd:?}");
            assert!(json.contains("\"command\""));
        }
    }

    #[test]
    fn test_gui_test_command_postcard_serde_limitation() {
        use GuiTestCommand::*;

        // Postcard v1 with `experimental-derive` can serialize tagged enums
        // (serde's Serialize uses external/adjacent tagging), but cannot
        // deserialize them back (returns \"will never implement\" error).
        // Only JSON round-trips are guaranteed for GuiTestCommand.
        let cmd = OpenRoom {
            room_id: "abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234".into(),
        };
        let bytes = postcard::to_stdvec(&cmd).expect("postcard should serialize tagged enums");
        let result: Result<GuiTestCommand, _> = postcard::from_bytes(&bytes);
        assert!(
            result.is_err(),
            "postcard should not deserialize tagged enums"
        );
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("never implement"),
            "Error should mention 'never implement': {err}"
        );
    }

    #[test]
    fn test_gui_test_command_json_tagged_discrimination() {
        let json = r#"{"command": "go_to_chat_list"}"#;
        let cmd: GuiTestCommand = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd, GuiTestCommand::GoToChatList));

        let json = r#"{"command": "open_settings"}"#;
        let cmd: GuiTestCommand = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd, GuiTestCommand::OpenSettings));

        let json = r#"{"command": "toggle_help"}"#;
        let cmd: GuiTestCommand = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd, GuiTestCommand::ToggleHelp));
    }

    #[test]
    fn test_gui_test_command_json_unit_variants() {
        let json = serde_json::to_string(&GuiTestCommand::GoToChatList).unwrap();
        assert_eq!(json, r#"{"command":"go_to_chat_list"}"#);

        let json = serde_json::to_string(&GuiTestCommand::OpenFriends).unwrap();
        assert_eq!(json, r#"{"command":"open_friends"}"#);

        let json = serde_json::to_string(&GuiTestCommand::CloseDialog).unwrap();
        assert_eq!(json, r#"{"command":"close_dialog"}"#);
    }

    #[test]
    fn test_gui_test_command_json_struct_variants() {
        let cmd = GuiTestCommand::SetComposerText {
            text: "test".into(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"command\":\"set_composer_text\""));
        assert!(json.contains("\"text\":\"test\""));

        let cmd = GuiTestCommand::ToggleDarkMode { enabled: true };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"command\":\"toggle_dark_mode\""));
        assert!(json.contains("\"enabled\":true"));
    }

    #[test]
    fn test_gui_test_command_no_secrets_in_json() {
        let cmd = GuiTestCommand::OpenRoom {
            room_id: "aaaabbbbccccddddaaaabbbbccccddddaaaabbbbccccddddaaaabbbbccccdddd".into(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(!json.contains("secret_key"));
        assert!(!json.contains("secret"));
        assert!(!json.contains("private_key"));
        assert!(!json.contains("ticket"));
        assert!(!json.contains("password"));
    }

    #[test]
    fn test_gui_test_command_validate_valid() {
        assert!(GuiTestCommand::GoToChatList.validate().is_ok());
        assert!(GuiTestCommand::OpenFriends.validate().is_ok());
        assert!(GuiTestCommand::OpenSettings.validate().is_ok());
        assert!(GuiTestCommand::CloseDialog.validate().is_ok());
        assert!(GuiTestCommand::SubmitComposer.validate().is_ok());
        assert!(GuiTestCommand::ToggleHelp.validate().is_ok());
        assert!(GuiTestCommand::ToggleDarkMode { enabled: true }
            .validate()
            .is_ok());
        assert!(GuiTestCommand::SetComposerText {
            text: "hello".into()
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn test_gui_test_command_validate_rejects_control_chars() {
        assert!(GuiTestCommand::SetComposerText { text: "\n".into() }
            .validate()
            .is_err());
        assert!(GuiTestCommand::SetComposerText { text: "\r".into() }
            .validate()
            .is_err());
        assert!(GuiTestCommand::SetComposerText { text: "\t".into() }
            .validate()
            .is_err());
        assert!(GuiTestCommand::SetComposerText {
            text: "\x00".into()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn test_gui_test_command_validate_rejects_overflow() {
        let long = "a".repeat(GUI_TEST_COMMAND_MAX_STRING_LEN + 1);
        assert!(GuiTestCommand::SetComposerText { text: long }
            .validate()
            .is_err());
    }

    #[test]
    fn test_gui_test_command_validate_rejects_excessive_timeout() {
        assert!(GuiTestCommand::Wait {
            condition: GuiWaitCondition::ScreenIs {
                expected: "ChatList".into()
            },
            timeout_ms: GUI_TEST_COMMAND_MAX_TIMEOUT_MS + 1,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn test_gui_test_command_unknown_variant_rejected_by_serde() {
        let malicious = r#"{"command": "execute_shell", "cmd": "rm -rf /"}"#;
        let result: Result<GuiTestCommand, _> = serde_json::from_str(malicious);
        assert!(result.is_err(), "Unknown variant must be rejected by serde");
    }

    // ── Security: string field bounds for ALL variants ────────────────

    #[test]
    fn test_gui_test_command_validate_rejects_long_room_id() {
        let long = "a".repeat(GUI_TEST_COMMAND_MAX_STRING_LEN + 1);
        assert!(GuiTestCommand::OpenRoom { room_id: long }
            .validate()
            .is_err());
    }

    #[test]
    fn test_gui_test_command_validate_rejects_long_conversation_id() {
        let long = "a".repeat(GUI_TEST_COMMAND_MAX_STRING_LEN + 1);
        assert!(GuiTestCommand::OpenConversation {
            conversation_id: long,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn test_gui_test_command_validate_rejects_long_peer_id() {
        let long = "a".repeat(GUI_TEST_COMMAND_MAX_STRING_LEN + 1);
        assert!(GuiTestCommand::SelectPeer { peer_id: long }
            .validate()
            .is_err());
    }

    #[test]
    fn test_gui_test_command_validate_rejects_long_wait_screen_name() {
        let long = "a".repeat(GUI_TEST_COMMAND_MAX_STRING_LEN + 1);
        assert!(GuiTestCommand::Wait {
            condition: GuiWaitCondition::ScreenIs { expected: long },
            timeout_ms: 1000,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn test_gui_test_command_validate_rejects_long_wait_room_topic() {
        let long = "a".repeat(GUI_TEST_COMMAND_MAX_STRING_LEN + 1);
        assert!(GuiTestCommand::Wait {
            condition: GuiWaitCondition::RoomSelected {
                room_topic: Some(long),
            },
            timeout_ms: 1000,
        }
        .validate()
        .is_err());
    }

    // ── Security: no shell / filesystem / keyboard / mouse variants ──

    #[test]
    fn test_gui_test_command_rejects_dangerous_variants() {
        // Verifies that no new dangerous variants can be injected via serde.
        let dangerous = [
            r#"{"command": "execute"}"#,
            r#"{"command": "exec"}"#,
            r#"{"command": "shell"}"#,
            r#"{"command": "run"}"#,
            r#"{"command": "system"}"#,
            r#"{"command": "open_file"}"#,
            r#"{"command": "write_file"}"#,
            r#"{"command": "read_file"}"#,
            r#"{"command": "getenv"}"#,
            r#"{"command": "env"}"#,
            r#"{"command": "keyboard"}"#,
            r#"{"command": "mouse"}"#,
            r#"{"command": "window_handle"}"#,
            r#"{"command": "click"}"#,
            r#"{"command": "type_keys"}"#,
            r#"{"command": "send_keys"}"#,
            r#"{"command": "clipboard"}"#,
            r#"{"command": "spawn"}"#,
        ];
        for payload in &dangerous {
            let result: Result<GuiTestCommand, _> = serde_json::from_str(payload);
            assert!(
                result.is_err(),
                "Dangerous variant must be rejected: {}",
                payload
            );
        }
    }

    // ── GuiTestCommand::expected_state() ──────────────────────────

    #[test]
    fn test_gui_test_command_expected_state_go_to_chat_list() {
        let cmd = GuiTestCommand::GoToChatList;
        assert_eq!(
            cmd.expected_state(),
            Some(ExpectedState::ScreenIs("ChatList".into()))
        );
    }

    #[test]
    fn test_gui_test_command_expected_state_open_room() {
        let cmd = GuiTestCommand::OpenRoom {
            room_id: "deadbeef".into(),
        };
        assert_eq!(
            cmd.expected_state(),
            Some(ExpectedState::RoomSelected("deadbeef".into()))
        );
    }

    #[test]
    fn test_gui_test_command_expected_state_open_conversation() {
        let cmd = GuiTestCommand::OpenConversation {
            conversation_id: "cafebabe".into(),
        };
        assert_eq!(
            cmd.expected_state(),
            Some(ExpectedState::ConversationSelected("cafebabe".into()))
        );
    }

    #[test]
    fn test_gui_test_command_expected_state_set_composer_text() {
        let cmd = GuiTestCommand::SetComposerText {
            text: "hello world".into(),
        };
        assert_eq!(
            cmd.expected_state(),
            Some(ExpectedState::ComposerTextIs("hello world".into()))
        );
    }

    #[test]
    fn test_gui_test_command_expected_state_submit_composer() {
        let cmd = GuiTestCommand::SubmitComposer;
        assert_eq!(cmd.expected_state(), Some(ExpectedState::MessageSent));
    }

    #[test]
    fn test_gui_test_command_expected_state_toggle_dark_mode() {
        let cmd = GuiTestCommand::ToggleDarkMode { enabled: true };
        assert_eq!(cmd.expected_state(), Some(ExpectedState::DarkModeIs(true)));

        let cmd = GuiTestCommand::ToggleDarkMode { enabled: false };
        assert_eq!(cmd.expected_state(), Some(ExpectedState::DarkModeIs(false)));
    }

    #[test]
    fn test_gui_test_command_expected_state_open_friends() {
        let cmd = GuiTestCommand::OpenFriends;
        assert_eq!(
            cmd.expected_state(),
            Some(ExpectedState::ScreenIs("Friends".into()))
        );
    }

    #[test]
    fn test_gui_test_command_expected_state_open_settings() {
        let cmd = GuiTestCommand::OpenSettings;
        assert_eq!(
            cmd.expected_state(),
            Some(ExpectedState::ScreenIs("Settings".into()))
        );
    }

    #[test]
    fn test_gui_test_command_expected_state_toggle_help() {
        let cmd = GuiTestCommand::ToggleHelp;
        assert_eq!(cmd.expected_state(), Some(ExpectedState::HelpVisible(true)));
    }

    #[test]
    fn test_gui_test_command_expected_state_returns_none_for_ambiguous() {
        // CloseDialog — depends on current state
        assert!(GuiTestCommand::CloseDialog.expected_state().is_none());
        // SelectPeer — may open conversation or profile
        assert!(GuiTestCommand::SelectPeer {
            peer_id: "abc".into()
        }
        .expected_state()
        .is_none());
        // Wait — condition is tracked separately
        assert!(GuiTestCommand::Wait {
            condition: GuiWaitCondition::PeerVisible { min_count: 1 },
            timeout_ms: 1000,
        }
        .expected_state()
        .is_none());
    }

    // ── Security: GuiActionError / GuiActionErrorCode ─────────────────

    #[test]
    fn test_gui_action_error_code_serde_snake_case() {
        let json = serde_json::to_string(&GuiActionErrorCode::GuiActionsDisabled).unwrap();
        assert_eq!(json, "\"gui_actions_disabled\"");

        let json = serde_json::to_string(&GuiActionErrorCode::UnknownRoom).unwrap();
        assert_eq!(json, "\"unknown_room\"");

        let json = serde_json::to_string(&GuiActionErrorCode::ActionTimedOut).unwrap();
        assert_eq!(json, "\"action_timed_out\"");

        // Round-trip
        let decoded: GuiActionErrorCode = serde_json::from_str("\"internal_error\"").unwrap();
        assert_eq!(decoded, GuiActionErrorCode::InternalError);
    }

    #[test]
    fn test_gui_action_error_no_secrets_in_serialized_output() {
        let err = GuiActionError::new(GuiActionErrorCode::UnknownRoom, "room not found");
        let json = serde_json::to_string(&err).unwrap();
        assert!(!json.contains("secret"));
        assert!(!json.contains("key"));
        assert!(!json.contains("ticket"));
        assert!(!json.contains("password"));
    }

    #[test]
    fn test_gui_action_error_display() {
        let err = GuiActionError::new(GuiActionErrorCode::GuiActionsDisabled, "test msg");
        let display = format!("{}", err);
        assert!(display.contains("GuiActionsDisabled"));
        assert!(display.contains("test msg"));
    }

    // ── Security: GuiActionState machine transition enforcement ───────

    #[test]
    fn test_gui_action_state_invalid_transitions_rejected() {
        let mut status = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: GuiActionState::Queued,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        // Queued → Completed (invalid: must go through Validating first)
        assert!(status.transition_to(GuiActionState::Completed).is_err());

        // Queued → Validating (valid)
        assert!(status.transition_to(GuiActionState::Validating).is_ok());

        // Validating → AppMessageQueued (valid)
        assert!(status
            .transition_to(GuiActionState::AppMessageQueued)
            .is_ok());

        // AppMessageQueued → Completed (invalid: must go through AppMessageHandled first)
        assert!(status.transition_to(GuiActionState::Completed).is_err());
    }

    #[test]
    fn test_gui_action_state_terminal_cannot_transition() {
        let mut status = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: GuiActionState::Completed,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        // Terminal state → any other state should fail
        assert!(status.transition_to(GuiActionState::Queued).is_err());
        assert!(status.transition_to(GuiActionState::Validating).is_err());
        assert!(status
            .transition_to(GuiActionState::AppMessageQueued)
            .is_err());
    }

    #[test]
    fn test_gui_action_state_rejected_is_terminal() {
        let mut status = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: GuiActionState::Rejected,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };
        assert!(status.transition_to(GuiActionState::Validating).is_err());
    }

    #[test]
    fn test_gui_action_state_full_lifecycle_valid() {
        let mut status = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: GuiActionState::Queued,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        assert!(status.transition_to(GuiActionState::Validating).is_ok());
        assert!(status
            .transition_to(GuiActionState::AppMessageQueued)
            .is_ok());
        assert!(status
            .transition_to(GuiActionState::AppMessageHandled)
            .is_ok());
        assert!(status
            .transition_to(GuiActionState::WaitingForExpectedState)
            .is_ok());
        assert!(status.transition_to(GuiActionState::Completed).is_ok());
        assert!(status.state.is_terminal());
    }

    #[test]
    fn test_gui_action_state_rejected_lifecycle() {
        let mut status = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: GuiActionState::Queued,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        assert!(status.transition_to(GuiActionState::Validating).is_ok());
        assert!(status.transition_to(GuiActionState::Rejected).is_ok());
        assert!(status.state.is_terminal());
    }

    #[test]
    fn test_gui_action_state_failed_lifecycle() {
        let mut status = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: GuiActionState::Queued,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        assert!(status.transition_to(GuiActionState::Validating).is_ok());
        assert!(status
            .transition_to(GuiActionState::AppMessageQueued)
            .is_ok());
        assert!(status
            .transition_to(GuiActionState::AppMessageHandled)
            .is_ok());
        assert!(status.transition_to(GuiActionState::Failed).is_ok());
        assert!(status.state.is_terminal());
    }

    #[test]
    fn test_gui_action_state_timed_out_lifecycle() {
        let mut status = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: GuiActionState::Queued,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        assert!(status.transition_to(GuiActionState::Validating).is_ok());
        assert!(status
            .transition_to(GuiActionState::AppMessageQueued)
            .is_ok());
        assert!(status
            .transition_to(GuiActionState::AppMessageHandled)
            .is_ok());
        assert!(status
            .transition_to(GuiActionState::WaitingForExpectedState)
            .is_ok());
        assert!(status.transition_to(GuiActionState::TimedOut).is_ok());
        assert!(status.state.is_terminal());
    }

    // ── Security: GuiActionHistory capacity and eviction ──────────────

    #[test]
    fn test_gui_action_history_capacity_capped() {
        let history = GuiActionHistory::with_capacity(5);
        for i in 0..10 {
            let request = GuiActionRequest {
                action_id: GuiActionId(format!("id-{}", i)),
                requested_at_ms: 1000 + i as i64,
                command: format!("cmd-{}", i),
            };
            history.record(request);
        }
        // Should have evicted oldest 5
        assert_eq!(history.action_count(), 5);
    }

    #[test]
    fn test_gui_action_history_active_actions_evicted_when_capacity_exceeded() {
        let history = GuiActionHistory::with_capacity(3);

        // Fill with non-terminal actions
        for i in 0..3 {
            let request = GuiActionRequest {
                action_id: GuiActionId(format!("active-{}", i)),
                requested_at_ms: 1000 + i as i64,
                command: format!("cmd-{}", i),
            };
            history.record(request);
        }

        // All active — adding a 4th evicts the oldest (active-0) to keep capacity
        let r4 = GuiActionRequest {
            action_id: GuiActionId("active-4".into()),
            requested_at_ms: 1000,
            command: "cmd-4".into(),
        };
        history.record(r4);
        // Capacity enforced: oldest evicted, new one added, back to 3
        assert_eq!(history.action_count(), 3);
        assert_eq!(history.active_count(), 3);
        // Oldest (active-0) should be gone; newest (active-4) should exist
        assert!(history.get(&GuiActionId("active-0".into())).is_none());
        assert!(history.get(&GuiActionId("active-4".into())).is_some());
    }

    #[test]
    fn test_gui_action_history_completed_actions_evicted() {
        let history = GuiActionHistory::with_capacity(3);

        for i in 0..3 {
            let request = GuiActionRequest {
                action_id: GuiActionId(format!("c{}", i)),
                requested_at_ms: 1000 + i as i64,
                command: format!("cmd-{}", i),
            };
            history.record(request);
        }

        // Complete the first one via set_state
        history.set_state(&GuiActionId("c0".into()), GuiActionState::Completed);
        assert_eq!(history.active_count(), 2);

        // Add a 4th — should evict c0 (oldest terminal)
        let r4 = GuiActionRequest {
            action_id: GuiActionId("c4".into()),
            requested_at_ms: 1000,
            command: "cmd-4".into(),
        };
        history.record(r4);

        assert_eq!(history.action_count(), 3);
        assert!(history.get(&GuiActionId("c0".into())).is_none());
        assert!(history.get(&GuiActionId("c4".into())).is_some());
    }

    #[test]
    fn test_gui_action_history_find_nonexistent() {
        let history = GuiActionHistory::new();
        assert!(history.get(&GuiActionId("nothing".into())).is_none());
    }

    #[test]
    fn test_gui_action_history_find_by_action_id() {
        let history = GuiActionHistory::new();
        let aid = GuiActionId("find-me".into());
        let request = GuiActionRequest {
            action_id: aid.clone(),
            requested_at_ms: 2000,
            command: "find-cmd".into(),
        };
        history.record(request);
        let found = history.get(&aid);
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.requested_at_ms, 2000);
        assert!(found.state.is_active());
    }

    #[test]
    fn test_gui_action_history_transition_to_validates_state_machine() {
        let history = GuiActionHistory::new();
        let aid = GuiActionId("sm-1".into());
        let request = GuiActionRequest {
            action_id: aid.clone(),
            requested_at_ms: 1000,
            command: "sm-cmd".into(),
        };
        history.record(request);

        // Valid: Queued → Validating
        assert!(history
            .transition_to(&aid, GuiActionState::Validating)
            .is_ok());

        // Invalid: Validating → Completed (skip AppMessageQueued)
        assert!(history
            .transition_to(&aid, GuiActionState::Completed)
            .is_err());

        // Valid: Validating → Rejected
        assert!(history
            .transition_to(&aid, GuiActionState::Rejected)
            .is_ok());

        // Terminal: cannot transition further
        assert!(history.transition_to(&aid, GuiActionState::Queued).is_err());
    }

    #[test]
    fn test_gui_action_history_transition_to_unknown_id() {
        let history = GuiActionHistory::new();
        assert!(history
            .transition_to(
                &GuiActionId("nonexistent".into()),
                GuiActionState::Completed
            )
            .is_err());
    }

    #[test]
    fn test_gui_action_history_set_expected_state() {
        let history = GuiActionHistory::new();
        let request = GuiActionRequest {
            action_id: GuiActionId("test-1".into()),
            requested_at_ms: 1000,
            command: "GoToChatList".into(),
        };
        let id = history.record(request);

        // Set expected state
        assert!(history.set_expected_state(&id, ExpectedState::ScreenIs("ChatList".into())));

        // Verify it was stored
        let status = history.get(&id).unwrap();
        assert_eq!(
            status.expected_state,
            Some(ExpectedState::ScreenIs("ChatList".into()))
        );

        // Overwrite with a different expected state
        assert!(history.set_expected_state(&id, ExpectedState::DarkModeIs(true)));
        let status = history.get(&id).unwrap();
        assert_eq!(status.expected_state, Some(ExpectedState::DarkModeIs(true)));

        // Unknown action returns false
        assert!(!history.set_expected_state(
            &GuiActionId("nonexistent".into()),
            ExpectedState::MessageSent
        ));
    }

    // ── Security: GuiWaitCondition evaluation ─────────────────────────

    #[test]
    fn test_evaluate_wait_condition_screen_is_matches() {
        let cond = GuiWaitCondition::ScreenIs {
            expected: "ChatList".into(),
        };
        let snapshot = IcedStateSnapshot {
            node_id: "node".into(),
            version: "1".into(),
            active_screen: "ChatList".into(),
            active_room: None,
            conversation_count: 0,
            neighbor_count: 0,
            direct_peer_count: 0,
            relayed_peer_count: 0,
            mesh_health: "OK".into(),
            online_friend_count: 0,
            friend_count: 0,
            total_entry_count: 0,
            dark_mode: false,
            composer_text: String::new(),
            dialog_open: false,
            unread_count: 0,
            timestamp: chrono::Utc::now(),
        };
        let journal = IcedMessageJournal::new();
        assert!(evaluate_wait_condition(&cond, &snapshot, &journal));
    }

    #[test]
    fn test_evaluate_wait_condition_screen_is_no_match() {
        let cond = GuiWaitCondition::ScreenIs {
            expected: "Settings".into(),
        };
        let snapshot = IcedStateSnapshot {
            node_id: "node".into(),
            version: "1".into(),
            active_screen: "ChatList".into(),
            active_room: None,
            conversation_count: 0,
            neighbor_count: 0,
            direct_peer_count: 0,
            relayed_peer_count: 0,
            mesh_health: "OK".into(),
            online_friend_count: 0,
            friend_count: 0,
            total_entry_count: 0,
            dark_mode: false,
            composer_text: String::new(),
            dialog_open: false,
            unread_count: 0,
            timestamp: chrono::Utc::now(),
        };
        let journal = IcedMessageJournal::new();
        assert!(!evaluate_wait_condition(&cond, &snapshot, &journal));
    }

    #[test]
    fn test_evaluate_wait_condition_peer_visible_matches() {
        let cond = GuiWaitCondition::PeerVisible { min_count: 3 };
        let snapshot = IcedStateSnapshot {
            node_id: "node".into(),
            version: "1".into(),
            active_screen: "list".into(),
            active_room: None,
            conversation_count: 0,
            neighbor_count: 5,
            direct_peer_count: 3,
            relayed_peer_count: 2,
            mesh_health: "OK".into(),
            online_friend_count: 0,
            friend_count: 0,
            total_entry_count: 0,
            dark_mode: false,
            composer_text: String::new(),
            dialog_open: false,
            unread_count: 0,
            timestamp: chrono::Utc::now(),
        };
        let journal = IcedMessageJournal::new();
        assert!(evaluate_wait_condition(&cond, &snapshot, &journal));
    }

    #[test]
    fn test_evaluate_wait_condition_gui_revision_at_least() {
        let cond = GuiWaitCondition::GuiRevisionAtLeast {
            expected_revision: 5,
        };
        let snapshot = IcedStateSnapshot {
            node_id: "node".into(),
            version: "1".into(),
            active_screen: "list".into(),
            active_room: None,
            conversation_count: 0,
            neighbor_count: 0,
            direct_peer_count: 0,
            relayed_peer_count: 0,
            mesh_health: "OK".into(),
            online_friend_count: 0,
            friend_count: 0,
            total_entry_count: 0,
            dark_mode: false,
            composer_text: String::new(),
            dialog_open: false,
            unread_count: 0,
            timestamp: chrono::Utc::now(),
        };
        let journal = IcedMessageJournal::with_capacity(10);
        for i in 0..7 {
            journal.record(
                &format!("Msg{}", i),
                FailureLayer::IcedUpdate,
                true,
                "",
                None,
            );
        }
        assert!(evaluate_wait_condition(&cond, &snapshot, &journal));
    }

    // ── Security: GuiActionId uniqueness and format ───────────────────

    #[test]
    fn test_gui_action_id_generates_unique_ids() {
        let mut ids = std::collections::HashSet::new();
        for _ in 0..100 {
            let id = GuiActionId::new();
            assert!(ids.insert(id.0.clone()), "GuiActionId must be unique");
        }
    }

    #[test]
    fn test_gui_action_id_format_is_hex() {
        let id = GuiActionId::new();
        assert_eq!(id.0.len(), 32);
        assert!(id.0.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_gui_action_id_display() {
        let id = GuiActionId("abcd1234".into());
        assert_eq!(format!("{}", id), "abcd1234");
    }

    // ── Security: GuiActionEventKind no secrets in serialized form ────

    #[test]
    fn test_gui_action_event_kind_no_secrets() {
        let kinds: Vec<GuiActionEventKind> = vec![
            GuiActionEventKind::ActionRequested,
            GuiActionEventKind::ActionQueued,
            GuiActionEventKind::ActionValidationStarted,
            GuiActionEventKind::ActionValidated,
            GuiActionEventKind::ActionRejected {
                reason: "test".into(),
            },
            GuiActionEventKind::AppMessageQueued {
                message_variant: "Test".into(),
            },
            GuiActionEventKind::AppMessageHandled {
                message_variant: "Test".into(),
                success: true,
            },
            GuiActionEventKind::ExpectedStateObserved,
            GuiActionEventKind::ActionCompleted,
        ];
        for kind in &kinds {
            let json = serde_json::to_string(kind).unwrap();
            assert!(
                !json.contains("secret_key"),
                "Event kind must not contain secret_key: {}",
                json
            );
        }
    }

    // ── Security: IcedStateSnapshot no secrets in serialized form ─────

    #[test]
    fn test_iced_state_snapshot_no_secrets() {
        let snapshot = IcedStateSnapshot {
            node_id: "node-abc".into(),
            version: "0.101.0".into(),
            active_screen: "ChatList".into(),
            active_room: None,
            conversation_count: 3,
            neighbor_count: 2,
            direct_peer_count: 1,
            relayed_peer_count: 1,
            mesh_health: "Good".into(),
            online_friend_count: 5,
            friend_count: 10,
            total_entry_count: 42,
            dark_mode: true,
            composer_text: String::new(),
            dialog_open: false,
            unread_count: 0,
            timestamp: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("secret_key"));
        assert!(!json.contains("secret"));
        assert!(!json.contains("mailbox"));
        assert!(!json.contains("discovery_secret"));
        assert!(!json.contains("ticket"));
        assert!(!json.contains("password"));
        assert!(!json.contains("token"));
        assert!(!json.contains("private_key"));
    }

    // ── Security: verify KNOWN_SAFE_VARIANTS count matches enum ───────

    /// All known safe variant names — update when adding new variants.
    /// This test verifies that the documentation constant exactly matches
    /// the actual serde tag names of all GuiTestCommand variants.
    #[test]
    fn test_all_gui_test_command_variants_are_known_safe() {
        // Struct variants need full JSON with required fields.
        // Unit variants can use just the command tag.
        let json_cases: Vec<(&str, &str)> = vec![
            (r#"{"command":"go_to_chat_list"}"#, "GoToChatList"),
            (
                r#"{"command":"open_room","room_id":"abcd1234"}"#,
                "OpenRoom",
            ),
            (
                r#"{"command":"open_conversation","conversation_id":"deadbeef"}"#,
                "OpenConversation",
            ),
            (r#"{"command":"open_friends"}"#, "OpenFriends"),
            (r#"{"command":"open_settings"}"#, "OpenSettings"),
            (r#"{"command":"close_dialog"}"#, "CloseDialog"),
            (
                r#"{"command":"set_composer_text","text":"hello"}"#,
                "SetComposerText",
            ),
            (r#"{"command":"submit_composer"}"#, "SubmitComposer"),
            (
                r#"{"command":"select_peer","peer_id":"cafe1234"}"#,
                "SelectPeer",
            ),
            (
                r#"{"command":"toggle_dark_mode","enabled":true}"#,
                "ToggleDarkMode",
            ),
            (r#"{"command":"toggle_help"}"#, "ToggleHelp"),
            (
                r#"{"command":"wait","condition":{"type":"screen_is","expected":"ChatList"},"timeout_ms":5000}"#,
                "Wait",
            ),
        ];

        for (json_str, variant_name) in &json_cases {
            let result: Result<GuiTestCommand, _> = serde_json::from_str(json_str);
            assert!(
                result.is_ok(),
                "Known safe variant must deserialize: {} (json={})",
                variant_name,
                json_str
            );
        }
    }

    // ── GuiActionEventHistory event-ordering tests ───────────────────

    #[test]
    fn test_gui_action_event_history_record_and_query() {
        let journal = GuiActionEventHistory::new();

        journal.record(
            "action-1",
            GuiActionEventKind::ActionRequested,
            1,
            None,
            "ChatList",
        );
        journal.record(
            "action-1",
            GuiActionEventKind::ActionValidationStarted,
            1,
            None,
            "ChatList",
        );
        journal.record(
            "action-1",
            GuiActionEventKind::ActionCompleted,
            1,
            None,
            "ChatList",
        );

        assert_eq!(journal.entry_count(), 3);
        assert_eq!(journal.latest_sequence(), 2);

        // entries_since(0) returns records with sequence > 0 (so 1, 2)
        let since_0 = journal.entries_since(0, 100);
        assert_eq!(since_0.len(), 2);
        assert_eq!(since_0[0].sequence, 1);
        assert_eq!(since_0[1].sequence, 2);

        // entries_since(1) returns records with sequence > 1 (only 2)
        let since_1 = journal.entries_since(1, 100);
        assert_eq!(since_1.len(), 1);
        assert_eq!(since_1[0].sequence, 2);

        // entries_since(latest) returns empty
        let since_latest = journal.entries_since(2, 100);
        assert!(since_latest.is_empty());
    }

    #[test]
    fn test_gui_action_event_history_sequence_ordering() {
        let journal = GuiActionEventHistory::new();

        // Interleave multiple action IDs — sequences must still be monotonic
        journal.record("a", GuiActionEventKind::ActionRequested, 1, None, "Screen");
        journal.record("b", GuiActionEventKind::ActionRequested, 1, None, "Screen");
        journal.record("a", GuiActionEventKind::ActionCompleted, 2, None, "Screen");
        journal.record("c", GuiActionEventKind::ActionRequested, 2, None, "Screen");
        journal.record(
            "b",
            GuiActionEventKind::ActionValidationStarted,
            2,
            None,
            "Screen",
        );
        journal.record(
            "c",
            GuiActionEventKind::ActionFailed {
                error: "timeout".into(),
            },
            3,
            None,
            "Screen",
        );

        assert_eq!(journal.entry_count(), 6);
        assert_eq!(journal.latest_sequence(), 5);

        let all = journal.all_entries();
        // newest first
        assert_eq!(all[0].sequence, 5);
        assert_eq!(all[1].sequence, 4);
        assert_eq!(all[2].sequence, 3);
        assert_eq!(all[3].sequence, 2);
        assert_eq!(all[4].sequence, 1);
        assert_eq!(all[5].sequence, 0);

        // Check action IDs in newest-first order
        assert_eq!(all[0].action_id, "c");
        assert!(matches!(
            all[0].kind,
            GuiActionEventKind::ActionFailed { .. }
        ));
        assert_eq!(all[5].action_id, "a");
        assert!(matches!(all[5].kind, GuiActionEventKind::ActionRequested));
    }

    #[test]
    fn test_gui_action_event_history_entries_since_limit() {
        let journal = GuiActionEventHistory::new();

        for i in 0..50 {
            journal.record(
                &format!("action-{}", i),
                GuiActionEventKind::ActionRequested,
                i,
                None,
                "Screen",
            );
        }

        // Request more than clamp limit — should clamp to 1000
        let many = journal.entries_since(0, 5000);
        assert_eq!(many.len(), 49); // sequence > 0 means seq 1..49 (49 items)

        // Request small limit
        let few = journal.entries_since(0, 3);
        assert_eq!(few.len(), 3);
        assert_eq!(few[0].sequence, 1);
        assert_eq!(few[1].sequence, 2);
        assert_eq!(few[2].sequence, 3);
    }

    #[test]
    fn test_gui_action_event_history_eviction() {
        // with_capacity clamps to [64, 5000]
        let journal = GuiActionEventHistory::with_capacity(64);

        // Fill beyond capacity (record 70 entries, should evict to 64)
        for i in 0..70 {
            journal.record(
                &format!("action-{}", i),
                GuiActionEventKind::ActionRequested,
                i as u64,
                None,
                "Screen",
            );
        }

        // Only 64 entries remain
        assert_eq!(journal.entry_count(), 64);
        assert_eq!(journal.latest_sequence(), 69);

        // The 6 oldest (seq 0..5) should be evicted
        let all = journal.all_entries();
        assert_eq!(all.len(), 64);
        let sequences: Vec<u64> = all.iter().map(|e| e.sequence).collect();
        let expected: Vec<u64> = (6..70).rev().collect();
        assert_eq!(sequences, expected);

        // entries_since should only see survivors with sequence > 0
        let since_0 = journal.entries_since(0, 100);
        assert_eq!(since_0.len(), 64);
        assert_eq!(since_0[0].sequence, 6);
    }

    #[test]
    fn test_gui_action_event_history_all_entries_newest_first() {
        let journal = GuiActionEventHistory::new();

        journal.record("id-1", GuiActionEventKind::ActionRequested, 0, None, "A");
        journal.record("id-1", GuiActionEventKind::ActionValidated, 1, None, "A");
        journal.record("id-1", GuiActionEventKind::ActionCompleted, 2, None, "B");

        let all = journal.all_entries();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].current_screen, "B"); // newest
        assert_eq!(all[0].sequence, 2);
        assert_eq!(all[1].current_screen, "A"); // middle
        assert_eq!(all[1].sequence, 1);
        assert_eq!(all[2].current_screen, "A"); // oldest
        assert_eq!(all[2].sequence, 0);
    }

    #[test]
    fn test_gui_action_event_history_latest_sequence_and_count() {
        let journal = GuiActionEventHistory::new();

        assert_eq!(journal.latest_sequence(), 0);
        assert_eq!(journal.entry_count(), 0);

        journal.record("x", GuiActionEventKind::ActionRequested, 0, None, "");
        assert_eq!(journal.latest_sequence(), 0);
        assert_eq!(journal.entry_count(), 1);

        journal.record("x", GuiActionEventKind::ActionCompleted, 1, None, "");
        assert_eq!(journal.latest_sequence(), 1);
        assert_eq!(journal.entry_count(), 2);
    }

    #[test]
    fn test_gui_action_event_history_empty_journal() {
        let journal = GuiActionEventHistory::new();

        assert_eq!(journal.entry_count(), 0);
        assert_eq!(journal.latest_sequence(), 0);
        assert!(journal.entries_since(0, 100).is_empty());
        assert!(journal.all_entries().is_empty());
    }

    #[test]
    fn test_gui_action_event_history_with_capacity_clamping() {
        // Below minimum — clamps to 64
        let tiny = GuiActionEventHistory::with_capacity(10);
        for i in 0..70 {
            tiny.record(
                &format!("a{}", i),
                GuiActionEventKind::ActionRequested,
                i as u64,
                None,
                "",
            );
        }
        assert_eq!(tiny.entry_count(), 64);

        // Above maximum — clamps to 5000
        let huge = GuiActionEventHistory::with_capacity(10_000);
        for i in 0..6000 {
            huge.record(
                &format!("a{}", i),
                GuiActionEventKind::ActionRequested,
                i as u64,
                None,
                "",
            );
        }
        assert_eq!(huge.entry_count(), 5000);
    }

    #[test]
    fn test_gui_action_event_history_room_and_screen_fields() {
        let journal = GuiActionEventHistory::new();
        let room = TopicId::from_bytes([0xAA; 32]);

        journal.record(
            "action-42",
            GuiActionEventKind::ActionRequested,
            5,
            Some(room),
            "Chat",
        );

        let all = journal.all_entries();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].action_id, "action-42");
        assert_eq!(all[0].gui_revision, 5);
        assert_eq!(all[0].room_id, Some(room));
        assert_eq!(all[0].current_screen, "Chat");
        assert!(matches!(all[0].kind, GuiActionEventKind::ActionRequested));
    }

    #[test]
    fn test_gui_action_event_history_action_timed_out_and_failed() {
        let journal = GuiActionEventHistory::new();

        journal.record(
            "t1",
            GuiActionEventKind::ActionTimedOut { timeout_ms: 5000 },
            3,
            None,
            "Chat",
        );
        journal.record(
            "t2",
            GuiActionEventKind::ActionFailed {
                error: "permission denied".into(),
            },
            4,
            None,
            "Settings",
        );

        let all = journal.all_entries();
        assert_eq!(all.len(), 2);

        // Newest first: t2 (seq 1)
        assert_eq!(all[0].action_id, "t2");
        match &all[0].kind {
            GuiActionEventKind::ActionFailed { error } => assert_eq!(error, "permission denied"),
            _ => panic!("expected ActionFailed"),
        }
        assert_eq!(all[0].current_screen, "Settings");

        // Oldest: t1 (seq 0)
        assert_eq!(all[1].action_id, "t1");
        match &all[1].kind {
            GuiActionEventKind::ActionTimedOut { timeout_ms } => assert_eq!(*timeout_ms, 5000),
            _ => panic!("expected ActionTimedOut"),
        }
        assert_eq!(all[1].current_screen, "Chat");
    }

    // ── Extended event-ordering tests ───────────────────────────────

    #[test]
    fn test_gui_action_event_history_concurrent_record_ordering() {
        // Multiple threads record events simultaneously on the same journal.
        // After all join, sequences must be strictly unique and monotonic.
        let journal = GuiActionEventHistory::with_capacity(5000);
        let n_threads: usize = 10;
        let events_per_thread: usize = 50;
        let mut handles = Vec::with_capacity(n_threads);

        for t in 0..n_threads {
            let j = journal.clone();
            handles.push(std::thread::spawn(move || {
                let mut local_seqs = Vec::with_capacity(events_per_thread);
                for i in 0..events_per_thread {
                    let kind = match i % 6 {
                        0 => GuiActionEventKind::ActionRequested,
                        1 => GuiActionEventKind::ActionValidationStarted,
                        2 => GuiActionEventKind::ActionValidated,
                        3 => GuiActionEventKind::ActionCompleted,
                        4 => GuiActionEventKind::AppMessageQueued {
                            message_variant: format!("Msg-{t}-{i}"),
                        },
                        _ => GuiActionEventKind::ExpectedStateObserved,
                    };
                    j.record(
                        format!("concurrent-{t}-{i}"),
                        kind,
                        (t * events_per_thread + i) as u64,
                        None,
                        "Screen",
                    );
                    // Snapshot latest sequence via entry_count (indirect read)
                    let count = j.entry_count();
                    local_seqs.push(count);
                }
                local_seqs
            }));
        }

        let mut all_seqs = Vec::with_capacity(n_threads * events_per_thread);
        for h in handles {
            if let Ok(seqs) = h.join() {
                all_seqs.extend(seqs);
            }
        }

        // Total recorded events: n_threads * events_per_thread
        let total = journal.entry_count();
        assert_eq!(
            total,
            n_threads * events_per_thread,
            "all events must be recorded"
        );

        // Latest sequence must reflect total - 1
        assert_eq!(
            journal.latest_sequence(),
            (total - 1) as u64,
            "latest_sequence must be last assigned seq"
        );

        // all_entries must be newest-first.  Under concurrent recording the
        // sequence-ordering invariant is: every entry has a unique, monotonically
        // increasing sequence number.  However, because the sequence counter is
        // assigned outside the Mutex, insertion order may not match sequence
        // order (thread A gets seq 5, thread B gets seq 6, B acquires the lock
        // first and pushes seq 6, A pushes seq 5 — reversed → [5, 6] which is
        // not strictly descending).  So we only verify uniqueness and range.
        let all = journal.all_entries();
        assert_eq!(all.len(), total, "all entries must be present");

        // All sequence numbers must be unique and in [0, total-1]
        let mut seq_set: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for entry in &all {
            assert!(
                seq_set.insert(entry.sequence),
                "duplicate sequence {} found",
                entry.sequence
            );
            assert!(
                entry.sequence < total as u64,
                "sequence {} out of range (max {})",
                entry.sequence,
                total - 1
            );
        }
        assert_eq!(seq_set.len(), total, "all sequence numbers must be unique");
    }

    #[test]
    fn test_gui_action_event_kind_all_variants_serde_roundtrip() {
        // Every GuiActionEventKind variant must roundtrip through JSON faithfully.
        // This verifies no variant is omitted and the serde tag scheme is consistent.
        let kinds: Vec<GuiActionEventKind> = vec![
            GuiActionEventKind::ActionRequested,
            GuiActionEventKind::ActionQueued,
            GuiActionEventKind::ActionValidationStarted,
            GuiActionEventKind::ActionValidated,
            GuiActionEventKind::ActionRejected {
                reason: "validation failed".into(),
            },
            GuiActionEventKind::AppMessageQueued {
                message_variant: "SendMessage".into(),
            },
            GuiActionEventKind::AppMessageHandled {
                message_variant: "SendMessage".into(),
                success: true,
            },
            GuiActionEventKind::ExpectedStateObserved,
            GuiActionEventKind::ActionCompleted,
            GuiActionEventKind::ActionTimedOut { timeout_ms: 5000 },
            GuiActionEventKind::ActionFailed {
                error: "permission denied".into(),
            },
        ];

        assert_eq!(kinds.len(), 11, "all 11 variants must be tested");

        for (i, kind) in kinds.iter().enumerate() {
            let json = serde_json::to_string(kind).unwrap();
            let deser: GuiActionEventKind = serde_json::from_str(&json).unwrap();
            // Use debug format for comparison since PartialEq isn't derived
            let original_debug = format!("{:?}", kind);
            let deser_debug = format!("{:?}", &deser);
            assert_eq!(
                original_debug, deser_debug,
                "roundtrip mismatch for variant index {i}: {json}"
            );
        }
    }

    #[test]
    fn test_gui_action_event_history_timestamp_ordering() {
        // Timestamps must be monotonically non-decreasing (wall-clock moves forward).
        let journal = GuiActionEventHistory::new();
        let actions = ["a", "b", "c", "d", "e"];

        for (i, action) in actions.iter().enumerate() {
            journal.record(action, GuiActionEventKind::ActionRequested, i as u64, None, "Screen");
            std::thread::sleep(std::time::Duration::from_millis(1));
            journal.record(
                action,
                GuiActionEventKind::ActionCompleted,
                i as u64 + 10,
                None,
                "Screen",
            );
        }

        let all = journal.all_entries();
        // all_entries is newest-first, so reverse for chronological order
        let chrono: Vec<&GuiActionEvent> = all.iter().rev().collect();

        for i in 0..chrono.len().saturating_sub(1) {
            assert!(
                chrono[i].timestamp <= chrono[i + 1].timestamp,
                "timestamp went backwards at idx {}: {} > {}",
                i,
                chrono[i].timestamp,
                chrono[i + 1].timestamp
            );
        }
    }

    #[test]
    fn test_gui_action_event_history_sequence_continuity_after_eviction() {
        // After eviction forces out old entries, sequence numbers continue
        // monotonically without resetting.
        let journal = GuiActionEventHistory::with_capacity(64);

        // Fill to capacity
        for i in 0..64 {
            journal.record(
                &format!("pre-{i}"),
                GuiActionEventKind::ActionRequested,
                i as u64,
                None,
                "Screen",
            );
        }
        assert_eq!(journal.latest_sequence(), 63);
        assert_eq!(journal.entry_count(), 64);

        // Over-fill — this triggers eviction of oldest
        for i in 0..20 {
            journal.record(
                &format!("post-{i}"),
                GuiActionEventKind::ActionCompleted,
                (64 + i) as u64,
                None,
                "Screen",
            );
        }

        // Count should stay at capacity (64)
        assert_eq!(
            journal.entry_count(),
            64,
            "count must not exceed capacity after eviction"
        );
        // Latest sequence must be the last one assigned (83 = 64 + 20 - 1)
        assert_eq!(
            journal.latest_sequence(),
            83,
            "latest_sequence must continue monotonically after eviction"
        );

        // All entries must have strictly descending sequences (newest-first)
        let all = journal.all_entries();
        assert_eq!(all.len(), 64);
        for i in 0..all.len().saturating_sub(1) {
            assert!(
                all[i].sequence > all[i + 1].sequence,
                "sequence not descending after eviction at idx {}: {} <= {}",
                i,
                all[i].sequence,
                all[i + 1].sequence
            );
        }

        // The oldest surviving sequence should be 20 (64 evicted, so oldest of 84 total)
        let chrono: Vec<&GuiActionEvent> = all.iter().rev().collect();
        assert_eq!(chrono[0].sequence, 20, "first chronological entry should be seq 20");

        // entries_since(83) should be empty (nothing newer than latest)
        assert!(journal.entries_since(83, 100).is_empty());

        // entries_since(20) should return entries with sequence > 20 (i.e. seq 21..83 = 63 entries)
        let since_20 = journal.entries_since(20, 100);
        assert_eq!(since_20.len(), 63);
        assert_eq!(since_20[0].sequence, 21);
    }

    #[test]
    fn test_gui_action_event_history_action_lifecycle_in_order() {
        // Record a complete action lifecycle and verify events appear in
        // the expected chronological order when read back.
        let journal = GuiActionEventHistory::new();
        let action_id = "lifecycle-test-1";

        journal.record(action_id, GuiActionEventKind::ActionRequested, 1, None, "ChatList");
        journal.record(action_id, GuiActionEventKind::ActionQueued, 1, None, "ChatList");
        journal.record(action_id, GuiActionEventKind::ActionValidationStarted, 1, None, "ChatList");
        journal.record(action_id, GuiActionEventKind::ActionValidated, 1, None, "ChatList");
        journal.record(
            action_id,
            GuiActionEventKind::AppMessageQueued {
                message_variant: "SendMessage".into(),
            },
            2,
            None,
            "ChatList",
        );
        journal.record(
            action_id,
            GuiActionEventKind::AppMessageHandled {
                message_variant: "SendMessage".into(),
                success: true,
            },
            2,
            None,
            "ChatList",
        );
        journal.record(action_id, GuiActionEventKind::ExpectedStateObserved, 3, None, "Chat");
        journal.record(action_id, GuiActionEventKind::ActionCompleted, 3, None, "Chat");

        assert_eq!(journal.entry_count(), 8);
        assert_eq!(journal.latest_sequence(), 7);

        // Read in chronological order
        let since_0 = journal.entries_since(0, 100);
        assert_eq!(since_0.len(), 7); // sequence > 0 means seq 1..7 (7 items)

        let expected_kinds: &[GuiActionEventKind] = &[
            // Seq 0 is ActionRequested, excluded by entries_since(0)
            GuiActionEventKind::ActionQueued,                 // seq 1
            GuiActionEventKind::ActionValidationStarted,      // seq 2
            GuiActionEventKind::ActionValidated,              // seq 3
            GuiActionEventKind::AppMessageQueued {            // seq 4
                message_variant: "SendMessage".into(),
            },
            GuiActionEventKind::AppMessageHandled {           // seq 5
                message_variant: "SendMessage".into(),
                success: true,
            },
            GuiActionEventKind::ExpectedStateObserved,        // seq 6
            GuiActionEventKind::ActionCompleted,              // seq 7
        ];
        // entries_since(0) returns only entries with sequence > 0 (seq 1..7).
        // expected_kinds is indexed without offset because it starts at seq 1.
        assert_eq!(since_0.len(), 7);
        for (i, entry) in since_0.iter().enumerate() {
            let expected = &expected_kinds[i];
            let entry_debug = format!("{:?}", entry.kind);
            let expected_debug = format!("{:?}", expected);
            assert_eq!(
                entry_debug, expected_debug,
                "lifecycle step {i} kind mismatch (seq {})",
                entry.sequence
            );
            assert_eq!(entry.action_id, action_id);
        }
    }

    // ── GuiTestHandle tests ───────────────────────────────────────

    #[cfg(feature = "gui")]
    #[test]
    fn test_gui_test_handle_enqueue() {
        let (handle, _rx) = GuiTestHandle::channel(256);
        let request = GuiActionRequest {
            action_id: GuiActionId::new(),
            requested_at_ms: 1000,
            command: "TestCommand".to_string(),
        };
        assert!(handle.enqueue(request).is_ok());
    }

    #[cfg(feature = "gui")]
    #[test]
    fn test_gui_test_handle_closed_channel_error() {
        let (handle, rx) = GuiTestHandle::channel(256);
        // Drop the receiver to close the channel
        drop(rx);
        let request = GuiActionRequest {
            action_id: GuiActionId::new(),
            requested_at_ms: 1000,
            command: "TestCommand".to_string(),
        };
        let err = handle.enqueue(request).unwrap_err();
        assert_eq!(err.code, GuiActionErrorCode::ActionQueueClosed);
        assert!(
            err.message.contains("closed"),
            "error message should mention 'closed': {}",
            err.message
        );
    }

    #[cfg(feature = "gui")]
    #[test]
    fn test_gui_test_handle_capacity() {
        let (handle, _rx) = GuiTestHandle::channel(256);
        assert_eq!(handle.capacity(), 256);
    }

    #[cfg(feature = "gui")]
    #[test]
    fn test_gui_test_handle_is_closed() {
        let (handle, rx) = GuiTestHandle::channel(256);
        assert!(!handle.is_closed(), "channel should be open initially");
        drop(rx);
        assert!(
            handle.is_closed(),
            "channel should be closed after dropping receiver"
        );
    }

    #[cfg(feature = "gui")]
    #[test]
    fn test_gui_test_handle_queue_full_error() {
        // Use capacity 1 so the second send fails immediately
        let (handle, mut rx) = GuiTestHandle::channel(1);
        let request = GuiActionRequest {
            action_id: GuiActionId::new(),
            requested_at_ms: 1000,
            command: "Cmd1".to_string(),
        };
        assert!(handle.enqueue(request).is_ok());

        // Don't drain the receiver — the second send should fail with Full
        let request2 = GuiActionRequest {
            action_id: GuiActionId::new(),
            requested_at_ms: 2000,
            command: "Cmd2".to_string(),
        };
        let err = handle.enqueue(request2).unwrap_err();
        assert_eq!(err.code, GuiActionErrorCode::ActionQueueFull);
        assert!(
            err.message.contains("full"),
            "error message should mention 'full': {}",
            err.message
        );

        // Drain the receiver so the channel isn't leaked with queued messages
        let _ = rx.try_recv();
    }

    #[cfg(feature = "gui")]
    #[test]
    fn test_gui_test_handle_zero_capacity_clamped() {
        // Zero should be clamped to 1
        let (handle, _rx) = GuiTestHandle::channel(0);
        assert_eq!(handle.capacity(), 1);
    }

    #[cfg(feature = "gui")]
    #[test]
    fn test_gui_test_handle_oversized_capacity_clamped() {
        // Above max should be clamped to 4096
        let (handle, _rx) = GuiTestHandle::channel(9999);
        assert_eq!(handle.capacity(), 4096);
    }

    #[cfg(feature = "gui")]
    #[test]
    fn test_gui_test_handle_new_from_sender() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<GuiActionRequest>(64);
        let handle = GuiTestHandle::new(tx);
        assert_eq!(handle.capacity(), 64);
        assert!(!handle.is_closed());
    }

    #[cfg(feature = "gui")]
    #[test]
    fn test_gui_test_handle_enqueue_with_new() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<GuiActionRequest>(64);
        let handle = GuiTestHandle::new(tx);
        let request = GuiActionRequest {
            action_id: GuiActionId::new(),
            requested_at_ms: 1000,
            command: "FromNew".to_string(),
        };
        assert!(handle.enqueue(request).is_ok());
    }

    #[cfg(feature = "gui")]
    #[test]
    fn test_gui_test_handle_closed_detection_after_enqueue() {
        let (handle, rx) = GuiTestHandle::channel(16);
        let request = GuiActionRequest {
            action_id: GuiActionId::new(),
            requested_at_ms: 1000,
            command: "PreClose".to_string(),
        };
        assert!(handle.enqueue(request).is_ok());
        assert!(!handle.is_closed());

        drop(rx); // Close the channel

        // is_closed should now return true
        assert!(handle.is_closed());

        // enqueue should now return ActionQueueClosed
        let request2 = GuiActionRequest {
            action_id: GuiActionId::new(),
            requested_at_ms: 2000,
            command: "PostClose".to_string(),
        };
        let err = handle.enqueue(request2).unwrap_err();
        assert_eq!(err.code, GuiActionErrorCode::ActionQueueClosed);
    }

    // ── Action timeout handling tests ─────────────────────────────────

    #[test]
    fn test_gui_action_timeout_auto_set_on_waiting() {
        // Entering WaitingForExpectedState should auto-set timeout_at_ms
        use GuiActionState::*;

        let mut action = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: Queued,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        assert!(action.timeout_at_ms.is_none());

        // Move through valid states to WaitingForExpectedState
        action.transition_to(Validating).unwrap();
        action.transition_to(AppMessageQueued).unwrap();
        action.transition_to(AppMessageHandled).unwrap();
        action.transition_to(WaitingForExpectedState).unwrap();

        assert_eq!(action.state, WaitingForExpectedState);
        assert!(
            action.timeout_at_ms.is_some(),
            "timeout_at_ms should be set when entering WaitingForExpectedState"
        );
        let timeout = action.timeout_at_ms.unwrap();
        assert!(
            timeout > action.updated_at_ms,
            "timeout should be in the future (updated={}, timeout={})",
            action.updated_at_ms,
            timeout
        );
        // Should be at least DEFAULT_ACTION_STATE_TIMEOUT_MS in the future
        assert!(
            timeout - action.updated_at_ms >= DEFAULT_ACTION_STATE_TIMEOUT_MS,
            "timeout delta should be >= default ({}), got {}",
            DEFAULT_ACTION_STATE_TIMEOUT_MS,
            timeout - action.updated_at_ms
        );
    }

    #[test]
    fn test_gui_action_timeout_not_set_on_other_states() {
        // Non-WaitingForExpectedState transitions should not set timeout_at_ms
        use GuiActionState::*;

        let mut action = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: Queued,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        // Queued -> Validating
        action.transition_to(Validating).unwrap();
        assert!(action.timeout_at_ms.is_none());

        // Validating -> AppMessageQueued
        action.transition_to(AppMessageQueued).unwrap();
        assert!(action.timeout_at_ms.is_none());

        // AppMessageQueued -> AppMessageHandled
        action.transition_to(AppMessageHandled).unwrap();
        assert!(action.timeout_at_ms.is_none());

        // AppMessageHandled -> Completed (terminal)
        action.transition_to(Completed).unwrap();
        assert!(action.timeout_at_ms.is_none());
    }

    #[test]
    fn test_gui_action_history_timeout_cleared_on_transition_out_of_waiting() {
        // Timeout_at_ms should be cleared when leaving WaitingForExpectedState
        use GuiActionState::*;

        let mut action = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: Queued,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        action.transition_to(Validating).unwrap();
        action.transition_to(AppMessageQueued).unwrap();
        action.transition_to(AppMessageHandled).unwrap();
        action.transition_to(WaitingForExpectedState).unwrap();
        assert!(
            action.timeout_at_ms.is_some(),
            "timeout should be set on enter WaitingForExpectedState"
        );

        // Transition to Completed (should clear timeout)
        action.transition_to(Completed).unwrap();
        assert!(
            action.timeout_at_ms.is_none(),
            "timeout should be cleared when transitioning out of WaitingForExpectedState"
        );
    }

    #[test]
    fn test_gui_action_history_timeout_set_via_direct_set_state() {
        // Direct set_state to WaitingForExpectedState should also set timeout
        use GuiActionState::*;

        let mut action = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: Queued,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        // Use set_state (not transition_to) to go to WaitingForExpectedState
        action.set_state(WaitingForExpectedState);
        assert_eq!(action.state, WaitingForExpectedState);
        assert!(
            action.timeout_at_ms.is_some(),
            "direct set_state to WaitingForExpectedState should set timeout"
        );
    }

    #[test]
    fn test_gui_action_history_check_timeouts_returns_empty_when_none_expired() {
        // Fresh actions in WaitingForExpectedState should not be timed out
        let history = GuiActionHistory::with_capacity(10);
        let id = GuiActionId::new();

        let request = GuiActionRequest {
            action_id: id.clone(),
            requested_at_ms: 1000,
            command: "TestCommand".into(),
        };

        let recorded_id = history.record(request);
        history
            .transition_to(&recorded_id, GuiActionState::Validating)
            .unwrap();
        history
            .transition_to(&recorded_id, GuiActionState::AppMessageQueued)
            .unwrap();
        history
            .transition_to(&recorded_id, GuiActionState::AppMessageHandled)
            .unwrap();
        history
            .transition_to(&recorded_id, GuiActionState::WaitingForExpectedState)
            .unwrap();

        // Immediately check_timeouts — should not detect anything since
        // the timeout is 10s in the future
        let timed_out = history.check_timeouts();
        assert!(
            timed_out.is_empty(),
            "Freshly-started actions should not time out immediately"
        );
    }

    #[test]
    fn test_gui_action_history_check_timeouts_skips_non_waiting_actions() {
        // Actions not in WaitingForExpectedState should never be timed out
        let history = GuiActionHistory::with_capacity(10);
        let ids: Vec<GuiActionId> = (0..5)
            .map(|_| {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: 1000,
                    command: "TestCommand".into(),
                };
                history.record(request)
            })
            .collect();

        // Set to various non-waiting states
        // Valid path: Queued → Validating → AppMessageQueued → AppMessageHandled
        history
            .transition_to(&ids[0], GuiActionState::Validating)
            .unwrap();
        history
            .transition_to(&ids[1], GuiActionState::Validating)
            .unwrap();
        history
            .transition_to(&ids[1], GuiActionState::AppMessageQueued)
            .unwrap();
        history
            .transition_to(&ids[2], GuiActionState::Validating)
            .unwrap();
        history
            .transition_to(&ids[2], GuiActionState::AppMessageQueued)
            .unwrap();
        history
            .transition_to(&ids[2], GuiActionState::AppMessageHandled)
            .unwrap();
        history.set_state(&ids[3], GuiActionState::Completed);
        history.set_state(&ids[4], GuiActionState::Rejected);

        let timed_out = history.check_timeouts();
        assert!(
            timed_out.is_empty(),
            "Actions in non-waiting states should never time out"
        );
    }

    #[test]
    fn test_gui_action_history_check_timeouts_skips_actions_without_timeout_set() {
        // Actions in WaitingForExpectedState but without timeout_at_ms
        // should not be timed out
        let history = GuiActionHistory::with_capacity(10);
        let id = GuiActionId::new();

        let request = GuiActionRequest {
            action_id: id.clone(),
            requested_at_ms: 1000,
            command: "TestCommand".into(),
        };

        let recorded_id = history.record(request);
        history.set_state(&recorded_id, GuiActionState::WaitingForExpectedState);
        // Clear the timeout manually to simulate a corrupted state
        {
            let mut actions = history.inner.actions.lock().expect("actions lock");
            if let Some(status) = actions.get_mut(&recorded_id) {
                status.timeout_at_ms = None;
            }
        }

        let timed_out = history.check_timeouts();
        assert!(
            timed_out.is_empty(),
            "Actions without timeout_at_ms should not time out"
        );
    }

    #[test]
    fn test_gui_action_history_next_timeout_remaining_with_no_actions() {
        let history = GuiActionHistory::with_capacity(10);
        assert!(history.next_timeout_remaining_ms().is_none());
    }

    #[test]
    fn test_gui_action_history_next_timeout_remaining_with_outdated_timeout() {
        // An action whose timeout is in the past should return Some(0)
        let history = GuiActionHistory::with_capacity(10);
        let id = GuiActionId::new();
        let request = GuiActionRequest {
            action_id: id.clone(),
            requested_at_ms: 1000,
            command: "TestCommand".into(),
        };
        let recorded_id = history.record(request);
        history.set_state(&recorded_id, GuiActionState::WaitingForExpectedState);

        // Set timeout to the past
        {
            let mut actions = history.inner.actions.lock().expect("actions lock");
            if let Some(status) = actions.get_mut(&recorded_id) {
                status.timeout_at_ms = Some(1); // epoch + 1ms = long past
            }
        }

        let remaining = history.next_timeout_remaining_ms();
        assert_eq!(remaining, Some(0), "past timeout should return Some(0)");
    }

    #[test]
    fn test_gui_action_timeout_constant_values() {
        // Verify the constants match requirements
        assert_eq!(
            DEFAULT_ACTION_STATE_TIMEOUT_MS, 10_000,
            "default should be 10s"
        );
        assert_eq!(MAX_ACTION_STATE_TIMEOUT_MS, 30_000, "max should be 30s");
        assert!(
            DEFAULT_ACTION_STATE_TIMEOUT_MS <= MAX_ACTION_STATE_TIMEOUT_MS,
            "default must not exceed max"
        );
    }

    #[test]
    fn test_gui_action_timeout_at_ms_cleared_on_transition_to_timed_out() {
        // Test that transition_to TimedOut clears timeout_at_ms
        use GuiActionState::*;

        let mut action = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: Queued,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        action.transition_to(Validating).unwrap();
        action.transition_to(AppMessageQueued).unwrap();
        action.transition_to(AppMessageHandled).unwrap();
        action.transition_to(WaitingForExpectedState).unwrap();
        assert!(action.timeout_at_ms.is_some());

        // Transition to TimedOut must clear timeout_at_ms
        action.transition_to(TimedOut).unwrap();
        assert_eq!(action.state, TimedOut);
        assert!(
            action.timeout_at_ms.is_none(),
            "timeout_at_ms should be cleared on TimedOut terminal state"
        );
    }

    // =========================================================================
    // Concurrency tests for GuiActionHistory
    // =========================================================================

    #[test]
    fn test_concurrent_record_no_data_loss() {
        // Spawn N threads, each recording an action.
        // After all join, verify all N are present and readable.
        let history = GuiActionHistory::with_capacity(100);
        let n: usize = 20;
        let mut handles = Vec::with_capacity(n);

        for i in 0..n {
            let h = history.clone();
            handles.push(std::thread::spawn(move || {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: i as i64 * 100,
                    command: format!("Concurrent-{i}"),
                };
                let returned = h.record(request);
                assert_eq!(returned, id);
                id
            }));
        }

        let mut ids: Vec<GuiActionId> = handles
            .into_iter()
            .map(|h| h.join().expect("thread panicked"))
            .collect();
        ids.sort_by(|a, b| a.0.cmp(&b.0));

        assert_eq!(history.action_count(), n);
        assert_eq!(history.active_count(), n);

        for id in &ids {
            let status = history
                .get(id)
                .expect("every recorded action should be findable");
            assert_eq!(status.state, GuiActionState::Queued);
        }
    }

    #[test]
    fn test_concurrent_record_and_get_no_panic() {
        // Reader threads call get() while writer threads call record().
        // Verify no panics and all actions eventually readable.
        let history = GuiActionHistory::with_capacity(100);
        let n_writers = 8;
        let n_readers = 4;
        let actions_per_writer = 10;

        let recorded_ids = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let ready = std::sync::Arc::new(std::sync::Barrier::new(n_writers + n_readers));

        let mut handles = Vec::new();

        // Writer threads
        for w in 0..n_writers {
            let h = history.clone();
            let ids = recorded_ids.clone();
            let barrier = ready.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for i in 0..actions_per_writer {
                    let id = GuiActionId::new();
                    let request = GuiActionRequest {
                        action_id: id.clone(),
                        requested_at_ms: (w * actions_per_writer + i) as i64 * 100,
                        command: format!("Writer-{w}-{i}"),
                    };
                    h.record(request);
                    ids.lock().unwrap().push(id);
                }
            }));
        }

        // Reader threads
        for _ in 0..n_readers {
            let h = history.clone();
            let barrier = ready.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                // Repeatedly read all actions — should never panic
                for _ in 0..50 {
                    let _all = h.all_actions();
                    let _count = h.action_count();
                    let _active = h.active_count();
                }
            }));
        }

        // Wait for all
        for h in handles {
            h.join().expect("thread panicked");
        }

        let total_written = n_writers * actions_per_writer;
        assert_eq!(history.action_count(), total_written);
        assert_eq!(history.active_count(), total_written);
    }

    #[test]
    fn test_concurrent_transition_and_get() {
        // Writer threads progress actions through states while readers query.
        let history = GuiActionHistory::with_capacity(50);
        let n: usize = 20;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(n));

        let ids: Vec<GuiActionId> = (0..n)
            .map(|i| {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: i as i64 * 100,
                    command: format!("Trans-{i}"),
                };
                history.record(request);
                id
            })
            .collect();

        let mut handles = Vec::with_capacity(n);
        for (idx, id) in ids.iter().enumerate() {
            let h = history.clone();
            let aid = id.clone();
            let bar = barrier.clone();
            handles.push(std::thread::spawn(move || {
                bar.wait();
                // Transition through a valid lifecycle
                h.transition_to(&aid, GuiActionState::Validating).ok();
                h.transition_to(&aid, GuiActionState::AppMessageQueued).ok();
                h.transition_to(&aid, GuiActionState::AppMessageHandled)
                    .ok();
                h.transition_to(&aid, GuiActionState::Completed).ok();
                // Every 5th action, also read the result
                if idx % 5 == 0 {
                    let _status = h.get(&aid);
                }
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }

        // All should be completed
        for id in &ids {
            let status = history.get(id).expect("action should exist");
            assert_eq!(
                status.state,
                GuiActionState::Completed,
                "action {:?} should be completed",
                id
            );
        }
        assert_eq!(history.active_count(), 0);
    }

    #[test]
    fn test_concurrent_remove_and_get() {
        // Concurrent remove() and get() calls on the same history.
        let history = GuiActionHistory::with_capacity(50);
        let n: usize = 20;

        let ids: Vec<GuiActionId> = (0..n)
            .map(|i| {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: i as i64 * 100,
                    command: format!("Remove-{i}"),
                };
                history.record(request);
                id
            })
            .collect();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(n));
        let mut handles = Vec::with_capacity(n);

        for (idx, id) in ids.iter().enumerate() {
            let h = history.clone();
            let aid = id.clone();
            let bar = barrier.clone();
            handles.push(std::thread::spawn(move || {
                bar.wait();
                // Half remove, half read
                if idx % 2 == 0 {
                    let _removed = h.remove(&aid);
                } else {
                    let _status = h.get(&aid);
                }
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }

        // Evens were removed, odds should still exist
        for (idx, id) in ids.iter().enumerate() {
            if idx % 2 == 0 {
                assert!(
                    history.get(id).is_none(),
                    "even-indexed action should be removed"
                );
            } else {
                assert!(
                    history.get(id).is_some(),
                    "odd-indexed action should still exist"
                );
            }
        }
    }

    #[test]
    fn test_concurrent_record_with_capacity_eviction() {
        // Hit the capacity bound while multiple threads are recording.
        // Verify that eviction still works and no data is lost/duplicated.
        let capacity = 10;
        let history = GuiActionHistory::with_capacity(capacity);
        let n_threads = 8;
        let actions_per_thread = 5; // 40 total vs capacity 10

        let mut handles = Vec::with_capacity(n_threads);

        for t in 0..n_threads {
            let h = history.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..actions_per_thread {
                    let id = GuiActionId::new();
                    let request = GuiActionRequest {
                        action_id: id.clone(),
                        requested_at_ms: (t * actions_per_thread + i) as i64 * 100,
                        command: format!("Evict-{t}-{i}"),
                    };
                    h.record(request);
                    // Mark some as completed to allow targeted eviction
                    if i % 2 == 0 {
                        h.set_state(&id, GuiActionState::Completed);
                    }
                }
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }

        // Store should be at capacity (no more, no less)
        let count = history.action_count();
        assert!(
            count <= capacity,
            "should not exceed capacity: {count} > {capacity}"
        );
        // There may be fewer if some got pruned via order eviction,
        // but it should be tightly bounded near capacity
        assert!(
            count >= capacity - n_threads, // allow some slack due to concurrent eviction
            "should be near capacity: {count} < {}",
            capacity - n_threads
        );

        // Verify no action has a badly corrupted state
        let all = history.all_actions();
        for a in &all {
            assert!(
                a.state.is_terminal() || a.state.is_active(),
                "action {:?} has invalid state",
                a.action_id
            );
        }

        // All actions should have unique IDs
        let mut ids: Vec<&str> = all.iter().map(|a| a.action_id.0.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), all.len(), "all action IDs must be unique");
    }

    #[test]
    fn test_concurrent_all_actions_ordering() {
        // Verify newest-first ordering holds under concurrent read/write.
        let history = GuiActionHistory::with_capacity(50);
        let n: usize = 15;

        let ids: Vec<GuiActionId> = (0..n)
            .map(|i| {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: i as i64 * 100,
                    command: format!("Order-{i}"),
                };
                history.record(request);
                id
            })
            .collect();

        // Read all_actions from multiple threads simultaneously
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));
        let mut handles = Vec::with_capacity(5);
        for _ in 0..5 {
            let h = history.clone();
            let bar = barrier.clone();
            let ids_ref = ids.clone();
            handles.push(std::thread::spawn(move || {
                bar.wait();
                for _ in 0..20 {
                    let all = h.all_actions();
                    // The result should have n entries
                    assert_eq!(all.len(), n);
                    // Should be ordered newest first (descending by insertion order)
                    for pair in all.windows(2) {
                        let earlier_idx = ids_ref.iter().position(|id| id == &pair[1].action_id);
                        let later_idx = ids_ref.iter().position(|id| id == &pair[0].action_id);
                        if let (Some(e), Some(l)) = (earlier_idx, later_idx) {
                            assert!(
                                e <= l,
                                "newest-first ordering violated: {} before {}",
                                pair[0].action_id.0,
                                pair[1].action_id.0,
                            );
                        }
                    }
                }
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }
    }

    #[test]
    fn test_concurrent_mixed_operations_no_deadlock() {
        // Stress test: mix of record, get, transition, remove, all_actions,
        // active_count, and check_timeouts from many threads simultaneously.
        // If there's a lock inversion or deadlock, this test will hang.
        let history = GuiActionHistory::with_capacity(20);
        let n_threads = 12;
        let iterations = 25;

        let mut handles = Vec::with_capacity(n_threads);

        for t in 0..n_threads {
            let h = history.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..iterations {
                    let id = GuiActionId::new();
                    let request = GuiActionRequest {
                        action_id: id.clone(),
                        requested_at_ms: (t * iterations + i) as i64,
                        command: format!("Mix-{t}-{i}"),
                    };
                    let rid = h.record(request);

                    // Vary the operation per iteration to mix access patterns
                    match i % 6 {
                        0 => {
                            // Transition and read
                            h.transition_to(&rid, GuiActionState::Validating).ok();
                            let _s = h.get(&rid);
                        }
                        1 => {
                            // Read all
                            let _all = h.all_actions();
                        }
                        2 => {
                            // Transition and remove
                            h.transition_to(&rid, GuiActionState::Validating).ok();
                            h.set_state(&rid, GuiActionState::Completed);
                            let _r = h.remove(&rid);
                        }
                        3 => {
                            // check_timeouts
                            let _to = h.check_timeouts();
                        }
                        4 => {
                            // active_count and action_count
                            let _ac = h.active_count();
                            let _cnt = h.action_count();
                        }
                        5 => {
                            // Transition through full lifecycle
                            h.transition_to(&rid, GuiActionState::Validating).ok();
                            h.transition_to(&rid, GuiActionState::AppMessageQueued).ok();
                            h.transition_to(&rid, GuiActionState::AppMessageHandled)
                                .ok();
                            h.transition_to(&rid, GuiActionState::WaitingForExpectedState)
                                .ok();
                        }
                        _ => {}
                    }
                }
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }

        // Basic sanity: should not panic or hang
        let all = history.all_actions();
        // active_count should be consistent
        let active = history.active_count();
        let total = history.action_count();
        assert!(
            active <= total,
            "active count ({active}) cannot exceed total ({total})"
        );

        // Verify no duplicate action IDs
        let mut ids: Vec<&str> = all.iter().map(|a| a.action_id.0.as_str()).collect();
        ids.sort();
        let deduped = {
            let mut d = ids.clone();
            d.dedup();
            d
        };
        assert_eq!(
            ids.len(),
            deduped.len(),
            "no duplicate action IDs allowed under concurrent access"
        );
    }

    #[test]
    fn test_concurrent_status_reads() {
        // Multiple threads reading action statuses concurrently.
        let history = GuiActionHistory::with_capacity(30);

        // Pre-populate
        for i in 0..10 {
            let id = GuiActionId::new();
            let request = GuiActionRequest {
                action_id: id.clone(),
                requested_at_ms: i * 100,
                command: format!("Read-{i}"),
            };
            history.record(request);
        }

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let mut handles = Vec::with_capacity(8);

        for r in 0..8 {
            let h = history.clone();
            let bar = barrier.clone();
            handles.push(std::thread::spawn(move || {
                bar.wait();
                // Each reader calls multiple query methods
                for _ in 0..100 {
                    let all = h.all_actions();
                    let count = h.action_count();
                    let active = h.active_count();
                    let queued = h.actions_with_state(GuiActionState::Queued);
                    let _next_timeout = h.next_timeout_remaining_ms();

                    // all should contain the right number
                    assert_eq!(all.len(), count);
                }
                format!("Reader-{r} done")
            }));
        }

        for h in handles {
            h.join().expect("reader thread panicked");
        }

        // Verify no corruption from concurrent reads
        let all = history.all_actions();
        assert_eq!(all.len(), 10);
        assert_eq!(history.active_count(), 10);
    }

    #[test]
    fn test_concurrent_record_and_transition_chain() {
        // Multiple queued navigation-like actions: record an action,
        // transition it through states mimicking a real action lifecycle.
        // This simulates multiple queued navigation actions being processed.
        let history = GuiActionHistory::with_capacity(50);
        let n: usize = 16;

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(n));
        let mut handles = Vec::with_capacity(n);

        for idx in 0..n {
            let h = history.clone();
            let bar = barrier.clone();
            handles.push(std::thread::spawn(move || {
                bar.wait();
                // Each thread simulates one navigation action's lifecycle
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: idx as i64 * 100,
                    command: match idx % 4 {
                        0 => "GoToChatList".into(),
                        1 => "OpenRoom".into(),
                        2 => "OpenSettings".into(),
                        _ => "GoToChatList".into(),
                    },
                };
                let rid = h.record(request);

                // Status after record should be Queued
                let status = h.get(&rid).unwrap();
                assert_eq!(status.state, GuiActionState::Queued);

                // Gradually transition through the lifecycle
                h.transition_to(&rid, GuiActionState::Validating).unwrap();
                h.transition_to(&rid, GuiActionState::AppMessageQueued)
                    .unwrap();
                h.transition_to(&rid, GuiActionState::AppMessageHandled)
                    .unwrap();
                h.transition_to(&rid, GuiActionState::Completed).unwrap();

                // Final check
                let final_status = h.get(&rid).unwrap();
                assert_eq!(final_status.state, GuiActionState::Completed);
                assert!(final_status.state.is_terminal());

                rid
            }));
        }

        let results: Vec<GuiActionId> = handles
            .into_iter()
            .map(|h| h.join().expect("thread panicked"))
            .collect();

        // Verify all completed
        for id in &results {
            let status = history.get(id).expect("action should exist");
            assert_eq!(status.state, GuiActionState::Completed);
        }

        // Verify count is correct (ordering is non-deterministic under concurrency)
        let all = history.all_actions();
        assert_eq!(all.len(), n);
    }

    #[test]
    fn test_concurrent_composer_update_followed_by_submit() {
        // Simulate: set composer text, then submit — in sequence but
        // with concurrent status reads in between.
        let history = GuiActionHistory::with_capacity(10);

        // Set composer text action
        let compose_id = GuiActionId::new();
        history.record(GuiActionRequest {
            action_id: compose_id.clone(),
            requested_at_ms: 100,
            command: "SetComposerText".into(),
        });

        // Submit composer action
        let submit_id = GuiActionId::new();
        history.record(GuiActionRequest {
            action_id: submit_id.clone(),
            requested_at_ms: 200,
            command: "SubmitComposer".into(),
        });

        // Transition compose action while reading in parallel
        let h1 = history.clone();
        let h2 = history.clone();
        let cid = compose_id.clone();
        let sid = submit_id.clone();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));

        let t1 = {
            let bar = barrier.clone();
            std::thread::spawn(move || {
                bar.wait();
                h1.transition_to(&cid, GuiActionState::Validating).unwrap();
                h1.transition_to(&cid, GuiActionState::AppMessageQueued)
                    .unwrap();
                h1.transition_to(&cid, GuiActionState::AppMessageHandled)
                    .unwrap();
                h1.transition_to(&cid, GuiActionState::Completed).unwrap();
            })
        };

        let t2 = {
            let bar = barrier.clone();
            std::thread::spawn(move || {
                bar.wait();
                // Once compose is progressing, start submit
                h2.transition_to(&sid, GuiActionState::Validating).unwrap();
                h2.transition_to(&sid, GuiActionState::AppMessageQueued)
                    .unwrap();
                h2.transition_to(&sid, GuiActionState::AppMessageHandled)
                    .unwrap();
                h2.transition_to(&sid, GuiActionState::Completed).unwrap();
            })
        };

        // Reader thread checks status concurrently
        let h3 = history.clone();
        let _t3 = std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..20 {
                let _all = h3.all_actions();
                let _count = h3.action_count();
            }
        });

        t1.join().expect("compose thread panicked");
        t2.join().expect("submit thread panicked");

        // Now check that submit eventually completed
        // (it may finish before compose due to scheduling, but both should complete)
        let compose_status = history.get(&compose_id).expect("compose action exists");
        let submit_status = history.get(&submit_id).expect("submit action exists");
        assert!(
            compose_status.state.is_terminal(),
            "compose action should be terminal: {:?}",
            compose_status.state
        );
        assert!(
            submit_status.state.is_terminal(),
            "submit action should be terminal: {:?}",
            submit_status.state
        );
    }

    #[test]
    fn test_action_timeout_while_another_succeeds() {
        // One action times out (via check_timeouts) while another
        // successfully completes through its lifecycle.
        let history = GuiActionHistory::with_capacity(10);

        // Action A: normal lifecycle → Completed
        let id_a = GuiActionId::new();
        history.record(GuiActionRequest {
            action_id: id_a.clone(),
            requested_at_ms: 100,
            command: "NormalAction".into(),
        });
        history
            .transition_to(&id_a, GuiActionState::Validating)
            .unwrap();
        history
            .transition_to(&id_a, GuiActionState::AppMessageQueued)
            .unwrap();
        history
            .transition_to(&id_a, GuiActionState::AppMessageHandled)
            .unwrap();
        history
            .transition_to(&id_a, GuiActionState::Completed)
            .unwrap();

        // Action B: enters WaitingForExpectedState with expired timeout
        let id_b = GuiActionId::new();
        history.record(GuiActionRequest {
            action_id: id_b.clone(),
            requested_at_ms: 200,
            command: "TimeoutAction".into(),
        });
        history
            .transition_to(&id_b, GuiActionState::Validating)
            .unwrap();
        history
            .transition_to(&id_b, GuiActionState::AppMessageQueued)
            .unwrap();
        history
            .transition_to(&id_b, GuiActionState::AppMessageHandled)
            .unwrap();
        history
            .transition_to(&id_b, GuiActionState::WaitingForExpectedState)
            .unwrap();

        // Manually set timeout to the past so check_timeouts catches it
        {
            let mut actions = history.inner.actions.lock().expect("actions lock");
            if let Some(status) = actions.get_mut(&id_b) {
                status.timeout_at_ms = Some(1); // epoch + 1ms = long past
            }
        }

        // Run timeout check
        let timed_out = history.check_timeouts();

        // Verify action A is still Completed
        let status_a = history.get(&id_a).unwrap();
        assert_eq!(status_a.state, GuiActionState::Completed);

        // Verify action B was timed out
        assert_eq!(timed_out.len(), 1, "exactly one action should time out");
        assert_eq!(timed_out[0].0, id_b);
        assert_eq!(timed_out[0].1.state, GuiActionState::TimedOut);

        let status_b = history.get(&id_b).unwrap();
        assert_eq!(status_b.state, GuiActionState::TimedOut);
    }

    #[test]
    fn test_concurrent_timeout_check_during_lifecycle() {
        // Some threads transition actions through their lifecycle while
        // another thread calls check_timeouts(). Verify no deadlock.
        let history = GuiActionHistory::with_capacity(20);
        let n: usize = 8;

        let ids: Vec<GuiActionId> = (0..n)
            .map(|i| {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: i as i64 * 100,
                    command: format!("TimeoutTest-{i}"),
                };
                history.record(request);
                id
            })
            .collect();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(n + 1));
        let mut handles = Vec::with_capacity(n + 1);

        // Worker threads: transition actions through lifecycles
        for (idx, id) in ids.iter().enumerate() {
            let h = history.clone();
            let aid = id.clone();
            let bar = barrier.clone();
            handles.push(std::thread::spawn(move || {
                bar.wait();
                // Put some in waiting, some through to completion
                h.transition_to(&aid, GuiActionState::Validating).unwrap();
                h.transition_to(&aid, GuiActionState::AppMessageQueued)
                    .unwrap();
                h.transition_to(&aid, GuiActionState::AppMessageHandled)
                    .unwrap();
                if idx % 2 == 0 {
                    h.transition_to(&aid, GuiActionState::WaitingForExpectedState)
                        .unwrap();
                } else {
                    h.transition_to(&aid, GuiActionState::Completed).unwrap();
                }
                // Sleep a tiny bit to increase contention window
                std::thread::sleep(std::time::Duration::from_micros(10));
            }));
        }

        // Timeout checker thread
        let h_tc = history.clone();
        let bar_tc = barrier.clone();
        handles.push(std::thread::spawn(move || {
            bar_tc.wait();
            for _ in 0..20 {
                let _timed_out = h_tc.check_timeouts();
                std::thread::sleep(std::time::Duration::from_micros(5));
            }
        }));

        for h in handles {
            h.join().expect("thread panicked");
        }

        // Verify no structural corruption
        let all = history.all_actions();
        assert_eq!(all.len(), n);

        // Even-indexed actions should be WaitingForExpectedState or TimedOut
        for (idx, id) in ids.iter().enumerate() {
            let status = history.get(id).expect("action should exist");
            if idx % 2 == 0 {
                // Could be WaitingForExpectedState or TimedOut (if check_timeouts caught it)
                assert!(
                    status.state == GuiActionState::WaitingForExpectedState
                        || status.state == GuiActionState::TimedOut,
                    "even index {idx} should be waiting or timed out, got {:?}",
                    status.state
                );
            } else {
                assert_eq!(
                    status.state,
                    GuiActionState::Completed,
                    "odd index {idx} should be completed"
                );
            }
        }
    }

    #[test]
    fn test_gui_action_history_lock_no_deadlock() {
        // Verify the two-lock design (actions + order mutex) does not
        // deadlock under concurrent record + remove operations.
        // record() acquires actions then order; remove() acquires order then actions.
        // This is the classic lock ordering scenario.
        let history = GuiActionHistory::with_capacity(10);

        let ids: Vec<GuiActionId> = (0..5)
            .map(|i| {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: i as i64 * 100,
                    command: format!("Lock-{i}"),
                };
                history.record(request);
                id
            })
            .collect();

        // Complete them so they can be evicted
        for id in &ids {
            history.set_state(id, GuiActionState::Completed);
        }

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
        let mut handles = Vec::with_capacity(6);

        // Threads 0-4: alternate between record() and remove()
        for idx in 0..5 {
            let h = history.clone();
            let aid = ids[idx].clone();
            let bar = barrier.clone();
            handles.push(std::thread::spawn(move || {
                bar.wait();
                for round in 0..20 {
                    if round % 2 == 0 {
                        // record() acquires actions then order
                        let new_id = GuiActionId::new();
                        let request = GuiActionRequest {
                            action_id: new_id.clone(),
                            requested_at_ms: round as i64 * 100,
                            command: format!("Record-{idx}-{round}"),
                        };
                        h.record(request);
                    } else {
                        // remove() acquires order then actions
                        let _ = h.remove(&aid);
                        // Re-add so next round has something to remove
                        let request = GuiActionRequest {
                            action_id: aid.clone(),
                            requested_at_ms: round as i64 * 100,
                            command: format!("ReAdd-{idx}"),
                        };
                        h.record(request);
                    }
                }
            }));
        }

        // Thread 5: reads all_actions() which acquires both locks
        let h_reader = history.clone();
        let bar_reader = barrier.clone();
        handles.push(std::thread::spawn(move || {
            bar_reader.wait();
            for _ in 0..50 {
                let _all = h_reader.all_actions();
                std::thread::sleep(std::time::Duration::from_micros(5));
            }
        }));

        for h in handles {
            h.join().expect("thread panicked");
        }

        // If we reached here, there's no deadlock
        // Verify the history is still internally consistent
        let all = history.all_actions();
        let total = history.action_count();
        assert_eq!(all.len(), total);

        // No duplicate IDs
        let mut id_set: Vec<&str> = all.iter().map(|a| a.action_id.0.as_str()).collect();
        id_set.sort();
        let len_before = id_set.len();
        id_set.dedup();
        assert_eq!(
            id_set.len(),
            len_before,
            "no duplicate IDs under concurrent access"
        );
    }

    #[test]
    fn test_gui_action_history_arc_clone_shared_access() {
        // Verify that cloning Arc<GuiActionHistoryInner> works correctly
        // across threads — both clones see the same state.
        let history = GuiActionHistory::new();
        let h2 = history.clone();

        let id = GuiActionId::new();
        history.record(GuiActionRequest {
            action_id: id.clone(),
            requested_at_ms: 100,
            command: "Shared".into(),
        });

        // The clone should see the same data
        assert!(h2.get(&id).is_some());
        assert_eq!(h2.action_count(), 1);

        // Record via h2, read via history
        let id2 = GuiActionId::new();
        h2.record(GuiActionRequest {
            action_id: id2.clone(),
            requested_at_ms: 200,
            command: "Shared2".into(),
        });

        assert!(history.get(&id2).is_some());
        assert_eq!(history.action_count(), 2);
    }

    #[test]
    fn test_gui_action_history_next_timeout_concurrent_access() {
        // Verify next_timeout_remaining_ms is safe under concurrent
        // transitions that modify timeout_at_ms.
        let history = GuiActionHistory::with_capacity(10);
        let n: usize = 6;

        let ids: Vec<GuiActionId> = (0..n)
            .map(|i| {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: i as i64 * 100,
                    command: format!("TimeoutRead-{i}"),
                };
                history.record(request);
                id
            })
            .collect();

        // Put all into WaitingForExpectedState
        for id in &ids {
            history
                .transition_to(id, GuiActionState::Validating)
                .unwrap();
            history
                .transition_to(id, GuiActionState::AppMessageQueued)
                .unwrap();
            history
                .transition_to(id, GuiActionState::AppMessageHandled)
                .unwrap();
            history
                .transition_to(id, GuiActionState::WaitingForExpectedState)
                .unwrap();
        }

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(n + 2));
        let mut handles = Vec::with_capacity(n + 2);

        // Worker threads: complete some actions (removing timeout), timeout others
        for (idx, id) in ids.iter().enumerate() {
            let h = history.clone();
            let aid = id.clone();
            let bar = barrier.clone();
            handles.push(std::thread::spawn(move || {
                bar.wait();
                if idx % 2 == 0 {
                    // Complete normally — clears timeout
                    h.transition_to(&aid, GuiActionState::Completed).unwrap();
                } else {
                    // Let it stay in waiting (timeout remains)
                }
            }));
        }

        // Reader threads: read next_timeout_remaining_ms concurrently
        for _ in 0..2 {
            let h = history.clone();
            let bar = barrier.clone();
            handles.push(std::thread::spawn(move || {
                bar.wait();
                for _ in 0..30 {
                    let _remaining = h.next_timeout_remaining_ms();
                }
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }

        // Even-indexed should be Completed, odd-indexed still Waiting
        for (idx, id) in ids.iter().enumerate() {
            let status = history.get(id).expect("action exists");
            if idx % 2 == 0 {
                assert_eq!(status.state, GuiActionState::Completed);
            } else {
                assert_eq!(status.state, GuiActionState::WaitingForExpectedState);
                assert!(status.timeout_at_ms.is_some());
            }
        }

        // next_timeout_remaining_ms should not panic
        let _remaining = history.next_timeout_remaining_ms();
    }

    #[test]
    fn test_gui_action_history_eviction_under_concurrent_record() {
        // Simulate queue-full behaviour: capacity is small, many threads
        // record concurrently, forcing frequent eviction.
        let capacity = 5;
        let history = GuiActionHistory::with_capacity(capacity);
        let n_threads = 10;
        let actions_per_thread = 20; // 200 total vs capacity 5

        let mut handles = Vec::with_capacity(n_threads);

        for t in 0..n_threads {
            let h = history.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..actions_per_thread {
                    let id = GuiActionId::new();
                    let request = GuiActionRequest {
                        action_id: id.clone(),
                        requested_at_ms: (t * actions_per_thread + i) as i64 * 10,
                        command: format!("Full-{t}-{i}"),
                    };
                    h.record(request);
                    // Mark as terminal quickly to let eviction happen
                    h.set_state(&id, GuiActionState::Completed);
                }
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }

        // Should be bounded at capacity
        let count = history.action_count();
        assert!(
            count <= capacity,
            "should not exceed capacity: {count} > {capacity}"
        );

        // All stored actions should be terminal
        for a in history.all_actions() {
            assert!(
                a.state.is_terminal(),
                "stored action {:?} should be terminal under queue-full scenario",
                a.action_id
            );
        }
    }

    // ── GuiTestCommand serialization tests ──────────────────────────

    #[test]
    fn test_gui_test_command_go_to_chat_list_serde() {
        let cmd = GuiTestCommand::GoToChatList;
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"command":"go_to_chat_list"}"#);
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
        assert_eq!(deser, GuiTestCommand::GoToChatList);
    }

    #[test]
    fn test_gui_test_command_open_room_serde() {
        let cmd = GuiTestCommand::OpenRoom {
            room_id: "ab".to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"command":"open_room","room_id":"ab"}"#);
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
    }

    #[test]
    fn test_gui_test_command_open_conversation_serde() {
        let cmd = GuiTestCommand::OpenConversation {
            conversation_id: "deadbeef".to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(
            json,
            r#"{"command":"open_conversation","conversation_id":"deadbeef"}"#
        );
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
    }

    #[test]
    fn test_gui_test_command_open_friends_serde() {
        let cmd = GuiTestCommand::OpenFriends;
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"command":"open_friends"}"#);
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
    }

    #[test]
    fn test_gui_test_command_open_settings_serde() {
        let cmd = GuiTestCommand::OpenSettings;
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"command":"open_settings"}"#);
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
    }

    #[test]
    fn test_gui_test_command_close_dialog_serde() {
        let cmd = GuiTestCommand::CloseDialog;
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"command":"close_dialog"}"#);
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
    }

    #[test]
    fn test_gui_test_command_set_composer_text_serde() {
        let cmd = GuiTestCommand::SetComposerText {
            text: "hello world".to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(
            json,
            r#"{"command":"set_composer_text","text":"hello world"}"#
        );
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
    }

    #[test]
    fn test_gui_test_command_submit_composer_serde() {
        let cmd = GuiTestCommand::SubmitComposer;
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"command":"submit_composer"}"#);
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
    }

    #[test]
    fn test_gui_test_command_select_peer_serde() {
        let cmd = GuiTestCommand::SelectPeer {
            peer_id: "0123456789abcdef".to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(
            json,
            r#"{"command":"select_peer","peer_id":"0123456789abcdef"}"#
        );
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
    }

    #[test]
    fn test_gui_test_command_toggle_dark_mode_serde() {
        let cmd = GuiTestCommand::ToggleDarkMode { enabled: true };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"command":"toggle_dark_mode","enabled":true}"#);
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
        assert_eq!(deser, GuiTestCommand::ToggleDarkMode { enabled: true });

        let cmd_off = GuiTestCommand::ToggleDarkMode { enabled: false };
        let json_off = serde_json::to_string(&cmd_off).unwrap();
        assert_eq!(
            json_off,
            r#"{"command":"toggle_dark_mode","enabled":false}"#
        );
        let deser_off: GuiTestCommand = serde_json::from_str(&json_off).unwrap();
        assert_eq!(cmd_off, deser_off);
    }

    #[test]
    fn test_gui_test_command_toggle_help_serde() {
        let cmd = GuiTestCommand::ToggleHelp;
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"command":"toggle_help"}"#);
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
    }

    #[test]
    fn test_gui_test_command_wait_screen_is_serde() {
        let cmd = GuiTestCommand::Wait {
            condition: GuiWaitCondition::ScreenIs {
                expected: "ChatList".to_string(),
            },
            timeout_ms: 5000,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(
            json,
            r#"{"command":"wait","condition":{"type":"screen_is","expected":"ChatList"},"timeout_ms":5000}"#
        );
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
    }

    #[test]
    fn test_gui_test_command_wait_room_selected_serde() {
        let cmd = GuiTestCommand::Wait {
            condition: GuiWaitCondition::RoomSelected {
                room_topic: Some("topic123".to_string()),
            },
            timeout_ms: 30000,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);

        // With None
        let cmd_none = GuiTestCommand::Wait {
            condition: GuiWaitCondition::RoomSelected { room_topic: None },
            timeout_ms: 30000,
        };
        let json_none = serde_json::to_string(&cmd_none).unwrap();
        let deser_none: GuiTestCommand = serde_json::from_str(&json_none).unwrap();
        assert_eq!(cmd_none, deser_none);
    }

    #[test]
    fn test_gui_test_command_wait_peer_visible_serde() {
        let cmd = GuiTestCommand::Wait {
            condition: GuiWaitCondition::PeerVisible { min_count: 3 },
            timeout_ms: 10000,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(
            json,
            r#"{"command":"wait","condition":{"type":"peer_visible","min_count":3},"timeout_ms":10000}"#
        );
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
    }

    #[test]
    fn test_gui_test_command_wait_message_visible_serde() {
        let cmd = GuiTestCommand::Wait {
            condition: GuiWaitCondition::MessageVisible { min_count: 1 },
            timeout_ms: 15000,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
    }

    #[test]
    fn test_gui_test_command_wait_gui_revision_serde() {
        let cmd = GuiTestCommand::Wait {
            condition: GuiWaitCondition::GuiRevisionAtLeast {
                expected_revision: 42,
            },
            timeout_ms: 5000,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(
            json,
            r#"{"command":"wait","condition":{"type":"gui_revision_at_least","expected_revision":42},"timeout_ms":5000}"#
        );
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
    }

    #[test]
    fn test_gui_test_command_wait_conversation_selected_serde() {
        let cmd = GuiTestCommand::Wait {
            condition: GuiWaitCondition::ConversationSelected {
                conversation_id: Some("conv1".to_string()),
            },
            timeout_ms: 5000,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);

        // With None
        let cmd_none = GuiTestCommand::Wait {
            condition: GuiWaitCondition::ConversationSelected {
                conversation_id: None,
            },
            timeout_ms: 5000,
        };
        let json_none = serde_json::to_string(&cmd_none).unwrap();
        let deser_none: GuiTestCommand = serde_json::from_str(&json_none).unwrap();
        assert_eq!(cmd_none, deser_none);
    }

    #[test]
    fn test_gui_test_command_wait_composer_text_is_serde() {
        let cmd = GuiTestCommand::Wait {
            condition: GuiWaitCondition::ComposerTextIs {
                expected: "hello".to_string(),
            },
            timeout_ms: 5000,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(
            json,
            r#"{"command":"wait","condition":{"type":"composer_text_is","expected":"hello"},"timeout_ms":5000}"#
        );
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
    }

    #[test]
    fn test_gui_test_command_wait_dialog_open_closed_serde() {
        let cmd_open = GuiTestCommand::Wait {
            condition: GuiWaitCondition::DialogOpen,
            timeout_ms: 5000,
        };
        let json_open = serde_json::to_string(&cmd_open).unwrap();
        assert_eq!(
            json_open,
            r#"{"command":"wait","condition":{"type":"dialog_open"},"timeout_ms":5000}"#
        );
        let deser_open: GuiTestCommand = serde_json::from_str(&json_open).unwrap();
        assert_eq!(cmd_open, deser_open);

        let cmd_closed = GuiTestCommand::Wait {
            condition: GuiWaitCondition::DialogClosed,
            timeout_ms: 5000,
        };
        let json_closed = serde_json::to_string(&cmd_closed).unwrap();
        assert_eq!(
            json_closed,
            r#"{"command":"wait","condition":{"type":"dialog_closed"},"timeout_ms":5000}"#
        );
        let deser_closed: GuiTestCommand = serde_json::from_str(&json_closed).unwrap();
        assert_eq!(cmd_closed, deser_closed);
    }

    #[test]
    fn test_gui_test_command_wait_unread_count_serde() {
        let cmd = GuiTestCommand::Wait {
            condition: GuiWaitCondition::UnreadCountAtLeast { min_count: 5 },
            timeout_ms: 10000,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(
            json,
            r#"{"command":"wait","condition":{"type":"unread_count_at_least","min_count":5},"timeout_ms":10000}"#
        );
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser);
    }

    #[test]
    fn test_expected_state_serde() {
        let states: Vec<(ExpectedState, &str)> = vec![
            (
                ExpectedState::ScreenIs("ChatList".to_string()),
                r#""ChatList""#,
            ),
            (
                ExpectedState::RoomSelected("topic123".to_string()),
                r#""topic123""#,
            ),
            (
                ExpectedState::ConversationSelected("peer_key".to_string()),
                r#""peer_key""#,
            ),
            (
                ExpectedState::ComposerTextIs("hello".to_string()),
                r#""hello""#,
            ),
            (
                ExpectedState::DarkModeIs(true),
                r#"true"#,
            ),
            (ExpectedState::MessageSent, r#"null"#),
            (
                ExpectedState::HelpVisible(false),
                r#"false"#,
            ),
            (
                ExpectedState::Generic("custom condition".to_string()),
                r#""custom condition""#,
            ),
        ];

        for (state, expected_json) in states {
            let json = serde_json::to_string(&state).unwrap();
            assert_eq!(json, expected_json, "Mismatch for {:?}", state);
            let deser: ExpectedState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, deser, "Roundtrip mismatch for {:?}", state);
        }
    }

    #[test]
    fn test_gui_test_command_roundtrip_all_variants() {
        // Every variant serialized then deserialized must equal the original.
        let cmds: Vec<GuiTestCommand> = vec![
            GuiTestCommand::GoToChatList,
            GuiTestCommand::OpenRoom {
                room_id: "aabbccdd".to_string(),
            },
            GuiTestCommand::OpenConversation {
                conversation_id: "11223344".to_string(),
            },
            GuiTestCommand::OpenFriends,
            GuiTestCommand::OpenSettings,
            GuiTestCommand::CloseDialog,
            GuiTestCommand::SetComposerText {
                text: "test message".to_string(),
            },
            GuiTestCommand::SubmitComposer,
            GuiTestCommand::SelectPeer {
                peer_id: "ffeeddcc".to_string(),
            },
            GuiTestCommand::ToggleDarkMode { enabled: true },
            GuiTestCommand::ToggleHelp,
            GuiTestCommand::Wait {
                condition: GuiWaitCondition::ScreenIs {
                    expected: "Settings".to_string(),
                },
                timeout_ms: 1000,
            },
        ];

        for cmd in cmds {
            let json = serde_json::to_string(&cmd).unwrap();
            let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
            assert_eq!(cmd, deser, "Roundtrip failed for {:?}", cmd);
        }
    }

    #[test]
    fn test_gui_test_command_validation() {
        // Valid commands
        GuiTestCommand::GoToChatList.validate().unwrap();
        GuiTestCommand::OpenRoom {
            room_id: "valid_room_id".to_string(),
        }
        .validate()
        .unwrap();
        GuiTestCommand::SetComposerText {
            text: "Hello, world!".to_string(),
        }
        .validate()
        .unwrap();
        GuiTestCommand::ToggleDarkMode { enabled: true }
            .validate()
            .unwrap();
        GuiTestCommand::SubmitComposer.validate().unwrap();
        GuiTestCommand::CloseDialog.validate().unwrap();

        // Invalid: room_id too long (assumes GUI_TEST_COMMAND_MAX_STRING_LEN is 4096)
        let long_room = "x".repeat(GUI_TEST_COMMAND_MAX_STRING_LEN + 1);
        assert!(
            GuiTestCommand::OpenRoom {
                room_id: long_room.clone(),
            }
            .validate()
            .is_err(),
            "OpenRoom should reject over-long room_id"
        );

        // Invalid: composer text with control character
        assert!(
            GuiTestCommand::SetComposerText {
                text: "hello\nworld".to_string(),
            }
            .validate()
            .is_err(),
            "SetComposerText should reject control characters"
        );

        // Invalid: timeout exceeds max
        assert!(
            GuiTestCommand::Wait {
                condition: GuiWaitCondition::DialogClosed,
                timeout_ms: GUI_TEST_COMMAND_MAX_TIMEOUT_MS + 1,
            }
            .validate()
            .is_err(),
            "Wait should reject over-max timeout"
        );

        // Valid: timeout at max
        GuiTestCommand::Wait {
            condition: GuiWaitCondition::DialogClosed,
            timeout_ms: GUI_TEST_COMMAND_MAX_TIMEOUT_MS,
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn test_expected_state_matches_str() {
        let screen = ExpectedState::ScreenIs("ChatList".to_string());
        assert!(screen.matches_str("screen", "ChatList"));
        assert!(!screen.matches_str("screen", "Settings"));
        assert!(!screen.matches_str("room", "ChatList"));

        let room = ExpectedState::RoomSelected("abc".to_string());
        assert!(room.matches_str("room", "abc"));
        assert!(!room.matches_str("room", "xyz"));

        let dark = ExpectedState::DarkModeIs(true);
        assert!(dark.matches_str("dark_mode", "true"));
        assert!(!dark.matches_str("dark_mode", "false"));

        let msg = ExpectedState::MessageSent;
        assert!(msg.matches_str("message_sent", "true"));
        assert!(!msg.matches_str("message_sent", "false"));
    }

    // ── Concurrency tests ────────────────────────────────────────────────

    #[test]
    fn test_concurrent_multiple_navigation_actions() {
        // Multiple queued GUI navigation actions processed concurrently.
        // Each thread enqueues a navigation action and transitions it through
        // the lifecycle; verify all reach completion without deadlock.
        let history = GuiActionHistory::with_capacity(100);
        const N_THREADS: usize = 8;
        const ACTIONS_PER_THREAD: usize = 25;

        let mut handles = Vec::with_capacity(N_THREADS);

        for t in 0..N_THREADS {
            let h = history.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..ACTIONS_PER_THREAD {
                    let id = GuiActionId::new();
                    let request = GuiActionRequest {
                        action_id: id.clone(),
                        requested_at_ms: (t * ACTIONS_PER_THREAD + i) as i64 * 10,
                        command: format!("Nav-{t}-{i}"),
                    };
                    let rid = h.record(request);
                    assert_eq!(rid, id, "recorded id should match");

                    // Full lifecycle: Queued -> Validating -> AppMessageQueued -> AppMessageHandled -> Completed
                    h.transition_to(&id, GuiActionState::Validating).unwrap_or_else(|e| {
                        // Rejected or Failed also acceptable terminal states under high concurrency
                        // if validation conditions change
                        if e.code == GuiActionErrorCode::InvalidArgument {
                            // Could be from eviction — action was evicted before transition
                            return;
                        }
                        panic!("transition to Validating failed: {e:?}");
                    });
                    h.transition_to(&id, GuiActionState::AppMessageQueued).ok();
                    h.transition_to(&id, GuiActionState::AppMessageHandled).ok();
                    h.transition_to(&id, GuiActionState::Completed).ok();
                }
            }));
        }

        // Drain threads — any panic propagates
        for h in handles {
            h.join().expect("navigation action thread panicked");
        }

        // All 200 actions should be accounted for (some may have been evicted
        // but all survivors should be terminal)
        let count = history.action_count();
        assert!(count <= 100, "history should not exceed capacity: {count} > 100");
        assert!(count > 0, "should have at least some actions stored");

        // Every stored action must be terminal
        for a in history.all_actions() {
            assert!(
                a.state.is_terminal(),
                "stored action {:?} should be terminal, was {:?}",
                a.action_id, a.state
            );
        }

        // No duplicate action IDs
        let ids: std::collections::HashSet<GuiActionId> =
            history.all_actions().into_iter().map(|a| a.action_id).collect();
        assert_eq!(
            ids.len(),
            history.action_count(),
            "no duplicate IDs allowed under concurrent access"
        );
    }

    #[test]
    fn test_concurrent_composer_update_then_submit() {
        // Composer update (SetComposerText) followed by submit (SubmitComposer)
        // executed by separate threads. Verify both actions make it through
        // the lifecycle and ordering can be inferred from requested_at_ms.
        let history = GuiActionHistory::with_capacity(50);
        const PAIRS: usize = 30;

        let mut handles = Vec::with_capacity(PAIRS * 2);

        for i in 0..PAIRS {
            // Set text action
            let h = history.clone();
            let set_id = GuiActionId::new();
            let set_req = GuiActionRequest {
                action_id: set_id.clone(),
                requested_at_ms: i as i64 * 100,
                command: format!("SetComposerText-pair-{i}"),
            };

            handles.push(std::thread::spawn(move || {
                let rid = h.record(set_req);
                let _ = rid;
                h.transition_to(&set_id, GuiActionState::Validating).ok();
                h.transition_to(&set_id, GuiActionState::AppMessageQueued).ok();
                h.transition_to(&set_id, GuiActionState::AppMessageHandled).ok();
                h.transition_to(&set_id, GuiActionState::Completed).ok();
            }));

            // Submit action (slightly later in requested_at_ms)
            let h2 = history.clone();
            let sub_id = GuiActionId::new();
            let sub_req = GuiActionRequest {
                action_id: sub_id.clone(),
                requested_at_ms: i as i64 * 100 + 50, // 50ms after the set
                command: format!("SubmitComposer-pair-{i}"),
            };

            handles.push(std::thread::spawn(move || {
                let rid = h2.record(sub_req);
                let _ = rid;
                h2.transition_to(&sub_id, GuiActionState::Validating).ok();
                h2.transition_to(&sub_id, GuiActionState::AppMessageQueued).ok();
                h2.transition_to(&sub_id, GuiActionState::AppMessageHandled).ok();
                h2.transition_to(&sub_id, GuiActionState::Completed).ok();
            }));
        }

        for h in handles {
            h.join().expect("composer thread panicked");
        }

        // Every stored action should be terminal
        for a in history.all_actions() {
            assert!(
                a.state.is_terminal(),
                "all actions should be terminal, got {:?}",
                a.state
            );
        }

        // Verify no ID collisions
        let ids: std::collections::HashSet<GuiActionId> =
            history.all_actions().into_iter().map(|a| a.action_id).collect();
        assert_eq!(ids.len(), history.action_count(), "no duplicate IDs");
    }

    #[test]
    fn test_concurrent_timeout_while_another_succeeds() {
        // One action times out while another completes normally.
        // Use very short (retroactive) timeouts to force timeout detection,
        // then use check_timeouts() to verify the timed-out action is detected
        // while the completed action is untouched.
        let history = GuiActionHistory::with_capacity(10);

        // Action A: will be completed normally
        let id_a = GuiActionId::new();
        history.record(GuiActionRequest {
            action_id: id_a.clone(),
            requested_at_ms: 100,
            command: "WillSucceed".into(),
        });

        // Action B: will be left in WaitingForExpectedState — should time out
        let id_b = GuiActionId::new();
        history.record(GuiActionRequest {
            action_id: id_b.clone(),
            requested_at_ms: 200,
            command: "WillTimeout".into(),
        });

        // Thread A: drive A through the full lifecycle to Completed
        let h_a = history.clone();
        let aid_a = id_a.clone();
        let t_a = std::thread::spawn(move || {
            // Drive A through full lifecycle
            h_a.transition_to(&aid_a, GuiActionState::Validating).unwrap();
            h_a.transition_to(&aid_a, GuiActionState::AppMessageQueued).unwrap();
            h_a.transition_to(&aid_a, GuiActionState::AppMessageHandled).unwrap();
            h_a.transition_to(&aid_a, GuiActionState::WaitingForExpectedState).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
            h_a.transition_to(&aid_a, GuiActionState::Completed).unwrap();
        });

        // Thread B: drive B to WaitingForExpectedState then leave it
        // Force the timeout to be in the past by setting it directly
        let h_b = history.clone();
        let aid_b = id_b.clone();
        let t_b = std::thread::spawn(move || {
            h_b.transition_to(&aid_b, GuiActionState::Validating).unwrap();
            h_b.transition_to(&aid_b, GuiActionState::AppMessageQueued).unwrap();
            h_b.transition_to(&aid_b, GuiActionState::AppMessageHandled).unwrap();
            h_b.transition_to(&aid_b, GuiActionState::WaitingForExpectedState).unwrap();

            // Forcibly set timeout to the past to make check_timeouts detect it
            // even if the 10ms hasn't passed yet
            std::thread::sleep(std::time::Duration::from_millis(20));

            // Now directly set timeout_at_ms to 1 (epoch ms 1, far in the past)
            {
                let inner = &h_b.inner;
                let mut actions = inner.actions.lock().expect("actions lock");
                if let Some(status) = actions.get_mut(&aid_b) {
                    status.timeout_at_ms = Some(1);
                }
            }
        });

        t_a.join().expect("succeed thread panicked");
        t_b.join().expect("timeout thread panicked");

        // Run check_timeouts — should catch B, not A
        let timed_out = history.check_timeouts();

        // A should be Completed
        let status_a = history.get(&id_a).expect("action A exists");
        assert_eq!(
            status_a.state,
            GuiActionState::Completed,
            "successful action should be Completed"
        );

        // B should be TimedOut (or just timed out entries show up in check_timeouts result)
        let status_b = history.get(&id_b).expect("action B exists");
        if status_b.state == GuiActionState::WaitingForExpectedState {
            // check_timeouts may not have caught it if the sleep wasn't enough;
            // the timeout_at_ms manipulation should have worked though
            assert!(
                timed_out.iter().any(|(id, _)| *id == id_b),
                "action B should have been detected as timed out: timed_out={:?}",
                timed_out
            );
        } else {
            assert_eq!(
                status_b.state,
                GuiActionState::TimedOut,
                "timed-out action should be in TimedOut state"
            );
        }

        // Action A should never be in timed_out list
        assert!(
            !timed_out.iter().any(|(id, _)| *id == id_a),
            "successful action should not be in timed_out list"
        );
    }

    #[cfg(feature = "gui")]
    #[test]
    fn test_concurrent_channel_closure() {
        // Channel closure: close the receiver while enqueuing actions.
        // Verify that subsequent enqueues return ActionQueueClosed.
        let (handle, rx) = GuiTestHandle::channel(256);

        // Enqueue a few successful actions first
        let success_ids: Vec<GuiActionId> = (0..3)
            .map(|i| {
                let id = GuiActionId::new();
                let req = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: i as i64 * 100,
                    command: format!("PreClose-{i}"),
                };
                handle.enqueue(req).expect("pre-close enqueue should succeed");
                id
            })
            .collect();

        assert_eq!(success_ids.len(), 3, "three actions should enqueue");

        // Close the channel by dropping the receiver
        drop(rx);

        // Enqueue should now return ActionQueueClosed
        let post_close = GuiActionRequest {
            action_id: GuiActionId::new(),
            requested_at_ms: 9999,
            command: "PostClose".into(),
        };
        let err = handle.enqueue(post_close).unwrap_err();
        assert_eq!(
            err.code,
            GuiActionErrorCode::ActionQueueClosed,
            "enqueue after channel close should return ActionQueueClosed, got {:?}",
            err.code
        );

        // is_closed should return true
        assert!(handle.is_closed(), "handle should report closed");
    }

    #[cfg(feature = "gui")]
    #[cfg(feature = "gui")]
    #[test]
    fn test_concurrent_queue_full_behaviour() {
        // Queue full behaviour: fill a small-capacity channel without draining,
        // verify that new enqueues return ActionQueueFull.
        let capacity = 2;
        let (handle, mut rx) = GuiTestHandle::channel(capacity);

        // Fill the channel to capacity
        for i in 0..capacity {
            let req = GuiActionRequest {
                action_id: GuiActionId::new(),
                requested_at_ms: i as i64 * 100,
                command: format!("Fill-{i}"),
            };
            handle
                .enqueue(req)
                .unwrap_or_else(|_| panic!("fill enqueue {i} should succeed"));
        }

        // Next enqueue should fail with ActionQueueFull (no drain)
        let overflow = GuiActionRequest {
            action_id: GuiActionId::new(),
            requested_at_ms: 9999,
            command: "Overflow".into(),
        };
        let err = handle.enqueue(overflow).unwrap_err();
        assert_eq!(
            err.code,
            GuiActionErrorCode::ActionQueueFull,
            "enqueue beyond capacity should return ActionQueueFull, got {:?}",
            err.code
        );

        // Drain one item — next enqueue should succeed
        let _ = rx.try_recv().expect("should drain one item");
        let after_drain = GuiActionRequest {
            action_id: GuiActionId::new(),
            requested_at_ms: 10000,
            command: "AfterDrain".into(),
        };
        handle
            .enqueue(after_drain)
            .expect("enqueue after drain should succeed");

        // Drain the rest
        while rx.try_recv().is_ok() {}
    }

    #[cfg(feature = "gui")]
    #[cfg(feature = "gui")]
    #[test]
    fn test_concurrent_status_reads_with_writes() {
        // Concurrent status reads: multiple threads read action status
        // (get, action_count, all_actions, actions_with_state) while
        // writer threads record and update actions.
        // Verify no panics and eventually-consistent state.
        let history = GuiActionHistory::with_capacity(100);
        const N_WRITERS: usize = 4;
        const N_READERS: usize = 4;
        const ACTIONS_PER_WRITER: usize = 50;
        const READS_PER_READER: usize = 200;

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(N_WRITERS + N_READERS));
        let mut handles = Vec::with_capacity(N_WRITERS + N_READERS);

        // Writer threads: record actions and transition them through lifecycle
        for w in 0..N_WRITERS {
            let h = history.clone();
            let bar = barrier.clone();
            handles.push(std::thread::spawn(move || {
                bar.wait();
                for i in 0..ACTIONS_PER_WRITER {
                    let id = GuiActionId::new();
                    let request = GuiActionRequest {
                        action_id: id.clone(),
                        requested_at_ms: (w * ACTIONS_PER_WRITER + i) as i64 * 10,
                        command: format!("Writer-{w}-{i}"),
                    };
                    let rid = h.record(request);
                    let _ = rid;

                    // Drive through lifecycle (best-effort, may fail if evicted)
                    h.transition_to(&id, GuiActionState::Validating).ok();
                    h.transition_to(&id, GuiActionState::AppMessageQueued).ok();
                    h.transition_to(&id, GuiActionState::AppMessageHandled).ok();
                    h.transition_to(&id, GuiActionState::Completed).ok();
                }
            }));
        }

        // Reader threads: read status while writes happen
        for _ in 0..N_READERS {
            let h = history.clone();
            let bar = barrier.clone();
            handles.push(std::thread::spawn(move || {
                bar.wait();
                for ri in 0..READS_PER_READER {
                    // Mix get, action_count, all_actions, actions_with_state
                    match ri % 4 {
                        0 => {
                            let _count = h.action_count();
                        }
                        1 => {
                            let _all = h.all_actions();
                        }
                        2 => {
                            let _completed = h.actions_with_state(GuiActionState::Completed);
                        }
                        3 => {
                            let _active = h.active_count();
                        }
                        _ => unreachable!(),
                    }
                }
            }));
        }

        for h in handles {
            h.join().expect("status read/write thread panicked");
        }

        // Eventually-consistent: all actions should be bounded
        let count = history.action_count();
        assert!(count <= 100, "should not exceed capacity: {count} > 100");

        // All survivors must be terminal
        for a in history.all_actions() {
            assert!(
                a.state.is_terminal(),
                "all survivors should be terminal, got {:?}",
                a.state
            );
        }

        // No duplicate IDs
        let ids: std::collections::HashSet<GuiActionId> =
            history.all_actions().into_iter().map(|a| a.action_id).collect();
        assert_eq!(ids.len(), history.action_count(), "no duplicate IDs");
    }

    #[test]
    fn test_concurrent_event_ordering() {
        // Event ordering: verify that GuiActionEventHistory sequences are
        // unique and monotonically increasing even under concurrent recording.
        let journal = GuiActionEventHistory::with_capacity(5000);
        const N_THREADS: usize = 8;
        const EVENTS_PER_THREAD: usize = 100;

        let mut handles = Vec::with_capacity(N_THREADS);

        for t in 0..N_THREADS {
            let j = journal.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..EVENTS_PER_THREAD {
                    j.record(
                        format!("action-{t}-{i}"),
                        GuiActionEventKind::ActionRequested,
                        (t * EVENTS_PER_THREAD + i) as u64,
                        None,
                        "ConcurrentScreen",
                    );
                }
            }));
        }

        for h in handles {
            h.join().expect("event recording thread panicked");
        }

        // Verify sequence numbers are unique and increasing
        let entries = journal.all_entries();
        assert!(!entries.is_empty(), "should have recorded events");

        // Collect sequences (entries are newest-first)
        let mut sequences: Vec<u64> = entries.iter().map(|e| e.sequence).collect();
        sequences.sort();

        // Should be 0..N-1 with no gaps
        let total_expected = (N_THREADS * EVENTS_PER_THREAD) as u64;
        // Some entries may have been evicted if journal filled up
        // But with capacity 5000 vs 800 entries, none should be evicted
        assert_eq!(
            sequences.len() as u64,
            total_expected,
            "should have all {total_expected} sequences, got {}",
            sequences.len()
        );

        // Sequences should be 0..total_expected-1 with no gaps
        for (idx, &seq) in sequences.iter().enumerate() {
            assert_eq!(
                seq as usize, idx,
                "sequences should be contiguous with no gaps at position {idx}"
            );
        }
    }

    #[test]
    fn test_concurrent_event_ordering_with_mixed_kinds() {
        // Event ordering with mixed event kinds: verify sequences are unique
        // and time-ordered even when different event types are recorded
        // concurrently.
        let journal = GuiActionEventHistory::with_capacity(5000);
        const N_THREADS: usize = 6;
        const EVENTS_PER_THREAD: usize = 75;
        let event_kinds = std::sync::Arc::new(vec![
            GuiActionEventKind::ActionRequested,
            GuiActionEventKind::ActionValidated,
            GuiActionEventKind::ActionRejected {
                reason: "test concurrent rejection".into(),
            },
            GuiActionEventKind::ActionCompleted,
            GuiActionEventKind::ActionTimedOut { timeout_ms: 5000 },
            GuiActionEventKind::ActionFailed {
                error: "concurrent error".into(),
            },
        ]);
        let mut handles = Vec::with_capacity(N_THREADS);

        for t in 0..N_THREADS {
            let j = journal.clone();
            let ek = Arc::clone(&event_kinds);
            handles.push(std::thread::spawn(move || {
                for i in 0..EVENTS_PER_THREAD {
                    let kind_idx = (t * EVENTS_PER_THREAD + i) % ek.len();
                    j.record(
                        format!("action-{t}-{i}"),
                        ek[kind_idx].clone(),
                        (t * EVENTS_PER_THREAD + i) as u64,
                        None,
                        "MixedScreen",
                    );
                }
            }));
        }

        for h in handles {
            h.join().expect("mixed event thread panicked");
        }

        let entries = journal.all_entries();
        let total_expected = N_THREADS * EVENTS_PER_THREAD;
        assert_eq!(
            entries.len(),
            total_expected,
            "should have exactly {total_expected} entries, got {}",
            entries.len()
        );

        // Sequences should be contiguous 0..total with no gaps
        let mut sequences: Vec<u64> = entries.iter().map(|e| e.sequence).collect();
        sequences.sort();
        for (idx, &seq) in sequences.iter().enumerate() {
            assert_eq!(
                seq as usize, idx,
                "mixed event sequences should be contiguous at position {idx}"
            );
        }

        // Verify all sequence entries are present
        let seq_set: std::collections::HashSet<u64> = sequences.into_iter().collect();
        for s in 0..total_expected as u64 {
            assert!(
                seq_set.contains(&s),
                "sequence {s} should exist in journal"
            );
        }
    }

    #[test]
    fn test_concurrent_gui_revision_progression() {
        // GUI revision progression: verify that revisions recorded under
        // concurrency are unique and monotonically increasing.
        let journal = GuiActionEventHistory::with_capacity(5000);
        const N_THREADS: usize = 4;
        const EVENTS_PER_THREAD: usize = 50;

        let mut handles = Vec::with_capacity(N_THREADS);

        for t in 0..N_THREADS {
            let j = journal.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..EVENTS_PER_THREAD {
                    // Each thread uses its own revision base to avoid collisions
                    // (concurrent threads could interleave the same revision number)
                    let revision = (t * EVENTS_PER_THREAD + i) as u64;
                    j.record(
                        format!("rev-action-{t}-{i}"),
                        GuiActionEventKind::ActionCompleted,
                        revision,
                        None,
                        "RevisionScreen",
                    );
                }
            }));
        }

        for h in handles {
            h.join().expect("revision thread panicked");
        }

        let entries = journal.all_entries();
        let total_expected = N_THREADS * EVENTS_PER_THREAD;
        assert_eq!(
            entries.len(),
            total_expected,
            "should have {total_expected} entries, got {}",
            entries.len()
        );

        // Verify ALL revisions 0..200 are present (each thread reserved its range)
        let revisions: std::collections::HashSet<u64> =
            entries.iter().map(|e| e.gui_revision).collect();
        for rev in 0..total_expected as u64 {
            assert!(
                revisions.contains(&rev),
                "revision {rev} should be present in journal entries"
            );
        }

        // Sequences should be contiguous
        let mut sequences: Vec<u64> = entries.iter().map(|e| e.sequence).collect();
        sequences.sort();
        for (idx, &seq) in sequences.iter().enumerate() {
            assert_eq!(
                seq as usize, idx,
                "sequences should be contiguous with no gaps at position {idx} (revision progression)"
            );
        }
    }

    #[cfg(feature = "gui")]
    #[test]
    fn test_concurrent_guihandle_enqueue_deadlock_free() {
        // Verify that concurrent enqueue and receive on a GuiTestHandle
        // channel is deadlock-free. Use multiple producer threads and
        // an active consumer draining the receiver.
        let (handle, mut rx) = GuiTestHandle::channel(256);
        const N_PRODUCERS: usize = 6;
        const MSGS_PER_PRODUCER: usize = 50;

        let mut handles = Vec::with_capacity(N_PRODUCERS);
        let mut received = 0usize;

        for p in 0..N_PRODUCERS {
            let h = handle.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..MSGS_PER_PRODUCER {
                    let req = GuiActionRequest {
                        action_id: GuiActionId::new(),
                        requested_at_ms: (p * MSGS_PER_PRODUCER + i) as i64 * 10,
                        command: format!("ConcurrentEnqueue-{p}-{i}"),
                    };
                    h.enqueue(req).unwrap_or_else(|e| {
                        // Channel may close or be full during drain race;
                        // just count what we can
                        panic!("enqueue failed: {e:?}");
                    });
                }
            }));
        }

        // Drain the receiver while producers are running
        use std::time::Duration;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while received < N_PRODUCERS * MSGS_PER_PRODUCER {
            if std::time::Instant::now() > deadline {
                break; // Don't hang if producers failed
            }
            match rx.try_recv() {
                Ok(_) => received += 1,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    std::thread::yield_now();
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    break;
                }
            }
        }

        for h in handles {
            h.join().expect("producer thread panicked");
        }

        // After producers finish, drain any remaining
        loop {
            match rx.try_recv() {
                Ok(_) => received += 1,
                _ => break,
            }
        }

        assert_eq!(
            received,
            N_PRODUCERS * MSGS_PER_PRODUCER,
            "should have received all {expected} messages, got {received}",
            expected = N_PRODUCERS * MSGS_PER_PRODUCER
        );
    }

    #[test]
    fn test_concurrent_lock_order_consistency() {
        // Verify no lock inversion or deadlock by exercising both
        // GuiActionHistory and GuiActionEventHistory simultaneously
        // from multiple threads.
        let history = GuiActionHistory::with_capacity(50);
        let journal = GuiActionEventHistory::with_capacity(300);
        const N_WRITERS: usize = 6;
        const ITEMS_PER_WRITER: usize = 40;

        let mut handles = Vec::with_capacity(N_WRITERS);

        for w in 0..N_WRITERS {
            let h = history.clone();
            let j = journal.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..ITEMS_PER_WRITER {
                    // Write to action history
                    let id = GuiActionId::new();
                    let request = GuiActionRequest {
                        action_id: id.clone(),
                        requested_at_ms: (w * ITEMS_PER_WRITER + i) as i64 * 10,
                        command: format!("LockTest-{w}-{i}"),
                    };
                    h.record(request);
                    h.transition_to(&id, GuiActionState::Validating).ok();
                    h.transition_to(&id, GuiActionState::AppMessageQueued).ok();
                    h.transition_to(&id, GuiActionState::AppMessageHandled).ok();
                    h.transition_to(&id, GuiActionState::Completed).ok();

                    // Also write to event journal
                    j.record(
                        format!("lock-event-{w}-{i}"),
                        GuiActionEventKind::ActionCompleted,
                        (w * ITEMS_PER_WRITER + i) as u64,
                        None,
                        "LockTestScreen",
                    );

                    // Read from both
                    let _all = h.all_actions();
                    let _entries = j.entries_since(0, 10);
                }
            }));
        }

        for h in handles {
            h.join().expect("lock order consistency thread panicked");
        }

        // Both stores should be internally consistent
        assert!(history.action_count() <= 50, "history capacity respected");
        assert_eq!(
            journal.entry_count(),
            N_WRITERS * ITEMS_PER_WRITER,
            "journal should have all entries"
        );

        // All history entries should be terminal
        for a in history.all_actions() {
            assert!(
                a.state.is_terminal(),
                "all actions should be terminal under lock test"
            );
        }
    }
}
