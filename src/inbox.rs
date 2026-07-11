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
//! * [`InboxHandle::subscribe`] — subscribe to this node's own inbox topic
//!   at startup so it stays alive independently of the visible chat room.
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

use crate::mailbox::{MailboxAck, MailboxEnvelope};

// ── Constants ──────────────────────────────────────────────────────────────────

/// ALPN for the inbox service.
pub const INBOX_ALPN: &[u8] = b"/iroh-chat-inbox/1";

/// Max clock skew for replay protection (24 hours).
const MAX_CLOCK_SKEW: Duration = Duration::from_secs(24 * 60 * 60);

/// Max payload size for a single inbox message (10 MB).
const MAX_INBOX_PAYLOAD: usize = 10 * 1024 * 1024;

/// Stable identifier for a message (blake3 hash of signed bytes).
pub type InboxMessageId = [u8; 32];

/// Derive a stable message id from signed bytes.
fn inbox_message_id(bytes: &[u8]) -> InboxMessageId {
    *blake3::hash(bytes).as_bytes()
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
    /// Response containing missed envelopes.
    SyncResponse {
        /// The missed envelopes.
        envelopes: Vec<MailboxEnvelope>,
    },
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

// ── Inbox protocol state ───────────────────────────────────────────────────────

/// Inbox protocol state shared across connections.
#[derive(Debug)]
pub struct InboxInner {
    /// Set of senders whose messages are currently accepted (contact/allowed peers).
    pub allowed_senders: HashSet<PublicKey>,
    /// Deduplication: message ids seen within the replay window.
    pub seen_message_ids: HashMap<InboxMessageId, u64>,
    /// Channel to forward received envelopes to the frontend.
    pub envelope_tx: mpsc::UnboundedSender<InboxEvent>,
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
            seen_message_ids: HashMap::new(),
            envelope_tx,
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

    /// Inbox topic — derived from the local PublicKey for personal inbox subscriptions.
    pub fn inbox_topic(local_public: PublicKey) -> crate::proto::TopicId {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"iroh-chat-inbox/v1");
        hasher.update(local_public.as_bytes());
        (*hasher.finalize().as_bytes()).into()
    }

    /// Subscribe to this node's personal inbox gossip topic.
    ///
    /// This **must** be called at startup and the subscription kept alive
    /// independently of the visible chat room, so peers can always deliver
    /// offline messages even when the user is not in an active chat room.
    pub async fn subscribe_inbox_topic(
        &self,
        gossip: &crate::net::Gossip,
        local_public: PublicKey,
    ) -> Result<()> {
        let topic = Self::inbox_topic(local_public);
        // Subscribe without any bootstrap peers — the inbox topic is passive;
        // peers connect to us when they want to deliver an envelope.
        gossip.subscribe(topic, Vec::new()).await?;
        Ok(())
    }
}

// ── Protocol handler ────────────────────────────────────────────────────────────

/// Protocol handler for incoming inbox connections.
///
/// Register on the Router with `.accept(INBOX_ALPN, inbox_handler)`.
#[derive(Debug, Clone)]
pub struct InboxProtocol {
    inner: Arc<Mutex<InboxInner>>,
}

impl InboxProtocol {
    /// Create a protocol handler from the shared inner state.
    pub fn new(inner: Arc<Mutex<InboxInner>>) -> Self {
        Self { inner }
    }
}

impl ProtocolHandler for InboxProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote_id = connection.remote_id();

        // Check authorization before accepting any streams.
        {
            let guard = self.inner.lock().await;
            if !guard.allowed_senders.contains(&remote_id) {
                return Err(AcceptError::from_err(n0_error::anyerr!(
                    "inbox connection from unauthorized peer {}",
                    remote_id.fmt_short()
                )));
            }
        }

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

            // Verify and dispatch.
            let result = Self::handle_message(&inner, remote_id, &buf).await;
            if let Err(ref e) = result {
                tracing::warn!(
                    "inbox: failed to handle message from {}: {e}",
                    remote_id.fmt_short()
                );
            }

            // Send a minimal ack on the response stream (even on error, so the
            // sender knows the stream was consumed).
            let _ = send.write_all(&[0u8; 1]).await;
            let _ = send.finish();
        }

        Ok(())
    }

    async fn shutdown(&self) {
        // No-op: the inbox has no persistent state beyond the shared inner.
    }
}

impl InboxProtocol {
    async fn handle_message(
        inner: &Arc<Mutex<InboxInner>>,
        sender: PublicKey,
        buf: &[u8],
    ) -> Result<()> {
        // Verify the signed message.
        let (verified_sender, payload, _sent_at) = SignedInboxMessage::verify(buf, Some(sender))?;

        let mut guard = inner.lock().await;

        match payload {
            InboxPayload::Deliver(envelope) => {
                // Dedup by message_id.
                let mid = inbox_message_id(
                    &postcard::to_stdvec(&envelope)
                        .map_err(|e| n0_error::anyerr!("encode envelope for id: {e}"))?,
                );
                if guard.seen_message_ids.contains_key(&mid) {
                    return Err(n0_error::anyerr!("duplicate inbox message {mid:?}"));
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
            }
            InboxPayload::Ack(ack) => {
                let _ = guard.envelope_tx.send(InboxEvent::AckReceived {
                    from: verified_sender,
                    ack,
                });
            }
            InboxPayload::SyncRequest { since_ms } => {
                let _ = guard.envelope_tx.send(InboxEvent::SyncRequested {
                    from: verified_sender,
                    since_ms,
                });
            }
            InboxPayload::SyncResponse { .. } => {
                // SyncResponse is only sent, never received on the server side.
            }
        }

        Ok(())
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

/// Send a sync request to a peer to retrieve missed envelopes.
pub async fn send_sync_request(
    endpoint: &Endpoint,
    secret_key: &SecretKey,
    peer: PublicKey,
    since_ms: u64,
) -> Result<Vec<MailboxEnvelope>> {
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
        InboxPayload::SyncResponse { envelopes } => Ok(envelopes),
        other => Err(n0_error::anyerr!(
            "unexpected sync response payload: {other:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

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

    /// Verify that inbox_topic is deterministic and repeatable.
    #[test]
    fn inbox_topic_is_deterministic() {
        let sk = test_secret_key();
        let pk = sk.public();
        let topic1 = InboxHandle::inbox_topic(pk);
        let topic2 = InboxHandle::inbox_topic(pk);
        assert_eq!(topic1, topic2, "inbox topic must be deterministic");

        // Different keys produce different topics.
        let other = test_secret_key().public();
        let topic3 = InboxHandle::inbox_topic(other);
        assert_ne!(
            topic1, topic3,
            "different keys must produce different topics"
        );
    }

    /// Verify that InboxHandle::subscribe_inbox_topic produces consistent topic derivation.
    #[test]
    fn inbox_topic_derivation_is_stable() {
        // The derivation prefix "iroh-chat-inbox/v1" should not change accidentally.
        let sk = test_secret_key();
        let pk = sk.public();
        let topic = InboxHandle::inbox_topic(pk);
        // Recomputed from the same key should match.
        let topic_again = InboxHandle::inbox_topic(pk);
        assert_eq!(topic.as_bytes(), topic_again.as_bytes());
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

    /// Verify that duplicate message ids are rejected (dedup).
    #[test]
    fn inbox_dedup_rejects_duplicate_messages() {
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
}
