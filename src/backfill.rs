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
use crate::contact::direct_topic;
use crate::proto::TopicId;
use crate::public_room::{public_lobby_topic, PublicNetwork};
use crate::public_room_safety::PublicRoomSafety;
use crate::storage::Storage;

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

// ── Authorization (server side) ───────────────────────────────────────────────

/// Centralized authorization for history backfill requests.
///
/// Decides whether a remote `peer` may currently request history for
/// `topic`.  The peer identity ALWAYS comes from the authenticated QUIC
/// connection ([`Connection::remote_id`]) — never from the request payload.
///
/// Policy:
/// - **Group epoch topics** — the peer must be an active member of the group
///   (state `Active`/`Member`/`Owner`) *and* the local node must still be an
///   active member, so a node removed from a group never serves stale group
///   history.
/// - **Direct-chat topics** — the peer must be the deterministic counterpart
///   of the local node for that topic (`direct_topic(peer, local) == topic`).
/// - **Public rooms** — the canonical public lobby and any topic advertised
///   in the public-room directory are readable by any authenticated peer.
/// - **Everything else** is denied without leaking whether the topic exists.
#[derive(Debug, Clone)]
pub struct BackfillAuthorizer {
    storage: Arc<Storage>,
    local_public: PublicKey,
}

impl BackfillAuthorizer {
    /// Create an authorizer for a node with the given local identity.
    pub fn new(storage: Arc<Storage>, local_public: PublicKey) -> Self {
        Self {
            storage,
            local_public,
        }
    }

    /// One authorization check: is `peer` currently allowed to backfill
    /// history for `topic`?
    ///
    /// This runs *before* any storage query that would reveal message
    /// IDs, counts, or metadata.  Unknown and forbidden topics both return
    /// `false` so an attacker cannot distinguish them externally.
    pub fn authorize(&self, peer: &PublicKey, topic: &TopicId) -> bool {
        // 1. Group epoch topic — membership is authoritative.  A group topic
        //    never falls through to the other checks even when the requester
        //    is not a member.
        if let Ok(Some(group)) = self.storage.find_group_by_topic(topic) {
            return is_active_group_member(&self.storage, &group.group_id, peer)
                && is_active_group_member(&self.storage, &group.group_id, &self.local_public);
        }

        // 2. Deterministic direct-chat topic — only the two participants can
        //    derive it, so the requester matching the topic IS the
        //    direct-chat relationship.
        if direct_topic(peer, &self.local_public) == *topic {
            return true;
        }

        // 3. Public-room policy — the canonical lobby and rooms advertised in
        //    the public-room directory are open to any authenticated peer.
        self.is_public_room_topic(topic)
    }

    fn is_public_room_topic(&self, topic: &TopicId) -> bool {
        if *topic == public_lobby_topic(PublicNetwork::Mainnet)
            || *topic == public_lobby_topic(PublicNetwork::Development)
            || *topic == public_lobby_topic(PublicNetwork::Test)
        {
            return true;
        }
        self.storage.is_public_room_topic(topic).unwrap_or(false)
    }
}

/// Active membership states — mirrored from the group UI filter in
/// `examples/iced_chat/app.rs` (view_group_member_list).
fn is_active_group_member(storage: &Storage, group_id: &[u8; 32], peer: &PublicKey) -> bool {
    match storage.list_group_members(group_id) {
        Ok(members) => members.iter().any(|m| {
            m.public_key.as_slice() == peer.as_bytes()
                && (m.state == "Active" || m.state == "Member" || m.state == "Owner")
        }),
        Err(_) => false,
    }
}

// ── Protocol handler (server side) ─────────────────────────────────────────────

/// Protocol handler for incoming backfill connections.
///
/// Register this on your [`Router`](iroh::protocol::Router):
///
/// ```ignore
/// router.accept(BACKFILL_ALPN, BackfillProtocolHandler::new(history_store.clone(), local_public));
/// ```
#[derive(Debug, Clone)]
pub struct BackfillProtocolHandler {
    /// Shared storage — used to respond to backfill requests.
    storage: Arc<Storage>,
    /// Centralized authorization for incoming requests.
    authorizer: BackfillAuthorizer,
    /// Per-peer rate-limiting state.
    rate_limit: Arc<Mutex<BackfillRateLimit>>,
    /// Global concurrency cap on backfill serve tasks.
    /// Prevents resource exhaustion when many peers request backfill simultaneously.
    backfill_semaphore: Arc<Semaphore>,
}

impl BackfillProtocolHandler {
    /// Create a new handler that reads history from the given storage.
    ///
    /// `local_public` is this node's own public key — it anchors the
    /// direct-chat authorization check ([`direct_topic`]) and is never
    /// taken from a request.
    pub fn new(storage: Arc<Storage>, local_public: PublicKey) -> Self {
        Self {
            authorizer: BackfillAuthorizer::new(storage.clone(), local_public),
            storage,
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

        let store = self.storage.clone();
        let authorizer = self.authorizer.clone();
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

            let result = serve_backfill(connection, &store, &authorizer).await;

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
///
/// # Authorization
///
/// A concrete topic is mandatory and the remote peer (from the connection
/// context, never the payload) must be authorized for it before any storage
/// query runs.  Unauthorized requests are rejected with a generic error
/// that does not reveal whether the topic exists or how much history it has.
async fn serve_backfill(
    connection: Connection,
    storage: &Storage,
    authorizer: &BackfillAuthorizer,
) -> Result<()> {
    // Enforce a hard timeout on the entire backfill exchange.
    tokio::time::timeout(BACKFILL_REQUEST_TIMEOUT, async {
        // accept_bi() returns (SendStream, RecvStream) — accepts the
        // stream the client opened, reads the request, and writes back.
        let (mut writer, mut reader) = connection
            .accept_bi()
            .await
            .map_err(|e| n0_error::anyerr!("backfill: accept_bi: {e}"))?;

        let remote_id = connection.remote_id();

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

        // Authorization gate — runs before any storage query.  A remote
        // request without a concrete topic is never served.  The
        // authorization queries read SQLite; run them on the blocking pool
        // so the QUIC accept worker is never stalled (BORU-AUDIT-18).
        let topic = match request.topic {
            Some(t) => t,
            None => {
                warn!(
                    peer = %remote_id.fmt_short(),
                    "backfill: denied — request omitted topic"
                );
                bail_any!("backfill: topic required");
            }
        };
        let authorized = {
            let authorizer = authorizer.clone();
            let remote_id = remote_id;
            let topic = topic;
            tokio::task::spawn_blocking(move || authorizer.authorize(&remote_id, &topic))
                .await
                .map_err(|join_err| {
                    n0_error::anyerr!("backfill: authorize worker panicked: {join_err}")
                })?
        };
        if !authorized {
            // Audit log: remote peer id + safe topic identifier only.
            // Message contents are never logged.
            warn!(
                peer = %remote_id.fmt_short(),
                topic = %topic.fmt_short(),
                "backfill: denied — peer not authorized for topic"
            );
            bail_any!("backfill: unauthorized");
        }

        // Hard cap on max_messages — server enforces its own limit
        let max_messages = request.max_messages.min(SERVER_MAX_BACKFILL);
        trace!(
            since_ms = request.since_ms,
            requested = request.max_messages,
            capped = max_messages,
            "backfill: received request"
        );

        // Query storage for recent messages for the authorized topic.
        let (resp_bytes, count) = {
            // Determine the total available count for accurate `skipped`.
            // SQLite read — run on the blocking pool so the QUIC accept
            // worker is never stalled (BORU-AUDIT-18).
            let total_available = storage
                .run_blocking("backfill.count_messages", {
                    let topic = topic;
                    move |s| {
                        s.count_chat_messages_for_topic(&topic)
                            .map_err(|e| anyhow::anyhow!("{e:#}"))
                    }
                })
                .await
                .unwrap_or(0);

            // Collect entries — bounded topic query only; the unscoped
            // recent-history query is never reachable from the network.
            let entries: Vec<_> = storage
                .run_blocking("backfill.recent_messages", {
                    let topic = topic;
                    move |s| {
                        s.get_recent_chat_messages_for_topic(&topic, max_messages as usize)
                            .map_err(|e| anyhow::anyhow!("{e:#}"))
                    }
                })
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(ts, bytes)| (ts, bytes))
                .collect();

            // Apply since_ms filter and cap at max_messages (newest-first
            // for relevance, then oldest-first in the response).
            let mut filtered: Vec<Bytes> = entries
                .into_iter()
                .filter(|(timestamp, _)| request.since_ms == 0 || *timestamp >= request.since_ms)
                .rev() // newest-first so we keep the most recent within the cap
                .take(max_messages as usize)
                .map(|(_, signed_bytes)| Bytes::from(signed_bytes))
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

            trace!(
                count,
                skipped,
                truncated_by_bytes,
                "backfill: sending response"
            );

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
        topic: TopicId,
        net_tx: mpsc::Sender<NetEvent>,
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
    /// * `topic` — The conversation topic to request history for.  A concrete
    ///   topic is required — the server rejects unscoped requests.
    /// * `net_tx` — Channel to inject decoded [`NetEvent::Message`] items into.
    ///
    /// Returns the number of messages that were decoded and injected, or an
    /// error if the request failed.
    pub async fn request_history(
        &self,
        addr: EndpointAddr,
        since_ms: u64,
        max_messages: u32,
        topic: TopicId,
        net_tx: mpsc::Sender<NetEvent>,
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
    /// `topic` — The conversation topic to request history for.  A concrete
    /// topic is required — the server rejects unscoped requests.
    ///
    /// Returns `Ok(Some(count))` on success, `Ok(None)` if not needed, or `Err` on failure.
    pub async fn try_backfill_from_peer(
        &self,
        endpoint: &Endpoint,
        peer: PublicKey,
        local_history_count: usize,
        topic: TopicId,
        net_tx: mpsc::Sender<NetEvent>,
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
                let result = do_backfill_request(
                    &endpoint,
                    addr,
                    since_ms,
                    max_messages,
                    topic,
                    net_tx,
                    safety,
                )
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
///
/// Single backfill round: connect to peer, send request, read and return
/// the response.  Has an explicit return type so it can be wrapped in
/// `tokio::time::timeout` without type-inference issues.
async fn backfill_round(
    endpoint: &Endpoint,
    addr: &EndpointAddr,
    since_ms: u64,
    max_messages: u32,
    topic: TopicId,
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
        topic: Some(topic),
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
    topic: TopicId,
    net_tx: mpsc::Sender<NetEvent>,
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
                        message: message.clone(),
                        sent_at,
                    };
                    crate::chat_core::remember_signed_message(from, &message, sent_at, raw);
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
                    if net_tx.send(net_event).await.is_err() {
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
    use crate::chat_core::Message;
    use crate::storage::{GroupEpochRow, GroupMemberRow, GroupRow};
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
        assert!(!decoded.truncated_by_bytes);
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

    /// The GUI has no scroll-triggered pagination: history is loaded
    /// wholesale on open, and backfill is network-driven, gated by
    /// [`BACKFILL_TRIGGER_THRESHOLD`].  This pins the gate itself — when the
    /// local history count meets the threshold no request is made (and no
    /// network round trip is attempted), and an unknown peer below the
    /// threshold degrades to `Ok(None)` rather than erroring.
    #[tokio::test]
    async fn try_backfill_skips_when_history_at_or_above_threshold() {
        let ep = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(SecretKey::generate())
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await
            .expect("bind endpoint");
        let handle = BackfillHandle::spawn(ep.clone());
        let peer = SecretKey::generate().public();
        let topic = TopicId::from_bytes([0u8; 32]);
        let (net_tx, _net_rx) = mpsc::channel(16);

        // At exactly the threshold: no backfill request.
        let at = handle
            .try_backfill_from_peer(
                &ep,
                peer,
                BACKFILL_TRIGGER_THRESHOLD,
                topic,
                net_tx.clone(),
                None,
            )
            .await
            .expect("threshold skip is not an error");
        assert_eq!(at, None, "at threshold → no backfill request");

        // Above the threshold: no backfill request.
        let above = handle
            .try_backfill_from_peer(
                &ep,
                peer,
                BACKFILL_TRIGGER_THRESHOLD + 10,
                topic,
                net_tx.clone(),
                None,
            )
            .await
            .expect("above-threshold skip is not an error");
        assert_eq!(above, None, "above threshold → no backfill request");

        // Below the threshold but no known route to the peer: Ok(None), not an
        // error — the caller simply gets no history this round.
        let below = handle
            .try_backfill_from_peer(&ep, peer, 0, topic, net_tx.clone(), None)
            .await
            .expect("unknown-peer below threshold degrades gracefully");
        assert_eq!(below, None, "no route → no backfill performed");
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
        storage: Arc<Storage>,
        authorizer: BackfillAuthorizer,
        delay: Duration,
    }

    impl ProtocolHandler for DelayedBackfillHandler {
        async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
            let storage = self.storage.clone();
            let authorizer = self.authorizer.clone();
            let delay = self.delay;
            tokio::task::spawn(async move {
                // Add the configured delay before processing
                tokio::time::sleep(delay).await;
                let _result = serve_backfill(connection, &storage, &authorizer).await;
            });
            Ok(())
        }
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

        // Empty SQLite storage — we never get to query it anyway because
        // the delay fires first on the server side.
        let storage = Arc::new(Storage::memory().unwrap());
        let slow_handler = DelayedBackfillHandler {
            storage: storage.clone(),
            authorizer: BackfillAuthorizer::new(storage.clone(), sk_responder.public()),
            // Delay long enough that the client timeout fires first.
            // With paused tokio time, this is virtual time — instant in wall-clock.
            delay: Duration::from_secs(7),
        };

        let _router = iroh::protocol::Router::builder(ep_responder.clone())
            .accept(BACKFILL_ALPN, slow_handler)
            .spawn();

        let sk_requester = SecretKey::generate();
        let ep_requester = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk_requester.clone())
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await
            .expect("bind requester endpoint");

        let addr =
            EndpointAddr::from_parts(sk_responder.public(), ep_responder.addr().addrs.clone());

        // Authorized direct-chat topic between requester and responder, so the
        // only reason the request fails is the server-side delay.
        let topic = direct_topic(&sk_requester.public(), &sk_responder.public());

        let (net_tx, _) = tokio::sync::mpsc::channel(64);

        // Spawn the backfill request in a background task so we can
        // advance time while it blocks waiting for the slow responder.
        // Clone the endpoint so the spawned task owns its own reference.
        let ep_for_task = ep_requester.clone();
        let handle = tokio::spawn(async move {
            do_backfill_request(&ep_for_task, addr, 0, 10, topic, net_tx, None).await
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

        // Set up an empty SQLite storage.
        let storage = Arc::new(Storage::memory().unwrap());

        let handler = BackfillProtocolHandler::new(storage.clone(), sk_responder.public());

        let _router = iroh::protocol::Router::builder(ep_responder.clone())
            .accept(BACKFILL_ALPN, handler)
            .spawn();

        let sk_requester = SecretKey::generate();
        let ep_requester = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk_requester.clone())
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await
            .expect("bind requester endpoint");

        let addr =
            EndpointAddr::from_parts(sk_responder.public(), ep_responder.addr().addrs.clone());

        // The requester is the direct-chat counterpart of the responder, so
        // authorization passes; the store is empty so 0 messages return.
        let topic = direct_topic(&sk_requester.public(), &sk_responder.public());

        let (net_tx, _) = tokio::sync::mpsc::channel(64);

        let result = do_backfill_request(&ep_requester, addr, 0, 10, topic, net_tx, None).await;

        // Even with an empty store, the backfill should succeed (returning 0 messages).
        assert!(
            result.is_ok(),
            "normal backfill should succeed: {:?}",
            result.err()
        );
        let count = result.unwrap();
        assert_eq!(count, 0, "empty store should return 0 messages");
    }

    // ── Authorization regression tests (BORU-AUDIT-01) ─────────────────

    /// Spawn a responder endpoint with the real backfill handler over the
    /// given storage; returns the responder's address and the kept-alive
    /// router.
    async fn spawn_responder(
        storage: Arc<Storage>,
        sk: &SecretKey,
    ) -> (EndpointAddr, iroh::protocol::Router) {
        let ep = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk.clone())
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await
            .expect("bind responder endpoint");
        let handler = BackfillProtocolHandler::new(storage.clone(), sk.public());
        let router = iroh::protocol::Router::builder(ep.clone())
            .accept(BACKFILL_ALPN, handler)
            .spawn();
        let addr = EndpointAddr::from_parts(sk.public(), ep.addr().addrs.clone());
        (addr, router)
    }

    /// Spawn a fresh requester endpoint and return it plus its public key.
    async fn spawn_requester() -> (Endpoint, PublicKey) {
        let sk = SecretKey::generate();
        (spawn_requester_with(&sk).await, sk.public())
    }

    /// Spawn a requester endpoint keyed by a specific secret key.
    async fn spawn_requester_with(sk: &SecretKey) -> Endpoint {
        Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk.clone())
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await
            .expect("bind requester endpoint")
    }

    /// Storage with one group (owner = responder, member = member_sk), one
    /// epoch topic, and one signed message from the member in that topic.
    fn make_group_storage(local_sk: &SecretKey, member_sk: &SecretKey) -> (Arc<Storage>, TopicId, [u8; 32]) {
        let storage = Arc::new(Storage::memory().unwrap());
        let group_id = [7u8; 32];
        let topic = TopicId::from_bytes([0xAB; 32]);
        storage
            .create_group(&GroupRow {
                group_id,
                name: "AuditGroup".into(),
                description: String::new(),
                owner_public_key: local_sk.public().as_bytes().to_vec(),
                current_epoch: 1,
                created_at_ms: 1,
                updated_at_ms: 1,
                archived: false,
            })
            .unwrap();
        storage
            .create_group_epoch(&GroupEpochRow {
                group_id,
                epoch: 1,
                topic_id: topic,
                discovery_secret: vec![1u8; 32],
                created_at_ms: 1,
            })
            .unwrap();
        let add_member = |pk: &PublicKey, role: &str, state: &str| {
            storage
                .add_group_member(&GroupMemberRow {
                    group_id,
                    public_key: pk.as_bytes().to_vec(),
                    role: role.into(),
                    joined_at_ms: 1,
                    invited_by: None,
                    epoch_joined: 1,
                    state: state.into(),
                })
                .unwrap();
        };
        add_member(&local_sk.public(), "Owner", "Owner");
        add_member(&member_sk.public(), "Member", "Active");
        let signed = SignedMessage::sign_and_encode(
            member_sk,
            &Message::Message {
                text: "audit hello".into(),
            },
        )
        .unwrap();
        storage
            .insert_chat_message(
                &[1u8; 32],
                &topic,
                member_sk.public().as_bytes(),
                1000,
                &signed,
            )
            .unwrap();
        (storage, topic, group_id)
    }

    /// Raw length-prefixed backfill exchange — lets a test send a request
    /// the normal client API can no longer produce (e.g. `topic: None`).
    /// Returns the decoded response or the transport-level error observed.
    async fn raw_backfill_request(
        ep: &Endpoint,
        addr: EndpointAddr,
        request: &BackfillRequest,
    ) -> Result<BackfillResponse, String> {
        let conn = ep
            .connect(addr, BACKFILL_ALPN)
            .await
            .map_err(|e| e.to_string())?;
        let (mut writer, mut reader) = conn.open_bi().await.map_err(|e| e.to_string())?;
        let req_bytes = postcard::to_stdvec(request).map_err(|e| e.to_string())?;
        writer
            .write_u32_le(req_bytes.len() as u32)
            .await
            .map_err(|e| e.to_string())?;
        writer
            .write_all(&req_bytes)
            .await
            .map_err(|e| e.to_string())?;
        writer.finish().map_err(|e| e.to_string())?;
        let resp_len = reader
            .read_u32_le()
            .await
            .map_err(|e| format!("read response: {e}"))?;
        if resp_len > 10 * 1024 * 1024 {
            return Err("response too large".into());
        }
        let mut buf = vec![0u8; resp_len as usize];
        reader
            .read_exact(&mut buf)
            .await
            .map_err(|e| e.to_string())?;
        postcard::from_bytes(&buf).map_err(|e| e.to_string())
    }

    /// Regression: a remote request with `topic = None` is rejected before
    /// any DB query.  The storage holds a message that an unscoped query
    /// would return — the client must observe a transport error instead.
    #[tokio::test]
    async fn backfill_rejects_request_without_topic() {
        let sk_responder = SecretKey::generate();
        let storage = Arc::new(Storage::memory().unwrap());
        let other = SecretKey::generate();
        let signed = SignedMessage::sign_and_encode(
            &other,
            &Message::Message {
                text: "private".into(),
            },
        )
        .unwrap();
        storage
            .insert_chat_message(
                &[2u8; 32],
                &TopicId::from_bytes([0xCD; 32]),
                other.public().as_bytes(),
                1,
                &signed,
            )
            .unwrap();

        let (addr, _router) = spawn_responder(storage, &sk_responder).await;
        let (ep, _pk) = spawn_requester().await;

        let result = raw_backfill_request(
            &ep,
            addr,
            &BackfillRequest {
                since_ms: 0,
                max_messages: 10,
                topic: None,
            },
        )
        .await;
        assert!(
            result.is_err(),
            "topic=None must be rejected, got a response: {result:?}"
        );
    }

    /// Regression: only active group members may backfill a group topic.
    /// Non-members and removed members are denied with zero message
    /// metadata; an active member receives the seeded history.
    #[tokio::test]
    async fn backfill_authorizes_group_membership() {
        let sk_responder = SecretKey::generate();
        let sk_member = SecretKey::generate();
        let (storage, topic, group_id) = make_group_storage(&sk_responder, &sk_member);
        let (addr, _router) = spawn_responder(storage.clone(), &sk_responder).await;

        // Outsider (never a member) → denied.
        let (ep_outsider, _) = spawn_requester().await;
        let result = do_backfill_request(
            &ep_outsider,
            addr.clone(),
            0,
            50,
            topic,
            mpsc::channel(64).0,
            None,
        )
        .await;
        assert!(
            result.is_err(),
            "non-member must be denied, got: {result:?}"
        );

        // Active member → succeeds and receives the seeded message.  The
        // requester endpoint must be keyed by the member's own identity.
        let ep_member = spawn_requester_with(&sk_member).await;
        let (net_tx, mut net_rx) = mpsc::channel(64);
        let result = do_backfill_request(&ep_member, addr.clone(), 0, 50, topic, net_tx, None)
            .await;
        assert!(
            result.is_ok(),
            "member backfill should succeed: {:?}",
            result.err()
        );
        assert_eq!(
            result.unwrap(),
            1,
            "member should receive the seeded message"
        );
        assert!(
            net_rx.recv().await.is_some(),
            "member should receive a decoded NetEvent"
        );

        // Former member after removal → denied immediately.
        storage
            .remove_group_member(&group_id, sk_member.public().as_bytes(), "Removed")
            .unwrap();
        let result2 = do_backfill_request(
            &ep_member,
            addr,
            0,
            50,
            topic,
            mpsc::channel(64).0,
            None,
        )
        .await;
        assert!(
            result2.is_err(),
            "removed member must be denied, got: {result2:?}"
        );
    }

    /// Regression: authorization is re-checked on every request.  A first
    /// page does not grant a permanent capability — after membership
    /// revocation the next (continued) page is denied.
    #[tokio::test]
    async fn backfill_rechecks_authorization_on_next_page() {
        let sk_responder = SecretKey::generate();
        let sk_member = SecretKey::generate();
        let (storage, topic, group_id) = make_group_storage(&sk_responder, &sk_member);
        let (addr, _router) = spawn_responder(storage.clone(), &sk_responder).await;
        let ep = spawn_requester_with(&sk_member).await;

        // Page 1 (since=0): authorized member succeeds.
        let page1 =
            do_backfill_request(&ep, addr.clone(), 0, 50, topic, mpsc::channel(64).0, None).await;
        assert!(
            page1.is_ok(),
            "first page should succeed: {:?}",
            page1.err()
        );

        // Revocation between pages.
        storage
            .remove_group_member(&group_id, sk_member.public().as_bytes(), "Removed")
            .unwrap();

        // Page 2 (continued since_ms): denied immediately.
        let page2 =
            do_backfill_request(&ep, addr, 1000, 50, topic, mpsc::channel(64).0, None).await;
        assert!(
            page2.is_err(),
            "next page after revocation must be denied: {page2:?}"
        );
    }

    /// Regression: unknown topics and forbidden topics are externally
    /// indistinguishable — both are denied with no response body, so an
    /// attacker cannot probe for topic existence or history size.
    #[tokio::test]
    async fn backfill_unknown_and_forbidden_topics_look_identical() {
        let sk_responder = SecretKey::generate();
        let sk_member = SecretKey::generate();
        let (storage, topic, _group_id) = make_group_storage(&sk_responder, &sk_member);
        let (addr, _router) = spawn_responder(storage, &sk_responder).await;
        let (ep, _) = spawn_requester().await;

        // Unknown topic: no local record at all.
        let unknown = TopicId::from_bytes([0xEE; 32]);
        // Forbidden topic: a real group the requester is not a member of.
        let forbidden = topic;

        let unknown_res = raw_backfill_request(
            &ep,
            addr.clone(),
            &BackfillRequest {
                since_ms: 0,
                max_messages: 10,
                topic: Some(unknown),
            },
        )
        .await;
        let forbidden_res = raw_backfill_request(
            &ep,
            addr,
            &BackfillRequest {
                since_ms: 0,
                max_messages: 10,
                topic: Some(forbidden),
            },
        )
        .await;

        assert!(
            unknown_res.is_err(),
            "unknown topic must be denied: {unknown_res:?}"
        );
        assert!(
            forbidden_res.is_err(),
            "forbidden topic must be denied: {forbidden_res:?}"
        );
        // Both fail at the same stage (reading the response that never
        // comes) — identical external error behavior.
        let unknown_msg = unknown_res.unwrap_err();
        let forbidden_msg = forbidden_res.unwrap_err();
        assert!(
            unknown_msg.starts_with("read response"),
            "unknown failure should be a response-read error: {unknown_msg}"
        );
        assert!(
            forbidden_msg.starts_with("read response"),
            "forbidden failure should be a response-read error: {forbidden_msg}"
        );
    }

    /// Regression: a direct-chat topic is only readable by its two
    /// participants.  The requester that matches the deterministic topic is
    /// authorized; a third party guessing the topic is denied.
    #[tokio::test]
    async fn backfill_direct_chat_only_authorizes_participants() {
        let sk_responder = SecretKey::generate();
        let storage = Arc::new(Storage::memory().unwrap());
        let (addr, _router) = spawn_responder(storage, &sk_responder).await;

        let sk_peer = SecretKey::generate();
        let topic = direct_topic(&sk_peer.public(), &sk_responder.public());

        // The peer IS a participant in this direct topic — build the
        // requester endpoint from the participant's key.
        let ep_peer = spawn_requester_with(&sk_peer).await;
        let res =
            do_backfill_request(&ep_peer, addr.clone(), 0, 10, topic, mpsc::channel(64).0, None)
                .await;
        assert!(
            res.is_ok(),
            "direct participant should be authorized: {:?}",
            res.err()
        );

        // A third party requesting the same topic is denied.
        let (ep_outsider, _) = spawn_requester().await;
        let res_out =
            do_backfill_request(&ep_outsider, addr, 0, 10, topic, mpsc::channel(64).0, None).await;
        assert!(
            res_out.is_err(),
            "non-participant must be denied: {res_out:?}"
        );
    }

    /// Regression: the canonical public lobby is readable by any
    /// authenticated peer (public-room policy).
    #[tokio::test]
    async fn backfill_public_lobby_is_open_to_any_peer() {
        let sk_responder = SecretKey::generate();
        let storage = Arc::new(Storage::memory().unwrap());
        let (addr, _router) = spawn_responder(storage, &sk_responder).await;
        let (ep, _) = spawn_requester().await;

        let lobby = public_lobby_topic(PublicNetwork::Mainnet);
        let res = do_backfill_request(&ep, addr, 0, 10, lobby, mpsc::channel(64).0, None).await;
        assert!(
            res.is_ok(),
            "public lobby must be readable by any peer: {:?}",
            res.err()
        );
        assert_eq!(res.unwrap(), 0, "empty lobby store returns no messages");
    }
}
