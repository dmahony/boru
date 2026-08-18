//! Client side of the backfill protocol: the [`BackfillHandle`] plus the
//! background actor and request rounds it drives.
//!
//! The actor serializes outgoing backfill operations (at most one in flight
//! at a time).  Request/response framing lives here; the wire types are in
//! [`super::wire`].

use std::sync::Arc;

use iroh::{Endpoint, EndpointAddr, PublicKey};
use n0_error::{bail_any, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use super::{
    wire::{BackfillRequest, BackfillResponse},
    BACKFILL_ALPN, BACKFILL_REQUEST_TIMEOUT, BACKFILL_TIMEOUT_MSG, BACKFILL_TRIGGER_THRESHOLD,
    CLIENT_MAX_BACKFILL_MESSAGES, DEFAULT_MAX_BACKFILL,
};
use crate::chat_core::{filter_net_event_with_safety, NetEvent, SignedMessage};
use crate::proto::TopicId;
use crate::public_room_safety::PublicRoomSafety;

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

/// Perform the actual backfill exchange (called by the actor).
pub(crate) async fn do_backfill_request(
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
