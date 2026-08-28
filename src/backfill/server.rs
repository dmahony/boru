#![allow(clippy::shadow_unrelated, clippy::redundant_locals)]

//! Server side of the backfill protocol: the protocol handler and the
//! per-connection `serve_backfill` exchange.
//!
//! Authorization lives in [`super::authorizer`], per-peer rate limiting in
//! [`super::rate_limit`], and the wire types in [`super::wire`].  This module
//! only wires them together on an accepted QUIC connection.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    PublicKey,
};
use n0_error::{bail_any, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;
use tracing::{debug, trace, warn};

use super::{
    rate_limit::BackfillRateLimit,
    wire::{BackfillRequest, BackfillResponse},
    BACKFILL_REQUEST_TIMEOUT, BACKFILL_TIMEOUT_MSG, MAX_CONCURRENT_BACKFILLS,
    SERVER_BACKFILL_BYTE_CAP, SERVER_MAX_BACKFILL,
};
use crate::backfill::authorizer::BackfillAuthorizer;
use crate::storage::Storage;

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
pub(crate) async fn serve_backfill(
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
