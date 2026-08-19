//! Chat log entry types.
//!
//! [`ChatEntry`] is the display-level record shared by both frontends.  It is
//! produced from protocol messages but is not itself a wire type.

use crate::chat_core::MessageHash;
use crate::chat_history::DeliveryState;
use crate::mentions::{mentions_local, Mention, MentionMember};

// ── Chat entry types ─────────────────────────────────────────────────────────

/// Whether a chat message originated locally, from a remote peer, or is a system notice.
#[derive(Clone, Debug)]
pub enum ChatKind {
    /// System notification (join/leave, errors, info).
    System,
    /// A message we sent ourselves.
    Local,
    /// A message from a remote peer.
    Remote,
}

/// A single entry in the chat log.
#[derive(Clone, Debug)]
pub struct ChatEntry {
    /// Kind of entry (system, local, remote).
    pub kind: ChatKind,
    /// Display label (e.g. nickname or "System").
    pub label: String,
    /// The message body text.
    pub body: String,
    /// Hash of the protocol message that produced this entry, when known.
    pub message_hash: Option<MessageHash>,
    /// Whether this entry has been edited after initial delivery.
    pub edited: bool,
    /// Emoji reactions attached to this entry.
    pub reactions: Vec<String>,
    /// Stable event id mapping to ChatHistoryStore entry (0 = unassigned).
    pub event_id: u64,
    /// Current delivery state of this message (only meaningful for Local kind).
    pub delivery_state: DeliveryState,
    /// Unix epoch milliseconds when this entry was created (UTC).
    /// None for entries created before this field was added.
    pub timestamp: Option<u64>,
    /// Stable peer-ID metadata for mentions in this entry.
    pub mentions: Vec<Mention>,
}

impl ChatEntry {
    /// Create a system notification entry.
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            kind: ChatKind::System,
            label: "System".to_string(),
            body: text.into(),
            message_hash: None,
            edited: false,
            reactions: Vec::new(),
            event_id: 0,
            delivery_state: DeliveryState::default(),
            timestamp: Some(crate::chat_core::now_ms()),
            mentions: Vec::new(),
        }
    }

    /// Create a local (self-sent) message entry.
    pub fn local(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind: ChatKind::Local,
            label: label.into(),
            body: text.into(),
            message_hash: None,
            edited: false,
            reactions: Vec::new(),
            event_id: 0,
            delivery_state: DeliveryState::default(),
            timestamp: Some(crate::chat_core::now_ms()),
            mentions: Vec::new(),
        }
    }

    /// Create a remote (received) message entry.
    pub fn remote(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind: ChatKind::Remote,
            label: label.into(),
            body: text.into(),
            message_hash: None,
            edited: false,
            reactions: Vec::new(),
            event_id: 0,
            delivery_state: DeliveryState::default(),
            timestamp: Some(crate::chat_core::now_ms()),
            mentions: Vec::new(),
        }
    }

    /// Attach a protocol message hash to this entry.
    pub fn with_message_hash(mut self, hash: MessageHash) -> Self {
        self.message_hash = Some(hash);
        self
    }

    /// Override the timestamp with a specific Unix epoch millisecond value.
    pub fn with_timestamp(mut self, timestamp_ms: Option<u64>) -> Self {
        self.timestamp = timestamp_ms;
        self
    }

    /// Attach structured mention metadata.
    pub fn with_mentions(mut self, mentions: Vec<Mention>) -> Self {
        self.mentions = mentions;
        self
    }

    /// Whether this entry addresses the local peer, including old text-only
    /// messages when an unambiguous room member label is available.
    pub fn mentions_local(&self, members: &[MentionMember], local_peer_id: &[u8; 32]) -> bool {
        mentions_local(&self.body, &self.mentions, members, local_peer_id)
    }

    /// Classify this entry into a semantic system-event kind.
    ///
    /// Returns `None` for non-system entries. System entries always map to a
    /// concrete [`SystemEventKind`](crate::system_events::SystemEventKind) —
    /// the mapping is total, so no incoming system message is silently
    /// discarded, and the original `body` text is never modified.
    pub fn system_event_kind(&self) -> Option<crate::system_events::SystemEventKind> {
        match self.kind {
            ChatKind::System => Some(crate::system_events::classify_system_event(&self.body)),
            ChatKind::Local | ChatKind::Remote => None,
        }
    }
}

