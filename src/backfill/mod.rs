//! History backfill protocol — lets late-joining peers request message history.
//!
//! # Protocol
//!
//! A peer that joins a topic and has few messages can request history from a
//! connected peer via a dedicated QUIC ALPN.  The protocol is a single
//! request/response round-trip:
//!
//! 1. Requester opens a bi-directional QUIC stream to the responder using
//!    [`BACKFILL_ALPN`](crate::backfill::BACKFILL_ALPN).
//! 2. Requester sends a length-prefixed, postcard-encoded [`BackfillRequest`](crate::backfill::BackfillRequest).
//! 3. Responder reads the request, queries its [`Storage`](crate::storage::Storage), and replies
//!    with a length-prefixed, postcard-encoded [`BackfillResponse`](crate::backfill::BackfillResponse) containing
//!    the raw signed message bytes.
//! 4. Requester decodes each message through
//!    [`SignedMessage::verify_and_decode`](crate::chat_core::SignedMessage::verify_and_decode) and feeds the result into its
//!    `NetEvent` channel as if they arrived over gossip.
//!
//! # Authorization
//!
//! Every remote request must name a concrete topic and pass the
//! [`BackfillAuthorizer`](crate::backfill::BackfillAuthorizer) gate, which checks the authenticated connection
//! peer against the topic's conversation type: active group membership,
//! the deterministic direct-chat pairing, or the public-room policy.
//! Unauthorized and unknown topics receive an identical generic denial
//! before any storage query runs.
//!
//! # Rate limiting
//!
//! The responding side enforces a per-peer concurrency limit: at most one
//! backfill request per remote [`PublicKey`](iroh::PublicKey) is served at a time.
//!
//! # Wire format
//!
//! Every message on the wire is length-prefixed:
//! - 4 bytes: little-endian `u32` payload length (excluding these 4 bytes)
//! - N bytes: postcard-encoded payload
//!
//! # Feature flag
//!
//! This module is behind the `net` feature flag.
//!
//! # Layout
//!
//! This module is a facade over the backfill engine:
//! - [`wire`]      – wire message types ([`BackfillRequest`], [`BackfillResponse`])
//! - [`authorizer`]– authorization gate ([`BackfillAuthorizer`], active-membership check)
//! - [`rate_limit`]– per-peer rate-limiting state ([`BackfillRateLimit`])
//! - [`server`]    – server-side protocol handler + `serve_backfill`
//! - [`client`]    – client-side [`BackfillHandle`] + actor + request rounds
//!
//! The public surface is [`BackfillHandle`], [`BackfillProtocolHandler`],
//! [`BackfillAuthorizer`], the wire types, and the ALPN constant
//! [`BACKFILL_ALPN`].

mod authorizer;
mod client;
mod rate_limit;
mod server;
mod wire;

#[cfg(test)]
pub(crate) mod tests;

pub use authorizer::BackfillAuthorizer;
pub use client::BackfillHandle;
pub use server::BackfillProtocolHandler;
pub use wire::{BackfillRequest, BackfillResponse};

/// Timeout error message emitted when a backfill request exceeds the deadline.
pub(crate) const BACKFILL_TIMEOUT_MSG: &str = "backfill timed out";

// ── Constants ──────────────────────────────────────────────────────────────────

/// ALPN for history backfill connections.
pub const BACKFILL_ALPN: &[u8] = b"/iroh-gossip-chat/backfill/1";

/// Default maximum number of messages to return in one backfill response.
pub const DEFAULT_MAX_BACKFILL: u32 = 50;

/// Threshold: request backfill from a neighbor when we have fewer than this
/// many messages in our local log.
pub const BACKFILL_TRIGGER_THRESHOLD: usize = 20;

/// Timeout for a single backfill request/response exchange.
pub const BACKFILL_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Server-enforced maximum messages per backfill response.
///
/// The requester may ask for any number via `max_messages`, but the server
/// caps it at this value.  Prevents one peer from requesting arbitrarily
/// large message batches.
pub const SERVER_MAX_BACKFILL: u32 = 50;

/// Server-enforced maximum serialized response size in bytes.
///
/// If the encoded response exceeds this, the server truncates the message
/// list before sending.  Prevents a single response from consuming
/// excessive memory or network resources.
pub const SERVER_BACKFILL_BYTE_CAP: usize = 2 * 1024 * 1024; // 2 MiB

/// Client-side cap on the number of messages to decode and inject from a
/// single backfill response.  Defense-in-depth: even if a misbehaving server
/// sends more, the client stops after this many messages.
pub const CLIENT_MAX_BACKFILL_MESSAGES: u32 = 50;

/// Maximum number of unique peers tracked in the backfill rate-limit map.
/// Prevents unbounded growth when many unique peers connect simultaneously.
/// Matches the `MAX_TRACKED_PEERS` pattern from `public_room_safety.rs`.
pub(crate) const MAX_ACTIVE_PEERS: usize = 4096;

/// Maximum number of concurrent backfill serve tasks globally.
/// Prevents resource exhaustion when many peers request backfill at once.
pub(crate) const MAX_CONCURRENT_BACKFILLS: usize = 32;
