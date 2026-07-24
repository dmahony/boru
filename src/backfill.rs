//! History backfill protocol — lets late-joining peers request message history.
//!
//! # Protocol
//!
//! A peer that joins a topic and has few messages can request history from a
//! connected peer via a dedicated QUIC ALPN.  The protocol is a single
//! request/response round-trip:
//!
//! 1. Requester opens a bi-directional QUIC stream to the responder using
//!    [`BACKFILL_ALPN`].
//! 2. Requester sends a length-prefixed, postcard-encoded [`BackfillRequest`].
//! 3. Responder reads the request, queries its [`ChatHistoryStore`], and replies
//!    with a length-prefixed, postcard-encoded [`BackfillResponse`] containing
//!    the raw signed message bytes.
//! 4. Requester decodes each message through
//!    [`SignedMessage::verify_and_decode`] and feeds the result into its
//!    `NetEvent` channel as if they arrived over gossip.
//!
//! # Rate limiting
//!
//! The responding side enforces a per-peer concurrency limit: at most one
//! backfill request per remote [`PublicKey`] is served at a time.
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

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

use bytes::Bytes;
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr, PublicKey,
};
use n0_error::{bail_any, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Semaphore};
use tracing::{debug, trace, warn};

/// Timeout error message emitted when a backfill request exceeds the deadline.
const BACKFILL_TIMEOUT_MSG: &str = "backfill timed out";

use crate::chat_core::{filter_net_event_with_safety, NetEvent, SignedMessage};
use crate::chat_history::ChatHistoryStore;
use crate::proto::TopicId;
use crate::public_room_safety::PublicRoomSafety;

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
const MAX_ACTIVE_PEERS: usize = 4096;

/// Maximum number of concurrent backfill serve tasks globally.
/// Prevents resource exhaustion when many peers request backfill at once.
const MAX_CONCURRENT_BACKFILLS: usize = 32;

// ── Wire messages ──────────────────────────────────────────────────────────────

/// Request for history backfill — sent by the requester.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackfillRequest {
    /// Only return messages with `timestamp >= since_ms` (milliseconds since UNIX epoch).
    /// Pass `0` to get the most recent messages regardless of age.
    pub since_ms: u64,
    /// Maximum number of messages to return.
    pub max_messages: u32,
    /// If set, only return messages for this topic.
    #[serde(default)]
    pub topic: Option<TopicId>,
}

/// Response containing backfilled message bytes — sent by the responder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackfillResponse {
    /// Raw signed message bytes from [`ChatHistoryStore`].
    ///
    /// Each element is a valid [`SignedMessage`] encoding that the requester
    /// can pass through [`SignedMessage::verify_and_decode`].
    pub messages: Vec<Bytes>,
    /// How many older messages were omitted due to `max_messages`.
    pub skipped: u32,
    /// Whether the response was truncated by the byte cap
    /// ([`SERVER_BACKFILL_BYTE_CAP`]).  When true, the client should
    /// issue a follow-up request with a higher `since_ms` to get the
    /// remaining messages.
    #[serde(default)]
    pub truncated_by_bytes: bool,
}

// ── Per-peer rate-limiting state (server side) ─────────────────────────────────

/// Tracks in-flight backfill requests per remote peer.
#[derive(Debug, Default)]
struct BackfillRateLimit {
    active: HashMap<PublicKey, Instant>,
}

impl BackfillRateLimit {
    /// Try to register an incoming request.
    /// Returns `true` if accepted, `false` if a request from this peer is already in flight
    /// or the active set has reached [`MAX_ACTIVE_PEERS`].
    fn try_accept(&mut self, peer: PublicKey) -> bool {
        if self.active.contains_key(&peer) {
            return false;
        }
        if self.active.len() >= MAX_ACTIVE_PEERS {
            return false;
        }
        self.active.insert(peer, Instant::now());
        true
    }

    /// Remove a peer from the active set (call after request completes).
    fn release(&mut self, peer: &PublicKey) {
        self.active.remove(peer);
    }

    /// Prune stale entries (requests that hung without cleanup).
    /// Returns the number of active entries remaining after pruning.
    fn prune_stale(&mut self, max_age: std::time::Duration) -> usize {
        let now = Instant::now();
        self.active
            .retain(|_, started| now.duration_since(*started) < max_age);
        self.active.len()
    }
}

// ── Protocol handler (server side) ─────────────────────────────────────────────

/// Protocol handler for incoming backfill connections.
///
/// Register this on your [`Router`](iroh::protocol::Router):
///
/// ```ignore
/// router.accept(BACKFILL_ALPN, BackfillProtocolHandler::new(history_store.clone()));
/// ```
#[derive(Debug, Clone)]
pub struct BackfillProtocolHandler {
    /// Shared chat history store — used to respond to backfill requests.
    history_store: Arc<Mutex<ChatHistoryStore>>,
    /// Per-peer rate-limiting state.
    rate_limit: Arc<Mutex<BackfillRateLimit>>,
    /// Global concurrency cap on backfill serve tasks.
    /// Prevents resource exhaustion when many peers request backfill simultaneously.
    backfill_semaphore: Arc<Semaphore>,
}

impl BackfillProtocolHandler {
    /// Create a new handler that reads history from the given store.
    pub fn new(history_store: Arc<Mutex<ChatHistoryStore>>) -> Self {
        Self {
            history_store,
            rate_limit: Arc::new(Mutex::new(BackfillRateLimit::default())),
            backfill_semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_BACKFILLS)),
        }
    }
}

impl ProtocolHandler for BackfillProtocolHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote_id = connection.remote_id();
        debug!(
            peer = %remote_id.fmt_short(),
            "backfill: incoming connection"
        );

        // Try to acquire a global concurrency permit before proceeding.
        // If all MAX_CONCURRENT_BACKFILLS permits are taken, drop the connection
        // immediately rather than queuing.
        let permit = match self.backfill_semaphore.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                debug!(
                    peer = %remote_id.fmt_short(),
                    "backfill: concurrency cap reached ({MAX_CONCURRENT_BACKFILLS}), dropping connection"
                );
                return Ok(());
            }
        };

        let store = self.history_store.clone();
        let rate_limit = self.rate_limit.clone();

        tokio::task::spawn(async move {
            // The permit is held for the duration of the task and released
            // automatically when it (or _permit) is dropped.
            let _permit = permit;

            // Rate-limit check
            {
                let mut rl = rate_limit.lock().unwrap();
                rl.prune_stale(BACKFILL_REQUEST_TIMEOUT);
                if !rl.try_accept(remote_id) {
                    debug!(
                        peer = %remote_id.fmt_short(),
                        "backfill: rate-limited (already active or at capacity)"
                    );
                    return;
                }
            }

            let result = serve_backfill(connection, &store).await;

            // Always release the rate limit slot.
            rate_limit.lock().unwrap().release(&remote_id);

            if let Err(e) = result {
                warn!(
                    peer = %remote_id.fmt_short(),
                    "backfill: serve error: {e:#}"
                );
            }
        });

        Ok(())
    }
}

/// Read a `BackfillRequest` from the connection and send back a `BackfillResponse`.
///
/// Uses the bi-directional stream in the already-accepted connection.
/// The entire exchange is bounded by [`BACKFILL_REQUEST_TIMEOUT`] — a slow
/// or stuck peer cannot hold resources indefinitely.
async fn serve_backfill(connection: Connection, store: &Mutex<ChatHistoryStore>) -> Result<()> {
    // Enforce a hard timeout on the entire backfill exchange.
    tokio::time::timeout(BACKFILL_REQUEST_TIMEOUT, async {
        // accept_bi() returns (SendStream, RecvStream) — accepts the
        // stream the client opened, reads the request, and writes back.
        let (mut writer, mut reader) = connection
            .accept_bi()
            .await
            .map_err(|e| n0_error::anyerr!("backfill: accept_bi: {e}"))?;

        // Read the length-prefixed request from the RecvStream
        let req_len = reader
            .read_u32_le()
            .await
            .map_err(|e| n0_error::anyerr!("backfill: read req_len: {e}"))?;
        if req_len > 1024 * 1024 {
            bail_any!("backfill request too large: {req_len} bytes");
        }
        let mut req_buf = vec![0u8; req_len as usize];
        reader
            .read_exact(&mut req_buf)
            .await
            .map_err(|e| n0_error::anyerr!("backfill: read request body: {e}"))?;
        let request: BackfillRequest =
            postcard::from_bytes(&req_buf).map_err(|e| n0_error::anyerr!("decode request: {e}"))?;

        // Hard cap on max_messages — server enforces its own limit
        let max_messages = request.max_messages.min(SERVER_MAX_BACKFILL);
        trace!(
            since_ms = request.since_ms,
            requested = request.max_messages,
            capped = max_messages,
            "backfill: received request"
        );

        // Query history store for recent messages.
        let topic_filter = request.topic;
        let (resp_bytes, count) = {
            let store = store.lock().unwrap();

            // Determine the total available count for accurate `skipped`.
            let total_available = match topic_filter.as_ref() {
                Some(t) => store.count_for_topic(t),
                None => store.len(),
            };

            // Collect entries — use bounded queries where possible.
            let entries: Vec<_> = match topic_filter {
                Some(ref t) => store
                    .get_recent_messages_for_topic(t, max_messages as usize)
                    .into_iter()
                    .cloned()
                    .collect(),
                None => store
                    .get_recent_messages(max_messages as usize)
                    .into_iter()
                    .cloned()
                    .collect(),
            };

            // Apply since_ms filter and cap at max_messages (newest-first
            // for relevance, then oldest-first in the response).
            let mut filtered: Vec<Bytes> = entries
                .into_iter()
                .filter(|entry| request.since_ms == 0 || entry.timestamp >= request.since_ms)
                .rev() // newest-first so we keep the most recent within the cap
                .take(max_messages as usize)
                .map(|entry| Bytes::from(entry.signed_bytes)) // entry is owned — move
                .collect();
            filtered.reverse(); // back to chronological order

            // Enforce byte cap — truncate messages if total raw bytes exceed limit.
            let mut raw_bytes = 0usize;
            let pre_byte_count = filtered.len();
            filtered.retain(|msg| {
                if raw_bytes + msg.len() <= SERVER_BACKFILL_BYTE_CAP {
                    raw_bytes += msg.len();
                    true
                } else {
                    false
                }
            });
            let truncated_by_bytes = filtered.len() < pre_byte_count;

            // skipped: how many messages in the store were not returned.
            // Uses total_available (topic-aware) minus what we're sending.
            let skipped = total_available.saturating_sub(filtered.len()) as u32;
            let count = filtered.len();

            trace!(count, skipped, truncated_by_bytes, "backfill: sending response");

            let response = BackfillResponse {
                messages: filtered,
                skipped,
                truncated_by_bytes,
            };
            let resp_bytes = postcard::to_stdvec(&response)
                .map_err(|e| n0_error::anyerr!("encode response: {e}"))?;
            (resp_bytes, count)
        };

        debug!(count, "backfill: writing response");
        let resp_len = resp_bytes.len() as u32;

        writer
            .write_u32_le(resp_len)
            .await
            .map_err(|e| n0_error::anyerr!("backfill: write resp_len: {e}"))?;
        writer
            .write_all(&resp_bytes)
            .await
            .map_err(|e| n0_error::anyerr!("backfill: write response body: {e}"))?;
        writer
            .finish()
            .map_err(|e| n0_error::anyerr!("backfill: finish writer: {e}"))?;

        // Wait for the client to close the connection so our FIN is actually sent.
        let _ = connection.closed().await;

        Ok(())
    })
    .await
    .map_err(|_elapsed| n0_error::anyerr!("{BACKFILL_TIMEOUT_MSG}"))?
}
// ── BackfillHandle (client side) ───────────────────────────────────────────────

/// Internal commands for the backfill actor.
enum Cmd {
    RequestHistory {
        addr: EndpointAddr,
        since_ms: u64,
        max_messages: u32,
        topic: Option<TopicId>,
        net_tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
        safety: Option<Arc<PublicRoomSafety>>,
        reply: tokio::sync::oneshot::Sender<Result<u32>>,
    },
}

/// Cloneable handle for requesting history backfill from peers.
///
/// Each clone shares the same background actor that serializes backfill
/// requests — the actor ensures at most one outgoing backfill operation
/// runs at a time.
#[derive(Debug, Clone)]
pub struct BackfillHandle {
    cmd_tx: mpsc::Sender<Cmd>,
}

impl BackfillHandle {
    /// Spawn a new backfill actor and return a handle.
    ///
    /// `endpoint` is used to connect to peers.
    pub fn spawn(endpoint: Endpoint) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel(16);
        tokio::task::spawn(backfill_actor(endpoint, cmd_rx));
        Self { cmd_tx }
    }

    /// Request history from a peer over a direct QUIC connection.
    ///
    /// * `addr` — The peer's [`EndpointAddr`].
    /// * `since_ms` — UNIX-epoch milliseconds; only messages at or after this
    ///   timestamp are returned.  Pass `0` for all recent messages.
    /// * `max_messages` — Cap on how many messages to request.
    /// * `topic` — If set, only request messages for this topic.
    /// * `net_tx` — Channel to inject decoded [`NetEvent::Message`] items into.
    ///
    /// Returns the number of messages that were decoded and injected, or an
    /// error if the request failed.
    pub async fn request_history(
        &self,
        addr: EndpointAddr,
        since_ms: u64,
        max_messages: u32,
        topic: Option<TopicId>,
        net_tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
        safety: Option<Arc<PublicRoomSafety>>,
    ) -> Result<u32> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.cmd_tx
            .send(Cmd::RequestHistory {
                addr,
                since_ms,
                max_messages,
                topic,
                net_tx,
                safety,
                reply,
            })
            .await
            .map_err(|_| n0_error::anyerr!("backfill actor stopped"))?;
        rx.await
            .map_err(|_| n0_error::anyerr!("backfill actor dropped reply channel"))?
    }

    /// Trigger a backfill from a neighbor if the local history count is below
    /// [`BACKFILL_TRIGGER_THRESHOLD`].
    ///
    /// Looks up the peer's [`EndpointAddr`] from the [`Endpoint`], requests up to
    /// `DEFAULT_MAX_BACKFILL` messages, and injects them into `net_tx`.
    ///
    /// `topic` — If set, only messages for this topic are requested.
    ///
    /// Returns `Ok(Some(count))` on success, `Ok(None)` if not needed, or `Err` on failure.
    pub async fn try_backfill_from_peer(
        &self,
        endpoint: &Endpoint,
        peer: PublicKey,
        local_history_count: usize,
        topic: Option<TopicId>,
        net_tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
        safety: Option<Arc<PublicRoomSafety>>,
    ) -> Result<Option<u32>> {
        if local_history_count >= BACKFILL_TRIGGER_THRESHOLD {
            return Ok(None);
        }
        let info = match endpoint.remote_info(peer).await {
            Some(info) => info,
            None => return Ok(None),
        };
        let addr = EndpointAddr::from_parts(peer, info.into_addrs().map(|addr| addr.into_addr()));
        let count = self
            .request_history(addr, 0, DEFAULT_MAX_BACKFILL, topic, net_tx, safety)
            .await?;
        Ok(Some(count))
    }
}

/// Background actor that serializes outgoing backfill requests.
async fn backfill_actor(endpoint: Endpoint, mut cmd_rx: mpsc::Receiver<Cmd>) {
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            Cmd::RequestHistory {
                addr,
                since_ms,
                max_messages,
                topic,
                net_tx,
                safety,
                reply,
            } => {
                let result =
                    do_backfill_request(&endpoint, addr, since_ms, max_messages, topic, net_tx, safety)
                        .await;
                let _ = reply.send(result);
            }
        }
    }
}

/// Perform a backfill request against a peer, following up when the
/// response is truncated by the server-side byte cap.
///
/// Each round is a connect → request → response cycle bounded by
/// [`BACKFILL_REQUEST_TIMEOUT`].  When [`BackfillResponse::truncated_by_bytes`]
/// is true and messages were received, a follow-up request is issued with
/// `since_ms` set to the highest timestamp seen so far (duplicates are
/// handled by the dedup layer in `handle_net_event_for_topic`).
///
/// Total messages across all rounds are capped at
/// [`CLIENT_MAX_BACKFILL_MESSAGES`] for defense-in-depth.

/// Single backfill round: connect to peer, send request, read and return
/// the response.  Has an explicit return type so it can be wrapped in
/// `tokio::time::timeout` without type-inference issues.
async fn backfill_round(
    endpoint: &Endpoint,
    addr: &EndpointAddr,
    since_ms: u64,
    max_messages: u32,
    topic: Option<TopicId>,
    peer_id: PublicKey,
    round: u32,
) -> Result<(BackfillResponse, u32)> {
    debug!(
        peer = %peer_id.fmt_short(),
        round,
        since_ms,
        "backfill: connecting to peer for history"
    );

    let conn = endpoint
        .connect(addr.clone(), BACKFILL_ALPN)
        .await
        .map_err(|e| n0_error::anyerr!("backfill connect: {e}"))?;

    let (mut writer, mut reader) = conn
        .open_bi()
        .await
        .map_err(|e| n0_error::anyerr!("backfill: open_bi: {e}"))?;

    let request = BackfillRequest {
        since_ms,
        max_messages,
        topic,
    };
    let req_bytes =
        postcard::to_stdvec(&request).map_err(|e| n0_error::anyerr!("encode request: {e}"))?;
    let req_len = req_bytes.len() as u32;

    writer
        .write_u32_le(req_len)
        .await
        .map_err(|e| n0_error::anyerr!("backfill: write req_len: {e}"))?;
    writer
        .write_all(&req_bytes)
        .await
        .map_err(|e| n0_error::anyerr!("backfill: write request body: {e}"))?;
    writer
        .finish()
        .map_err(|e| n0_error::anyerr!("backfill: finish writer: {e}"))?;

    let resp_len = reader
        .read_u32_le()
        .await
        .map_err(|e| n0_error::anyerr!("backfill: read resp_len: {e}"))?;
    if resp_len > 10 * 1024 * 1024 {
        bail_any!("backfill response too large: {resp_len} bytes");
    }
    let mut resp_buf = vec![0u8; resp_len as usize];
    reader
        .read_exact(&mut resp_buf)
        .await
        .map_err(|e| n0_error::anyerr!("backfill: read response body: {e}"))?;

    let response: BackfillResponse =
        postcard::from_bytes(&resp_buf).map_err(|e| n0_error::anyerr!("decode response: {e}"))?;

    let msg_count = response.messages.len() as u32;
    debug!(
        peer = %peer_id.fmt_short(),
        round,
        count = msg_count,
        skipped = response.skipped,
        truncated_by_bytes = response.truncated_by_bytes,
        "backfill: received response, decoding and injecting"
    );

    Ok((response, msg_count))
}

async fn do_backfill_request(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    since_ms: u64,
    max_messages: u32,
    topic: Option<TopicId>,
    net_tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
    safety: Option<Arc<PublicRoomSafety>>,
) -> Result<u32> {
    let peer_id = addr.id;
    let mut total_injected = 0u32;
    let mut current_since_ms = since_ms;
    // Safety: cap the number of follow-up rounds so a malicious server
    // claiming truncation forever can't pin us in an infinite loop.
    const MAX_FOLLOW_UP_ROUNDS: u32 = 10;

    for round in 0..=MAX_FOLLOW_UP_ROUNDS {
        let remaining_cap = CLIENT_MAX_BACKFILL_MESSAGES.saturating_sub(total_injected);
        if remaining_cap == 0 {
            debug!(
                peer = %peer_id.fmt_short(),
                total_injected,
                "backfill: client message cap reached, stopping follow-ups"
            );
            break;
        }

        let (response, msg_count) = tokio::time::timeout(
            BACKFILL_REQUEST_TIMEOUT,
            backfill_round(
                endpoint,
                &addr,
                current_since_ms,
                max_messages.min(remaining_cap),
                topic,
                peer_id,
                round,
            ),
        )
        .await
        .map_err(|_elapsed| n0_error::anyerr!("{BACKFILL_TIMEOUT_MSG}"))??;

        if msg_count == 0 {
            // No messages in this round — nothing more to fetch.
            break;
        }

        // Track the highest timestamp seen in this round for the follow-up.
        let mut max_ts = current_since_ms;

        // Decode and inject each message.
        let mut round_injected = 0u32;
        for raw in &response.messages {
            if total_injected >= CLIENT_MAX_BACKFILL_MESSAGES {
                debug!(
                    peer = %peer_id.fmt_short(),
                    cap = CLIENT_MAX_BACKFILL_MESSAGES,
                    "backfill: hit client-side message cap",
                );
                break;
            }
            match SignedMessage::verify_and_decode(raw) {
                Ok((from, message, sent_at)) => {
                    max_ts = max_ts.max(sent_at);
                    let net_event = NetEvent::Message {
                        from,
                        message,
                        sent_at,
                    };
                    let net_event = match &safety {
                        Some(s) => match filter_net_event_with_safety(net_event, s) {
                            Some(ev) => ev,
                            None => {
                                trace!(
                                    "backfill: safety-filtered message from {}",
                                    peer_id.fmt_short(),
                                );
                                continue;
                            }
                        },
                        None => net_event,
                    };
                    if net_tx.send(net_event).is_err() {
                        warn!("backfill: net_tx closed, stopping injection");
                        break;
                    }
                    total_injected += 1;
                    round_injected += 1;
                }
                Err(e) => {
                    trace!("backfill: decode error for one message: {e}");
                }
            }
        }

        if !response.truncated_by_bytes {
            break;
        }

        // Prepare the next round's since_ms.  Use the max timestamp seen
        // so the server applies the same filter; duplicates are harmless
        // because handle_net_event_for_topic deduplicates by
        // (from, hash, sent_at).
        current_since_ms = max_ts;
        debug!(
            peer = %peer_id.fmt_short(),
            round,
            round_injected,
            total_injected,
            next_since_ms = current_since_ms,
            "backfill: response truncated, issuing follow-up"
        );
    }

    debug!(
        peer = %peer_id.fmt_short(),
        total_injected,
        "backfill: complete"
    );

    Ok(total_injected)
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;
    use std::time::Duration;

    #[test]
    fn backfill_request_roundtrips() {
        let req = BackfillRequest {
            since_ms: 1000,
            max_messages: 50,
            topic: None,
        };
        let bytes = postcard::to_stdvec(&req).unwrap();
        let decoded: BackfillRequest = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.since_ms, 1000);
        assert_eq!(decoded.max_messages, 50);
    }

    #[test]
    fn backfill_response_roundtrips() {
        let resp = BackfillResponse {
            messages: vec![Bytes::from(vec![1u8; 64]), Bytes::from(vec![2u8; 64])],
            skipped: 10,
            truncated_by_bytes: false,
        };
        let bytes = postcard::to_stdvec(&resp).unwrap();
        let decoded: BackfillResponse = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.messages.len(), 2);
        assert_eq!(decoded.skipped, 10);
        assert_eq!(decoded.truncated_by_bytes, false);
        assert_eq!(decoded.messages[0].as_ref(), &[1u8; 64]);
    }

    #[test]
    fn backfill_rate_limit_accept_once() {
        let mut rl = BackfillRateLimit::default();
        let pk = SecretKey::generate().public();
        assert!(rl.try_accept(pk));
        assert!(!rl.try_accept(pk));
        rl.release(&pk);
        assert!(rl.try_accept(pk));
    }

    #[test]
    fn backfill_rate_limit_multiple_peers() {
        let mut rl = BackfillRateLimit::default();
        let pk1 = SecretKey::generate().public();
        let pk2 = SecretKey::generate().public();
        assert!(rl.try_accept(pk1));
        assert!(rl.try_accept(pk2));
        assert!(!rl.try_accept(pk1));
        assert!(!rl.try_accept(pk2));
    }

    #[tokio::test]
    async fn test_backfill_handle_spawn_and_drop() {
        let ep = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(SecretKey::generate())
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await
            .expect("bind endpoint");
        let handle = BackfillHandle::spawn(ep);
        // Just verify it doesn't panic and can be dropped
        drop(handle);
    }

    /// A ProtocolHandler that delays before serving backfill.
    /// Used to test timeout behaviour.
    #[derive(Debug, Clone)]
    struct DelayedBackfillHandler {
        history_store: Arc<Mutex<ChatHistoryStore>>,
        delay: Duration,
    }

    impl ProtocolHandler for DelayedBackfillHandler {
        async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
            let store = self.history_store.clone();
            let delay = self.delay;
            tokio::task::spawn(async move {
                // Add the configured delay before processing
                tokio::time::sleep(delay).await;
                let _result = serve_backfill(connection, &store).await;
            });
            Ok(())
        }
    }

    /// Helper: create a fresh temp directory for a ChatHistoryStore.
    fn temp_store_path(name: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("backfill_test_{}", name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Test that a slow peer triggers a timeout on the client side.
    ///
    /// The responder delays for 7s (above the 5s timeout), so the
    /// client's timeout fires before the server finishes sleeping.
    #[tokio::test]
    async fn test_backfill_slow_peer_times_out() {
        // Use tokio time manipulation so the 5s timeout is instant.
        tokio::time::pause();

        let sk_responder = SecretKey::generate();
        let ep_responder = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk_responder.clone())
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await
            .expect("bind responder endpoint");

        // Empty history store — we never get to query it anyway because
        // the delay fires first on the server side.
        let data_dir = temp_store_path("slow_timeout");
        let history_store = ChatHistoryStore::empty_at(data_dir);
        // Save so the store file exists (serves backfill reads)
        let _ = history_store.save();
        let store = Arc::new(Mutex::new(history_store));
        let slow_handler = DelayedBackfillHandler {
            history_store: store.clone(),
            // Delay long enough that the client timeout fires first.
            // With paused tokio time, this is virtual time — instant in wall-clock.
            delay: Duration::from_secs(7),
        };

        let _router = iroh::protocol::Router::builder(ep_responder.clone())
            .accept(BACKFILL_ALPN, slow_handler)
            .spawn();

        let sk_requester = SecretKey::generate();
        let ep_requester = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk_requester)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await
            .expect("bind requester endpoint");

        let addr =
            EndpointAddr::from_parts(sk_responder.public(), ep_responder.addr().addrs.clone());

        let (net_tx, _) = tokio::sync::mpsc::unbounded_channel();

        // Spawn the backfill request in a background task so we can
        // advance time while it blocks waiting for the slow responder.
        // Clone the endpoint so the spawned task owns its own reference.
        let ep_for_task = ep_requester.clone();
        let handle = tokio::spawn(async move {
            do_backfill_request(&ep_for_task, addr, 0, 10, None, net_tx, None).await
        });

        // Advance time past the client's 5s timeout.  The server's 7s
        // delay hasn't expired yet, so the client's timeout fires first.
        tokio::time::advance(BACKFILL_REQUEST_TIMEOUT + Duration::from_secs(1)).await;

        let result = handle.await.expect("backfill task panicked");
        let err = result.expect_err("slow backfill should time out");
        let err_msg = err.to_string();
        assert!(
            err_msg.contains(BACKFILL_TIMEOUT_MSG),
            "expected timeout error, got: {err_msg}"
        );

        tokio::time::resume();
    }

    /// Test that a normal (fast) backfill succeeds against a handler with no delay.
    #[tokio::test]
    async fn test_backfill_normal_succeeds() {
        let sk_responder = SecretKey::generate();
        let ep_responder = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk_responder.clone())
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await
            .expect("bind responder endpoint");

        // Set up an empty history store.
        let data_dir = temp_store_path("normal_success");
        let store = Arc::new(Mutex::new(ChatHistoryStore::empty_at(data_dir.clone())));

        let handler = BackfillProtocolHandler::new(store.clone());

        let _router = iroh::protocol::Router::builder(ep_responder.clone())
            .accept(BACKFILL_ALPN, handler)
            .spawn();

        let sk_requester = SecretKey::generate();
        let ep_requester = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk_requester)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await
            .expect("bind requester endpoint");

        let addr =
            EndpointAddr::from_parts(sk_responder.public(), ep_responder.addr().addrs.clone());

        let (net_tx, _) = tokio::sync::mpsc::unbounded_channel();

        let result = do_backfill_request(&ep_requester, addr, 0, 10, None, net_tx, None).await;

        // Even with an empty store, the backfill should succeed (returning 0 messages).
        assert!(
            result.is_ok(),
            "normal backfill should succeed: {:?}",
            result.err()
        );
        let count = result.unwrap();
        assert_eq!(count, 0, "empty store should return 0 messages");
    }
}
