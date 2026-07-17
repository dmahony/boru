//! Durable chat history storage for boru-chat.
//!
//! Chat messages are persisted atomically so room history remains available
//! after restart. Outgoing messages additionally live in the durable outbox
//! until transport delivery has been observed.
//!
//! **NOTE**: Writing to `chat_history.json` has been removed.  This module
//! now serves as a migration reader and provides the [`DeliveryState`] type.
//! SQLite via [`MessageStore`](crate::store::MessageStore) is the durable
//! source of DM state.
use std::{
    fmt, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};

use crate::proto::TopicId;

/// Current schema version — bump on breaking format changes.
const SCHEMA_VERSION: u32 = 1;

/// Name of the on-disk history file (lives beside `secret_key.txt`).
pub const HISTORY_FILE_NAME: &str = "chat_history.json";

// ── Delivery state ─────────────────────────────────────────────────────

/// Delivery status of a chat message.
///
/// Messages begin in [`Queued`](DeliveryState::Queued) and proceed through
/// a directed acyclic graph of transitions:
///
/// ```text
///                            ┌──────────────────────────────────────────┐
///                            │                                          │
///                            ▼                                          │
///  Queued ──→ Sending ──→ Sent ──→ Delivered ──→ Seen                  │
///    │          │                     │               (terminal)        │
///    ├──→ Cancelled                   └──→ Failed                       │
///    │      (terminal)                       (terminal)                 │
///    ├──→ Expired                                                       │
///    │      (terminal)                                                   │
///    └──→ Failed                                                        │
///          (terminal)         ┌──────────────────────────────┐           │
///                             │                              │           │
///                    Retrying ──→ Sending (retry loop)       │           │
///                       │         │                          │           │
///                       ├──→ Failed                         │           │
///                       ├──→ Expired                        │           │
///                       └──→ Cancelled                      │           │
///                                                            └──────────┘
/// ```
///
/// Key semantic: **Delivered** means the peer's transport confirmed receipt
/// (i.e. the message reached the peer's node), while **Seen** means the
/// peer acknowledged the user actually viewed/read the message.  This
/// distinction lets frontends show a two-tick (Delivered) vs. two-blue-tick
/// (Seen) read-receipt pattern.
///
/// Terminal states (`Seen`, `Failed`, `Expired`, `Cancelled`) never
/// transition to another state (except identity no-op).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DeliveryState {
    /// Message has been composed and queued for sending (default).
    #[default]
    Queued,
    /// Message is currently being transmitted over the wire.
    Sending,
    /// Message was handed off to the transport layer (gossip / QUIC).
    Sent,
    /// Peer confirmed receipt of the message payload.
    Delivered,
    /// Peer confirmed the recipient viewed the message.
    Seen,
    /// Send failed with a retryable error; a retry is pending.
    Retrying,
    /// Non-retryable failure (permanent error).
    Failed,
    /// Message TTL exceeded before delivery completed.
    Expired,
    /// User cancelled the message before delivery.
    Cancelled,
}

impl DeliveryState {
    /// Return `true` if a transition from `self` to `other` is valid.
    ///
    /// The rules, in order:
    ///
    /// | From        | To          | Valid? |
    /// |-------------|-------------|--------|
    /// | Queued      | Sending     | ✓      |
    /// | Queued      | Sent        | ✓      |
    /// | Queued      | Failed      | ✓      |
    /// | Queued      | Expired     | ✓      |
    /// | Queued      | Cancelled   | ✓      |
    /// | Sending     | Sent        | ✓      |
    /// | Sending     | Retrying    | ✓      |
    /// | Sending     | Failed      | ✓      |
    /// | Sending     | Expired     | ✓      |
    /// | Sending     | Cancelled   | ✓      |
    /// | Sent        | Delivered   | ✓      |
    /// | Sent        | Seen        | ✓      |
    /// | Sent        | Failed      | ✓      |
    /// | Sent        | Retrying    | ✓      |
    /// | Delivered   | Seen        | ✓      |
    /// | Delivered   | Failed      | ✓      |
    /// | Retrying    | Sending     | ✓      |
    /// | Retrying    | Failed      | ✓      |
    /// | Retrying    | Expired     | ✓      |
    /// | Retrying    | Cancelled   | ✓      |
    /// | anything    | same state  | ✓ (no-op, useful for idempotent updates) |
    /// | anything    | anything else | ✗    |
    ///
    /// Terminal states (`Seen`, `Failed`, `Expired`, `Cancelled`) only
    /// accept identity transitions.
    ///
    /// A transition to the current state (identity) is always valid so
    /// callers may safely re-apply known state without error handling.
    pub fn can_transition_to(&self, other: &DeliveryState) -> bool {
        if self == other {
            return true; // identity is always a no-op
        }
        matches!(
            (self, other),
            // ── Forward path ──────────────────────────────────────────────
            (DeliveryState::Queued, DeliveryState::Sending)
                | (DeliveryState::Sending, DeliveryState::Sent)
                | (DeliveryState::Sent, DeliveryState::Delivered)
                | (DeliveryState::Sent, DeliveryState::Seen)
                | (DeliveryState::Delivered, DeliveryState::Seen)
            // ── Failure from any in-flight state ──────────────────────────
            | (DeliveryState::Queued, DeliveryState::Failed)
            | (DeliveryState::Sending, DeliveryState::Failed)
            | (DeliveryState::Sent, DeliveryState::Failed)
            | (DeliveryState::Delivered, DeliveryState::Failed)
            // ── Retry loop ────────────────────────────────────────────────
            | (DeliveryState::Sending, DeliveryState::Retrying)
            | (DeliveryState::Sent, DeliveryState::Retrying)
            | (DeliveryState::Retrying, DeliveryState::Sending)
            // ── Manual user retry (reset to Queued for re-sending) ──────
            | (DeliveryState::Retrying, DeliveryState::Queued)
            | (DeliveryState::Failed, DeliveryState::Queued)
            | (DeliveryState::Expired, DeliveryState::Queued)
            // ── Expiry / cancellation from non-terminal states ────────────
            | (DeliveryState::Queued, DeliveryState::Expired)
            | (DeliveryState::Queued, DeliveryState::Cancelled)
            | (DeliveryState::Sending, DeliveryState::Expired)
            | (DeliveryState::Sending, DeliveryState::Cancelled)
            | (DeliveryState::Retrying, DeliveryState::Failed)
            | (DeliveryState::Retrying, DeliveryState::Expired)
            | (DeliveryState::Retrying, DeliveryState::Cancelled)
        )
    }
}

impl DeliveryState {
    /// Return a short display-style icon for the delivery state.
    /// Suitable for appending to message labels in a chat UI.
    pub fn display_icon(&self) -> &'static str {
        match self {
            DeliveryState::Queued => "\u{25CC}",
            DeliveryState::Sending => "\u{2191}",
            DeliveryState::Sent => "\u{2713}",
            DeliveryState::Delivered => "\u{2713}\u{2713}",
            DeliveryState::Seen => "\u{1F441}",
            DeliveryState::Retrying => "\u{21BB}",
            DeliveryState::Failed => "\u{2717}",
            DeliveryState::Expired => "\u{231B}",
            DeliveryState::Cancelled => "\u{2298}",
        }
    }

    /// Return a concise user-facing label (no internal jargon).
    pub fn user_label(&self) -> &'static str {
        match self {
            DeliveryState::Queued => "Queued",
            DeliveryState::Sending => "Sending",
            DeliveryState::Sent => "Sent",
            DeliveryState::Delivered => "Delivered",
            DeliveryState::Seen => "Seen",
            DeliveryState::Retrying => "Retrying",
            DeliveryState::Failed => "Failed",
            DeliveryState::Expired => "Expired",
            DeliveryState::Cancelled => "Cancelled",
        }
    }

    /// Return `true` if this is a terminal state (message will never
    /// transition again automatically).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Seen | Self::Failed | Self::Expired | Self::Cancelled
        )
    }

    /// Return `true` if the user can manually retry from this state.
    pub fn can_retry(&self) -> bool {
        matches!(self, Self::Failed | Self::Retrying | Self::Expired)
    }

    /// Return `true` if the user can cancel from this state.
    pub fn can_cancel(&self) -> bool {
        matches!(self, Self::Queued | Self::Sending | Self::Retrying)
    }
}

/// Error returned when an invalid delivery-state transition is attempted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidTransition {
    /// The event id whose state could not be updated.
    pub event_id: u64,
    /// The current state.
    pub current: DeliveryState,
    /// The attempted new state.
    pub attempted: DeliveryState,
}

impl fmt::Display for InvalidTransition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid delivery-state transition for event {}: {:?} \u{2192} {:?}",
            self.event_id, self.current, self.attempted,
        )
    }
}

impl std::error::Error for InvalidTransition {}

// ── Helpers ────────────────────────────────────────────────────────────

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

fn default_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn history_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(HISTORY_FILE_NAME)
}

/// Compute the blake3 hex hash of a byte slice.
pub fn blake3_hex(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

// ── Entry type ─────────────────────────────────────────────────────────

/// One stored message in the chat history.
///
/// Each entry records a stable locally-assigned event id, the
/// content-addressed hash (blake3 of the signed message bytes), the
/// sender's public key, a millisecond-precision timestamp, a human-readable
/// message kind, a short text preview, and the current delivery state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Stable, monotonically-increasing event identifier assigned locally.
    ///
    /// Unlike the content-addressed `hash`, this id is unique per event in
    /// the local session and stable across state transitions.
    pub event_id: u64,
    /// blake3 hex hash of the raw signed message bytes.
    pub hash: String,
    /// Hex-encoded public key of the sender.
    pub sender: String,
    /// Unix-epoch milliseconds when this entry was *stored locally*.
    pub timestamp: u64,
    /// Human-readable kind: "text", "file", "about_me", "goodbye", "system".
    pub kind: String,
    /// The gossip topic this message belongs to.
    pub topic: TopicId,
    /// Short text preview (first 120 chars, or empty for binary/system).
    pub text_preview: String,
    /// The raw signed message bytes that were broadcast over gossip.
    /// Stored so peers can replay the exact bytes through
    /// `SignedMessage::verify_and_decode`.
    pub signed_bytes: Vec<u8>,
    /// Current delivery state of this message.
    #[serde(default)]
    pub delivery_state: DeliveryState,
    /// Decoded image bytes for inline rendering, if this is an image message.
    /// Stored so images persist when switching rooms within the same session.
    #[serde(skip)]
    pub image_bytes: Option<Vec<u8>>,
    /// Storage identifier returned by the [`ImageStore`](crate::image_store::ImageStore)
    /// for this image, if it was stored via the per-user image storage system.
    /// The identifier has the form `<user-hash>/<content-hash>.<ext>` and is
    /// a relative path within the store's files root — never an absolute
    /// filesystem path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_identifier: Option<String>,
}

impl HistoryEntry {
    /// Create a new history entry from the raw signed message bytes.
    ///
    /// The hash is computed as blake3 of `signed_bytes`.  `kind` and
    /// `text_preview` should be derived from the decoded `Message`.
    /// The entry starts with `event_id = 0` and `delivery_state = Queued`;
    /// use [`ChatHistoryStore::push_with_id`] to assign a proper id.
    pub fn new(
        topic: TopicId,
        sender: impl Into<String>,
        signed_bytes: Vec<u8>,
        kind: impl Into<String>,
        text_preview: impl Into<String>,
    ) -> Self {
        let hash = blake3_hex(&signed_bytes);
        Self {
            event_id: 0,
            hash,
            sender: sender.into(),
            timestamp: default_now(),
            kind: kind.into(),
            topic,
            text_preview: text_preview.into(),
            signed_bytes,
            delivery_state: DeliveryState::Queued,
            image_bytes: None,
            image_identifier: None,
        }
    }

    /// Short one-line summary for `/history` command output.
    pub fn summary(&self) -> String {
        let preview = if self.text_preview.len() > 60 {
            format!("{}\u{2026}", &self.text_preview[..60])
        } else {
            self.text_preview.clone()
        };
        format!(
            "{:16} {:8} {}",
            &self.sender[..16.min(self.sender.len())],
            self.kind,
            preview
        )
    }
}

// ── In-memory store (migration reader) ─────────────────────────────────

/// In-memory chat message entries for the active process only.
///
/// The `data_dir` field is retained only to preserve the API used by the
/// frontends; it is never used for writing chat data.
///
/// **NOTE**: `save()` has been removed.  SQLite via
/// [`MessageStore`](crate::store::MessageStore) is the durable source.
/// This type is retained for migration reading and in-memory operation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatHistoryStore {
    /// Format version for future migrations.
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    /// All stored history entries, in insertion order (oldest first).
    pub entries: Vec<HistoryEntry>,
    /// Data directory used for load/save.
    #[serde(skip)]
    data_dir: PathBuf,
    /// Monotonically-increasing counter for stable [`HistoryEntry::event_id`]
    /// assignment.  Each call to [`push_with_id`](Self::push_with_id)
    /// advances this counter and assigns the next value.
    #[serde(default)]
    next_event_id: u64,
}

impl ChatHistoryStore {
    /// Create an empty store bound to a data directory.
    pub fn empty_at(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            entries: Vec::new(),
            data_dir: data_dir.into(),
            next_event_id: 1,
        }
    }

    /// Return the on-disk history file path.
    pub fn file_path(&self) -> PathBuf {
        history_file_path(&self.data_dir)
    }

    /// Load the persisted chat history, if present.
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Option<Self>> {
        let data_dir = data_dir.as_ref();
        let path = history_file_path(data_dir);
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path)
            .with_std_context(|_| format!("failed to read history file {}", path.display()))?;
        let mut store: Self = serde_json::from_str(&raw)
            .with_std_context(|_| format!("failed to parse history file {}", path.display()))?;
        if store.schema_version != SCHEMA_VERSION {
            return Err(n0_error::anyerr!(
                "unsupported history schema version {} in {}",
                store.schema_version,
                path.display()
            ));
        }
        store.data_dir = data_dir.to_path_buf();
        store.next_event_id = store
            .entries
            .iter()
            .map(|entry| entry.event_id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        Ok(Some(store))
    }

    /// Load history, falling back to an empty store on any failure.
    pub fn load_or_default(data_dir: impl AsRef<Path>) -> Self {
        let data_dir = data_dir.as_ref();
        match Self::load(data_dir) {
            Ok(Some(store)) => store,
            Ok(None) => Self::empty_at(data_dir),
            Err(err) => {
                eprintln!(
                    "warning: failed to load chat history from {}: {err}",
                    history_file_path(data_dir).display()
                );
                Self::empty_at(data_dir)
            }
        }
    }

    /// Append a new entry.
    pub fn push(&mut self, entry: HistoryEntry) {
        self.entries.push(entry);
    }

    /// Append a new entry, assigning a stable [`event_id`](HistoryEntry::event_id).
    ///
    /// Returns the assigned event id.  The entry is initialised with
    /// [`delivery_state`](HistoryEntry::delivery_state) set to
    /// [`Queued`](DeliveryState::Queued) by [`HistoryEntry::new`].
    pub fn push_with_id(&mut self, mut entry: HistoryEntry) -> u64 {
        let id = self.next_event_id;
        self.next_event_id += 1;
        entry.event_id = id;
        self.entries.push(entry);
        id
    }

    /// Advance the [`delivery_state`](HistoryEntry::delivery_state) of an
    /// entry identified by `event_id`.
    ///
    /// Returns an [`InvalidTransition`] error if the entry cannot be found
    /// or if the transition is not allowed by the state machine (see
    /// [`DeliveryState::can_transition_to`]).
    ///
    /// A no-op transition (current → current) is always accepted and
    /// returns `Ok(())` without modifying anything.
    pub fn update_delivery_state(
        &mut self,
        event_id: u64,
        new_state: DeliveryState,
    ) -> std::result::Result<(), InvalidTransition> {
        let entry = self
            .get_by_event_id_mut(event_id)
            .ok_or_else(|| InvalidTransition {
                event_id,
                current: DeliveryState::Queued,
                attempted: new_state.clone(),
            })?;
        if entry.delivery_state.can_transition_to(&new_state) {
            entry.delivery_state = new_state;
            Ok(())
        } else {
            Err(InvalidTransition {
                event_id,
                current: entry.delivery_state.clone(),
                attempted: new_state,
            })
        }
    }

    /// Find an entry by its stable [`event_id`](HistoryEntry::event_id).
    pub fn get_by_event_id(&self, event_id: u64) -> Option<&HistoryEntry> {
        self.entries.iter().find(|e| e.event_id == event_id)
    }

    /// Find a mutable entry by its stable [`event_id`](HistoryEntry::event_id).
    pub fn get_by_event_id_mut(&mut self, event_id: u64) -> Option<&mut HistoryEntry> {
        self.entries.iter_mut().find(|e| e.event_id == event_id)
    }

    /// Return all entries (oldest first).
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Return the latest blob hash, or `None` if the history is empty.
    pub fn latest_hash(&self) -> Option<&str> {
        self.entries.last().map(|e| e.hash.as_str())
    }

    /// Number of stored entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Remove all entries for a topic.
    ///
    /// Returns the number of entries removed.
    pub fn remove_topic(&mut self, topic: &TopicId) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| e.topic != *topic);
        before - self.entries.len()
    }

    /// Return entries that are our own outgoing messages (Queued or Sent state)
    /// for a given topic, in insertion order.
    ///
    /// These are messages that should be replayed when reconnecting.
    pub fn get_outgoing_queue(&self, topic: &TopicId, local_hex: &str) -> Vec<&HistoryEntry> {
        self.entries
            .iter()
            .filter(|e| {
                e.topic == *topic
                    && e.sender == local_hex
                    && matches!(
                        e.delivery_state,
                        DeliveryState::Queued | DeliveryState::Sending | DeliveryState::Sent
                    )
            })
            .collect()
    }

    /// Find an entry by its blake3 hex hash.
    pub fn find_by_hash(&self, hash: &str) -> Option<&HistoryEntry> {
        self.entries.iter().find(|e| e.hash == hash)
    }

    /// Filter entries by topic, returning matching entries in insertion
    /// order.
    pub fn for_topic(&self, topic: &TopicId) -> Vec<&HistoryEntry> {
        self.entries.iter().filter(|e| e.topic == *topic).collect()
    }

    /// Return up to `count` of the most recent entries, oldest-first.
    pub fn get_recent_messages(&self, count: usize) -> Vec<&HistoryEntry> {
        if count == 0 {
            return Vec::new();
        }
        let start = self.entries.len().saturating_sub(count);
        self.entries[start..].iter().collect()
    }

    /// The path to the history file, for display purposes.
    pub fn display_path(&self) -> String {
        self.file_path().display().to_string()
    }
}
