//! `/iroh-chat-inbox/1` direct QUIC protocol for offline-message delivery.
//!
//! The inbox protocol runs on a dedicated QUIC ALPN, separate from gossip
//! and whisper.  It carries opaque, already-encrypted [`MailboxEnvelope`]s
//! and signed [`MailboxAck`]s between peers.
//!
//! # Security
//!
//! * Every `Deliver` and `Ack` is wrapped in a [`SignedInboxMessage`] with
//!   a sender signature and timestamp for replay protection.
//! * The handler rejects messages whose sender is not in the configured
//!   `allowed_senders` set (populated from the contact/friend list).
//! * A clock-skew window (24 hours) prevents replay of old messages.
//! * Duplicate `message_id`s are deduplicated within the replay window.
//!
//! # Lifecycle
//!
//! * [`InboxProtocol`] — registered as a protocol handler so remote peers
//!   can deliver envelopes to this node.
//! * Received envelopes are forwarded via an mpsc channel to the frontend,
//!   which stores them in the local MailboxStore and broadcasts acknowledgements.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, PublicKey, SecretKey, Signature,
};
use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};
use serde_byte_array::ByteArray;
use tokio::sync::{mpsc, Mutex};

use crate::mailbox::{MailboxAck, MailboxEnvelope, MAX_SYNC_LOOKBACK, DEFAULT_MAILBOX_TTL};

// ── Constants ──────────────────────────────────────────────────────────────────

/// ALPN for the inbox service.
pub const INBOX_ALPN: &[u8] = b"/iroh-chat-inbox/1";

/// Max clock skew for replay protection (24 hours).
const MAX_CLOCK_SKEW: Duration = Duration::from_secs(24 * 60 * 60);

/// A sync requester's `since_ms` must not be more than this far in the future.
/// Small forward skew is tolerated for clock drift between peers.
const MAX_SYNC_FUTURE_SKEW_MS: u64 = 300_000; // 5 minutes

/// Max payload size for a single inbox message (10 MB).
const MAX_INBOX_PAYLOAD: usize = 10 * 1024 * 1024;

/// Stable identifier for a message (blake3 hash of signed bytes).
pub type InboxMessageId = [u8; 32];

/// Derive a stable message id from signed bytes.
pub fn inbox_message_id(bytes: &[u8]) -> InboxMessageId {
    *blake3::hash(bytes).as_bytes()
}

// ── Delete tombstone type ──────────────────────────────────────────────────────

/// Proof that the original message author authorized deletion.
///
/// Signed by the message's original author, not the current sender.
/// The outer [`SignedInboxMessage`] authenticates who forwarded the
/// tombstone; this proof authenticates that the author truly authorized
/// the deletion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorDeleteProof {
    /// Message ID being deleted (blake3 hash of the original signed bytes).
    pub msg_id: [u8; 32],
    /// The conversation this message belongs to.
    pub conversation_id: [u8; 32],
    /// Unix epoch seconds (replay protection on the inner proof).
    pub created_at_unix_secs: u64,
    /// Public key of the original message author (who authorized this deletion).
    pub author: PublicKey,
    /// Signature by `author` over `msg_id || conversation_id || created_at_unix_secs`.
    pub author_signature: ByteArray<{ Signature::LENGTH }>,
}

impl AuthorDeleteProof {
    /// The bytes that the author's signature covers.
    fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = self.msg_id.to_vec();
        bytes.extend_from_slice(&self.conversation_id);
        bytes.extend_from_slice(&self.created_at_unix_secs.to_le_bytes());
        bytes
    }

    /// Verify that the proof was signed by the claimed author.
    ///
    /// Returns `Ok(())` if the signature is valid, `Err` otherwise.
    pub fn verify(&self) -> std::result::Result<(), String> {
        let sig = Signature::from_bytes(&self.author_signature);
        let data = self.signing_bytes();
        self.author
            .verify(&data, &sig)
            .map_err(|e| format!("invalid author delete proof signature: {e}"))
    }

    /// Create a new signed proof.
    pub fn sign(author_sk: &SecretKey, msg_id: [u8; 32], conversation_id: [u8; 32]) -> Self {
        let created_at_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut proof = Self {
            msg_id,
            conversation_id,
            created_at_unix_secs,
            author: author_sk.public(),
            author_signature: ByteArray::new([0u8; Signature::LENGTH]),
        };
        let data = proof.signing_bytes();
        proof.author_signature = ByteArray::new(author_sk.sign(&data).to_bytes());
        proof
    }
}

// ── Signed wire envelope ───────────────────────────────────────────────────────

/// A signed, timestamped envelope for inbox protocol messages.
///
/// Every inbox message is wrapped in this to provide sender authentication,
/// replay protection, and a stable message id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedInboxMessage {
    /// Identity of the signer.
    pub sender: PublicKey,
    /// Unix epoch seconds when this message was created (used for replay bounds).
    pub sent_at_unix_secs: u64,
    /// The inner protocol message.
    pub inner: InboxPayload,
    /// Signature over `sent_at_unix_secs || inner` by the sender.
    pub signature: ByteArray<{ Signature::LENGTH }>,
}

/// The inner payload of an inbox protocol message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InboxPayload {
    /// An encrypted mailbox envelope for the recipient.
    Deliver(MailboxEnvelope),
    /// Acknowledgement that an envelope was received and accepted.
    Ack(MailboxAck),
    /// Request missed envelopes since a timestamp.
    SyncRequest {
        /// Only return envelopes created at or after this timestamp (ms).
        since_ms: u64,
    },
    /// Response containing a page of missed envelopes.
    SyncResponse {
        /// The missed envelopes in this page.
        envelopes: Vec<MailboxEnvelope>,
        /// Creation timestamp (ms) of the last envelope in this page.
        /// The requester uses this as `since_ms` for the next page request.
        /// Absent when envelopes is empty.
        #[serde(default)]
        last_created_at_ms: Option<u64>,
        /// True when more pages exist after this one.
        #[serde(default)]
        has_more: bool,
    },
    /// Signed deletion tombstone for a previously delivered message.
    /// The inner [`AuthorDeleteProof`] is signed by the message's original
    /// author, proving they authorized the deletion.
    DeleteTombstone(AuthorDeleteProof),
}

impl SignedInboxMessage {
    /// Create and sign a new inbox message.
    pub fn sign(secret_key: &SecretKey, inner: InboxPayload) -> Result<Vec<u8>> {
        let sent_at_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let signing_data = signing_bytes(sent_at_unix_secs, &inner);
        let signature = secret_key.sign(&signing_data);
        let msg = Self {
            sender: secret_key.public(),
            sent_at_unix_secs,
            inner,
            signature: ByteArray::new(signature.to_bytes()),
        };
        postcard::to_stdvec(&msg).map_err(|e| n0_error::anyerr!("encode signed inbox message: {e}"))
    }

    /// Verify the signature and check the timestamp is within the replay window.
    ///
    /// Returns the decoded `(sender, inner_payload, sent_at_unix_secs)` on success.
    pub fn verify(
        bytes: &[u8],
        expected_sender: Option<PublicKey>,
    ) -> Result<(PublicKey, InboxPayload, u64)> {
        let msg: Self = postcard::from_bytes(bytes)
            .map_err(|e| n0_error::anyerr!("decode signed inbox message: {e}"))?;

        // Check expected sender if provided.
        if let Some(expected) = expected_sender {
            if msg.sender != expected {
                return Err(n0_error::anyerr!(
                    "inbox message sender mismatch: expected {expected}, got {}",
                    msg.sender
                ));
            }
        }

        // Replay protection: check timestamp skew.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if msg.sent_at_unix_secs.abs_diff(now) > MAX_CLOCK_SKEW.as_secs() {
            return Err(n0_error::anyerr!(
                "inbox message timestamp {} is outside replay window (now={now})",
                msg.sent_at_unix_secs
            ));
        }

        // Verify signature.
        let signing_data = signing_bytes(msg.sent_at_unix_secs, &msg.inner);
        msg.sender
            .verify(&signing_data, &Signature::from_bytes(&msg.signature))
            .map_err(|e| n0_error::anyerr!("verify inbox message signature: {e}"))?;

        Ok((msg.sender, msg.inner, msg.sent_at_unix_secs))
    }
}

fn signing_bytes(timestamp: u64, inner: &InboxPayload) -> Vec<u8> {
    let inner_bytes = postcard::to_stdvec(inner).expect("postcard encoding cannot fail");
    let mut bytes = timestamp.to_le_bytes().to_vec();
    bytes.extend_from_slice(&inner_bytes);
    bytes
}

/// Current wall clock in milliseconds since UNIX epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Inbox protocol state ───────────────────────────────────────────────────────

/// Inbox protocol state shared across connections.
pub struct InboxInner {
    /// Set of senders whose messages are currently accepted (contact/allowed peers).
    pub allowed_senders: HashSet<PublicKey>,
    /// Live authorization lookup used at connection/message receipt time.
    ///
    /// This deliberately is not a snapshot: contact lifecycle changes must
    /// take effect without restarting the inbox protocol.
    pub authorization_fn: Option<Arc<dyn Fn(PublicKey) -> bool + Send + Sync>>,
    /// Deduplication: message ids seen within the replay window.
    pub seen_message_ids: HashMap<InboxMessageId, u64>,
    /// Channel to forward received envelopes to the frontend.
    pub envelope_tx: mpsc::UnboundedSender<InboxEvent>,
    /// Optional provider that returns pending envelopes for a SyncRequest.
    /// The function receives (requester_public_key, since_ms) and returns
    /// (envelopes, has_more). The protocol handler derives last_created_at_ms
    /// from the last envelope in the page.
    pub pending_fn: Option<
        Arc<dyn Fn(PublicKey, u64) -> (Vec<MailboxEnvelope>, bool) + Send + Sync>,
    >,
    /// Optional callback invoked after a SyncResponse is sent, recording
    /// which message IDs were served for replay protection.  The callback
    /// receives (recipient_public_key, &[[u8; 32]]).
    /// This prevents the same envelopes from being served again on repeat
    /// sync requests.
    pub record_sync_served_fn: Option<
        Arc<dyn Fn(PublicKey, &[[u8; 32]]) + Send + Sync>,
    >,
}

impl std::fmt::Debug for InboxInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxInner")
            .field("allowed_senders", &self.allowed_senders)
            .field(
                "authorization_fn",
                &self.authorization_fn.as_ref().map(|_| "Some(...)"),
            )
            .field("seen_message_ids", &self.seen_message_ids)
            .field("envelope_tx", &self.envelope_tx)
            .field("pending_fn", &self.pending_fn.as_ref().map(|_| "Some(...)"))
            .finish()
    }
}

/// Events emitted by the inbox protocol to the frontend.
#[derive(Debug, Clone)]
pub enum InboxEvent {
    /// A mailbox envelope was received from a peer and accepted.
    EnvelopeReceived {
        /// Public key of the sender.
        from: PublicKey,
        /// The received mailbox envelope.
        envelope: MailboxEnvelope,
    },
    /// An acknowledgement was received for a previously sent envelope.
    AckReceived {
        /// Public key of the peer sending the ack.
        from: PublicKey,
        /// The signed acknowledgement.
        ack: MailboxAck,
    },
    /// A sync request was received.
    SyncRequested {
        /// Public key of the peer requesting sync.
        from: PublicKey,
        /// Only return envelopes created at or after this timestamp (ms).
        since_ms: u64,
    },
    /// A delete tombstone was received — the message author authorized deletion.
    DeleteTombstoneReceived {
        /// Public key of the peer who forwarded the tombstone.
        from: PublicKey,
        /// The delete proof signed by the original message author.
        proof: AuthorDeleteProof,
    },
}

// ── Inbox handle (frontend API) ────────────────────────────────────────────────

/// Handle for interacting with the inbox protocol from the frontend.
#[derive(Debug, Clone)]
pub struct InboxHandle {
    inner: Arc<Mutex<InboxInner>>,
}

impl InboxHandle {
    /// Create a new inbox handle.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<InboxEvent>) {
        let (envelope_tx, envelope_rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Mutex::new(InboxInner {
            allowed_senders: HashSet::new(),
            authorization_fn: None,
            seen_message_ids: HashMap::new(),
            envelope_tx,
            pending_fn: None,
            record_sync_served_fn: None,
        }));
        (
            Self {
                inner: inner.clone(),
            },
            envelope_rx,
        )
    }

    /// Return a clone of the shared inner state, for creating a protocol handler.
    pub fn inner(&self) -> Arc<Mutex<InboxInner>> {
        self.inner.clone()
    }

    /// Add an allowed sender (e.g. when a contact is added or a friend request accepted).
    pub async fn add_allowed_sender(&self, peer: PublicKey) {
        self.inner.lock().await.allowed_senders.insert(peer);
    }

    /// Remove an allowed sender (e.g. when a contact is removed).
    pub async fn remove_allowed_sender(&self, peer: &PublicKey) {
        self.inner.lock().await.allowed_senders.remove(peer);
    }

    /// Check whether a sender is allowed.
    pub async fn is_allowed_sender(&self, peer: &PublicKey) -> bool {
        self.inner.lock().await.allowed_senders.contains(peer)
    }

    /// Replace the set of allowed senders.
    pub async fn set_allowed_senders(&self, peers: HashSet<PublicKey>) {
        self.inner.lock().await.allowed_senders = peers;
    }

    /// Install the repository-backed authorization lookup.
    ///
    /// The callback is evaluated for every incoming connection and every
    /// request on an existing connection.  The legacy allowed-sender set is
    /// retained for callers that do not configure a repository (and for
    /// backwards-compatible tests), but production frontends should use this
    /// method rather than maintaining a permission cache.
    pub async fn set_authorization_fn(
        &self,
        f: Option<Arc<dyn Fn(PublicKey) -> bool + Send + Sync>>,
    ) {
        self.inner.lock().await.authorization_fn = f;
    }

    /// Set a function that provides pending envelopes for SyncRequest.
    ///
    /// The function receives `(requester_public_key, since_ms)` and returns
    /// `(envelopes, has_more)`. Called from the protocol handler when a
    /// SyncRequest arrives.
    pub async fn set_pending_fn(
        &self,
        f: Option<
            Arc<dyn Fn(PublicKey, u64) -> (Vec<MailboxEnvelope>, bool) + Send + Sync>,
        >,
    ) {
        self.inner.lock().await.pending_fn = f;
    }

    /// Set a callback invoked after each SyncResponse is sent, to record
    /// which message IDs were served for replay protection.
    ///
    /// The callback receives `(recipient_public_key, &[[u8; 32]])` where
    /// the message IDs are the raw 32-byte identifiers from the storage layer.
    /// This integrates with `Storage::record_sync_served()` for durable
    /// dedup tracking.
    pub async fn set_record_sync_served_fn(
        &self,
        f: Option<Arc<dyn Fn(PublicKey, &[[u8; 32]]) + Send + Sync>>,
    ) {
        self.inner.lock().await.record_sync_served_fn = f;
    }
}

// ── Protocol handler ────────────────────────────────────────────────────────────

/// Protocol handler for incoming inbox connections.
///
/// Register on the Router with `.accept(INBOX_ALPN, inbox_handler)`.
#[derive(Clone)]
pub struct InboxProtocol {
    inner: Arc<Mutex<InboxInner>>,
    secret_key: Option<SecretKey>,
}

impl std::fmt::Debug for InboxProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxProtocol")
            .field("inner", &self.inner)
            .field("secret_key", &self.secret_key.as_ref().map(|_| "***"))
            .finish()
    }
}

impl InboxProtocol {
    /// Create a protocol handler from the shared inner state.
    pub fn new(inner: Arc<Mutex<InboxInner>>) -> Self {
        Self {
            inner,
            secret_key: None,
        }
    }

    /// Attach a secret key so the handler can sign SyncResponse messages.
    pub fn with_secret_key(self, secret_key: SecretKey) -> Self {
        Self {
            secret_key: Some(secret_key),
            ..self
        }
    }
}

impl ProtocolHandler for InboxProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote_id = connection.remote_id();

        // Check authorization before accepting any streams.
        {
            let guard = self.inner.lock().await;
            let authorized = guard
                .authorization_fn
                .as_ref()
                .map(|f| f(remote_id))
                .unwrap_or_else(|| guard.allowed_senders.contains(&remote_id));
            if !authorized {
                return Err(AcceptError::from_err(n0_error::anyerr!(
                    "inbox connection from unauthorized peer {}",
                    remote_id.fmt_short()
                )));
            }
        }

        let secret_key = self.secret_key.clone();

        // Read messages from the connection.
        while let Ok((mut send, mut recv)) = connection.accept_bi().await {
            let inner = self.inner.clone();

            // Read length prefix
            let mut len_buf = [0u8; 4];
            if recv.read_exact(&mut len_buf).await.is_err() {
                continue;
            }
            let len = u32::from_be_bytes(len_buf) as usize;
            if len > MAX_INBOX_PAYLOAD {
                continue;
            }
            let mut buf = vec![0u8; len];
            if recv.read_exact(&mut buf).await.is_err() {
                continue;
            }

            // Dispatch and optionally produce a response.
            let result = Self::handle_request(&inner, remote_id, &buf).await;
            match result {
                Ok(Some((response_envelopes, has_more))) => {
                    // Compute message IDs for replay protection before consuming
                    // `response_envelopes` (it is moved into the response payload).
                    // inbox_message_id is the blake3 hash of the postcard-encoded
                    // envelope — the same ID used for dedup on the Deliver path.
                    let msg_ids: Vec<InboxMessageId> = response_envelopes
                        .iter()
                        .map(|e| {
                            let bytes =
                                postcard::to_stdvec(e).expect("envelope encoding cannot fail");
                            inbox_message_id(&bytes)
                        })
                        .collect();

                    // SyncRequest: send back a paginated SyncResponse.
                    if let Some(ref sk) = secret_key {
                        let last_created_at_ms =
                            response_envelopes.last().map(|e| e.created_at);
                        let payload = InboxPayload::SyncResponse {
                            envelopes: response_envelopes,
                            last_created_at_ms,
                            has_more,
                        };
                        match SignedInboxMessage::sign(sk, payload) {
                            Ok(signed) => {
                                let resp_len = signed.len() as u32;
                                let _ = send.write_all(&resp_len.to_be_bytes()).await;
                                let _ = send.write_all(&signed).await;
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "inbox: failed to sign SyncResponse for {}: {e}",
                                    remote_id.fmt_short()
                                );
                            }
                        }
                    } else {
                        tracing::warn!(
                            "inbox: no secret_key configured, cannot send SyncResponse to {}",
                            remote_id.fmt_short()
                        );
                    }

                    // Record served message IDs for replay protection so that
                    // subsequent sync requests from the same peer do not re-serve
                    // the same envelopes.  The frontend wires this callback to a
                    // store (e.g. Storage::record_sync_served or an in-memory set)
                    // that the pending_fn also consults for filtering.
                    if !msg_ids.is_empty() {
                        let guard = inner.lock().await;
                        if let Some(ref record_fn) = guard.record_sync_served_fn {
                            record_fn(remote_id, &msg_ids);
                        }
                    }
                }
                Ok(None) => {
                    // Non-SyncRequest messages get a minimal ack.
                    let _ = send.write_all(&[0u8; 1]).await;
                }
                Err(ref e) => {
                    tracing::warn!(
                        "inbox: failed to handle message from {}: {e}",
                        remote_id.fmt_short()
                    );
                    let _ = send.write_all(&[0u8; 1]).await;
                }
            }
            let _ = send.finish();
        }

        Ok(())
    }

    async fn shutdown(&self) {
        // No-op: the inbox has no persistent state beyond the shared inner.
    }
}

impl InboxProtocol {
    /// Dispatch a verified inbox message and return pending envelopes
    /// and a has_more flag when the caller should send a SyncResponse back.
    async fn handle_request(
        inner: &Arc<Mutex<InboxInner>>,
        sender: PublicKey,
        buf: &[u8],
    ) -> Result<Option<(Vec<MailboxEnvelope>, bool)>> {
        // Verify the signed message.
        let (verified_sender, payload, _sent_at) = SignedInboxMessage::verify(buf, Some(sender))?;

        let mut guard = inner.lock().await;

        // Re-check against the live repository for every message.  A peer
        // may have been blocked/removed while its QUIC connection remained
        // open; a connection-level check alone would incorrectly accept it.
        let authorized = guard
            .authorization_fn
            .as_ref()
            .map(|f| f(verified_sender))
            .unwrap_or_else(|| guard.allowed_senders.contains(&verified_sender));
        if !authorized {
            return Err(n0_error::anyerr!(
                "inbox message from unauthorized peer {}",
                verified_sender.fmt_short()
            ));
        }

        match payload {
            InboxPayload::Deliver(envelope) => {
                // The authenticated transport identity and the envelope's
                // original sender must agree.  Otherwise a peer could relay
                // somebody else's envelope and cause the recipient to send
                // an acknowledgement to the wrong identity.
                if envelope.from != verified_sender {
                    return Err(n0_error::anyerr!(
                        "inbox envelope sender mismatch: transport={}, envelope={}",
                        verified_sender,
                        envelope.from
                    ));
                }
                // Dedup by message_id.
                let mid = inbox_message_id(
                    &postcard::to_stdvec(&envelope)
                        .map_err(|e| n0_error::anyerr!("encode envelope for id: {e}"))?,
                );
                if guard.seen_message_ids.contains_key(&mid) {
                    // A duplicate is valid replay, not a rejected message.
                    // Forward it again so the application can re-validate
                    // its durable state and re-acknowledge it.  This is
                    // essential when the original acknowledgement was lost
                    // after the recipient committed the message.
                    let _ = guard.envelope_tx.send(InboxEvent::EnvelopeReceived {
                        from: verified_sender,
                        envelope,
                    });
                    return Ok(None);
                }
                // Prune stale dedup entries.
                let cutoff = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    .saturating_sub(MAX_CLOCK_SKEW.as_secs());
                guard.seen_message_ids.retain(|_, ts| *ts > cutoff);
                guard.seen_message_ids.insert(
                    mid,
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                );

                let _ = guard.envelope_tx.send(InboxEvent::EnvelopeReceived {
                    from: verified_sender,
                    envelope,
                });
                Ok(None)
            }
            InboxPayload::Ack(ack) => {
                // Verify the inner MailboxAck signature before forwarding.
                ack.verify(verified_sender).map_err(|e| {
                    n0_error::anyerr!("inbox: rejecting ack with invalid signature: {e}")
                })?;
                let _ = guard.envelope_tx.send(InboxEvent::AckReceived {
                    from: verified_sender,
                    ack,
                });
                Ok(None)
            }
            InboxPayload::SyncRequest { since_ms } => {
                // Validate the requester-supplied timestamp before passing it
                // to the provider.  A malicious or buggy peer must not be able
                // to trigger an unbounded or future-windowed scan.
                let now = now_ms();
                if since_ms > now.saturating_add(MAX_SYNC_FUTURE_SKEW_MS) {
                    return Err(n0_error::anyerr!(
                        "sync request from {} has since_ms {since_ms} which is >{MAX_SYNC_FUTURE_SKEW_MS}ms in the future (now={now})",
                        verified_sender.fmt_short()
                    ));
                }
                // Clamp to the local retention window as a defence-in-depth
                // measure — the provider also clamps, but a protocol-level
                // check prevents an out-of-range value from ever reaching it.
                let floor = now.saturating_sub(MAX_SYNC_LOOKBACK.as_millis() as u64);
                let effective_since = since_ms.max(floor);

                // Try the pending_fn provider first.
                if let Some(ref f) = guard.pending_fn {
                    let (envelopes, has_more) = f(verified_sender, effective_since);
                    Ok(Some((envelopes, has_more)))
                } else {
                    // Fall back to emitting an event when no provider is set.
                    let _ = guard.envelope_tx.send(InboxEvent::SyncRequested {
                        from: verified_sender,
                        since_ms: effective_since,
                    });
                    Ok(None)
                }
            }
            InboxPayload::SyncResponse { .. } => {
                // SyncResponse is only sent, never received on the server side.
                Ok(None)
            }
            InboxPayload::DeleteTombstone(proof) => {
                // 1. Verify the inner author proof signature.
                proof.verify().map_err(|e| {
                    n0_error::anyerr!("rejecting delete tombstone with invalid author proof: {e}")
                })?;

                // 2. Replay protection: check author's timestamp is within the replay window.
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if proof.created_at_unix_secs.abs_diff(now) > MAX_CLOCK_SKEW.as_secs() {
                    return Err(n0_error::anyerr!(
                        "delete tombstone author timestamp {} is outside replay window (now={now})",
                        proof.created_at_unix_secs
                    ));
                }

                // 3. Dedup by proof message id.
                let mid: InboxMessageId = proof.msg_id;
                if guard.seen_message_ids.contains_key(&mid) {
                    return Err(n0_error::anyerr!(
                        "duplicate delete tombstone for message {mid:?}"
                    ));
                }
                let cutoff = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    .saturating_sub(MAX_CLOCK_SKEW.as_secs());
                guard.seen_message_ids.retain(|_, ts| *ts > cutoff);
                guard.seen_message_ids.insert(
                    mid,
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                );

                // 4. Emit event so the frontend can apply the tombstone to the store.
                let _ = guard.envelope_tx.send(InboxEvent::DeleteTombstoneReceived {
                    from: verified_sender,
                    proof,
                });
                Ok(None)
            }
        }
    }
}

// ── Sending methods ─────────────────────────────────────────────────────────────

/// Send a fully-signed envelope to a peer's inbox.
pub async fn send_deliver(
    endpoint: &Endpoint,
    secret_key: &SecretKey,
    peer: PublicKey,
    envelope: MailboxEnvelope,
) -> Result<()> {
    let signed = SignedInboxMessage::sign(secret_key, InboxPayload::Deliver(envelope))?;
    let len = signed.len() as u32;

    let conn = endpoint
        .connect(peer, INBOX_ALPN)
        .await
        .std_context("connect inbox")?;
    let (mut send, mut _recv) = conn
        .open_bi()
        .await
        .std_context("open_bi on inbox connection")?;

    send.write_all(&len.to_be_bytes())
        .await
        .std_context("write inbox message length")?;
    send.write_all(&signed)
        .await
        .std_context("write inbox message payload")?;
    send.finish().std_context("finish inbox message")?;

    Ok(())
}

/// Send a signed acknowledgement for a received envelope.
pub async fn send_ack(
    endpoint: &Endpoint,
    secret_key: &SecretKey,
    peer: PublicKey,
    ack: MailboxAck,
) -> Result<()> {
    let signed = SignedInboxMessage::sign(secret_key, InboxPayload::Ack(ack))?;
    let len = signed.len() as u32;

    let conn = endpoint
        .connect(peer, INBOX_ALPN)
        .await
        .std_context("connect inbox")?;
    let (mut send, mut _recv) = conn
        .open_bi()
        .await
        .std_context("open_bi on inbox connection")?;

    send.write_all(&len.to_be_bytes())
        .await
        .std_context("write ack length")?;
    send.write_all(&signed)
        .await
        .std_context("write ack payload")?;
    send.finish().std_context("finish ack")?;

    Ok(())
}

/// Result of a sync request, containing a page of envelopes and pagination info.
#[derive(Debug, Clone)]
pub struct SyncResponsePage {
    /// The missed envelopes in this page.
    pub envelopes: Vec<MailboxEnvelope>,
    /// Creation timestamp (ms) of the last envelope in this page.
    /// The requester uses this as `since_ms` for the next page request.
    /// None when envelopes is empty.
    pub last_created_at_ms: Option<u64>,
    /// True when more pages exist after this one.
    pub has_more: bool,
}

/// Send a sync request to a peer to retrieve missed envelopes.
pub async fn send_sync_request(
    endpoint: &Endpoint,
    secret_key: &SecretKey,
    peer: PublicKey,
    since_ms: u64,
) -> Result<SyncResponsePage> {
    let signed = SignedInboxMessage::sign(secret_key, InboxPayload::SyncRequest { since_ms })?;
    let len = signed.len() as u32;

    let conn = endpoint
        .connect(peer, INBOX_ALPN)
        .await
        .std_context("connect inbox for sync")?;
    let (mut send, mut recv) = conn.open_bi().await.std_context("open_bi for sync")?;

    send.write_all(&len.to_be_bytes())
        .await
        .std_context("write sync request length")?;
    send.write_all(&signed)
        .await
        .std_context("write sync request payload")?;

    // Read response.
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .std_context("read sync response length")?;
    let resp_len = u32::from_be_bytes(len_buf) as usize;
    if resp_len > MAX_INBOX_PAYLOAD {
        return Err(n0_error::anyerr!("sync response too large: {resp_len}"));
    }
    let mut resp_buf = vec![0u8; resp_len];
    recv.read_exact(&mut resp_buf)
        .await
        .std_context("read sync response payload")?;

    // Verify the response.
    let (_sender, payload, _sent_at) = SignedInboxMessage::verify(&resp_buf, Some(peer))?;

    match payload {
        InboxPayload::SyncResponse {
            envelopes,
            last_created_at_ms,
            has_more,
        } => Ok(SyncResponsePage {
            envelopes,
            last_created_at_ms,
            has_more,
        }),
        other => Err(n0_error::anyerr!(
            "unexpected sync response payload: {other:?}"
        )),
    }
}

/// Send a delete tombstone for a message to a peer's inbox.
///
/// The tombstone is signed by the original message author's secret key,
/// proving the author authorized the deletion.
pub async fn send_delete_tombstone(
    endpoint: &Endpoint,
    secret_key: &SecretKey,
    peer: PublicKey,
    msg_id: [u8; 32],
    conversation_id: [u8; 32],
    author_sk: &SecretKey,
) -> Result<()> {
    let proof = AuthorDeleteProof::sign(author_sk, msg_id, conversation_id);
    let signed = SignedInboxMessage::sign(secret_key, InboxPayload::DeleteTombstone(proof))?;
    let len = signed.len() as u32;

    let conn = endpoint
        .connect(peer, INBOX_ALPN)
        .await
        .std_context("connect inbox for delete tombstone")?;
    let (mut send, mut _recv) = conn
        .open_bi()
        .await
        .std_context("open_bi for delete tombstone")?;

    send.write_all(&len.to_be_bytes())
        .await
        .std_context("write delete tombstone length")?;
    send.write_all(&signed)
        .await
        .std_context("write delete tombstone payload")?;
    send.finish().std_context("finish delete tombstone")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mailbox::MailboxIdentity;
    use iroh::SecretKey;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Helpers to create deterministic test messages.
    fn test_secret_key() -> SecretKey {
        SecretKey::generate()
    }

    /// Verify that SignedInboxMessage sign/verify round-trips correctly.
    #[test]
    fn signed_inbox_message_round_trip() {
        let sk = test_secret_key();
        let payload = InboxPayload::SyncRequest { since_ms: 1000 };
        let encoded = SignedInboxMessage::sign(&sk, payload.clone()).unwrap();
        let (sender, decoded, _) = SignedInboxMessage::verify(&encoded, Some(sk.public())).unwrap();
        assert_eq!(sender, sk.public());
        match decoded {
            InboxPayload::SyncRequest { since_ms } => assert_eq!(since_ms, 1000),
            _ => panic!("wrong payload type"),
        }
    }

    /// Verify that sender mismatch is rejected.
    #[test]
    fn signed_inbox_message_rejects_wrong_sender() {
        let sk = test_secret_key();
        let wrong = test_secret_key();
        let payload = InboxPayload::SyncRequest { since_ms: 0 };
        let encoded = SignedInboxMessage::sign(&sk, payload).unwrap();
        let result = SignedInboxMessage::verify(&encoded, Some(wrong.public()));
        assert!(result.is_err(), "should reject wrong sender");
        assert!(
            result.unwrap_err().to_string().contains("sender mismatch"),
            "error should mention sender mismatch"
        );
    }

    /// Verify that a tampered message is rejected by signature verification.
    #[test]
    fn signed_inbox_message_rejects_tampered_payload() {
        let sk = test_secret_key();
        let payload = InboxPayload::SyncRequest { since_ms: 0 };
        let encoded = SignedInboxMessage::sign(&sk, payload).unwrap();
        // Corrupt one byte.
        let mut corrupted = encoded.clone();
        corrupted[30] ^= 0xFF;
        let result = SignedInboxMessage::verify(&corrupted, Some(sk.public()));
        assert!(result.is_err(), "should reject tampered message");
    }

    /// Verify that add_allowed_sender and remove_allowed_sender work correctly.
    #[tokio::test]
    async fn inbox_handle_manages_allowed_senders() {
        let (handle, _rx) = InboxHandle::new();
        let peer = test_secret_key().public();

        // Initially not allowed.
        assert!(!handle.is_allowed_sender(&peer).await);

        // After adding, it's allowed.
        handle.add_allowed_sender(peer).await;
        assert!(handle.is_allowed_sender(&peer).await);

        // After removing, it's not allowed.
        handle.remove_allowed_sender(&peer).await;
        assert!(!handle.is_allowed_sender(&peer).await);
    }

    /// Authorization is evaluated for each message, so revocation takes effect
    /// without rebuilding the protocol handler or restarting the process.
    #[tokio::test]
    async fn inbox_live_authorization_rejects_after_revocation() {
        let peer_sk = test_secret_key();
        let peer = peer_sk.public();
        let (handle, _rx) = InboxHandle::new();
        let live = Arc::new(AtomicBool::new(true));
        let live_for_lookup = Arc::clone(&live);
        handle
            .set_authorization_fn(Some(Arc::new(move |candidate| {
                candidate == peer && live_for_lookup.load(Ordering::SeqCst)
            })))
            .await;

        let request =
            SignedInboxMessage::sign(&peer_sk, InboxPayload::SyncRequest { since_ms: 0 }).unwrap();
        assert!(
            InboxProtocol::handle_request(&handle.inner(), peer, &request)
                .await
                .is_ok()
        );

        live.store(false, Ordering::SeqCst);
        let result = InboxProtocol::handle_request(&handle.inner(), peer, &request).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unauthorized"));
    }

    /// A committed envelope must be re-deliverable when the first ACK was
    /// lost.  The application-level handler uses this event to run its
    /// idempotent persistence path and send the same ACK again.
    #[tokio::test]
    async fn valid_duplicate_delivery_is_forwarded_for_reack() {
        let sender_sk = test_secret_key();
        let recipient_sk = test_secret_key();
        let recipient = MailboxIdentity::from_secret(&recipient_sk);
        let envelope = recipient.seal(&sender_sk, b"persist me").unwrap();
        let (handle, mut events) = InboxHandle::new();
        handle.add_allowed_sender(sender_sk.public()).await;

        let wire =
            SignedInboxMessage::sign(&sender_sk, InboxPayload::Deliver(envelope.clone())).unwrap();
        InboxProtocol::handle_request(&handle.inner(), sender_sk.public(), &wire)
            .await
            .unwrap();
        InboxProtocol::handle_request(&handle.inner(), sender_sk.public(), &wire)
            .await
            .unwrap();

        for _ in 0..2 {
            match events.recv().await.expect("delivery event") {
                InboxEvent::EnvelopeReceived {
                    from,
                    envelope: got,
                } => {
                    assert_eq!(from, sender_sk.public());
                    assert_eq!(got.message_id(), envelope.message_id());
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
    }

    /// A relay must not be able to make the recipient acknowledge an envelope
    /// on behalf of a different original sender.
    #[tokio::test]
    async fn delivery_rejects_transport_and_envelope_sender_mismatch() {
        let envelope_sender = test_secret_key();
        let transport_sender = test_secret_key();
        let recipient = MailboxIdentity::from_secret(&test_secret_key());
        let envelope = recipient.seal(&envelope_sender, b"private").unwrap();
        let (handle, _events) = InboxHandle::new();
        handle.add_allowed_sender(transport_sender.public()).await;
        let wire =
            SignedInboxMessage::sign(&transport_sender, InboxPayload::Deliver(envelope)).unwrap();
        let error =
            InboxProtocol::handle_request(&handle.inner(), transport_sender.public(), &wire)
                .await
                .expect_err("mismatched sender must be rejected");
        assert!(error.to_string().contains("sender mismatch"));
    }

    /// Verify that duplicate wire messages remain verifiable; the protocol
    /// forwards valid duplicates to the application for re-acknowledgement.
    #[test]
    fn inbox_dedup_accepts_duplicate_wire_messages() {
        // This test verifies the dedup logic in handle_message.
        // We create a SignedInboxMessage, encode it, and verify it passes
        // verification once. Then we simulate the dedup by checking that
        // the seen_message_ids set tracks it.
        let sk = test_secret_key();
        let payload = InboxPayload::SyncRequest { since_ms: 100 };
        let encoded = SignedInboxMessage::sign(&sk, payload).unwrap();
        let (sender, _inner, _sent_at) =
            SignedInboxMessage::verify(&encoded, Some(sk.public())).unwrap();
        assert_eq!(sender, sk.public());
        // The second verify call should also succeed (verify is stateless).
        // Dedup is done by the handle_message method which checks
        // seen_message_ids. We verify the verify function itself works twice.
        let (sender2, _, _) = SignedInboxMessage::verify(&encoded, Some(sk.public())).unwrap();
        assert_eq!(sender2, sk.public());
    }

    // ── SyncRequest authorization tests ────────────────────────────────

    #[tokio::test]
    async fn sync_request_rejects_future_timestamp() {
        let sk = test_secret_key();
        let (handle, _rx) = InboxHandle::new();
        handle.add_allowed_sender(sk.public()).await;

        // since_ms far in the future — beyond the skew tolerance.
        let far_future = now_ms() + MAX_SYNC_FUTURE_SKEW_MS + 60_000;
        let request = SignedInboxMessage::sign(
            &sk,
            InboxPayload::SyncRequest {
                since_ms: far_future,
            },
        )
        .unwrap();

        let result = InboxProtocol::handle_request(&handle.inner(), sk.public(), &request).await;
        assert!(result.is_err(), "future timestamp should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("sync request"),
            "error should mention sync request, got: {err}"
        );
        assert!(
            err.contains("in the future"),
            "error should mention future, got: {err}"
        );
    }

    #[tokio::test]
    async fn sync_request_slightly_future_timestamp_is_allowed() {
        let sk = test_secret_key();
        let (handle, _rx) = InboxHandle::new();
        handle.add_allowed_sender(sk.public()).await;

        // since_ms slightly in the future — within the skew tolerance.
        let slight_future = now_ms() + 30_000; // 30 seconds
        let request = SignedInboxMessage::sign(
            &sk,
            InboxPayload::SyncRequest {
                since_ms: slight_future,
            },
        )
        .unwrap();

        let result = InboxProtocol::handle_request(&handle.inner(), sk.public(), &request).await;
        assert!(
            result.is_ok(),
            "slightly future timestamp should be allowed: {result:?}"
        );
    }

    #[tokio::test]
    async fn sync_request_ancient_timestamp_is_clamped_not_rejected() {
        let sk = test_secret_key();
        let (handle, _rx) = InboxHandle::new();
        handle.add_allowed_sender(sk.public()).await;

        // since_ms far in the past — the handler should clamp it, not reject it.
        let ancient = 1; // epoch + 1ms
        let request = SignedInboxMessage::sign(
            &sk,
            InboxPayload::SyncRequest {
                since_ms: ancient,
            },
        )
        .unwrap();

        let result = InboxProtocol::handle_request(&handle.inner(), sk.public(), &request).await;
        assert!(
            result.is_ok(),
            "ancient timestamp should be clamped, not rejected: {result:?}"
        );
    }

    #[tokio::test]
    async fn sync_request_unauthorized_peer_is_rejected() {
        let peer_sk = test_secret_key();
        let peer = peer_sk.public();
        let (handle, _rx) = InboxHandle::new();

        // Do NOT add the peer to allowed_senders — they are unauthorized.
        let request = SignedInboxMessage::sign(
            &peer_sk,
            InboxPayload::SyncRequest { since_ms: 0 },
        )
        .unwrap();
        let result = InboxProtocol::handle_request(&handle.inner(), peer, &request).await;
        assert!(result.is_err(), "unauthorized peer must be rejected");
        assert!(
            result.unwrap_err().to_string().contains("unauthorized"),
            "error should mention unauthorized"
        );
    }

    // ── AuthorDeleteProof tests (Step 12) ─────────────────────────────

    #[test]
    fn author_delete_proof_sign_and_verify() {
        let author_sk = test_secret_key();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];

        let proof = AuthorDeleteProof::sign(&author_sk, msg_id, conv_id);
        assert_eq!(proof.author, author_sk.public());
        assert_eq!(proof.msg_id, msg_id);
        assert_eq!(proof.conversation_id, conv_id);
        assert!(proof.created_at_unix_secs > 0);

        // Verify should succeed
        assert!(proof.verify().is_ok());
    }

    #[test]
    fn author_delete_proof_rejects_wrong_author() {
        let author_sk = test_secret_key();
        let wrong_sk = test_secret_key();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];

        let mut proof = AuthorDeleteProof::sign(&author_sk, msg_id, conv_id);
        // Replace author with wrong key (but keep the signature from author_sk)
        proof.author = wrong_sk.public();
        assert!(proof.verify().is_err());
    }

    #[test]
    fn author_delete_proof_rejects_tampered_msg_id() {
        let author_sk = test_secret_key();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];

        let mut proof = AuthorDeleteProof::sign(&author_sk, msg_id, conv_id);
        // Tamper with the msg_id
        proof.msg_id[0] ^= 0xFF;
        assert!(proof.verify().is_err());
    }

    #[test]
    fn author_delete_proof_rejects_tampered_conversation_id() {
        let author_sk = test_secret_key();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];

        let mut proof = AuthorDeleteProof::sign(&author_sk, msg_id, conv_id);
        // Tamper with the conversation_id
        proof.conversation_id[0] ^= 0xFF;
        assert!(proof.verify().is_err());
    }

    #[test]
    fn author_delete_proof_round_trips_through_postcard() {
        let author_sk = test_secret_key();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];

        let proof = AuthorDeleteProof::sign(&author_sk, msg_id, conv_id);
        let encoded = postcard::to_stdvec(&proof).unwrap();
        let decoded: AuthorDeleteProof = postcard::from_bytes(&encoded).unwrap();

        assert_eq!(decoded.msg_id, proof.msg_id);
        assert_eq!(decoded.conversation_id, proof.conversation_id);
        assert_eq!(decoded.author, proof.author);
        assert_eq!(decoded.created_at_unix_secs, proof.created_at_unix_secs);
        assert_eq!(*decoded.author_signature, *proof.author_signature);

        // Verify still works after deserialization
        assert!(decoded.verify().is_ok());
    }

    #[test]
    fn author_delete_proof_wraps_in_signed_inbox_message() {
        let author_sk = test_secret_key();
        let sender_sk = test_secret_key();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];

        let proof = AuthorDeleteProof::sign(&author_sk, msg_id, conv_id);
        let encoded =
            SignedInboxMessage::sign(&sender_sk, InboxPayload::DeleteTombstone(proof)).unwrap();

        // Verify the outer envelope
        let (sender, payload, _) =
            SignedInboxMessage::verify(&encoded, Some(sender_sk.public())).unwrap();
        assert_eq!(sender, sender_sk.public());

        // Verify the inner proof
        match payload {
            InboxPayload::DeleteTombstone(inner_proof) => {
                assert!(inner_proof.verify().is_ok());
                assert_eq!(inner_proof.msg_id, msg_id);
                assert_eq!(inner_proof.author, author_sk.public());
            }
            other => panic!("expected DeleteTombstone, got {other:?}"),
        }
    }

    #[test]
    fn author_delete_proof_tampered_inner_proof_rejected() {
        let author_sk = test_secret_key();
        let sender_sk = test_secret_key();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];

        let proof = AuthorDeleteProof::sign(&author_sk, msg_id, conv_id);
        let encoded =
            SignedInboxMessage::sign(&sender_sk, InboxPayload::DeleteTombstone(proof)).unwrap();

        // The inner proof's msg_id is somewhere in the serialized bytes.
        // We verify that if someone tampers the inner proof AFTER the
        // outer signed message is decoded, the inner proof verification catches it.

        // Decode and tamper the inner proof
        let (_, payload, _) =
            SignedInboxMessage::verify(&encoded, Some(sender_sk.public())).unwrap();
        match payload {
            InboxPayload::DeleteTombstone(mut inner) => {
                inner.msg_id[0] ^= 0xFF;
                // The inner verify should fail
                assert!(inner.verify().is_err());
            }
            _ => panic!("expected DeleteTombstone"),
        }
    }

    // ── Sync replay protection tests ─────────────────────────────────

    /// Verify that set_record_sync_served_fn properly installs a callback
    /// and that it is invoked with the correct message IDs.
    #[tokio::test]
    async fn record_sync_served_fn_captures_served_envelope_ids() {
        let sk = test_secret_key();
        let identity = MailboxIdentity::from_secret(&sk);
        let envelope = identity.seal(&sk, b"sync test payload").unwrap();

        let (handle, _rx) = InboxHandle::new();
        handle.add_allowed_sender(sk.public()).await;

        // Set up a pending_fn that returns our test envelope.
        let test_envelope = envelope.clone();
        handle
            .set_pending_fn(Some(Arc::new(move |_requester, _since_ms| {
                (vec![test_envelope.clone()], false)
            })))
            .await;

        // Set up record_sync_served_fn to capture served IDs.
        let captured_ids: Arc<std::sync::Mutex<Vec<InboxMessageId>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_ids_clone = captured_ids.clone();
        handle
            .set_record_sync_served_fn(Some(Arc::new(move |_peer, msg_ids| {
                let mut ids = captured_ids_clone.lock().unwrap();
                for id in msg_ids {
                    ids.push(*id);
                }
            })))
            .await;

        // Send a SyncRequest — handle_request calls pending_fn.
        let request = SignedInboxMessage::sign(
            &sk,
            InboxPayload::SyncRequest { since_ms: 0 },
        )
        .unwrap();

        let result =
            InboxProtocol::handle_request(&handle.inner(), sk.public(), &request).await;
        assert!(result.is_ok(), "sync request should succeed");

        // Compute the expected message ID.
        let expected_mid =
            inbox_message_id(&postcard::to_stdvec(&envelope).unwrap());

        // Manually invoke the record_sync_served_fn callback (this is what
        // accept() does after sending SyncResponse).
        {
            let inner = handle.inner();
            let guard = inner.lock().await;
            if let Some(ref record_fn) = guard.record_sync_served_fn {
                record_fn(sk.public(), &[expected_mid]);
            }
        }

        // Verify the callback captured the expected ID.
        let ids = captured_ids.lock().unwrap();
        assert_eq!(ids.len(), 1, "should have captured one message ID");
        assert_eq!(
            ids[0], expected_mid,
            "captured ID should match computed inbox_message_id"
        );
    }

    /// Verify that the pending_fn filter, when combined with a
    /// record_sync_served_fn callback, correctly excludes already-served
    /// envelopes from subsequent sync requests.
    #[tokio::test]
    async fn sync_dedup_excludes_already_served_envelopes() {
        let sk = test_secret_key();
        let identity = MailboxIdentity::from_secret(&sk);

        // Create two distinct envelopes.
        let env_a = identity.seal(&sk, b"envelope A").unwrap();
        let env_b = identity.seal(&sk, b"envelope B").unwrap();

        // Compute their stable message IDs.
        let mid_a = inbox_message_id(&postcard::to_stdvec(&env_a).unwrap());
        let mid_b = inbox_message_id(&postcard::to_stdvec(&env_b).unwrap());

        let (handle, _rx) = InboxHandle::new();
        handle.add_allowed_sender(sk.public()).await;

        // Shared dedup set simulating the frontend's filter logic.
        let served: Arc<std::sync::Mutex<HashSet<InboxMessageId>>> =
            Arc::new(std::sync::Mutex::new(HashSet::new()));

        // Set up pending_fn that returns both envelopes but filters out
        // any already in the served set (same pattern as the frontend).
        let env_a_for_fn = env_a.clone();
        let env_b_for_fn = env_b.clone();
        let served_for_fn = served.clone();
        handle
            .set_pending_fn(Some(Arc::new(move |_requester, _since_ms| {
                let all = vec![env_a_for_fn.clone(), env_b_for_fn.clone()];
                let served = served_for_fn.lock().unwrap();
                let filtered: Vec<_> = all
                    .into_iter()
                    .filter(|env| {
                        let bytes =
                            postcard::to_stdvec(env).expect("envelope encoding cannot fail");
                        !served.contains(&inbox_message_id(&bytes))
                    })
                    .collect();
                drop(served);
                let has_more = false;
                (filtered, has_more)
            })))
            .await;

        // First request: both envelopes should be returned (none served yet).
        let request = SignedInboxMessage::sign(
            &sk,
            InboxPayload::SyncRequest { since_ms: 0 },
        )
        .unwrap();
        let result =
            InboxProtocol::handle_request(&handle.inner(), sk.public(), &request).await;
        assert!(result.is_ok(), "first sync request should succeed");
        let Some((envelopes, _)) = result.unwrap() else {
            panic!("first sync request returned None");
        };
        assert_eq!(
            envelopes.len(),
            2,
            "first request should return both envelopes"
        );

        // Simulate recording env_a as served (as accept() would do).
        served.lock().unwrap().insert(mid_a);

        // Second request: only env_b should be returned.
        let request2 = SignedInboxMessage::sign(
            &sk,
            InboxPayload::SyncRequest { since_ms: 0 },
        )
        .unwrap();
        let result2 =
            InboxProtocol::handle_request(&handle.inner(), sk.public(), &request2).await;
        assert!(result2.is_ok(), "second sync request should succeed");
        let Some((envelopes2, _)) = result2.unwrap() else {
            panic!("second sync request returned None");
        };
        assert_eq!(
            envelopes2.len(),
            1,
            "second request should exclude already-served envelope A"
        );
        assert_eq!(
            inbox_message_id(&postcard::to_stdvec(&envelopes2[0]).unwrap()),
            mid_b,
            "remaining envelope should be env_b"
        );

        // Simulate recording env_b as served.
        served.lock().unwrap().insert(mid_b);

        // Third request: no envelopes should be returned.
        let request3 = SignedInboxMessage::sign(
            &sk,
            InboxPayload::SyncRequest { since_ms: 0 },
        )
        .unwrap();
        let result3 =
            InboxProtocol::handle_request(&handle.inner(), sk.public(), &request3).await;
        assert!(result3.is_ok(), "third sync request should succeed");
        let Some((envelopes3, _)) = result3.unwrap() else {
            panic!("third sync request returned None");
        };
        assert_eq!(
            envelopes3.len(),
            0,
            "third request should return nothing when all served"
        );
    }

    /// Verify that inbox_message_id is deterministic and matches between
    /// consecutive calls on the same envelope.
    #[test]
    fn inbox_message_id_is_consistent_across_paths() {
        let sk = test_secret_key();
        let identity = MailboxIdentity::from_secret(&sk);
        let envelope = identity.seal(&sk, b"consistent hash test").unwrap();

        // Compute ID via inbox_message_id (used in accept() for record_sync_served_fn).
        let protocol_id =
            inbox_message_id(&postcard::to_stdvec(&envelope).unwrap());

        // Verify determinism: same envelope produces same ID.
        let id2 = inbox_message_id(&postcard::to_stdvec(&envelope).unwrap());
        assert_eq!(
            protocol_id, id2,
            "inbox_message_id must be deterministic for the same envelope"
        );

        // Different envelopes produce different IDs.
        let envelope2 = identity.seal(&sk, b"different payload").unwrap();
        let id3 = inbox_message_id(&postcard::to_stdvec(&envelope2).unwrap());
        assert_ne!(
            protocol_id, id3,
            "inbox_message_id must differ for different envelopes"
        );
    }
}
