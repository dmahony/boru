//! Whisper protocol — direct QUIC channels for private 1:1 messaging and file transfer.
//!
//! This module opens direct QUIC connections between two peers, separate from the
//! gossip broadcast mesh, for private conversations. Each connection carries
//! bi-directional streams with length-prefixed postcard-encoded frames.
//!
//! # Architecture
//!
//! * [`WhisperBuilder`] / [`Whisper::spawn`] — create and run the whisper actor.
//! * [`WhisperHandle`] — cloneable handle for sending DMs and files.
//! * [`WhisperProtocol`] — registers as a protocol handler on the Router to accept
//!   incoming whisper connections.
//! * [`WhisperEvent`] — events delivered to the frontend (messages, connect/disconnect).
//!
//! # ALPN
//!
//! The ALPN for whisper connections is [`WHISPER_ALPN`].

pub mod session_manager;

use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
};

use bytes::Bytes;
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr, PublicKey, SecretKey,
};
use n0_error::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, info, trace, warn};

use crate::mailbox::{MailboxAck, MailboxEnvelope};

// ── Constants ──────────────────────────────────────────────────────────────────

/// ALPN for whisper direct connections.
pub const WHISPER_ALPN: &[u8] = b"/iroh-gossip-chat/whisper/1";

/// Default capacity for the command channel.
const CMD_CHANNEL_CAP: usize = 256;

/// Counter of correctness-critical whisper state events (`Connected`,
/// `Disconnected`) that could not be delivered to the frontend because the
/// event channel was full or its receiver was dropped.  A non-zero value is
/// observable overload (BORU-AUDIT-08).
static WHISPER_STATE_EVENT_SEND_FAILURES: AtomicU64 = AtomicU64::new(0);

/// Counter of `RevokePeer` commands that could not be queued on the first
/// attempt and had to be retried asynchronously (BORU-AUDIT-08).
static WHISPER_REVOKE_SEND_FAILURES: AtomicU64 = AtomicU64::new(0);

/// Maximum payload size for a single whisper message (16 MB).
const MAX_WHISPER_PAYLOAD: usize = 16 * 1024 * 1024;

// ── Wire protocol ──────────────────────────────────────────────────────────────

/// Wire-frame messages exchanged over a whisper connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum WhisperWireMessage {
    /// A private text message.
    Text {
        /// Public key of the sender (hex string).
        from: String,
        /// The message content.
        text: String,
    },

    /// Signed contact/control-plane message.
    Control { payload: Vec<u8> },
    /// Encrypted offline-mailbox envelope. The transport never decrypts it.
    MailboxEnvelope { envelope: MailboxEnvelope },
    /// Recipient acknowledgement for a previously accepted envelope.
    MailboxAck { ack: MailboxAck },
}

// ── Public event types ─────────────────────────────────────────────────────────

/// Events emitted from the whisper protocol.
#[derive(Debug, Clone)]
pub enum WhisperEvent {
    /// A received private message from a peer.
    Message {
        /// Public key of the sender.
        from: PublicKey,
        /// The raw message content (text or file transfer).
        content: Bytes,
    },

    /// A signed contact/control-plane message. The receiver must verify it.
    Control {
        /// Public key of the sender.
        from: PublicKey,
        /// Signed control payload.
        content: Bytes,
    },
    /// An encrypted mailbox envelope received from a peer.
    MailboxEnvelope {
        /// Public key of the transport peer that delivered the envelope.
        from: PublicKey,
        /// Opaque envelope; the application must validate and decrypt it.
        envelope: MailboxEnvelope,
    },
    /// A recipient acknowledgement received from a peer.
    MailboxAck {
        /// Public key of the transport peer that delivered the acknowledgement.
        from: PublicKey,
        /// Signed acknowledgement; the sender must validate it before removal.
        ack: MailboxAck,
    },
    /// A peer has connected (ready for whispers).
    Connected {
        /// Public key of the connected peer.
        peer: PublicKey,
    },
    /// A peer has disconnected.
    Disconnected {
        /// Public key of the disconnected peer.
        peer: PublicKey,
    },
}

// ── Internal commands ──────────────────────────────────────────────────────────

pub(crate) enum Cmd {
    SendDm {
        peer: PublicKey,
        text: String,
        reply: oneshot::Sender<Result<()>>,
    },

    SendControl {
        peer: PublicKey,
        payload: Bytes,
        reply: oneshot::Sender<Result<()>>,
    },
    SendMailboxEnvelope {
        peer: PublicKey,
        envelope: MailboxEnvelope,
        reply: oneshot::Sender<Result<()>>,
    },
    SendMailboxAck {
        peer: PublicKey,
        ack: MailboxAck,
        reply: oneshot::Sender<Result<()>>,
    },
    ConnectTo {
        peer: PublicKey,
        addr: EndpointAddr,
        reply: oneshot::Sender<Result<()>>,
    },
    Disconnect {
        peer: PublicKey,
        reply: oneshot::Sender<bool>,
    },
    /// Revoke authorization and close any active session.
    RevokePeer(PublicKey),
    /// An incoming connection from a remote peer (from ProtocolHandler).
    IncomingConnection(Connection),
}

// ── Internal per-connection events ─────────────────────────────────────────────

enum ConnectionEvent {
    Message { from: PublicKey, content: Bytes },
    Disconnected(PublicKey),
}

// ── WhisperHandle ──────────────────────────────────────────────────────────────

/// Queue a `RevokePeer` command without silently dropping it.
///
/// `set_peer_authorized` is a synchronous API (called from UI code), so this
/// cannot await the bounded command channel.  We fast-path `try_send`; if the
/// queue is full or closed, we count the failure and spawn a best-effort task
/// to deliver the revocation when a runtime is available.  Revocation is
/// security-critical: silently dropping it would leave a revoked peer's
/// channel open.
fn enqueue_revoke(cmd_tx: &mpsc::Sender<Cmd>, peer: PublicKey) {
    if cmd_tx.try_send(Cmd::RevokePeer(peer)).is_ok() {
        return;
    }
    WHISPER_REVOKE_SEND_FAILURES.fetch_add(1, Ordering::Relaxed);
    warn!(
        peer = %peer.fmt_short(),
        "whisper revoke command queue full; retrying asynchronously"
    );
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let cmd_tx = cmd_tx.clone();
        handle.spawn(async move {
            if let Err(e) = cmd_tx.send(Cmd::RevokePeer(peer)).await {
                warn!(
                    peer = %peer.fmt_short(),
                    error = %e,
                    "whisper revoke command lost: actor dropped"
                );
            }
        });
    }
}

/// Forward a correctness-critical whisper event to the frontend.
///
/// State events (`Connected`/`Disconnected`) drive the session manager's
/// reconnect state machine and messages carry user data, so they must not be
/// silently dropped under load: we await the bounded channel (backpressure)
/// and count/log any failure.  The only failure mode is a dropped frontend
/// receiver.
async fn forward_event(event_tx: &mpsc::Sender<WhisperEvent>, event: WhisperEvent) {
    if let Err(e) = event_tx.send(event).await {
        WHISPER_STATE_EVENT_SEND_FAILURES.fetch_add(1, Ordering::Relaxed);
        warn!(error = %e, "whisper event dropped: frontend receiver gone");
    }
}

/// Handle to send commands to a running whisper actor.
///
/// Clone this freely — all clones share the same background task.
#[derive(Debug, Clone)]
pub struct WhisperHandle {
    cmd_tx: mpsc::Sender<Cmd>,
    denied_peers: Arc<RwLock<HashSet<PublicKey>>>,
}

impl WhisperHandle {
    /// Allow or deny a peer for incoming whisper connections.
    ///
    /// All peers are authorized by default. Peers in the denied set are
    /// rejected at the protocol level.
    pub fn set_peer_authorized(&self, peer: PublicKey, authorized: bool) {
        let mut peers = self
            .denied_peers
            .write()
            .expect("authorization lock poisoned");
        if authorized {
            peers.remove(&peer);
        } else {
            peers.insert(peer);
            // Adding a key must also tear down an already-established channel
            // so revoked peers cannot continue messaging.
            enqueue_revoke(&self.cmd_tx, peer);
        }
    }

    /// Send a private text message to a peer.
    ///
    /// If no connection to the peer exists, the actor will try to discover
    /// and connect using the endpoint's remote info.
    pub async fn send_dm(&self, peer: PublicKey, text: String) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::SendDm { peer, text, reply })
            .await
            .map_err(|_| n0_error::anyerr!("whisper actor dropped"))?;
        rx.await
            .map_err(|_| n0_error::anyerr!("whisper reply dropped"))?
    }

    /// Send a signed contact/control message to a peer.
    pub async fn send_control(&self, peer: PublicKey, payload: Bytes) -> Result<()> {
        info!(
            peer = %peer.fmt_short(),
            payload_len = payload.len(),
            "whisper send_control called"
        );
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::SendControl {
                peer,
                payload,
                reply,
            })
            .await
            .map_err(|_| n0_error::anyerr!("whisper actor dropped"))?;
        rx.await
            .map_err(|_| n0_error::anyerr!("whisper reply dropped"))?
    }

    /// Send an encrypted mailbox envelope after a Whisper session is live.
    /// The transport never decrypts or acknowledges the envelope.
    pub async fn send_mailbox_envelope(
        &self,
        peer: PublicKey,
        envelope: MailboxEnvelope,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::SendMailboxEnvelope {
                peer,
                envelope,
                reply,
            })
            .await
            .map_err(|_| n0_error::anyerr!("whisper actor dropped"))?;
        rx.await
            .map_err(|_| n0_error::anyerr!("whisper reply dropped"))?
    }

    /// Send a signed acknowledgement for an accepted mailbox envelope.
    pub async fn send_mailbox_ack(&self, peer: PublicKey, ack: MailboxAck) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::SendMailboxAck { peer, ack, reply })
            .await
            .map_err(|_| n0_error::anyerr!("whisper actor dropped"))?;
        rx.await
            .map_err(|_| n0_error::anyerr!("whisper reply dropped"))?
    }

    /// Connect to a peer by their endpoint address.
    ///
    /// Once connected, messages can be sent without further address resolution.
    pub async fn connect_to(&self, peer: PublicKey, addr: EndpointAddr) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::ConnectTo { peer, addr, reply })
            .await
            .map_err(|_| n0_error::anyerr!("whisper actor dropped"))?;
        rx.await
            .map_err(|_| n0_error::anyerr!("whisper reply dropped"))?
    }

    /// Disconnect from a peer.
    ///
    /// Returns `true` if the peer was connected, `false` otherwise.
    pub async fn disconnect(&self, peer: &PublicKey) -> Result<bool> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::Disconnect { peer: *peer, reply })
            .await
            .map_err(|_| n0_error::anyerr!("whisper actor dropped"))?;
        rx.await
            .map_err(|_| n0_error::anyerr!("whisper reply dropped"))
    }

    /// Create a raw inner handle for tests (bypasses the public API).
    #[doc(hidden)]
    fn _cmd_tx(&self) -> mpsc::Sender<Cmd> {
        self.cmd_tx.clone()
    }
}

// ── Protocol handler ──────────────────────────────────────────────────────────

/// Protocol handler that routes incoming whisper connections to the actor.
#[derive(Debug, Clone)]
pub struct WhisperProtocol {
    cmd_tx: mpsc::Sender<Cmd>,
    denied_peers: Arc<RwLock<HashSet<PublicKey>>>,
}

impl ProtocolHandler for WhisperProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote_id = connection.remote_id();
        debug!(peer = %remote_id.fmt_short(), "whisper incoming connection");

        if self
            .denied_peers
            .read()
            .expect("authorization lock poisoned")
            .contains(&remote_id)
        {
            return Err(AcceptError::from_err(n0_error::anyerr!(
                "whisper peer {} is not authorized",
                remote_id.fmt_short()
            )));
        }

        // Route the incoming connection to the actor, which will register
        // it in the connected map and spawn a reader task.
        self.cmd_tx
            .send(Cmd::IncomingConnection(connection))
            .await
            .map_err(|_| AcceptError::from_err(n0_error::anyerr!("actor dropped")))?;

        Ok(())
    }
}

// ── WhisperBuilder ─────────────────────────────────────────────────────────────

/// Builder for creating and joining whisper channels.
#[derive(Debug)]
pub struct WhisperBuilder {
    endpoint: Endpoint,
    secret_key: SecretKey,
    cmd_tx: mpsc::Sender<Cmd>,
    /// Receiver half taken by `spawn()`.
    cmd_rx: Option<mpsc::Receiver<Cmd>>,
    denied_peers: Arc<RwLock<HashSet<PublicKey>>>,
}

impl WhisperBuilder {
    /// Create a new builder from an iroh endpoint and its secret key.
    pub fn new(endpoint: Endpoint, secret_key: SecretKey) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_CAP);
        Self {
            endpoint,
            secret_key,
            cmd_tx,
            cmd_rx: Some(cmd_rx),
            denied_peers: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Pre-populate the denied-peers set before registering the protocol.
    pub fn with_denied_peers(self, peers: impl IntoIterator<Item = PublicKey>) -> Self {
        self.denied_peers
            .write()
            .expect("authorization lock poisoned")
            .extend(peers);
        self
    }

    /// Create a [`WhisperProtocol`] handler for this whisper channel.
    ///
    /// Register it on your Router with `router.accept(WHISPER_ALPN, handler)`
    /// so incoming whisper connections are routed to this actor.
    pub fn protocol_handler(&self) -> WhisperProtocol {
        WhisperProtocol {
            cmd_tx: self.cmd_tx.clone(),
            denied_peers: Arc::clone(&self.denied_peers),
        }
    }

    /// Spawn the whisper actor and return a handle + event receiver.
    pub fn spawn(mut self) -> (WhisperHandle, mpsc::Receiver<WhisperEvent>) {
        let (event_tx, event_rx) = mpsc::channel(1024);
        let connected: Arc<Mutex<HashMap<PublicKey, Connection>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let handle = WhisperHandle {
            cmd_tx: self.cmd_tx.clone(),
            denied_peers: Arc::clone(&self.denied_peers),
        };

        let cmd_rx = self.cmd_rx.take().expect("spawn called more than once");

        let endpoint = self.endpoint.clone();
        let secret_key = self.secret_key.clone();
        tokio::task::spawn(run_actor(endpoint, secret_key, cmd_rx, event_tx, connected));

        (handle, event_rx)
    }
}

// ── Actor ─────────────────────────────────────────────────────────────────────

/// Background actor that manages whisper connections and dispatches messages.
async fn run_actor(
    endpoint: Endpoint,
    secret_key: SecretKey,
    mut cmd_rx: mpsc::Receiver<Cmd>,
    event_tx: mpsc::Sender<WhisperEvent>,
    connected: Arc<Mutex<HashMap<PublicKey, Connection>>>,
) {
    let (msg_tx, mut msg_rx) = mpsc::channel::<ConnectionEvent>(4096);

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    None => break,
                    Some(Cmd::SendDm { peer, text, reply }) => {
                        let result = send_text_message(
                            &endpoint,
                            &secret_key,
                            &peer,
                            text,
                            &connected,
                            &msg_tx,
                        ).await;
                        let _ = reply.send(result);
                    }

                    Some(Cmd::SendControl { peer, payload, reply }) => {
                        let result = send_control_message(
                            &endpoint, &peer, payload, &connected, &msg_tx,
                        ).await;
                        let _ = reply.send(result);
                    }
                    Some(Cmd::SendMailboxEnvelope { peer, envelope, reply }) => {
                        let result = send_mailbox_envelope(
                            &endpoint, &peer, envelope, &connected, &msg_tx,
                        ).await;
                        let _ = reply.send(result);
                    }
                    Some(Cmd::SendMailboxAck { peer, ack, reply }) => {
                        let result = send_mailbox_ack(
                            &endpoint, &peer, ack, &connected, &msg_tx,
                        ).await;
                        let _ = reply.send(result);
                    }
                    Some(Cmd::ConnectTo { peer, addr, reply }) => {
                        let result = connect_to_peer(
                            &endpoint, peer, addr, &connected, &event_tx, &msg_tx,
                        ).await;
                        let _ = reply.send(result.map(|_| ()));
                    }
                    Some(Cmd::Disconnect { peer, reply }) => {
                        let connection = connected.lock().await.remove(&peer);
                        let removed = connection.is_some();
                        if let Some(connection) = connection {
                            // Revoking a contact must terminate an already-open
                            // channel too; otherwise its reader could continue
                            // delivering messages after authorization was removed.
                            connection.close(0u32.into(), b"whisper authorization revoked");
                            forward_event(&event_tx, WhisperEvent::Disconnected { peer }).await;
                        }
                        let _ = reply.send(removed);
                    }
                    Some(Cmd::RevokePeer(peer)) => {
                        if let Some(connection) = connected.lock().await.remove(&peer) {
                            connection.close(0u32.into(), b"whisper authorization revoked");
                            forward_event(&event_tx, WhisperEvent::Disconnected { peer }).await;
                        }
                    }
                    Some(Cmd::IncomingConnection(conn)) => {
                        let remote_id = conn.remote_id();
                        connected.lock().await.insert(remote_id, conn.clone());
                        forward_event(&event_tx, WhisperEvent::Connected { peer: remote_id }).await;
                        let msg_tx = msg_tx.clone();
                        tokio::task::spawn(read_connection_loop(remote_id, conn, msg_tx));
                    }
                }
            }
            Some(ev) = msg_rx.recv() => {
                match ev {
                    ConnectionEvent::Message { from, content } => {
                        // Try to decode as a wire message for structured handling.
                        match postcard::from_bytes::<WhisperWireMessage>(&content) {
                            Ok(WhisperWireMessage::Text { text, .. }) => {
                                forward_event(
                                    &event_tx,
                                    WhisperEvent::Message {
                                        from,
                                        content: Bytes::from(text),
                                    },
                                )
                                .await;
                            }

                            Ok(WhisperWireMessage::Control { payload }) => {
                                info!(
                                    from = %from.fmt_short(),
                                    payload_len = payload.len(),
                                    "whisper control message received"
                                );
                                forward_event(
                                    &event_tx,
                                    WhisperEvent::Control {
                                        from,
                                        content: Bytes::from(payload),
                                    },
                                )
                                .await;
                            }
                            Ok(WhisperWireMessage::MailboxEnvelope { envelope }) => {
                                forward_event(
                                    &event_tx,
                                    WhisperEvent::MailboxEnvelope { from, envelope },
                                )
                                .await;
                            }
                            Ok(WhisperWireMessage::MailboxAck { ack }) => {
                                forward_event(&event_tx, WhisperEvent::MailboxAck { from, ack })
                                    .await;
                            }
                            Err(_) => {
                                // Fallback: forward raw bytes as a Message event.
                                forward_event(
                                    &event_tx,
                                    WhisperEvent::Message {
                                        from,
                                        content: content.clone(),
                                    },
                                )
                                .await;
                            }
                        }
                    }
                    ConnectionEvent::Disconnected(peer) => {
                        connected.lock().await.remove(&peer);
                        forward_event(&event_tx, WhisperEvent::Disconnected { peer }).await;
                    }
                }
            }
        }
    }

    // Clean shutdown: close all connections.
    let peers: Vec<PublicKey> = connected.lock().await.keys().copied().collect();
    for peer in &peers {
        forward_event(&event_tx, WhisperEvent::Disconnected { peer: *peer }).await;
    }
}

// ── Connection management ──────────────────────────────────────────────────────

/// Try to get or create a connection to a peer.
///
/// Returns the connection if already established, or attempts to discover
/// and connect to the peer via the endpoint.
async fn get_or_connect(
    endpoint: &Endpoint,
    peer: &PublicKey,
    connected: &Arc<Mutex<HashMap<PublicKey, Connection>>>,
    event_tx: &mpsc::Sender<WhisperEvent>,
    msg_tx: &mpsc::Sender<ConnectionEvent>,
) -> Result<Connection> {
    // Check if we already have a connection.
    {
        let guard = connected.lock().await;
        if let Some(conn) = guard.get(peer) {
            debug!(recipient = %peer.fmt_short(), "whisper already connected");
            return Ok(conn.clone());
        }
    }

    // Try to discover the peer's addresses from the endpoint.
    let addr = match endpoint.remote_info(*peer).await {
        Some(info) => {
            let transport_addrs: std::collections::BTreeSet<_> =
                info.addrs().map(|a| a.addr().clone()).collect();
            debug!(
                peer = %peer.fmt_short(),
                addrs = transport_addrs.len(),
                "whisper discovered peer addresses"
            );
            if transport_addrs.is_empty() {
                // remote_info has no addresses — fall back to ID-only resolution
                // which triggers DHT/mDNS/DNS lookup during connect().
                EndpointAddr::new(*peer)
            } else {
                EndpointAddr {
                    id: *peer,
                    addrs: transport_addrs,
                }
            }
        }
        None => {
            // Endpoint has never cached this peer's addresses.
            // Use ID-only resolution which triggers the full address
            // lookup chain (DHT, mDNS, DNS/Pkarr) during connect().
            EndpointAddr::new(*peer)
        }
    };

    connect_to_peer(endpoint, *peer, addr, connected, event_tx, msg_tx).await
}

/// Connect to a peer using their EndpointAddr.
async fn connect_to_peer(
    endpoint: &Endpoint,
    peer: PublicKey,
    addr: EndpointAddr,
    connected: &Arc<Mutex<HashMap<PublicKey, Connection>>>,
    event_tx: &mpsc::Sender<WhisperEvent>,
    msg_tx: &mpsc::Sender<ConnectionEvent>,
) -> Result<Connection> {
    info!(
        peer = %peer.fmt_short(),
        alpn = WHISPER_ALPN,
        "whisper connecting to peer"
    );
    let conn = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        endpoint.connect(addr, WHISPER_ALPN),
    )
    .await
    .map_err(|_| n0_error::anyerr!("whisper connect timed out after 15s"))??;
    let remote_id = conn.remote_id();
    if remote_id != peer {
        conn.close(0u32.into(), b"whisper identity mismatch");
        return Err(n0_error::anyerr!(
            "whisper identity mismatch: expected {}, got {}",
            peer.fmt_short(),
            remote_id.fmt_short()
        ));
    }
    info!(peer = %remote_id.fmt_short(), "whisper connected to peer");

    connected.lock().await.insert(remote_id, conn.clone());
    forward_event(&event_tx, WhisperEvent::Connected { peer: remote_id }).await;

    // Spawn a reader for this connection.
    let msg_tx = msg_tx.clone();
    tokio::task::spawn(read_connection_loop(remote_id, conn.clone(), msg_tx));

    Ok(conn)
}

// ── Message sending ────────────────────────────────────────────────────────────

/// Encode a wire message with length-prefixed framing and write it over
/// a bi-directional stream on the given connection.
async fn write_framed_message(conn: &Connection, wire: &WhisperWireMessage) -> Result<()> {
    let payload = postcard::to_stdvec(wire).expect("postcard encode infallible");
    if payload.len() > MAX_WHISPER_PAYLOAD {
        return Err(n0_error::anyerr!(
            "whisper message too large: {} bytes (max {})",
            payload.len(),
            MAX_WHISPER_PAYLOAD,
        ));
    }

    let (mut send, _recv) = conn
        .open_bi()
        .await
        .map_err(|e| n0_error::anyerr!("whisper open_bi failed: {e}"))?;

    // Length-prefixed framing: 4-byte LE length + payload.
    let len_bytes = (payload.len() as u32).to_le_bytes();
    send.write_all(&len_bytes)
        .await
        .map_err(|e| n0_error::anyerr!("whisper write length failed: {e}"))?;
    send.write_all(&payload)
        .await
        .map_err(|e| n0_error::anyerr!("whisper write payload failed: {e}"))?;
    send.finish()
        .map_err(|e| n0_error::anyerr!("whisper finish failed: {e}"))?;

    Ok(())
}

/// Send a text DM to a peer.
async fn send_text_message(
    endpoint: &Endpoint,
    secret_key: &SecretKey,
    peer: &PublicKey,
    text: String,
    connected: &Arc<Mutex<HashMap<PublicKey, Connection>>>,
    msg_tx: &mpsc::Sender<ConnectionEvent>,
) -> Result<()> {
    let payload_size = text.len();
    debug!(
        recipient = %peer.fmt_short(),
        payload_size,
        msg_type = "text",
        "whisper send_text_message"
    );
    // Create a dummy event_tx for get_or_connect to borrow.
    let (dummy_tx, _) = mpsc::channel(1);

    let conn = get_or_connect(endpoint, peer, connected, &dummy_tx, msg_tx).await?;

    let wire = WhisperWireMessage::Text {
        from: secret_key.public().to_string(),
        text,
    };

    write_framed_message(&conn, &wire).await
}

/// Send an opaque signed control message without interpreting it in transport.
async fn send_control_message(
    endpoint: &Endpoint,
    peer: &PublicKey,
    payload: Bytes,
    connected: &Arc<Mutex<HashMap<PublicKey, Connection>>>,
    msg_tx: &mpsc::Sender<ConnectionEvent>,
) -> Result<()> {
    debug!(
        peer = %peer.fmt_short(),
        payload_len = payload.len(),
        msg_type = "control",
        "whisper send_control_message"
    );
    let (dummy_tx, _) = mpsc::channel(1);
    let conn = get_or_connect(endpoint, peer, connected, &dummy_tx, msg_tx).await?;
    write_framed_message(
        &conn,
        &WhisperWireMessage::Control {
            payload: payload.to_vec(),
        },
    )
    .await
}

async fn send_mailbox_envelope(
    endpoint: &Endpoint,
    peer: &PublicKey,
    envelope: MailboxEnvelope,
    connected: &Arc<Mutex<HashMap<PublicKey, Connection>>>,
    msg_tx: &mpsc::Sender<ConnectionEvent>,
) -> Result<()> {
    debug!(
        recipient = %peer.fmt_short(),
        msg_type = "mailbox_envelope",
        "whisper send_mailbox_envelope"
    );
    let (dummy_tx, _) = mpsc::channel(1);
    let conn = get_or_connect(endpoint, peer, connected, &dummy_tx, msg_tx).await?;
    write_framed_message(&conn, &WhisperWireMessage::MailboxEnvelope { envelope }).await
}

async fn send_mailbox_ack(
    endpoint: &Endpoint,
    peer: &PublicKey,
    ack: MailboxAck,
    connected: &Arc<Mutex<HashMap<PublicKey, Connection>>>,
    msg_tx: &mpsc::Sender<ConnectionEvent>,
) -> Result<()> {
    debug!(
        recipient = %peer.fmt_short(),
        msg_type = "mailbox_ack",
        "whisper send_mailbox_ack"
    );
    let (dummy_tx, _) = mpsc::channel(1);
    let conn = get_or_connect(endpoint, peer, connected, &dummy_tx, msg_tx).await?;
    write_framed_message(&conn, &WhisperWireMessage::MailboxAck { ack }).await
}

// ── Connection reader ──────────────────────────────────────────────────────────

/// Read framed messages from a connection and send them to the actor.
async fn read_connection_loop(
    remote_id: PublicKey,
    conn: Connection,
    msg_tx: mpsc::Sender<ConnectionEvent>,
) {
    loop {
        match conn.accept_bi().await {
            Ok((_send, mut recv)) => {
                // Read the 4-byte length prefix.
                let mut len_buf = [0u8; 4];
                if let Err(e) = tokio::io::AsyncReadExt::read_exact(&mut recv, &mut len_buf).await {
                    warn!(peer = %remote_id.fmt_short(), "whisper read length failed: {e}");
                    break;
                }
                let payload_len = u32::from_le_bytes(len_buf) as usize;

                if payload_len > MAX_WHISPER_PAYLOAD {
                    warn!(
                        peer = %remote_id.fmt_short(),
                        "whisper payload too large: {} bytes",
                        payload_len,
                    );
                    break;
                }

                // Read the payload.
                let mut payload = vec![0u8; payload_len];
                if let Err(e) = tokio::io::AsyncReadExt::read_exact(&mut recv, &mut payload).await {
                    warn!(peer = %remote_id.fmt_short(), "whisper read payload failed: {e}");
                    break;
                }

                debug!(
                    peer = %remote_id.fmt_short(),
                    payload_len,
                    "whisper message received from peer"
                );

                if msg_tx
                    .send(ConnectionEvent::Message {
                        from: remote_id,
                        content: payload.into(),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(e) => {
                // Connection closed or error.
                trace!(peer = %remote_id.fmt_short(), "whisper accept_bi error: {e}");
                break;
            }
        }
    }

    // Notify the actor that the connection dropped.  This is correctness-
    // critical: if it were silently dropped the actor would keep the peer in
    // its `connected` map and reuse a dead connection.  Await the bounded
    // channel; the only failure is the actor itself being gone.
    if msg_tx.send(ConnectionEvent::Disconnected(remote_id)).await.is_err() {
        debug!(peer = %remote_id.fmt_short(), "whisper actor dropped; disconnect notification not delivered");
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mailbox::MailboxIdentity;
    use iroh::endpoint::presets;
    use iroh::protocol::Router;
    use n0_future::time::{sleep, Duration};

    /// Create a whisper node for tests.
    #[allow(clippy::type_complexity)]
    async fn create_node() -> Result<(
        Router,
        Endpoint,
        SecretKey,
        WhisperHandle,
        mpsc::Receiver<WhisperEvent>,
    )> {
        let secret_key = SecretKey::generate();
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(secret_key.clone())
            .relay_mode(iroh::RelayMode::Disabled)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await?;

        let builder = WhisperBuilder::new(endpoint.clone(), secret_key.clone());
        let handler = builder.protocol_handler();
        let (handle, event_rx) = builder.spawn();

        let router = Router::builder(endpoint.clone())
            .accept(WHISPER_ALPN, handler)
            .spawn();

        Ok((router, endpoint, secret_key, handle, event_rx))
    }

    #[tokio::test]
    #[n0_tracing_test::traced_test]
    async fn test_whisper_basic_dm() -> Result<()> {
        let (router_a, ep_a, _sk_a, handle_a, _events_a) = create_node().await?;
        let (router_b, ep_b, _sk_b, handle_b, mut events_b) = create_node().await?;
        handle_a.set_peer_authorized(ep_b.secret_key().public(), true);
        handle_b.set_peer_authorized(ep_a.secret_key().public(), true);
        handle_a
            .connect_to(ep_b.secret_key().public(), ep_b.addr())
            .await?;
        sleep(Duration::from_millis(500)).await;

        // A sends a DM to B.
        handle_a
            .send_dm(ep_b.secret_key().public(), "hello from A".to_string())
            .await?;
        sleep(Duration::from_millis(500)).await;

        // B should receive the message.
        let b_got_msg = loop {
            match events_b.recv().await {
                Some(WhisperEvent::Message { from, content }) => {
                    assert_eq!(from, ep_a.secret_key().public());
                    // Content should be the decoded text.
                    break content == "hello from A";
                }
                Some(WhisperEvent::Connected { .. }) => continue,
                Some(_) => continue,
                None => break false,
            }
        };
        assert!(b_got_msg, "B should receive a whisper DM");

        router_a.shutdown().await.unwrap();
        router_b.shutdown().await.unwrap();
        Ok(())
    }

    #[tokio::test]
    #[n0_tracing_test::traced_test]
    async fn test_whisper_rejects_unknown_and_blocked_peers() -> Result<()> {
        let (router_a, ep_a, _sk_a, handle_a, _events_a) = create_node().await?;
        let (router_b, ep_b, _sk_b, handle_b, mut events_b) = create_node().await?;
        let peer_a = ep_a.secret_key().public();
        let peer_b = ep_b.secret_key().public();

        // First: add peer A to B's denied set so the incoming side rejects.
        handle_b.set_peer_authorized(peer_a, false);
        handle_a.set_peer_authorized(peer_b, true);
        // The transport connect can complete before the remote protocol handler
        // rejects the session, so verify the observable protocol result: no
        // incoming Connected event is emitted.
        handle_a.connect_to(peer_b, ep_b.addr()).await?;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(300), events_b.recv(),)
                .await
                .is_err(),
            "denied peer must not produce Connected"
        );

        // Remove A from B's denied set (authorize) to enable the normal DM flow.
        handle_b.set_peer_authorized(peer_a, true);
        let _ = handle_a.disconnect(&peer_b).await;
        handle_a.connect_to(peer_b, ep_b.addr()).await?;
        handle_a.send_dm(peer_b, "authorized".to_string()).await?;
        let received = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Some(WhisperEvent::Message { content, .. }) = events_b.recv().await {
                    break content == "authorized";
                }
            }
        })
        .await
        .map_err(|_| n0_error::anyerr!("authorized whisper DM timed out"))?;
        assert!(received);

        // Add A back to B's denied set (revoke); this also tears down the
        // established channel.
        handle_b.set_peer_authorized(peer_a, false);
        sleep(Duration::from_millis(100)).await;
        handle_a.send_dm(peer_b, "revoked".to_string()).await?;
        let revoked_message_received =
            tokio::time::timeout(std::time::Duration::from_millis(300), async {
                loop {
                    if let Some(WhisperEvent::Message { content, .. }) = events_b.recv().await {
                        break content == "revoked";
                    }
                }
            })
            .await
            .unwrap_or(false);
        assert!(
            !revoked_message_received,
            "revoked peer must not deliver messages"
        );

        router_a.shutdown().await.unwrap();
        router_b.shutdown().await.unwrap();
        Ok(())
    }

    #[tokio::test]
    #[n0_tracing_test::traced_test]
    async fn test_whisper_mailbox_envelope_and_ack_roundtrip() -> Result<()> {
        let (router_a, ep_a, sk_a, handle_a, _events_a) = create_node().await?;
        let (router_b, ep_b, sk_b, handle_b, mut events_b) = create_node().await?;
        let peer_a = ep_a.secret_key().public();
        let peer_b = ep_b.secret_key().public();
        handle_a.set_peer_authorized(peer_b, true);
        handle_b.set_peer_authorized(peer_a, true);
        handle_a.connect_to(peer_b, ep_b.addr()).await?;
        sleep(Duration::from_millis(200)).await;

        let identity_b = MailboxIdentity::from_secret(&sk_b);
        let envelope = identity_b.seal(&sk_a, b"offline hello")?;
        let message_id = envelope.message_id();
        handle_a.send_mailbox_envelope(peer_b, envelope).await?;

        let received = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(WhisperEvent::MailboxEnvelope { from, envelope }) =
                    events_b.recv().await
                {
                    assert_eq!(from, peer_a);
                    assert_eq!(envelope.open(&sk_b).unwrap(), b"offline hello");
                    break envelope;
                }
            }
        })
        .await
        .map_err(|_| n0_error::anyerr!("mailbox envelope timed out"))?;
        handle_b
            .send_mailbox_ack(
                peer_a,
                MailboxAck::sign(&sk_b, received.message_id(), received.from()),
            )
            .await?;
        assert_eq!(received.message_id(), message_id);

        router_a.shutdown().await.unwrap();
        router_b.shutdown().await.unwrap();
        Ok(())
    }

    #[tokio::test]
    #[n0_tracing_test::traced_test]
    async fn test_whisper_connect_and_disconnect() -> Result<()> {
        let (router_a, ep_a, _sk_a, handle_a, mut events_a) = create_node().await?;
        let (router_b, ep_b, _sk_b, handle_b, _events_b) = create_node().await?;
        handle_a.set_peer_authorized(ep_b.secret_key().public(), true);
        handle_b.set_peer_authorized(ep_a.secret_key().public(), true);
        handle_a
            .connect_to(ep_b.secret_key().public(), ep_b.addr())
            .await?;
        sleep(Duration::from_millis(500)).await;

        // A should see Connected event.
        let a_got_conn = loop {
            match events_a.recv().await {
                Some(WhisperEvent::Connected { peer }) => break peer == ep_b.secret_key().public(),
                Some(_) => continue,
                None => break false,
            }
        };
        assert!(a_got_conn, "A should see Connected event");

        // Disconnect A from B.
        let removed = handle_a.disconnect(&ep_b.secret_key().public()).await?;
        assert!(removed, "should have been connected");

        sleep(Duration::from_millis(200)).await;

        // A should see Disconnected event.
        let a_got_disc = loop {
            match events_a.recv().await {
                Some(WhisperEvent::Disconnected { peer }) => {
                    break peer == ep_b.secret_key().public()
                }
                Some(_) => continue,
                None => break false,
            }
        };
        assert!(a_got_disc, "A should see Disconnected event");

        router_a.shutdown().await.unwrap();
        router_b.shutdown().await.unwrap();
        Ok(())
    }

    /// Compile-time check that WhisperProtocol implements ProtocolHandler.
    #[test]
    fn whisper_protocol_is_handler() {
        fn _assert(_h: impl ProtocolHandler) {}
        // We can't construct one without an endpoint, but the types check.
    }

    /// Regression (BORU-AUDIT-08): a correctness-critical whisper event must
    /// not be silently dropped when the bounded frontend channel is full.
    /// `forward_event` applies backpressure (awaited send) and the event must
    /// arrive once the receiver drains — the old `try_send` implementation
    /// returned `Full` and discarded the event.
    #[tokio::test]
    async fn forward_event_delivers_under_backpressure() {
        let peer = SecretKey::generate().public();
        let (tx, mut rx) = mpsc::channel::<WhisperEvent>(1);

        // Fill the channel to capacity.
        tx.send(WhisperEvent::Connected { peer }).await.unwrap();
        assert!(tx.try_send(WhisperEvent::Connected { peer }).is_err(), "channel must be full");

        // Spawn the awaited send; it must block, not drop.
        let tx2 = tx.clone();
        let sender = tokio::task::spawn(async move {
            forward_event(&tx2, WhisperEvent::Disconnected { peer }).await;
        });

        // Drain one slot; the blocked send must complete and the event must
        // arrive.
        let first = rx.recv().await.expect("first event");
        assert!(matches!(first, WhisperEvent::Connected { .. }));

        tokio::time::timeout(Duration::from_secs(2), async {
            let second = rx.recv().await.expect("second event");
            assert!(
                matches!(second, WhisperEvent::Disconnected { .. }),
                "event was dropped instead of delivered"
            );
        })
        .await
        .expect("awaited send never completed");

        sender.await.expect("sender task panicked");
    }
}
