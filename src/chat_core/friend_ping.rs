//! Friend ping manager — periodically probes peer reachability and reports status changes.
//!
//! The core component is [`FriendPingManager`], which maintains a set of configured
//! friends (by [`PublicKey`]) and runs a background task that periodically attempts to
//! connect to each friend.
//!
//! # How it works
//!
//! 1. Friends are added with a `PublicKey` and, optionally, a cached `EndpointAddr`.
//! 2. On each tick the manager tries to obtain address information for each friend:
//!    - If a cached `EndpointAddr` was provided at add-time, it uses that directly.
//!    - Otherwise it asks the local [`Endpoint`] for known addresses via
//!      [`Endpoint::remote_info`] and builds an `EndpointAddr` from what it finds.
//! 3. It calls [`Endpoint::connect_with_opts`] with the gossip ALPN and a short
//!    per-ping timeout (wrapped via `tokio::time::timeout`).
//! 4. Success → mark Online; failure → mark Offline.
//! 5. Transitions are emitted on an `UnboundedReceiver<FriendEvent>` that the frontend
//!    polls alongside other event streams.

use std::{
    collections::{BTreeSet, HashMap},
    time::Duration,
};

use iroh::{Endpoint, EndpointAddr, PublicKey};
use n0_error::{bail_any, Result};
use tokio::sync::{mpsc, oneshot};
use tracing::trace;

// ── Constants ──────────────────────────────────────────────────────────────────

/// ALPN used by the friend ping manager to test connectivity.
///
/// Peers that want to accept friend pings must register a handler for this ALPN on
/// their Router (see [`PingHandler`]).
pub const FRIEND_PING_ALPN: &[u8] = b"/iroh-gossip-chat/friend-ping/1";

/// Default interval between friend ping cycles.
pub const DEFAULT_PING_INTERVAL: Duration = Duration::from_secs(30);

/// Default per-ping connect timeout.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

// ── Public types ───────────────────────────────────────────────────────────────

/// Connection status of a friend peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FriendStatus {
    /// Not yet checked (initial state).
    Unknown,
    /// Successfully reached on the last ping attempt.
    Online,
    /// Could not be reached on the last ping attempt.
    Offline,
}

impl FriendStatus {
    /// Returns `true` if we believe the peer is currently reachable.
    pub fn is_online(self) -> bool {
        matches!(self, Self::Online)
    }
}

/// Event emitted from the [`FriendPingManager`] when a friend's status changes.
#[derive(Debug, Clone)]
pub enum FriendEvent {
    /// A friend's reachability status has changed.
    StatusChanged {
        /// The peer whose status changed.
        peer: PublicKey,
        /// The new status.
        status: FriendStatus,
    },
}

// ── Internal commands ──────────────────────────────────────────────────────────

enum Cmd {
    AddFriend {
        peer: PublicKey,
        addr: Option<EndpointAddr>,
        reply: oneshot::Sender<bool>,
    },
    RemoveFriend {
        peer: PublicKey,
        reply: oneshot::Sender<bool>,
    },
    QueryStatus {
        peer: PublicKey,
        reply: oneshot::Sender<Option<FriendStatus>>,
    },
    ListFriends {
        reply: oneshot::Sender<Vec<(PublicKey, FriendStatus)>>,
    },
}

// ── Ping protocol handler ──────────────────────────────────────────────────────

/// A trivial handler that accepts friend ping connections and immediately closes them.
///
/// Register this on your Router alongside the gossip handler:
///
/// ```ignore
/// let router = Router::builder(endpoint.clone())
///     .accept(GOSSIP_ALPN, gossip.clone())
///     .accept(FRIEND_PING_ALPN, PingHandler)
///     .spawn();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct PingHandler;

impl iroh::protocol::ProtocolHandler for PingHandler {
    async fn accept(
        &self,
        _connection: iroh::endpoint::Connection,
    ) -> std::result::Result<(), iroh::protocol::AcceptError> {
        Ok(())
    }
}

// ── FriendPingManager ──────────────────────────────────────────────────────────

/// Handle to a running friend ping manager.
///
/// Clone this freely — all clones share the same background task.
#[derive(Debug, Clone)]
pub struct FriendPingManager {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
}

impl FriendPingManager {
    /// Spawn a new friend ping manager background task.
    ///
    /// Returns a handle and an event receiver.  The receiver yields
    /// [`FriendEvent`] items whenever a friend transitions between
    /// online/offline.
    pub fn spawn(
        endpoint: Endpoint,
        ping_interval: Duration,
        connect_timeout: Duration,
    ) -> (Self, mpsc::UnboundedReceiver<FriendEvent>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let actor = FriendPingActor {
            endpoint,
            cmd_rx,
            event_tx,
            ping_interval,
            connect_timeout,
            friends: HashMap::new(),
        };

        tokio::task::spawn(actor.run());

        (Self { cmd_tx }, event_rx)
    }

    /// Add a friend to track.
    ///
    /// If `addr` is provided, it is cached and used for pinging directly.
    /// If `None`, the manager will try to discover the peer's addresses via
    /// [`Endpoint::remote_info`].
    ///
    /// Returns `true` if the friend was newly added, `false` if already tracked.
    pub async fn add_friend(&self, peer: PublicKey, addr: Option<EndpointAddr>) -> Result<bool> {
        let (reply, rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(Cmd::AddFriend { peer, addr, reply })
            .is_err()
        {
            bail_any!("friend ping actor stopped");
        }
        rx.await
            .map_err(|_| n0_error::anyerr!("friend ping actor dropped reply channel"))
    }

    /// Remove a friend from tracking.
    ///
    /// Returns `true` if the friend was being tracked, `false` otherwise.
    pub async fn remove_friend(&self, peer: &PublicKey) -> Result<bool> {
        let (reply, rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(Cmd::RemoveFriend { peer: *peer, reply })
            .is_err()
        {
            bail_any!("friend ping actor stopped");
        }
        rx.await
            .map_err(|_| n0_error::anyerr!("friend ping actor dropped reply channel"))
    }

    /// Query the current status of a tracked friend.
    ///
    /// Returns `None` if the friend is not being tracked.
    pub async fn friend_status(&self, peer: &PublicKey) -> Result<Option<FriendStatus>> {
        let (reply, rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(Cmd::QueryStatus { peer: *peer, reply })
            .is_err()
        {
            bail_any!("friend ping actor stopped");
        }
        rx.await
            .map_err(|_| n0_error::anyerr!("friend ping actor dropped reply channel"))
    }

    /// List all tracked friends and their current status.
    pub async fn list_friends(&self) -> Result<Vec<(PublicKey, FriendStatus)>> {
        let (reply, rx) = oneshot::channel();
        if self.cmd_tx.send(Cmd::ListFriends { reply }).is_err() {
            bail_any!("friend ping actor stopped");
        }
        rx.await
            .map_err(|_| n0_error::anyerr!("friend ping actor dropped reply channel"))
    }
}

// ── Internal per-friend state ───────────────────────────────────────────────────

#[derive(Debug)]
struct FriendState {
    status: FriendStatus,
    addr: Option<EndpointAddr>,
}

impl FriendState {
    fn new(addr: Option<EndpointAddr>) -> Self {
        Self {
            status: FriendStatus::Unknown,
            addr,
        }
    }
}

// ── Background actor ────────────────────────────────────────────────────────────

#[derive(Debug)]
struct FriendPingActor {
    endpoint: Endpoint,
    cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    event_tx: mpsc::UnboundedSender<FriendEvent>,
    ping_interval: Duration,
    connect_timeout: Duration,
    friends: HashMap<PublicKey, FriendState>,
}

impl FriendPingActor {
    async fn run(mut self) {
        // First tick fires immediately so we do a fast initial scan.
        let mut tick = tokio::time::interval(self.ping_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;

                Some(cmd) = self.cmd_rx.recv() => {
                    match cmd {
                        Cmd::AddFriend { peer, addr, reply } => {
                            let already_present = self.friends.contains_key(&peer);
                            if !already_present {
                                self.friends.insert(peer, FriendState::new(addr));
                                trace!(%peer, "added friend for ping tracking");
                            }
                            let _ = reply.send(!already_present);
                        }
                        Cmd::RemoveFriend { peer, reply } => {
                            let removed = self.friends.remove(&peer).is_some();
                            let _ = reply.send(removed);
                        }
                        Cmd::QueryStatus { peer, reply } => {
                            let status = self.friends.get(&peer).map(|s| s.status);
                            let _ = reply.send(status);
                        }
                        Cmd::ListFriends { reply } => {
                            let list: Vec<_> = self.friends
                                .iter()
                                .map(|(k, v)| (*k, v.status))
                                .collect();
                            let _ = reply.send(list);
                        }
                    }
                }

                _ = tick.tick() => {
                    self.ping_all().await;
                }
            }
        }
    }

    async fn ping_all(&mut self) {
        let peer_list: Vec<PublicKey> = self.friends.keys().copied().collect();
        for peer in peer_list {
            self.ping_one(peer).await;
        }
    }

    async fn ping_one(&mut self, peer: PublicKey) {
        let addrs = self.resolve_addrs(peer).await;

        let addrs = match addrs {
            Some(a) => a,
            None => {
                // No addresses known yet — leave status as-is.
                return;
            }
        };

        let connected = self.try_connect(peer, &addrs).await;

        let new_status = if connected {
            FriendStatus::Online
        } else {
            FriendStatus::Offline
        };

        if let Some(state) = self.friends.get_mut(&peer) {
            if state.status != new_status {
                // Emit event on every transition including the first scan.
                // Frontends suppress the "is now ONLINE/OFFLINE" system message
                // for friends that have no prior history in the store, avoiding
                // a startup notification burst.
                let _ = self.event_tx.send(FriendEvent::StatusChanged {
                    peer,
                    status: new_status,
                });
            }
            state.status = new_status;
        }
    }

    /// Try to resolve address information for a peer.
    async fn resolve_addrs(&self, peer: PublicKey) -> Option<EndpointAddr> {
        // 1. Use a cached address if available.
        if let Some(state) = self.friends.get(&peer) {
            if state.addr.is_some() {
                return state.addr.clone();
            }
        }

        // 2. Try to discover addresses from the local endpoint's remote info.
        let info = self.endpoint.remote_info(peer).await?;
        let transport_addrs: BTreeSet<_> = info.addrs().map(|a| a.addr().clone()).collect();

        if transport_addrs.is_empty() {
            return None;
        }

        Some(EndpointAddr {
            id: peer,
            addrs: transport_addrs,
        })
    }

    /// Try to connect to the peer and report success/failure.
    async fn try_connect(&self, peer: PublicKey, addrs: &EndpointAddr) -> bool {
        if addrs.addrs.is_empty() {
            return false;
        }

        // Wrap the connect call with a global timeout so we don't hang on misbehaving peers.
        let connect_fut =
            self.endpoint
                .connect_with_opts(addrs.clone(), FRIEND_PING_ALPN, Default::default());

        match tokio::time::timeout(self.connect_timeout, connect_fut).await {
            Ok(Ok(connecting)) => {
                // Wait for the handshake to complete (with the same timeout).
                match tokio::time::timeout(self.connect_timeout, connecting).await {
                    Ok(Ok(conn)) => {
                        // Connection established — close it immediately.
                        conn.close(0u32.into(), b"ping");
                        true
                    }
                    Ok(Err(err)) => {
                        trace!(%peer, "ping handshake failed: {err:#}");
                        false
                    }
                    Err(_) => {
                        trace!(%peer, "ping handshake timed out");
                        false
                    }
                }
            }
            Ok(Err(err)) => {
                trace!(%peer, "ping connect_with_opts failed: {err:#}");
                false
            }
            Err(_) => {
                trace!(%peer, "ping connect_with_opts timed out");
                false
            }
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    #[test]
    fn friend_status_defaults() {
        assert!(!FriendStatus::Unknown.is_online());
        assert!(FriendStatus::Online.is_online());
        assert!(!FriendStatus::Offline.is_online());
    }

    #[tokio::test]
    async fn test_add_and_remove_friend() -> Result<()> {
        let secret_key = SecretKey::generate();
        let endpoint = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(secret_key)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await?;

        let (mgr, _events) =
            FriendPingManager::spawn(endpoint, DEFAULT_PING_INTERVAL, DEFAULT_CONNECT_TIMEOUT);

        let peer = SecretKey::generate().public();
        assert!(
            mgr.add_friend(peer, None).await?,
            "should return true for new friend"
        );
        assert!(
            !mgr.add_friend(peer, None).await?,
            "should return false for duplicate"
        );

        assert_eq!(mgr.friend_status(&peer).await?, Some(FriendStatus::Unknown));
        assert!(mgr.remove_friend(&peer).await?);
        assert_eq!(mgr.friend_status(&peer).await?, None);
        Ok(())
    }

    /// Compile-time check that PingHandler implements ProtocolHandler.
    #[test]
    fn ping_handler_is_protocol_handler() {
        fn _assert(_h: impl iroh::protocol::ProtocolHandler) {}
        _assert(PingHandler);
    }

    // ── Two-endpoint integration tests ─────────────────────────────────────

    /// A trivial ProtocolHandler that accepts connections and closes them immediately.
    /// Used as the GOSSIP_ALPN handler on the target peer during friend ping tests.
    #[derive(Debug)]
    struct AcceptCloseHandler;

    impl iroh::protocol::ProtocolHandler for AcceptCloseHandler {
        async fn accept(
            &self,
            _connection: iroh::endpoint::Connection,
        ) -> std::result::Result<(), iroh::protocol::AcceptError> {
            Ok(())
        }
    }

    /// Test: adding a friend with a reachable EndpointAddr, then pinging results in Online status.
    #[tokio::test]
    async fn test_ping_online_peer_status_online() -> Result<()> {
        let sk1 = SecretKey::generate();
        let sk2 = SecretKey::generate();
        let pk2 = sk2.public();

        let ep1 = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk1)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await?;

        let ep2 = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk2)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await?;

        // ep2 needs to accept FRIEND_PING_ALPN connections for friend pings to succeed.
        let _router2 = iroh::protocol::Router::builder(ep2.clone())
            .accept(FRIEND_PING_ALPN, AcceptCloseHandler)
            .spawn();

        let (mgr, _events) = FriendPingManager::spawn(
            ep1.clone(),
            Duration::from_millis(100),
            Duration::from_secs(2),
        );

        mgr.add_friend(pk2, Some(ep2.addr())).await?;

        // Wait for the initial ping cycle to fire (interval is 100ms).
        tokio::time::sleep(Duration::from_millis(300)).await;

        let status = mgr.friend_status(&pk2).await?;
        assert_eq!(
            status,
            Some(FriendStatus::Online),
            "reachable peer should be Online after ping"
        );

        Ok(())
    }

    /// Test: adding a friend with no known addresses stays Unknown (no address info to ping).
    #[tokio::test]
    async fn test_ping_unknown_peer_stays_unknown() -> Result<()> {
        let sk = SecretKey::generate();
        let ep = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await?;

        let (mgr, _events) =
            FriendPingManager::spawn(ep, Duration::from_millis(100), Duration::from_millis(500));

        let peer = SecretKey::generate().public();
        mgr.add_friend(peer, None).await?;

        // Wait for ping — since no addresses are known, status stays Unknown.
        tokio::time::sleep(Duration::from_millis(300)).await;

        let status = mgr.friend_status(&peer).await?;
        assert_eq!(
            status,
            Some(FriendStatus::Unknown),
            "peer with no addresses should remain Unknown"
        );

        Ok(())
    }

    /// Test: adding a friend with an unreachable (bogus) address results in Offline status.
    #[tokio::test]
    async fn test_ping_unreachable_peer_offline() -> Result<()> {
        let sk = SecretKey::generate();
        let ep = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await?;

        let (mgr, _events) =
            FriendPingManager::spawn(ep, Duration::from_millis(100), Duration::from_millis(500));

        // Use a real-looking public key but with no transport addresses — connect fails.
        let peer = SecretKey::generate().public();
        let addr = EndpointAddr::new(peer);
        mgr.add_friend(peer, Some(addr)).await?;

        // Wait for ping — connect should fail (no addresses), status goes Offline.
        tokio::time::sleep(Duration::from_millis(300)).await;

        let status = mgr.friend_status(&peer).await?;
        assert_eq!(
            status,
            Some(FriendStatus::Offline),
            "unreachable peer should be Offline after failed ping"
        );

        Ok(())
    }

    /// Test: FriendEvent is emitted on Online ↔ Offline transitions.
    #[tokio::test]
    async fn test_friend_event_emitted_on_status_change() -> Result<()> {
        let sk1 = SecretKey::generate();
        let sk2 = SecretKey::generate();
        let pk2 = sk2.public();

        let ep1 = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk1)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await?;

        let ep2 = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk2)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await?;

        let _router2 = iroh::protocol::Router::builder(ep2.clone())
            .accept(FRIEND_PING_ALPN, AcceptCloseHandler)
            .spawn();

        let (mgr, mut events) = FriendPingManager::spawn(
            ep1.clone(),
            Duration::from_millis(100),
            Duration::from_secs(2),
        );

        mgr.add_friend(pk2, Some(ep2.addr())).await?;

        // First ping: Unknown → Online (event IS emitted on first scan).
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            mgr.friend_status(&pk2).await?,
            Some(FriendStatus::Online),
            "should be online after first ping"
        );

        // Read the first-scan Online event.
        let first_event = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("should receive first-scan online event within timeout")
            .expect("event channel should not be closed");
        match first_event {
            FriendEvent::StatusChanged { peer, status } => {
                assert_eq!(peer, pk2);
                assert_eq!(
                    status,
                    FriendStatus::Online,
                    "first-scan event should report Online"
                );
            }
        }

        // Drop the router and endpoint so the peer becomes unreachable.
        drop(_router2);
        drop(ep2);

        // Second ping (after peer is gone): Online → Offline (event SHOULD be emitted).
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Read the events channel — we should see a StatusChanged event.
        let event = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("should receive friend event within timeout")
            .expect("event channel should not be closed");

        match event {
            FriendEvent::StatusChanged { peer, status } => {
                assert_eq!(peer, pk2);
                assert_eq!(
                    status,
                    FriendStatus::Offline,
                    "event should report Offline after peer disappears"
                );
            }
        }

        Ok(())
    }

    /// Test: an event IS emitted on the first scan (Unknown → Online/Offline now fires).
    #[tokio::test]
    async fn test_event_emitted_on_first_scan() -> Result<()> {
        let sk = SecretKey::generate();
        let ep = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await?;

        let (mgr, mut events) =
            FriendPingManager::spawn(ep, Duration::from_millis(100), Duration::from_millis(500));

        // Add a peer with a bogus address — first scan yields Offline but emits event.
        let peer = SecretKey::generate().public();
        let addr = EndpointAddr::new(peer);
        mgr.add_friend(peer, Some(addr)).await?;

        // Wait for the first ping cycle to fire.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Status should be Offline after the ping.
        assert_eq!(
            mgr.friend_status(&peer).await?,
            Some(FriendStatus::Offline),
            "should be offline after first ping"
        );

        // An event SHOULD be emitted on the first scan.
        let event = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("should receive event within timeout")
            .expect("event channel should not be closed");

        match event {
            FriendEvent::StatusChanged { peer: p, status } => {
                assert_eq!(p, peer);
                assert_eq!(
                    status,
                    FriendStatus::Offline,
                    "event should report Offline for unreachable peer"
                );
            }
        }

        Ok(())
    }

    /// Test: list_friends returns all tracked peers with correct statuses.
    #[tokio::test]
    async fn test_list_friends_returns_all() -> Result<()> {
        let sk = SecretKey::generate();
        let ep = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await?;

        let (mgr, _events) =
            FriendPingManager::spawn(ep, DEFAULT_PING_INTERVAL, DEFAULT_CONNECT_TIMEOUT);

        let p1 = SecretKey::generate().public();
        let p2 = SecretKey::generate().public();
        let p3 = SecretKey::generate().public();

        mgr.add_friend(p1, None).await?;
        mgr.add_friend(p2, None).await?;
        mgr.add_friend(p3, None).await?;

        let list = mgr.list_friends().await?;
        assert_eq!(list.len(), 3);

        let keys: std::collections::BTreeSet<_> = list.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&p1));
        assert!(keys.contains(&p2));
        assert!(keys.contains(&p3));

        for (_key, status) in &list {
            assert_eq!(
                *status,
                FriendStatus::Unknown,
                "all friends should be Unknown before any ping"
            );
        }

        Ok(())
    }

    /// Test: FriendPingManager clone shares state.
    #[tokio::test]
    async fn test_clone_shares_state() -> Result<()> {
        let sk = SecretKey::generate();
        let ep = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
            .secret_key(sk)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
            .unwrap()
            .bind()
            .await?;

        let (mgr, _events) =
            FriendPingManager::spawn(ep, DEFAULT_PING_INTERVAL, DEFAULT_CONNECT_TIMEOUT);

        let mgr2 = mgr.clone();
        let peer = SecretKey::generate().public();

        // Add via original handle.
        mgr.add_friend(peer, None).await?;

        // Query via clone.
        let status = mgr2.friend_status(&peer).await?;
        assert_eq!(status, Some(FriendStatus::Unknown));

        // Remove via clone.
        assert!(mgr2.remove_friend(&peer).await?);

        // Original should reflect removal.
        assert_eq!(mgr.friend_status(&peer).await?, None);

        Ok(())
    }

    // ── GUI frontend runtime-context regression tests ───────────────────
    //
    // Both chat-gui.rs and iced_chat/main.rs do:
    //   let runtime = Runtime::new()?;
    //   runtime.block_on(async { /* setup endpoints etc */ });
    //   // ← back in sync code, no runtime context
    //   // The FIX: runtime.handle().enter() before FriendPingManager::spawn()
    //
    // These tests verify that the EnterGuard pattern works and that the
    // original broken pattern is properly guarded against.

    #[test]
    fn spawn_outside_block_on_with_enter_guard() {
        // Reproduce the EXACT pattern from the GUI frontends.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ep = rt.block_on(async {
            Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
                .secret_key(SecretKey::generate())
                .bind()
                .await
                .expect("ep bind")
        });
        // The fix: enter the runtime context temporarily.
        let _guard = rt.handle().enter();
        let (_mgr, _rx) =
            FriendPingManager::spawn(ep, DEFAULT_PING_INTERVAL, DEFAULT_CONNECT_TIMEOUT);
        drop(_guard);
    }

    #[test]
    fn spawn_outside_block_on_without_enter_guard_panics() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ep = rt.block_on(async {
            Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
                .secret_key(SecretKey::generate())
                .bind()
                .await
                .expect("ep bind")
        });
        // No EnterGuard → should panic with "no reactor running".
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (_mgr, _rx) =
                FriendPingManager::spawn(ep, DEFAULT_PING_INTERVAL, DEFAULT_CONNECT_TIMEOUT);
        }));
        assert!(result.is_err(), "spawn outside runtime should panic");
    }
}
