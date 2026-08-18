//! Boru diagnostics submodule (structural split BORU-CORE-002).

use super::*;

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
/// `AppMessage` variant (as a string summary).
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
