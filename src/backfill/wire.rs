//! Wire message types for the history backfill protocol.
//!
//! These are the serializable request/response payloads exchanged over the
//! [`BACKFILL_ALPN`](crate::backfill::BACKFILL_ALPN) stream.  They carry no
//! protocol-side effects — the authorization gate and the storage queries
//! live in [`super::server`]/[`super::authorizer`].

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::proto::TopicId;

// ── Wire messages ──────────────────────────────────────────────────────────────

/// Request for history backfill — sent by the requester.
///
/// # Security
///
/// `topic` is REQUIRED for remote requests.  The responding side rejects
/// `None` before any storage query — an unscoped remote history query is
/// never served.  The field stays `Option` on the wire for backward
/// compatibility with older clients that omit it (they are denied), while
/// new clients always send `Some(topic)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackfillRequest {
    /// Only return messages with `timestamp >= since_ms` (milliseconds since UNIX epoch).
    /// Pass `0` to get the most recent messages regardless of age.
    pub since_ms: u64,
    /// Maximum number of messages to return.
    pub max_messages: u32,
    /// The conversation topic to backfill.  `None` is rejected by the
    /// server — every remote request must name a concrete topic.
    #[serde(default)]
    pub topic: Option<TopicId>,
}

/// Response containing backfilled message bytes — sent by the responder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackfillResponse {
    /// Raw signed message bytes from the history store.
    ///
    /// Each element is a valid [`SignedMessage`] encoding that the requester
    /// can pass through [`SignedMessage::verify_and_decode`].
    pub messages: Vec<Bytes>,
    /// How many older messages were omitted due to `max_messages`.
    pub skipped: u32,
    /// Whether the response was truncated by the byte cap
    /// ([`SERVER_BACKFILL_BYTE_CAP`](crate::backfill::SERVER_BACKFILL_BYTE_CAP)).
    /// When true, the client should issue a follow-up request with a higher
    /// `since_ms` to get the remaining messages.
    #[serde(default)]
    pub truncated_by_bytes: bool,
}
