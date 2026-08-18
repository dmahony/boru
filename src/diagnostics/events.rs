//! Boru diagnostics submodule (structural split BORU-CORE-002).

use super::*;

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

    // ── Store-layer persistence events ──────────────────────────────
    /// A new message was persisted to the inbox.
    IncomingPersisted {
        message_id_short: Option<String>,
        conversation_id_prefix: Option<String>,
        peer_id: Option<String>,
        delivery_state: String,
    },
    /// A duplicate message was received (not persisted).
    DuplicateReceived {
        message_id_short: Option<String>,
        conversation_id_prefix: Option<String>,
        peer_id: Option<String>,
    },
    /// An outbound message was queued for delivery.
    MessageQueued {
        message_id_short: Option<String>,
        conversation_id_prefix: Option<String>,
        peer_id: Option<String>,
        delivery_state: String,
    },
    /// An acknowledgement was received for an outbound message.
    AckReceived {
        message_id_short: Option<String>,
        conversation_id_prefix: Option<String>,
        peer_id: Option<String>,
        attempt_count: u32,
        elapsed_ms: Option<u64>,
    },
    /// A delivery retry was scheduled after an attempt.
    RetryScheduled {
        message_id_short: Option<String>,
        conversation_id_prefix: Option<String>,
        peer_id: Option<String>,
        attempt_count: u32,
        retry_delay_ms: u64,
        failure_category: String,
    },
    /// A delivery attempt for an outbound message started.
    DeliveryAttemptStarted {
        message_id_short: Option<String>,
        conversation_id_prefix: Option<String>,
        peer_id: Option<String>,
        attempt_count: u32,
        retry_delay_ms: Option<u64>,
    },
    /// An outbound message expired and was removed from the outbox.
    MessageExpired {
        message_id_short: Option<String>,
        conversation_id_prefix: Option<String>,
        peer_id: Option<String>,
        delivery_state: String,
    },
    /// A delivery stuck in ``Sending`` was recovered back to ``Pending``.
    SendingRecovered { count: usize },

    // ── Catalogue events ─────────────────────────────────────────
    /// A catalogue fetch from a remote peer has started.
    CatalogueFetchStarted {
        /// Optional known revision sent with the request.
        known_revision: Option<u64>,
    },
    /// A catalogue fetch from a remote peer completed successfully.
    CatalogueFetchCompleted {
        /// Catalogue revision at fetch time.
        revision: u64,
        /// Number of files in the received catalogue.
        file_count: usize,
        /// Number of collections in the received catalogue.
        collection_count: usize,
    },
    /// A catalogue fetch from a remote peer failed.
    CatalogueFetchFailed {
        /// Human-readable error description.
        error: String,
    },
    /// A catalogue signature verification was rejected.
    CatalogueSignatureRejected {
        /// Reason for the rejection.
        error: String,
    },
    /// A notification/advertisement of a new catalogue was received
    /// from a remote peer (e.g. via gossip or direct discovery).
    ///
    /// The peer identity is carried via `record_with_peer`'s `peer_id`
    /// parameter — this variant only holds the revision metadata from
    /// the notice message.
    CatalogueNoticeReceived {
        /// The catalogue revision advertised by the remote peer, if known.
        known_revision: Option<u64>,
    },
    /// A new catalogue revision was successfully installed in local storage.
    ///
    /// This is recorded after a fetched catalogue passes all validation
    /// checks and is persisted to the database.
    CatalogueRevisionInstalled {
        /// The installed revision number.
        revision: u64,
        /// Number of files in the installed catalogue.
        file_count: usize,
        /// Number of collections in the installed catalogue.
        collection_count: usize,
    },
    /// Locally cached catalogue data was used instead of a remote fetch.
    ///
    /// Recorded when the local store already has the current revision or
    /// when a cache-hit policy chooses to serve the previous revision
    /// without contacting the remote peer.
    CatalogueCachedDataUsed {
        /// The revision of the cached catalogue data that was used.
        cached_revision: u64,
    },

    // ── Blob transfer events ──────────────────────────────────────
    /// A blob transfer has started.
    TransferStarted {
        /// Short transfer identifier.
        transfer_id: String,
        /// Total expected bytes.
        total_bytes: u64,
    },
    /// A blob transfer verification step completed (or failed).
    TransferVerification {
        /// Short transfer identifier.
        transfer_id: String,
        /// Bytes received so far.
        bytes: u64,
        /// Total expected bytes.
        total_bytes: u64,
        /// Whether the verification passed.
        success: bool,
    },
    /// A blob transfer completed successfully.
    BlobTransferCompleted {
        /// Short transfer identifier.
        transfer_id: String,
        /// Total bytes transferred.
        total_bytes: u64,
        /// Content hash of the transferred data (hex).
        content_hash: String,
    },
    /// A blob transfer failed.
    BlobTransferFailed {
        /// Short transfer identifier.
        transfer_id: String,
        /// Human-readable error description.
        error: String,
    },
    /// A structured transfer lifecycle event conforming to the v1 contract
    /// (see docs/design/transfer-lifecycle-events.md).
    TransferLifecycle(TransferLifecycleEvent),
}

// =============================================================================
// TransferLifecycleEvent — structured envelope (section 1 + 3 of the contract)
// =============================================================================

/// A single transfer lifecycle event conforming to the v1 event contract
/// (see `docs/design/transfer-lifecycle-events.md`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferLifecycleEvent {
    /// Contract version.  `1` for this document.
    pub schema_version: u32,
    /// Locally generated opaque event identifier, unique for the retention
    /// period.  Not a transfer identifier.
    pub event_id: String,
    /// Stable lowercase snake_case event name (section 3).
    pub event_name: String,
    /// Short stable identifier for the logical transfer (section 2).
    pub transfer_id: String,
    /// Monotonic sequence within this transfer, starting at `0`.
    pub sequence: u64,
    /// Local Unix epoch milliseconds when the event was observed.
    pub occurred_at_ms: u64,
    /// Attempt number, starting at `1`.  A retry increments this value;
    /// pause/resume does not.
    pub attempt: u32,
    /// Event-specific payload.  Absent when the event has no optional fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

// =============================================================================
// ErrorCategory — stable bounded taxonomy (section 5 of the contract)
// =============================================================================

/// Stable error categories for transfer failure events.
///
/// The taxonomy is closed for v1.  New implementation errors must map to
/// [`Self::Unknown`] until a future contract revision adds a category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    /// Access was refused or the grant expired without authorisation.
    PermissionDenied,
    /// The requested shared object is not available on the remote peer.
    NotFound,
    /// The remote peer is offline, unreachable, or disconnected.
    PeerUnavailable,
    /// A configured transfer or operation deadline elapsed.
    Timeout,
    /// Admission or remote policy rejected the operation temporarily.
    RateLimited,
    /// Cancellation interrupted the attempt.
    Cancelled,
    /// The operation stopped because it was paused.
    Paused,
    /// Received size differs from the expected size.
    SizeMismatch,
    /// Verification failed for the received bytes.
    IntegrityMismatch,
    /// The remote version changed while the transfer was pending.
    VersionMismatch,
    /// Local temporary-file, database, or installation operation failed.
    StorageError,
    /// The peer response or transfer protocol was invalid.
    ProtocolError,
    /// Local queue, concurrency, memory, or disk limits prevented progress.
    ResourceExhausted,
    /// An error cannot safely be classified into the published categories.
    Unknown,
}

impl ErrorCategory {
    /// Return the stable lowercase identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PermissionDenied => "permission_denied",
            Self::NotFound => "not_found",
            Self::PeerUnavailable => "peer_unavailable",
            Self::Timeout => "timeout",
            Self::RateLimited => "rate_limited",
            Self::Cancelled => "cancelled",
            Self::Paused => "paused",
            Self::SizeMismatch => "size_mismatch",
            Self::IntegrityMismatch => "integrity_mismatch",
            Self::VersionMismatch => "version_mismatch",
            Self::StorageError => "storage_error",
            Self::ProtocolError => "protocol_error",
            Self::ResourceExhausted => "resource_exhausted",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stable event name strings for transfer lifecycle events.
pub mod event_names {
    /// The user selected a local file for a direct offer.
    pub const FILE_SELECTED: &str = "file_selected";
    /// The sender registered the local-only offer metadata.
    pub const OFFER_REGISTERED: &str = "offer_registered";
    /// The offer announcement was broadcast over gossip.
    pub const OFFER_BROADCAST: &str = "offer_broadcast";
    /// A receiver accepted an offer announcement.
    pub const OFFER_RECEIVED: &str = "offer_received";
    /// A receiver requested the raw direct download stream.
    pub const DIRECT_DOWNLOAD_REQUESTED: &str = "direct_download_requested";
    /// The sender wrote the first raw byte to the direct stream.
    pub const FIRST_BYTE_SENT: &str = "first_byte_sent";
    /// The receiver read the first raw byte from the direct stream.
    pub const FIRST_BYTE_RECEIVED: &str = "first_byte_received";
    /// Background blob ingestion started after the offer announcement.
    pub const BLOB_INGEST_STARTED: &str = "blob_ingest_started";
    /// Background blob ingestion completed.
    pub const BLOB_INGEST_COMPLETED: &str = "blob_ingest_completed";
    /// The BlobTicket upgrade was announced.
    pub const BLOB_TICKET_ANNOUNCED: &str = "blob_ticket_announced";

    /// The latency-critical sender path, in expected order.
    pub const OFFER_LATENCY_SEQUENCE: [&str; 3] =
        [FILE_SELECTED, OFFER_REGISTERED, OFFER_BROADCAST];

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn direct_offer_latency_sequence_is_registration_before_broadcast() {
            assert_eq!(
                OFFER_LATENCY_SEQUENCE,
                ["file_selected", "offer_registered", "offer_broadcast"]
            );
            assert_ne!(OFFER_BROADCAST, BLOB_INGEST_COMPLETED);
            assert_ne!(OFFER_BROADCAST, BLOB_TICKET_ANNOUNCED);
        }
    }

    /// A direct file offer was announced before local blob ingestion.
    pub const DIRECT_FILE_OFFER_ANNOUNCED: &str = "direct_file_offer_announced";
    /// Background local blob ingestion started after the direct offer.
    pub const BACKGROUND_BLOB_INGEST_STARTED: &str = "background_blob_ingest_started";
    /// Background local blob ingestion completed.
    pub const BACKGROUND_BLOB_INGEST_COMPLETED: &str = "background_blob_ingest_completed";
    /// Background local blob ingestion failed without invalidating the offer.
    pub const BACKGROUND_BLOB_INGEST_FAILED: &str = "background_blob_ingest_failed";
    /// Application lifecycle event: a direct file offer was announced.
    pub const FILE_OFFER_ANNOUNCED: &str = "file_offer_announced";
    /// Application lifecycle event: an announced offer was cached.
    pub const FILE_OFFER_CACHED: &str = "file_offer_cached";
    /// Application lifecycle event: caching failed without invalidating the offer.
    pub const FILE_OFFER_CACHE_FAILED: &str = "file_offer_cache_failed";
    /// Durable download work was accepted before networking was scheduled.
    pub const DOWNLOAD_QUEUED: &str = "download_queued";
    /// A fresh access/permission request was sent.
    pub const ACCESS_REQUESTED: &str = "access_requested";
    /// The access response authorised the transfer.
    pub const ACCESS_GRANTED: &str = "access_granted";
    /// Byte transfer began for the current attempt.
    pub const TRANSFER_STARTED: &str = "transfer_started";
    /// A sampled cumulative progress point.
    pub const PROGRESS_CHECKPOINT: &str = "progress_checkpoint";
    /// Work was deliberately suspended.
    pub const PAUSE: &str = "pause";
    /// A paused logical transfer was resumed.
    pub const RESUME: &str = "resume";
    /// Local size/integrity verification finished.
    pub const VERIFICATION: &str = "verification";
    /// Verified content was installed and the download reached its successful
    /// terminal state.
    pub const COMPLETION: &str = "completion";
    /// The current attempt failed.
    pub const FAILURE: &str = "failure";
    /// The transfer was cancelled and reached its terminal cancelled state.
    pub const CANCELLATION: &str = "cancellation";
}

/// Produce a short human-friendly identifier from a full id/order.
/// Useful for diagnostic logs where a full id would be too verbose.
pub fn short_transfer_id(id: impl std::fmt::Display) -> String {
    let s = id.to_string();
    if s.len() <= 8 {
        s
    } else {
        format!("{}…", &s[..8])
    }
}
