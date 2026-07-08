//! Minimal iroh-doc-style small-room messaging for <=10 members.
//!
//! Instead of using the gossip broadcast tree, each peer opens a direct QUIC
//! connection to every other peer in the room. Messages are appended to a local
//! in-memory log (the "doc") and broadcast over all active connections.
//!
//! Every message carries a monotonic send timestamp so the receiver can measure
//! one-way latency (modulo local clock offset -- both peers use `Instant` on
//! their own machine).
//!
//! # Latency measurement
//!
//! For each received message, `received_at - sent_at` gives the one-way
//! latency from the sender's perspective (wall-clock). Because both
//! timestamps come from `Instant::now()` on their respective machines,
//! this is only meaningful when system clocks are synchronised (NTP).
//!
//! A more accurate approach for local testing (same machine) is to
//! record `recv_local_at` at the receiver and compare it to the sender's
//! `sent_at` -- since both are `Instant` on the same machine when the
//! test co-locates peers, this gives true one-way latency.

use std::{collections::HashMap, sync::Arc, time::Instant};

use bytes::Bytes;
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr, PublicKey, SecretKey,
};
use n0_error::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, trace, warn};

// -- Constants -----------------------------------------------------------------

/// ALPN for small-room direct connections.
pub const SMALL_ROOM_ALPN: &[u8] = b"/iroh-gossip-chat/small-room/1";

/// Maximum number of members in a room before falling back to gossip broadcast.
///
/// Rooms with `<= SMALL_ROOM_MAX_SIZE` members should use direct QUIC
/// connections (small_room) instead of the gossip broadcast tree, because
/// the direct-connect approach has lower latency for small groups.
pub const SMALL_ROOM_MAX_SIZE: usize = 10;

/// Returns `true` if the given member count fits within the small-room threshold.
///
/// Use this as the decision hook: when opening/joining a room, check whether
/// `room_size_fits_small_room(num_members)` is `true`. If so, use
/// [`SmallRoomBuilder`] / [`SMALL_ROOM_ALPN`] instead of the gossip protocol.
pub fn room_size_fits_small_room(member_count: usize) -> bool {
    // If the room has 0 members it's just being created — use small_room
    // since it will be small.  If it has ≤SMALL_ROOM_MAX_SIZE, the
    // direct-connect approach is appropriate.
    member_count == 0 || member_count <= SMALL_ROOM_MAX_SIZE
}

/// Default capacity for the command channel.
const CMD_CHANNEL_CAP: usize = 256;

// -- Types ---------------------------------------------------------------------

/// A single entry in the room document (append-only log).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocEntry {
    /// Public key of the sender (hex-encoded for serialization simplicity).
    pub from: String,
    /// The message text.
    pub text: String,
    /// Monotonic timestamp on the sender's machine when this entry was created
    /// (stored as nanoseconds since process start for serialization).
    #[serde(default)]
    pub sent_at_ns: u128,
    /// Monotonic timestamp on the receiver's machine when this entry arrived.
    #[serde(default)]
    pub received_at_ns: u128,
}

impl DocEntry {
    /// Approximate monotonic Instant corresponding to the sent_at_ns value.
    pub fn sent_instant(&self) -> Instant {
        let now = Instant::now();
        let now_ns = duration_to_nanos(now.duration_since(Instant::now()));
        now.checked_sub(nanos_to_duration(now_ns.saturating_sub(self.sent_at_ns)))
            .unwrap_or(now)
    }
}

/// A wire-frame message exchanged between peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum WireMessage {
    /// A chat message from a peer.
    Message {
        from: String,
        text: String,
        send_instant_ns: u128,
    },
    /// Ping for keepalive / latency probing.
    Ping { send_instant_ns: u128 },
    /// Pong response to a Ping.
    Pong {
        send_instant_ns: u128,
        recv_instant_ns: u128,
    },
}

/// Events emitted from the small-room protocol.
#[derive(Debug, Clone)]
pub enum SmallRoomEvent {
    /// A decoded message from a peer.
    Message {
        /// Public key of the sender.
        from: PublicKey,
        /// The decoded message text.
        text: String,
        /// Timestamp when the message was sent (sender's `Instant`).
        sent_at: Instant,
        /// Timestamp when the message was received (receiver's `Instant`).
        received_at: Instant,
    },
    /// A peer connected to us.
    PeerConnected(PublicKey),
    /// A peer disconnected from us.
    PeerDisconnected(PublicKey),
    /// The room has been closed.
    Closed,
    /// An error occurred within the room protocol.
    Error(String),
}

/// Latency sample recorded by the latency probe.
#[derive(Debug, Clone)]
pub struct LatencySample {
    /// The peer this sample measures.
    pub peer: PublicKey,
    /// Round-trip time of the ping-pong exchange.
    pub rtt: std::time::Duration,
    /// Monotonic timestamp when this sample was recorded.
    pub at: Instant,
}

/// Handle to send commands and subscribe to events for a small room.
#[derive(Debug, Clone)]
pub struct SmallRoomHandle {
    cmd_tx: mpsc::Sender<Cmd>,
    log: Arc<RwLock<Vec<DocEntry>>>,
    latency_log: Arc<Mutex<Vec<LatencySample>>>,
    connected: Arc<Mutex<HashMap<PublicKey, Connection>>>,
}

impl SmallRoomHandle {
    /// Broadcast a text message to all connected peers.
    pub async fn broadcast(&self, text: String) -> Result<()> {
        self.cmd_tx
            .send(Cmd::Broadcast(text))
            .await
            .map_err(|_| n0_error::anyerr!("small room actor dropped"))?;
        Ok(())
    }

    /// Read the current document log (all entries received so far).
    pub async fn read_log(&self) -> Vec<DocEntry> {
        self.log.read().await.clone()
    }

    /// Read entries since a given index.
    pub async fn read_log_since(&self, since: usize) -> Vec<DocEntry> {
        let log = self.log.read().await;
        if since >= log.len() {
            return Vec::new();
        }
        log[since..].to_vec()
    }

    /// Get the latest message, if any.
    pub async fn latest_entry(&self) -> Option<DocEntry> {
        let log = self.log.read().await;
        log.last().cloned()
    }

    /// Number of entries in the doc log.
    pub async fn log_len(&self) -> usize {
        self.log.read().await.len()
    }

    /// Get recent latency samples.
    pub async fn latency_samples(&self) -> Vec<LatencySample> {
        self.latency_log.lock().await.clone()
    }

    /// Get a snapshot of live connections.
    pub async fn connected_peers(&self) -> Vec<PublicKey> {
        self.connected.lock().await.keys().copied().collect()
    }

    /// Connect to a peer using their EndpointAddr.
    pub async fn connect_to(&self, addr: EndpointAddr) -> Result<()> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.cmd_tx
            .send(Cmd::ConnectTo { addr, reply })
            .await
            .map_err(|_| n0_error::anyerr!("small room actor dropped"))?;
        let _ = rx
            .await
            .map_err(|_| n0_error::anyerr!("actor dropped reply"))?;
        Ok(())
    }
}

// -- Protocol handler ---------------------------------------------------------

/// Protocol handler that routes incoming connections to the small-room actor.
#[derive(Clone, Debug)]
pub struct SmallRoomProtocol {
    cmd_tx: mpsc::Sender<Cmd>,
}

impl ProtocolHandler for SmallRoomProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote_id = connection.remote_id();
        debug!(peer = %remote_id.fmt_short(), "small-room incoming connection");
        self.cmd_tx
            .send(Cmd::IncomingConnection(connection))
            .await
            .map_err(|_| AcceptError::from_err(n0_error::anyerr!("actor dropped")))?;
        Ok(())
    }
}

// -- Internal commands / actor -------------------------------------------------

enum Cmd {
    Broadcast(String),
    IncomingConnection(Connection),
    ConnectTo {
        addr: EndpointAddr,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
}

/// Run the small-room actor for a single peer.
#[allow(clippy::too_many_arguments)]
async fn run_actor(
    endpoint: Endpoint,
    secret_key: SecretKey,
    mut cmd_rx: mpsc::Receiver<Cmd>,
    event_tx: mpsc::UnboundedSender<SmallRoomEvent>,
    log: Arc<RwLock<Vec<DocEntry>>>,
    latency_log: Arc<Mutex<Vec<LatencySample>>>,
    connected: Arc<Mutex<HashMap<PublicKey, Connection>>>,
) {
    let local_pk = secret_key.public();
    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<ConnectionEvent>();

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    None => break,
                    Some(Cmd::Broadcast(text)) => {
                        let now = Instant::now();
                        let now_ns = nanos_since_epoch(now);
                        let wire = WireMessage::Message {
                            from: local_pk.to_string(),
                            text: text.clone(),
                            send_instant_ns: now_ns,
                        };
                        let payload = postcard::to_stdvec(&wire)
                            .expect("postcard encode infallible");
                        let peers = connected.lock().await;
                        for (_peer, conn) in peers.iter() {
                            match conn.open_uni().await {
                                Ok(mut send) => {
                                    if let Err(e) = send.write_all(&payload).await {
                                        warn!("write error: {e}");
                                    }
                                    let _ = send.finish();
                                }
                                Err(e) => {
                                    warn!("open_uni error: {e}");
                                }
                            }
                        }
                        log.write().await.push(DocEntry {
                            from: local_pk.to_string(),
                            text,
                            sent_at_ns: now_ns,
                            received_at_ns: 0,
                        });
                    }
                    Some(Cmd::IncomingConnection(conn)) => {
                        let remote_id = conn.remote_id();
                        connected.lock().await.insert(remote_id, conn.clone());
                        let _ = event_tx.send(SmallRoomEvent::PeerConnected(remote_id));
                        let msg_tx = msg_tx.clone();
                        tokio::task::spawn(read_connection_loop(remote_id, conn, msg_tx));
                    }
                    Some(Cmd::ConnectTo { addr, reply }) => {
                        let result = connect_to_peer(
                            &endpoint, addr, &connected, &event_tx, &msg_tx,
                        ).await;
                        let _ = reply.send(result);
                    }
                }
            }
            Some(ev) = msg_rx.recv() => {
                match ev {
                    ConnectionEvent::Message { from, payload } => {
                        let now = Instant::now();
                        let now_ns = nanos_since_epoch(now);
                        match postcard::from_bytes::<WireMessage>(&payload) {
                            Ok(WireMessage::Message { from: from_str, text, send_instant_ns }) => {
                                let sent_at = instant_from_nanos(send_instant_ns);
                                log.write().await.push(DocEntry {
                                    from: from_str,
                                    text: text.clone(),
                                    sent_at_ns: send_instant_ns,
                                    received_at_ns: now_ns,
                                });
                                let _ = event_tx.send(SmallRoomEvent::Message {
                                    from,
                                    text,
                                    sent_at,
                                    received_at: now,
                                });
                            }
                            Ok(WireMessage::Ping { send_instant_ns }) => {
                                let pong = WireMessage::Pong {
                                    send_instant_ns,
                                    recv_instant_ns: now_ns,
                                };
                                let payload = postcard::to_stdvec(&pong).expect("infallible");
                                let peers = connected.lock().await;
                                if let Some(conn) = peers.get(&from) {
                                    if let Ok(mut send) = conn.open_uni().await {
                                        let _ = send.write_all(&payload).await;
                                        let _ = send.finish();
                                    }
                                }
                            }
                            Ok(WireMessage::Pong { send_instant_ns, .. }) => {
                                let elapsed = now_ns.saturating_sub(send_instant_ns);
                                let rtt = std::time::Duration::from_nanos(
                                    elapsed.min(u64::MAX as u128) as u64
                                );
                                latency_log.lock().await.push(LatencySample {
                                    peer: from,
                                    rtt,
                                    at: now,
                                });
                                trace!(peer = %from.fmt_short(), ?rtt, "RTT sample");
                            }
                            Err(e) => {
                                warn!("failed to decode wire message from {}: {e}", from.fmt_short());
                            }
                        }
                    }
                    ConnectionEvent::Disconnected(peer) => {
                        connected.lock().await.remove(&peer);
                        let _ = event_tx.send(SmallRoomEvent::PeerDisconnected(peer));
                    }
                }
            }
        }
    }

    let _ = event_tx.send(SmallRoomEvent::Closed);
}

enum ConnectionEvent {
    Message { from: PublicKey, payload: Bytes },
    Disconnected(PublicKey),
}

async fn read_connection_loop(
    remote_id: PublicKey,
    conn: Connection,
    msg_tx: mpsc::UnboundedSender<ConnectionEvent>,
) {
    loop {
        match conn.accept_uni().await {
            Ok(mut recv) => {
                let mut buf = Vec::new();
                if let Err(e) = tokio::io::AsyncReadExt::read_to_end(&mut recv, &mut buf).await {
                    warn!(peer = %remote_id.fmt_short(), "read error: {e}");
                    break;
                }
                if buf.is_empty() {
                    trace!(peer = %remote_id.fmt_short(), "empty frame");
                    continue;
                }
                if msg_tx
                    .send(ConnectionEvent::Message {
                        from: remote_id,
                        payload: buf.into(),
                    })
                    .is_err()
                {
                    break;
                }
            }
            Err(e) => {
                warn!(peer = %remote_id.fmt_short(), "accept_uni error: {e}");
                break;
            }
        }
    }
    let _ = msg_tx.send(ConnectionEvent::Disconnected(remote_id));
}

async fn connect_to_peer(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    connected: &Arc<Mutex<HashMap<PublicKey, Connection>>>,
    event_tx: &mpsc::UnboundedSender<SmallRoomEvent>,
    msg_tx: &mpsc::UnboundedSender<ConnectionEvent>,
) -> Result<()> {
    let conn = endpoint.connect(addr, SMALL_ROOM_ALPN).await?;
    let remote_id = conn.remote_id();
    debug!(peer = %remote_id.fmt_short(), "connected to peer");

    connected.lock().await.insert(remote_id, conn.clone());
    let _ = event_tx.send(SmallRoomEvent::PeerConnected(remote_id));

    let msg_tx = msg_tx.clone();
    tokio::task::spawn(read_connection_loop(remote_id, conn, msg_tx));
    Ok(())
}

// -- Timestamp helpers ---------------------------------------------------------

/// Shared monotonic base clock for the entire process.
///
/// Both `nanos_since_epoch` and `instant_from_nanos` use the same
/// [`OnceLock`] so that a nanosecond offset produced by one function
/// can be faithfully reconstructed as an `Instant` by the other.
/// This is critical for latency measurement when all peers share a
/// process (benchmark mode).
fn ts_base() -> &'static Instant {
    static BASE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    BASE.get_or_init(Instant::now)
}

/// Returns nanoseconds since the shared process-wide epoch.
fn nanos_since_epoch(now: Instant) -> u128 {
    now.duration_since(*ts_base()).as_nanos()
}

fn duration_to_nanos(d: std::time::Duration) -> u128 {
    d.as_nanos()
}

fn nanos_to_duration(ns: u128) -> std::time::Duration {
    std::time::Duration::from_nanos(ns.min(u64::MAX as u128) as u64)
}

/// Reconstruct an `Instant` from a nanosecond offset produced by
/// [`nanos_since_epoch`].
fn instant_from_nanos(ns: u128) -> Instant {
    let base = ts_base();
    base.checked_add(nanos_to_duration(ns))
        .unwrap_or(Instant::now())
}

// -- Builder -------------------------------------------------------------------

/// Builder for creating and joining a small room.
#[derive(Debug)]
pub struct SmallRoomBuilder {
    endpoint: Endpoint,
    secret_key: SecretKey,
    cmd_tx: mpsc::Sender<Cmd>,
    /// The receiver half of the command channel.
    /// Taken by `spawn()` and passed to the actor.
    cmd_rx: Option<mpsc::Receiver<Cmd>>,
}

impl SmallRoomBuilder {
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

    /// Create a [`SmallRoomProtocol`] handler for this room.
    ///
    /// Register it on your [`Router`](iroh::protocol::Router) with
    /// `router.accept(SMALL_ROOM_ALPN, handler)` so incoming small-room
    /// connections are routed to this actor.
    pub fn protocol_handler(&self) -> SmallRoomProtocol {
        SmallRoomProtocol {
            cmd_tx: self.cmd_tx.clone(),
        }
    }

    /// Spawn a new small room and return a handle.
    pub async fn spawn(mut self) -> (SmallRoomHandle, mpsc::UnboundedReceiver<SmallRoomEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let log: Arc<RwLock<Vec<DocEntry>>> = Arc::new(RwLock::new(Vec::new()));
        let latency_log: Arc<Mutex<Vec<LatencySample>>> = Arc::new(Mutex::new(Vec::new()));
        let connected: Arc<Mutex<HashMap<PublicKey, Connection>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let handle = SmallRoomHandle {
            cmd_tx: self.cmd_tx.clone(),
            log: log.clone(),
            latency_log: latency_log.clone(),
            connected: connected.clone(),
        };

        let cmd_rx = self.cmd_rx.take().expect("spawn called more than once");

        let endpoint = self.endpoint.clone();
        let secret_key = self.secret_key;
        tokio::task::spawn(run_actor(
            endpoint,
            secret_key,
            cmd_rx,
            event_tx,
            log,
            latency_log,
            connected,
        ));

        (handle, event_rx)
    }
}

// -- Tests ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::endpoint::presets;
    use iroh::protocol::Router;
    use iroh::RelayMode;
    use n0_future::time::{sleep, Duration};
    use rand::SeedableRng;

    #[allow(clippy::type_complexity)]
    async fn create_node(
        rng: &mut rand::rngs::StdRng,
    ) -> Result<(
        Router,
        Endpoint,
        SecretKey,
        SmallRoomHandle,
        mpsc::UnboundedReceiver<SmallRoomEvent>,
    )> {
        let secret_key = SecretKey::generate();
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(secret_key.clone())
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?;

        let builder = SmallRoomBuilder::new(endpoint.clone(), secret_key.clone());
        let handler = builder.protocol_handler();
        let (handle, event_rx) = builder.spawn().await;

        let router = Router::builder(endpoint.clone())
            .accept(SMALL_ROOM_ALPN, handler)
            .spawn();

        Ok((router, endpoint, secret_key, handle, event_rx))
    }

    #[tokio::test]
    #[n0_tracing_test::traced_test]
    async fn test_small_room_basic_connect_and_send() -> Result<()> {
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let (router_a, _ep_a, _sk_a, handle_a, _events_a) = create_node(&mut rng).await?;
        let (router_b, ep_b, _sk_b, handle_b, mut events_b) = create_node(&mut rng).await?;

        handle_a.connect_to(ep_b.addr()).await?;
        sleep(Duration::from_millis(500)).await;

        handle_a.broadcast("hello from A".to_string()).await?;
        sleep(Duration::from_millis(500)).await;

        let b_log = handle_b.read_log().await;
        assert!(!b_log.is_empty(), "B should have received messages");
        assert!(
            b_log.iter().any(|e| e.text == "hello from A"),
            "B should have 'hello from A'"
        );

        let b_got_msg = loop {
            match events_b.recv().await {
                Some(SmallRoomEvent::Message { text, .. }) => break text == "hello from A",
                Some(_) => continue,
                None => break false,
            }
        };
        assert!(b_got_msg, "B should receive a Message event");

        let a_log = handle_a.read_log().await;
        assert!(
            a_log.iter().any(|e| e.text == "hello from A"),
            "A's log should include its own message"
        );

        router_a.shutdown().await.unwrap();
        router_b.shutdown().await.unwrap();
        Ok(())
    }

    #[tokio::test]
    #[n0_tracing_test::traced_test]
    async fn test_small_room_three_peers() -> Result<()> {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);

        let (router_a, ep_a, _sk_a, handle_a, _events_a) = create_node(&mut rng).await?;
        let (router_b, ep_b, _sk_b, handle_b, _events_b) = create_node(&mut rng).await?;
        let (router_c, ep_c, _sk_c, handle_c, mut events_c) = create_node(&mut rng).await?;

        handle_a.connect_to(ep_b.addr()).await?;
        handle_a.connect_to(ep_c.addr()).await?;
        sleep(Duration::from_millis(500)).await;

        handle_a.broadcast("hello everyone".to_string()).await?;
        sleep(Duration::from_millis(500)).await;

        let b_log = handle_b.read_log().await;
        let c_log = handle_c.read_log().await;
        assert!(b_log.iter().any(|e| e.text == "hello everyone"));
        assert!(c_log.iter().any(|e| e.text == "hello everyone"));

        let c_got = loop {
            match events_c.recv().await {
                Some(SmallRoomEvent::Message { text, .. }) => break text == "hello everyone",
                Some(_) => continue,
                None => break false,
            }
        };
        assert!(c_got);

        router_a.shutdown().await.unwrap();
        router_b.shutdown().await.unwrap();
        router_c.shutdown().await.unwrap();
        Ok(())
    }

    #[tokio::test]
    #[n0_tracing_test::traced_test]
    async fn test_small_room_latency_tracking() -> Result<()> {
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);

        let (router_a, _ep_a, _sk_a, handle_a, _events_a) = create_node(&mut rng).await?;
        let (router_b, ep_b, _sk_b, handle_b, _events_b) = create_node(&mut rng).await?;

        handle_a.connect_to(ep_b.addr()).await?;
        sleep(Duration::from_millis(500)).await;

        handle_a.broadcast("latency test".to_string()).await?;
        sleep(Duration::from_millis(500)).await;

        let b_log = handle_b.read_log().await;
        let msg = b_log
            .iter()
            .find(|e| e.text == "latency test")
            .expect("B should have the message");
        assert!(
            msg.received_at_ns > 0,
            "received_at_ns should be set for remote messages"
        );

        let a_log = handle_a.read_log().await;
        let local_msg = a_log
            .iter()
            .find(|e| e.text == "latency test")
            .expect("A should have its own message");
        assert!(
            local_msg.received_at_ns == 0,
            "local messages should have received_at_ns = 0"
        );

        router_a.shutdown().await.unwrap();
        router_b.shutdown().await.unwrap();
        Ok(())
    }
}
