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
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use crate::chat_core::{NetEvent, SignedMessage};
use crate::chat_history::ChatHistoryStore;

// ── Constants ──────────────────────────────────────────────────────────────────

/// ALPN for history backfill connections.
pub const BACKFILL_ALPN: &[u8] = b"/iroh-gossip-chat/backfill/1";

/// Default maximum number of messages to return in one backfill response.
pub const DEFAULT_MAX_BACKFILL: u32 = 100;

/// Threshold: request backfill from a neighbor when we have fewer than this
/// many messages in our local log.
pub const BACKFILL_TRIGGER_THRESHOLD: usize = 20;

/// Timeout for a single backfill request/response exchange.
pub const BACKFILL_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

// ── Wire messages ──────────────────────────────────────────────────────────────

/// Request for history backfill — sent by the requester.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackfillRequest {
    /// Only return messages with `timestamp >= since_ms` (milliseconds since UNIX epoch).
    /// Pass `0` to get the most recent messages regardless of age.
    pub since_ms: u64,
    /// Maximum number of messages to return.
    pub max_messages: u32,
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
}

// ── Per-peer rate-limiting state (server side) ─────────────────────────────────

/// Tracks in-flight backfill requests per remote peer.
#[derive(Debug, Default)]
struct BackfillRateLimit {
    active: HashMap<PublicKey, Instant>,
}

impl BackfillRateLimit {
    /// Try to register an incoming request.
    /// Returns `true` if accepted, `false` if a request from this peer is already in flight.
    fn try_accept(&mut self, peer: PublicKey) -> bool {
        if self.active.contains_key(&peer) {
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
    fn prune_stale(&mut self, max_age: std::time::Duration) {
        let now = Instant::now();
        self.active
            .retain(|_, started| now.duration_since(*started) < max_age);
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
}

impl BackfillProtocolHandler {
    /// Create a new handler that reads history from the given store.
    pub fn new(history_store: Arc<Mutex<ChatHistoryStore>>) -> Self {
        Self {
            history_store,
            rate_limit: Arc::new(Mutex::new(BackfillRateLimit::default())),
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

        let store = self.history_store.clone();
        let rate_limit = self.rate_limit.clone();

        tokio::task::spawn(async move {
            // Rate-limit check
            {
                let mut rl = rate_limit.lock().unwrap();
                rl.prune_stale(BACKFILL_REQUEST_TIMEOUT);
                if !rl.try_accept(remote_id) {
                    debug!(
                        peer = %remote_id.fmt_short(),
                        "backfill: rate-limited (already active)"
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
/// `open_bi()` on an accepted connection returns `(SendStream, RecvStream)`
/// where SendStream implements `AsyncWrite` and RecvStream implements `AsyncRead`.
async fn serve_backfill(connection: Connection, store: &Mutex<ChatHistoryStore>) -> Result<()> {
    // open_bi() returns (SendStream, RecvStream).
    // SendStream = writer (AsyncWrite), RecvStream = reader (AsyncRead).
    let (mut writer, mut reader) = connection
        .open_bi()
        .await
        .map_err(|e| n0_error::anyerr!("backfill: open_bi: {e}"))?;

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

    trace!(
        since_ms = request.since_ms,
        max = request.max_messages,
        "backfill: received request"
    );

    // Query history store for recent messages.
    let (resp_bytes, count) = {
        let store = store.lock().unwrap();
        let recent_entries = store.get_recent_messages(request.max_messages as usize);
        let messages: Vec<Bytes> = recent_entries
            .into_iter()
            .filter(|entry| request.since_ms == 0 || entry.timestamp >= request.since_ms)
            .map(|entry| Bytes::from(entry.signed_bytes.clone()))
            .collect();
        let skipped = store.len().saturating_sub(messages.len()) as u32;
        let count = messages.len();

        trace!(count, skipped, "backfill: sending response");

        // Encode the response while still under the lock (sync, no await).
        let response = BackfillResponse { messages, skipped };
        let resp_bytes = postcard::to_stdvec(&response)
            .map_err(|e| n0_error::anyerr!("encode response: {e}"))?;
        (resp_bytes, count)
    }; // MutexGuard drops here, before any await.

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

    Ok(())
}

// ── BackfillHandle (client side) ───────────────────────────────────────────────

/// Internal commands for the backfill actor.
enum Cmd {
    RequestHistory {
        addr: EndpointAddr,
        since_ms: u64,
        max_messages: u32,
        net_tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
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
    /// * `net_tx` — Channel to inject decoded [`NetEvent::Message`] items into.
    ///
    /// Returns the number of messages that were decoded and injected, or an
    /// error if the request failed.
    pub async fn request_history(
        &self,
        addr: EndpointAddr,
        since_ms: u64,
        max_messages: u32,
        net_tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
    ) -> Result<u32> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.cmd_tx
            .send(Cmd::RequestHistory {
                addr,
                since_ms,
                max_messages,
                net_tx,
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
    /// Returns `Ok(Some(count))` on success, `Ok(None)` if not needed, or `Err` on failure.
    pub async fn try_backfill_from_peer(
        &self,
        endpoint: &Endpoint,
        peer: PublicKey,
        local_history_count: usize,
        net_tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
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
            .request_history(addr, 0, DEFAULT_MAX_BACKFILL, net_tx)
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
                net_tx,
                reply,
            } => {
                let result =
                    do_backfill_request(&endpoint, addr, since_ms, max_messages, net_tx).await;
                let _ = reply.send(result);
            }
        }
    }
}

/// Perform a single backfill request: connect, send request, read response,
/// decode messages, and inject them into the net_tx channel.
async fn do_backfill_request(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    since_ms: u64,
    max_messages: u32,
    net_tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
) -> Result<u32> {
    let peer_id = addr.id;
    debug!(
        peer = %peer_id.fmt_short(),
        "backfill: connecting to peer for history"
    );

    // endpoint.connect() returns a Result<Connection, ConnectionError>.
    // The .await completes the full handshake.
    let conn = endpoint
        .connect(addr, BACKFILL_ALPN)
        .await
        .map_err(|e| n0_error::anyerr!("backfill connect: {e}"))?;

    // open_bi() returns (SendStream, RecvStream)
    let (mut writer, mut reader) = conn
        .open_bi()
        .await
        .map_err(|e| n0_error::anyerr!("backfill: open_bi: {e}"))?;

    // Send request on SendStream
    let request = BackfillRequest {
        since_ms,
        max_messages,
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

    // Read response from RecvStream
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

    let count = response.messages.len() as u32;
    debug!(
        peer = %peer_id.fmt_short(),
        count,
        skipped = response.skipped,
        "backfill: received response, decoding and injecting"
    );

    // Decode each signed message and inject into net_tx
    let mut injected = 0u32;
    for raw in &response.messages {
        match SignedMessage::verify_and_decode(raw) {
            Ok((from, message, sent_at)) => {
                if net_tx
                    .send(NetEvent::Message {
                        from,
                        message,
                        sent_at,
                    })
                    .is_err()
                {
                    warn!("backfill: net_tx closed, stopping injection");
                    break;
                }
                injected += 1;
            }
            Err(e) => {
                trace!("backfill: decode error for one message: {e}");
                // Skip corrupt messages but keep going
            }
        }
    }

    debug!(
        peer = %peer_id.fmt_short(),
        injected,
        "backfill: complete"
    );

    Ok(injected)
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    #[test]
    fn backfill_request_roundtrips() {
        let req = BackfillRequest {
            since_ms: 1000,
            max_messages: 50,
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
        };
        let bytes = postcard::to_stdvec(&resp).unwrap();
        let decoded: BackfillResponse = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.messages.len(), 2);
        assert_eq!(decoded.skipped, 10);
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
}
