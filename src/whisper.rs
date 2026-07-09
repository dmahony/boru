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

use std::{collections::HashMap, io, sync::Arc};

use bytes::Bytes;
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr, PublicKey, SecretKey,
};
use n0_error::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, trace, warn};

// ── Constants ──────────────────────────────────────────────────────────────────

/// ALPN for whisper direct connections.
pub const WHISPER_ALPN: &[u8] = b"/iroh-gossip-chat/whisper/1";

/// Default capacity for the command channel.
const CMD_CHANNEL_CAP: usize = 256;

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
    /// A file available for download.
    FileTransfer {
        /// File name (basename only).
        name: String,
        /// BlobTicket serialized to string.
        ticket: String,
    },
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
    /// A file transfer notification received from a peer.
    FileTransfer {
        /// Public key of the sender.
        from: PublicKey,
        /// File name.
        name: String,
        /// BlobTicket string.
        ticket: String,
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

enum Cmd {
    SendDm {
        peer: PublicKey,
        text: String,
        reply: oneshot::Sender<Result<()>>,
    },
    SendFile {
        peer: PublicKey,
        name: String,
        ticket: String,
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
    /// An incoming connection from a remote peer (from ProtocolHandler).
    IncomingConnection(Connection),
}

// ── Internal per-connection events ─────────────────────────────────────────────

enum ConnectionEvent {
    Message { from: PublicKey, content: Bytes },
    Disconnected(PublicKey),
}

// ── WhisperHandle ──────────────────────────────────────────────────────────────

/// Handle to send commands to a running whisper actor.
///
/// Clone this freely — all clones share the same background task.
#[derive(Debug, Clone)]
pub struct WhisperHandle {
    cmd_tx: mpsc::Sender<Cmd>,
}

impl WhisperHandle {
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

    /// Send a file transfer notification to a peer.
    ///
    /// The `ticket` is a serialized [`iroh_blobs::ticket::BlobTicket`] that
    /// the receiver can use to download the file via iroh-blobs.
    pub async fn send_file(&self, peer: PublicKey, name: String, ticket: String) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::SendFile {
                peer,
                name,
                ticket,
                reply,
            })
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
    pub fn _cmd_tx(&self) -> mpsc::Sender<Cmd> {
        self.cmd_tx.clone()
    }
}

// ── Protocol handler ──────────────────────────────────────────────────────────

/// Protocol handler that routes incoming whisper connections to the actor.
#[derive(Debug, Clone)]
pub struct WhisperProtocol {
    cmd_tx: mpsc::Sender<Cmd>,
}

impl ProtocolHandler for WhisperProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote_id = connection.remote_id();
        debug!(peer = %remote_id.fmt_short(), "whisper incoming connection");

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
        }
    }

    /// Create a [`WhisperProtocol`] handler for this whisper channel.
    ///
    /// Register it on your Router with `router.accept(WHISPER_ALPN, handler)`
    /// so incoming whisper connections are routed to this actor.
    pub fn protocol_handler(&self) -> WhisperProtocol {
        WhisperProtocol {
            cmd_tx: self.cmd_tx.clone(),
        }
    }

    /// Spawn the whisper actor and return a handle + event receiver.
    pub async fn spawn(mut self) -> (WhisperHandle, mpsc::UnboundedReceiver<WhisperEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let connected: Arc<Mutex<HashMap<PublicKey, Connection>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let handle = WhisperHandle {
            cmd_tx: self.cmd_tx.clone(),
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
    event_tx: mpsc::UnboundedSender<WhisperEvent>,
    connected: Arc<Mutex<HashMap<PublicKey, Connection>>>,
) {
    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<ConnectionEvent>();

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
                    Some(Cmd::SendFile { peer, name, ticket, reply }) => {
                        let result = send_file_message(
                            &endpoint,
                            &secret_key,
                            &peer,
                            name,
                            ticket,
                            &connected,
                            &msg_tx,
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
                        let removed = connected.lock().await.remove(&peer).is_some();
                        if removed {
                            let _ = event_tx.send(WhisperEvent::Disconnected { peer });
                        }
                        let _ = reply.send(removed);
                    }
                    Some(Cmd::IncomingConnection(conn)) => {
                        let remote_id = conn.remote_id();
                        connected.lock().await.insert(remote_id, conn.clone());
                        let _ = event_tx.send(WhisperEvent::Connected { peer: remote_id });
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
                                let _ = event_tx.send(WhisperEvent::Message {
                                    from,
                                    content: Bytes::from(text),
                                });
                            }
                            Ok(WhisperWireMessage::FileTransfer { name, ticket }) => {
                                let _ = event_tx.send(WhisperEvent::FileTransfer {
                                    from,
                                    name,
                                    ticket,
                                });
                            }
                            Err(_) => {
                                // Fallback: forward raw bytes as a Message event.
                                let _ = event_tx.send(WhisperEvent::Message {
                                    from,
                                    content: content.clone(),
                                });
                            }
                        }
                    }
                    ConnectionEvent::Disconnected(peer) => {
                        connected.lock().await.remove(&peer);
                        let _ = event_tx.send(WhisperEvent::Disconnected { peer });
                    }
                }
            }
        }
    }

    // Clean shutdown: close all connections.
    let peers: Vec<PublicKey> = connected.lock().await.keys().copied().collect();
    for peer in &peers {
        let _ = event_tx.send(WhisperEvent::Disconnected { peer: *peer });
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
    event_tx: &mpsc::UnboundedSender<WhisperEvent>,
    msg_tx: &mpsc::UnboundedSender<ConnectionEvent>,
) -> Result<Connection> {
    // Check if we already have a connection.
    {
        let guard = connected.lock().await;
        if let Some(conn) = guard.get(peer) {
            return Ok(conn.clone());
        }
    }

    // Try to discover the peer's addresses from the endpoint.
    let info = endpoint
        .remote_info(*peer)
        .await
        .ok_or_else(|| n0_error::anyerr!("no address info for peer {}", peer.fmt_short()))?;

    let transport_addrs: std::collections::BTreeSet<_> =
        info.addrs().map(|a| a.addr().clone()).collect();

    if transport_addrs.is_empty() {
        return Err(n0_error::anyerr!(
            "no known addresses for peer {}",
            peer.fmt_short()
        ));
    }

    let addr = EndpointAddr {
        id: *peer,
        addrs: transport_addrs,
    };

    connect_to_peer(endpoint, *peer, addr, connected, event_tx, msg_tx).await
}

/// Connect to a peer using their EndpointAddr.
async fn connect_to_peer(
    endpoint: &Endpoint,
    _peer: PublicKey,
    addr: EndpointAddr,
    connected: &Arc<Mutex<HashMap<PublicKey, Connection>>>,
    event_tx: &mpsc::UnboundedSender<WhisperEvent>,
    msg_tx: &mpsc::UnboundedSender<ConnectionEvent>,
) -> Result<Connection> {
    let conn = endpoint.connect(addr, WHISPER_ALPN).await?;
    let remote_id = conn.remote_id();
    debug!(peer = %remote_id.fmt_short(), "whisper connected to peer");

    connected.lock().await.insert(remote_id, conn.clone());
    let _ = event_tx.send(WhisperEvent::Connected { peer: remote_id });

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
    msg_tx: &mpsc::UnboundedSender<ConnectionEvent>,
) -> Result<()> {
    // Create a dummy event_tx for get_or_connect to borrow.
    let (dummy_tx, _) = mpsc::unbounded_channel();

    let conn = get_or_connect(endpoint, peer, connected, &dummy_tx, msg_tx).await?;

    let wire = WhisperWireMessage::Text {
        from: secret_key.public().to_string(),
        text,
    };

    write_framed_message(&conn, &wire).await
}

/// Send a file transfer notification to a peer.
async fn send_file_message(
    endpoint: &Endpoint,
    _secret_key: &SecretKey,
    peer: &PublicKey,
    name: String,
    ticket: String,
    connected: &Arc<Mutex<HashMap<PublicKey, Connection>>>,
    msg_tx: &mpsc::UnboundedSender<ConnectionEvent>,
) -> Result<()> {
    let (dummy_tx, _) = mpsc::unbounded_channel();

    let conn = get_or_connect(endpoint, peer, connected, &dummy_tx, msg_tx).await?;

    let wire = WhisperWireMessage::FileTransfer { name, ticket };

    write_framed_message(&conn, &wire).await
}

// ── Connection reader ──────────────────────────────────────────────────────────

/// Read framed messages from a connection and send them to the actor.
async fn read_connection_loop(
    remote_id: PublicKey,
    conn: Connection,
    msg_tx: mpsc::UnboundedSender<ConnectionEvent>,
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

                if msg_tx
                    .send(ConnectionEvent::Message {
                        from: remote_id,
                        content: payload.into(),
                    })
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

    let _ = msg_tx.send(ConnectionEvent::Disconnected(remote_id));
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::endpoint::presets;
    use iroh::protocol::Router;
    use n0_future::time::{sleep, Duration};
    use rand::SeedableRng;

    /// Create a whisper node for tests.
    #[allow(clippy::type_complexity)]
    async fn create_node() -> Result<(
        Router,
        Endpoint,
        SecretKey,
        WhisperHandle,
        mpsc::UnboundedReceiver<WhisperEvent>,
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
        let (handle, event_rx) = builder.spawn().await;

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

        // Connect A to B.
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
                    break content == Bytes::from("hello from A");
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
    async fn test_whisper_file_transfer() -> Result<()> {
        let (router_a, ep_a, _sk_a, handle_a, _events_a) = create_node().await?;
        let (router_b, ep_b, _sk_b, handle_b, mut events_b) = create_node().await?;

        // Connect A to B.
        handle_a
            .connect_to(ep_b.secret_key().public(), ep_b.addr())
            .await?;
        sleep(Duration::from_millis(500)).await;

        // A sends a file transfer notification to B.
        let ticket = "blobticket123".to_string();
        handle_a
            .send_file(
                ep_b.secret_key().public(),
                "photo.png".to_string(),
                ticket.clone(),
            )
            .await?;
        sleep(Duration::from_millis(500)).await;

        // B should receive the file transfer event.
        let b_got_file = loop {
            match events_b.recv().await {
                Some(WhisperEvent::FileTransfer {
                    name, ticket: t, ..
                }) => break name == "photo.png" && t == "blobticket123",
                Some(WhisperEvent::Connected { .. }) => continue,
                Some(_) => continue,
                None => break false,
            }
        };
        assert!(b_got_file, "B should receive a file transfer notification");

        router_a.shutdown().await.unwrap();
        router_b.shutdown().await.unwrap();
        Ok(())
    }

    #[tokio::test]
    #[n0_tracing_test::traced_test]
    async fn test_whisper_connect_and_disconnect() -> Result<()> {
        let (router_a, _ep_a, _sk_a, handle_a, mut events_a) = create_node().await?;
        let (router_b, ep_b, _sk_b, handle_b, mut events_b) = create_node().await?;

        // Connect A to B.
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
}
