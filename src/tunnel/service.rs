//! State management for configured secure tunnels.
//!
//! This service intentionally owns only tunnel metadata and lifecycle state. The
//! shared Iroh endpoint and stream forwarding remain in the networking layer;
//! this keeps GUI callers independent from the transport implementation.

use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, RwLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use tokio_util::sync::CancellationToken;

use super::TunnelId;

/// Maximum number of owner-configured tunnels held by one service.
pub const MAX_ACTIVE_SHARED_TUNNELS: usize = 32;
/// Maximum number of simultaneously received tunnel streams.
pub const MAX_ACTIVE_RECEIVED_TUNNELS: usize = 32;
/// Default maximum number of simultaneous streams per tunnel.
pub const DEFAULT_MAX_CONNECTIONS_PER_TUNNEL: usize = 16;
/// Maximum new tunnel streams accepted from one peer in the rate window.
pub const MAX_CONNECTION_ATTEMPTS_PER_INTERVAL: usize = 8;
/// Window used by the per-peer connection-attempt limiter.
pub const CONNECTION_ATTEMPT_INTERVAL: Duration = Duration::from_secs(60);
const MAX_TRACKED_ATTEMPT_PEERS: usize = 256;

/// A local service target exposed through a tunnel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelTarget {
    /// A TCP service. Callers should constrain this to loopback addresses.
    Tcp {
        /// IP address of the local service.
        host: IpAddr,
        /// TCP port of the local service.
        port: u16,
    },
}

impl TunnelTarget {
    /// Construct a TCP target.
    pub const fn tcp(host: IpAddr, port: u16) -> Self {
        Self::Tcp { host, port }
    }

    /// Return whether this target is restricted to the local machine.
    pub fn is_loopback(&self) -> bool {
        match self {
            Self::Tcp { host, .. } => host.is_loopback(),
        }
    }
}

/// Lifecycle state for a configured tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelStatus {
    /// The tunnel is configured and available to connect.
    Active,
    /// A connection attempt has been handed to the transport layer.
    Connecting,
    /// The transport has established the tunnel.
    Connected,
    /// The tunnel was revoked and is no longer connectable.
    Revoked,
}

/// Best-effort route classification for a live tunnel connection.
///
/// The transport layer records this from Iroh's own path information and
/// never guesses: when no reliable path data exists the value stays
/// [`TunnelRoute::Connected`], and the GUI shows the neutral "Connected"
/// label for ordinary users.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelRoute {
    /// Iroh reports the selected path is a direct IP path.
    Direct,
    /// Iroh reports the selected path is a relay path.
    Relay,
    /// Iroh exposes path information, but it is neither clearly direct nor a
    /// relay (for example a custom transport).
    Unknown,
    /// No reliable path information is available.
    Connected,
}

impl TunnelRoute {
    /// Human-readable route label. [`TunnelRoute::Connected`] is the neutral
    /// fallback shown when Iroh has no path information, so the UI never
    /// invents a route for ordinary users.
    pub fn label(self) -> &'static str {
        match self {
            Self::Direct => "Direct",
            Self::Relay => "Relay",
            Self::Unknown => "Unknown",
            Self::Connected => "Connected",
        }
    }
}

impl Default for TunnelRoute {
    fn default() -> Self {
        Self::Connected
    }
}

/// Lightweight, best-effort metrics for one tunnel connection.
///
/// These values are only populated when Iroh exposes them; zero fields simply
/// mean the information is not available yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TunnelConnectionInfo {
    /// Route classification from Iroh path data, or [`TunnelRoute::Unknown`].
    pub route: TunnelRoute,
    /// Bytes sent through the tunnel (local -> remote), when known.
    pub bytes_sent: u64,
    /// Bytes received through the tunnel (remote -> local), when known.
    pub bytes_received: u64,
    /// Unix epoch milliseconds when the current connection started, when known.
    pub connected_at_ms: u64,
    /// Number of TCP connections currently using this tunnel.
    pub tcp_connections: usize,
}

/// Live, best-effort connection metadata shared between the transport layer
/// and the GUI. The transport records route and byte counters; the GUI reads
/// a snapshot for display. All updates are lightweight atomic writes.
#[derive(Debug, Default)]
pub struct TunnelLiveInfo {
    route: RwLock<TunnelRoute>,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    connected_at_ms: AtomicU64,
    tcp_connections: AtomicUsize,
}

impl TunnelLiveInfo {
    /// Record the route observed from Iroh path data.
    pub fn set_route(&self, route: TunnelRoute) {
        *self.route.write().expect("tunnel live info lock poisoned") = route;
    }

    /// Record bytes forwarded in each direction.
    pub fn add_bytes(&self, sent: u64, received: u64) {
        self.bytes_sent.fetch_add(sent, Ordering::Relaxed);
        self.bytes_received.fetch_add(received, Ordering::Relaxed);
    }

    /// Record the start of one TCP connection using the tunnel.
    pub fn connection_opened(&self, connected_at_ms: u64) {
        self.connected_at_ms
            .compare_exchange(0, connected_at_ms, Ordering::Relaxed, Ordering::Relaxed)
            .ok();
        self.tcp_connections.fetch_add(1, Ordering::Relaxed);
    }

    /// Record the end of one TCP connection.
    pub fn connection_closed(&self) {
        self.tcp_connections
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_sub(1))
            })
            .ok();
    }

    /// Snapshot the current values for display.
    pub fn snapshot(&self) -> TunnelConnectionInfo {
        TunnelConnectionInfo {
            route: *self.route.read().expect("tunnel live info lock poisoned"),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            connected_at_ms: self.connected_at_ms.load(Ordering::Relaxed),
            tcp_connections: self.tcp_connections.load(Ordering::Relaxed),
        }
    }
}

/// User-facing tunnel authorisation durations converted to timestamps at creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelDuration {
    /// Ten minutes.
    TenMinutes,
    /// Thirty minutes.
    ThirtyMinutes,
    /// One hour.
    OneHour,
    /// Eight hours.
    EightHours,
    /// Until the owner exits.
    UntilExit,
}

impl TunnelDuration {
    fn expires_at_ms(self, created_at_ms: u64) -> u64 {
        match self {
            Self::TenMinutes => created_at_ms.saturating_add(10 * 60 * 1_000),
            Self::ThirtyMinutes => created_at_ms.saturating_add(30 * 60 * 1_000),
            Self::OneHour => created_at_ms.saturating_add(60 * 60 * 1_000),
            Self::EightHours => created_at_ms.saturating_add(8 * 60 * 60 * 1_000),
            Self::UntilExit => u64::MAX,
        }
    }
}

impl std::fmt::Display for TunnelDuration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::TenMinutes => "10 minutes",
            Self::ThirtyMinutes => "30 minutes",
            Self::OneHour => "1 hour",
            Self::EightHours => "8 hours",
            Self::UntilExit => "Until Boru exits",
        };
        f.write_str(label)
    }
}

/// Metadata for one configured tunnel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelDefinition {
    /// Stable tunnel identifier.
    pub id: TunnelId,
    /// Endpoint identity that owns the tunnel.
    pub owner: iroh::PublicKey,
    /// Local service target.
    pub target: TunnelTarget,
    /// Endpoint identity authorised to connect.
    pub allowed_peer: iroh::PublicKey,
    /// Unix epoch milliseconds when the tunnel was created.
    pub created_at_ms: u64,
    /// Unix epoch milliseconds after which the tunnel expires.
    pub expires_at_ms: u64,
    /// Current lifecycle state.
    pub status: TunnelStatus,
    /// Maximum number of simultaneous connections for this tunnel.
    pub max_connections: usize,
    /// Number of connections currently using this tunnel.
    pub active_connections: usize,
}

/// Errors returned by [`TunnelService`] lifecycle operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelServiceError {
    /// The identifier is already configured.
    AlreadyExists,
    /// No active tunnel has this identifier.
    NotFound,
    /// The expiry must be later than creation.
    InvalidExpiry,
    /// The operation is not valid for the current lifecycle state.
    InvalidState,
    /// The target is not a loopback address.
    NonLoopbackTarget,
    /// The configured simultaneous connection limit has been reached.
    ConnectionLimitReached,
    /// The configured simultaneous connection limit is invalid.
    InvalidConnectionLimit,
    /// The tunnel's authorisation window has elapsed.
    Expired,
    /// The service-wide tunnel limit has been reached.
    TunnelLimitReached,
    /// The service-wide received-stream limit has been reached.
    ReceivedTunnelLimitReached,
    /// The peer has exceeded the connection-attempt rate limit.
    ConnectionAttemptLimitReached,
}

/// In-memory owner of configured tunnel metadata.
#[derive(Debug, Default)]
pub struct TunnelService {
    tunnels: RwLock<HashMap<TunnelId, TunnelDefinition>>,
    cancellation: RwLock<HashMap<TunnelId, CancellationToken>>,
    attempt_times: RwLock<HashMap<iroh::PublicKey, VecDeque<Instant>>>,
    live_info: RwLock<HashMap<TunnelId, Arc<TunnelLiveInfo>>>,
}

impl TunnelService {
    /// Construct an empty service.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tunnel in the active state.
    pub fn create_tunnel(
        &self,
        id: TunnelId,
        owner: iroh::PublicKey,
        target: TunnelTarget,
        allowed_peer: iroh::PublicKey,
        created_at_ms: u64,
        expires_at_ms: u64,
    ) -> Result<TunnelDefinition, TunnelServiceError> {
        self.create_tunnel_with_limit(
            id,
            owner,
            target,
            allowed_peer,
            created_at_ms,
            expires_at_ms,
            DEFAULT_MAX_CONNECTIONS_PER_TUNNEL,
        )
    }

    /// Register a tunnel using a supported duration choice.
    pub fn create_tunnel_for_duration(
        &self,
        id: TunnelId,
        owner: iroh::PublicKey,
        target: TunnelTarget,
        allowed_peer: iroh::PublicKey,
        duration: TunnelDuration,
    ) -> Result<TunnelDefinition, TunnelServiceError> {
        let created_at_ms = unix_epoch_ms();
        self.create_tunnel(
            id,
            owner,
            target,
            allowed_peer,
            created_at_ms,
            duration.expires_at_ms(created_at_ms),
        )
    }

    /// Register a tunnel with an explicit simultaneous connection limit.
    pub fn create_tunnel_with_limit(
        &self,
        id: TunnelId,
        owner: iroh::PublicKey,
        target: TunnelTarget,
        allowed_peer: iroh::PublicKey,
        created_at_ms: u64,
        expires_at_ms: u64,
        max_connections: usize,
    ) -> Result<TunnelDefinition, TunnelServiceError> {
        if !target.is_loopback() {
            tracing::warn!(tunnel = %super::tunnel_id_label(id), "tunnel rejected: target is not loopback");
            return Err(TunnelServiceError::NonLoopbackTarget);
        }
        if expires_at_ms <= created_at_ms {
            tracing::warn!(tunnel = %super::tunnel_id_label(id), "tunnel rejected: invalid expiry");
            return Err(TunnelServiceError::InvalidExpiry);
        }
        if max_connections == 0 {
            tracing::warn!(tunnel = %super::tunnel_id_label(id), "tunnel rejected: invalid connection limit");
            return Err(TunnelServiceError::InvalidConnectionLimit);
        }

        let definition = TunnelDefinition {
            id,
            owner,
            target,
            allowed_peer,
            created_at_ms,
            expires_at_ms,
            status: TunnelStatus::Active,
            max_connections,
            active_connections: 0,
        };
        let mut tunnels = self.tunnels.write().expect("tunnel service lock poisoned");
        if tunnels.contains_key(&id) {
            tracing::warn!(tunnel = %super::tunnel_id_label(id), "tunnel rejected: already exists");
            return Err(TunnelServiceError::AlreadyExists);
        }
        if tunnels.len() >= MAX_ACTIVE_SHARED_TUNNELS {
            tracing::warn!(tunnel = %super::tunnel_id_label(id), "tunnel rejected: tunnel limit reached");
            return Err(TunnelServiceError::TunnelLimitReached);
        }
        tunnels.insert(id, definition.clone());
        self.cancellation
            .write()
            .expect("tunnel cancellation lock poisoned")
            .insert(id, CancellationToken::new());
        tracing::info!(tunnel = %super::tunnel_id_label(id), "tunnel created");
        Ok(definition)
    }

    /// Return active and connecting/connected tunnel snapshots in stable order.
    pub fn list_tunnels(&self) -> Vec<TunnelDefinition> {
        let mut tunnels: Vec<_> = self
            .tunnels
            .read()
            .expect("tunnel service lock poisoned")
            .values()
            .cloned()
            .collect();
        tunnels.sort_by_key(|tunnel| tunnel.id.0);
        tunnels
    }

    /// Return one non-revoked tunnel snapshot.
    pub fn get_tunnel(&self, id: TunnelId) -> Option<TunnelDefinition> {
        self.tunnels
            .read()
            .expect("tunnel service lock poisoned")
            .get(&id)
            .cloned()
    }

    /// Return the live connection-info handle for a tunnel, if one exists.
    ///
    /// The handle is created lazily by [`Self::record_route`] /
    /// [`Self::record_transfer`] so an idle tunnel has no live-info entry.
    pub fn live_info(&self, id: TunnelId) -> Option<Arc<TunnelLiveInfo>> {
        self.live_info
            .read()
            .expect("tunnel service lock poisoned")
            .get(&id)
            .cloned()
    }

    /// Record the Iroh-reported route for a tunnel, creating the live-info
    /// entry on first use.
    pub fn record_route(&self, id: TunnelId, route: TunnelRoute) {
        let info = self.live_info_entry(id);
        info.set_route(route);
    }

    /// Record transferred bytes for a tunnel, creating the live-info entry on
    /// first use.
    pub fn record_transfer(&self, id: TunnelId, sent: u64, received: u64) {
        let info = self.live_info_entry(id);
        info.add_bytes(sent, received);
    }

    /// Record the start of one TCP connection using a tunnel.
    pub fn record_connection_opened(&self, id: TunnelId) {
        let info = self.live_info_entry(id);
        info.connection_opened(unix_epoch_ms());
    }

    /// Record the end of one TCP connection using a tunnel.
    pub fn record_connection_closed(&self, id: TunnelId) {
        if let Some(info) = self.live_info(id) {
            info.connection_closed();
        }
    }

    /// Return a snapshot of the current connection info for a tunnel.
    pub fn connection_info(&self, id: TunnelId) -> Option<TunnelConnectionInfo> {
        self.live_info(id).map(|info| info.snapshot())
    }

    fn live_info_entry(&self, id: TunnelId) -> Arc<TunnelLiveInfo> {
        let mut live = self
            .live_info
            .write()
            .expect("tunnel service lock poisoned");
        live.entry(id)
            .or_insert_with(|| Arc::new(TunnelLiveInfo::default()))
            .clone()
    }

    /// Revoke and remove a tunnel, returning its final metadata snapshot.
    /// Existing streams continue until closed; use
    /// [`Self::revoke_tunnel_with_termination`] to cancel them.
    pub fn revoke_tunnel(&self, id: TunnelId) -> Result<TunnelDefinition, TunnelServiceError> {
        self.revoke_tunnel_with_termination(id, false)
    }

    /// Revoke a tunnel and optionally terminate existing streams immediately.
    pub fn revoke_tunnel_with_termination(
        &self,
        id: TunnelId,
        terminate_existing: bool,
    ) -> Result<TunnelDefinition, TunnelServiceError> {
        let mut tunnels = self.tunnels.write().expect("tunnel service lock poisoned");
        let mut definition = tunnels.remove(&id).ok_or(TunnelServiceError::NotFound)?;
        definition.status = TunnelStatus::Revoked;
        self.live_info
            .write()
            .expect("tunnel service lock poisoned")
            .remove(&id);
        if let Some(token) = self
            .cancellation
            .write()
            .expect("tunnel cancellation lock poisoned")
            .remove(&id)
        {
            if terminate_existing {
                token.cancel();
            }
        }
        Ok(definition)
    }

    /// Get the cancellation token associated with an active stream.
    pub fn cancellation_token(
        &self,
        id: TunnelId,
    ) -> Result<CancellationToken, TunnelServiceError> {
        self.cancellation
            .read()
            .expect("tunnel cancellation lock poisoned")
            .get(&id)
            .cloned()
            .ok_or(TunnelServiceError::NotFound)
    }

    /// Mark an active tunnel as handed to the transport for connection.
    pub fn connect_tunnel(&self, id: TunnelId) -> Result<TunnelDefinition, TunnelServiceError> {
        let mut tunnels = self.tunnels.write().expect("tunnel service lock poisoned");
        let definition = tunnels.get_mut(&id).ok_or(TunnelServiceError::NotFound)?;
        if unix_epoch_ms() > definition.expires_at_ms {
            tracing::info!(tunnel = %super::tunnel_id_label(id), "tunnel expired");
            return Err(TunnelServiceError::Expired);
        }
        if definition.status != TunnelStatus::Active {
            return Err(TunnelServiceError::InvalidState);
        }
        definition.status = TunnelStatus::Connecting;
        tracing::debug!(tunnel = %super::tunnel_id_label(id), "tunnel connection started");
        Ok(definition.clone())
    }

    /// Record that the transport completed a connection attempt.
    pub fn mark_connected(&self, id: TunnelId) -> Result<TunnelDefinition, TunnelServiceError> {
        let mut tunnels = self.tunnels.write().expect("tunnel service lock poisoned");
        let definition = tunnels.get_mut(&id).ok_or(TunnelServiceError::NotFound)?;
        if !matches!(
            definition.status,
            TunnelStatus::Connecting | TunnelStatus::Connected
        ) {
            return Err(TunnelServiceError::InvalidState);
        }
        definition.status = TunnelStatus::Connected;
        tracing::info!(tunnel = %super::tunnel_id_label(id), "tunnel connected");
        Ok(definition.clone())
    }

    /// Reserve one connection slot, allowing multiple streams on one tunnel.
    pub fn try_acquire_connection(
        &self,
        id: TunnelId,
    ) -> Result<TunnelDefinition, TunnelServiceError> {
        let mut tunnels = self.tunnels.write().expect("tunnel service lock poisoned");
        let total_connections: usize = tunnels.values().map(|t| t.active_connections).sum();
        if total_connections >= MAX_ACTIVE_RECEIVED_TUNNELS {
            return Err(TunnelServiceError::ReceivedTunnelLimitReached);
        }
        let definition = tunnels.get_mut(&id).ok_or(TunnelServiceError::NotFound)?;
        if unix_epoch_ms() > definition.expires_at_ms {
            tracing::info!(tunnel = %super::tunnel_id_label(id), "tunnel expired");
            return Err(TunnelServiceError::Expired);
        }
        if definition.active_connections >= definition.max_connections {
            return Err(TunnelServiceError::ConnectionLimitReached);
        }
        definition.active_connections += 1;
        if definition.status == TunnelStatus::Active {
            definition.status = TunnelStatus::Connecting;
        }
        Ok(definition.clone())
    }

    /// Release a previously reserved connection slot.
    pub fn release_connection(&self, id: TunnelId) {
        let mut tunnels = self.tunnels.write().expect("tunnel service lock poisoned");
        if let Some(definition) = tunnels.get_mut(&id) {
            definition.active_connections = definition.active_connections.saturating_sub(1);
            if definition.active_connections == 0 && definition.status == TunnelStatus::Connected {
                definition.status = TunnelStatus::Active;
            }
            tracing::debug!(tunnel = %super::tunnel_id_label(id), active_connections = definition.active_connections, "tunnel connection closed");
        }
    }

    /// Admit one connection attempt from a peer using a bounded sliding window.
    pub fn record_connection_attempt(
        &self,
        peer: iroh::PublicKey,
    ) -> Result<(), TunnelServiceError> {
        let now = Instant::now();
        let cutoff = now.checked_sub(CONNECTION_ATTEMPT_INTERVAL).unwrap_or(now);
        let mut attempts = self
            .attempt_times
            .write()
            .expect("tunnel attempt limiter lock poisoned");
        if !attempts.contains_key(&peer) && attempts.len() >= MAX_TRACKED_ATTEMPT_PEERS {
            return Err(TunnelServiceError::ConnectionAttemptLimitReached);
        }
        let history = attempts.entry(peer).or_default();
        while history.front().is_some_and(|time| *time <= cutoff) {
            history.pop_front();
        }
        if history.len() >= MAX_CONNECTION_ATTEMPTS_PER_INTERVAL {
            return Err(TunnelServiceError::ConnectionAttemptLimitReached);
        }
        history.push_back(now);
        Ok(())
    }
}

fn unix_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use iroh::SecretKey;

    use super::{
        TunnelLiveInfo, TunnelRoute, TunnelService, TunnelServiceError, TunnelStatus, TunnelTarget,
    };
    use crate::tunnel::TunnelId;

    fn fixture() -> (TunnelService, iroh::PublicKey, iroh::PublicKey, TunnelId) {
        let owner = SecretKey::generate().public();
        let peer = SecretKey::generate().public();
        let id = TunnelId([7; 32]);
        (TunnelService::new(), owner, peer, id)
    }

    #[test]
    fn create_and_list_returns_active_tunnel_metadata() {
        let (service, owner, peer, id) = fixture();
        let target = TunnelTarget::tcp("127.0.0.1".parse::<IpAddr>().unwrap(), 3000);

        let created = service
            .create_tunnel(
                id,
                owner,
                target.clone(),
                peer,
                super::unix_epoch_ms(),
                super::unix_epoch_ms() + 60_000,
            )
            .unwrap();

        assert_eq!(created.id, id);
        assert_eq!(created.owner, owner);
        assert_eq!(created.target, target);
        assert_eq!(created.allowed_peer, peer);
        assert!(created.created_at_ms > 0);
        assert!(created.expires_at_ms > created.created_at_ms);
        assert_eq!(created.status, TunnelStatus::Active);
        assert_eq!(service.list_tunnels(), vec![created]);
    }

    #[test]
    fn revoke_removes_tunnel_from_active_list_and_marks_snapshot_revoked() {
        let (service, owner, peer, id) = fixture();
        service
            .create_tunnel(
                id,
                owner,
                TunnelTarget::tcp("127.0.0.1".parse().unwrap(), 3000),
                peer,
                super::unix_epoch_ms(),
                super::unix_epoch_ms() + 60_000,
            )
            .unwrap();

        let revoked = service.revoke_tunnel(id).unwrap();

        assert_eq!(revoked.status, TunnelStatus::Revoked);
        assert!(service.list_tunnels().is_empty());
        assert!(service.get_tunnel(id).is_none());
    }

    #[test]
    fn connect_moves_active_tunnel_to_connecting() {
        let (service, owner, peer, id) = fixture();
        service
            .create_tunnel(
                id,
                owner,
                TunnelTarget::tcp("127.0.0.1".parse().unwrap(), 3000),
                peer,
                super::unix_epoch_ms(),
                super::unix_epoch_ms() + 60_000,
            )
            .unwrap();

        let connecting = service.connect_tunnel(id).unwrap();

        assert_eq!(connecting.status, TunnelStatus::Connecting);
        assert_eq!(
            service.get_tunnel(id).unwrap().status,
            TunnelStatus::Connecting
        );
    }

    #[test]
    fn create_rejects_non_loopback_targets() {
        let (service, owner, peer, id) = fixture();
        let result = service.create_tunnel(
            id,
            owner,
            TunnelTarget::tcp("192.168.1.10".parse().unwrap(), 3000),
            peer,
            100,
            200,
        );
        assert_eq!(result, Err(TunnelServiceError::NonLoopbackTarget));
    }

    #[test]
    fn connection_limit_is_enforced_and_released() {
        let (service, owner, peer, id) = fixture();
        service
            .create_tunnel_with_limit(
                id,
                owner,
                TunnelTarget::tcp("127.0.0.1".parse().unwrap(), 3000),
                peer,
                super::unix_epoch_ms(),
                super::unix_epoch_ms() + 60_000,
                2,
            )
            .unwrap();

        assert_eq!(
            service
                .try_acquire_connection(id)
                .unwrap()
                .active_connections,
            1
        );
        assert_eq!(
            service
                .try_acquire_connection(id)
                .unwrap()
                .active_connections,
            2
        );
        assert_eq!(
            service.try_acquire_connection(id),
            Err(TunnelServiceError::ConnectionLimitReached)
        );
        service.release_connection(id);
        assert_eq!(
            service
                .try_acquire_connection(id)
                .unwrap()
                .active_connections,
            2
        );
    }

    #[test]
    fn zero_connection_limit_is_rejected() {
        let (service, owner, peer, id) = fixture();
        assert_eq!(
            service.create_tunnel_with_limit(
                id,
                owner,
                TunnelTarget::tcp("127.0.0.1".parse().unwrap(), 3000),
                peer,
                super::unix_epoch_ms(),
                super::unix_epoch_ms() + 60_000,
                0,
            ),
            Err(TunnelServiceError::InvalidConnectionLimit)
        );
    }

    #[test]
    fn connect_and_release_preserve_existing_lifecycle_api() {
        let (service, owner, peer, id) = fixture();
        service
            .create_tunnel_with_limit(
                id,
                owner,
                TunnelTarget::tcp("127.0.0.1".parse().unwrap(), 3000),
                peer,
                super::unix_epoch_ms(),
                super::unix_epoch_ms() + 60_000,
                1,
            )
            .unwrap();
        assert_eq!(
            service.connect_tunnel(id).unwrap().status,
            TunnelStatus::Connecting
        );
        assert_eq!(
            service.mark_connected(id).unwrap().status,
            TunnelStatus::Connected
        );
        service.release_connection(id);
        assert_eq!(service.get_tunnel(id).unwrap().status, TunnelStatus::Active);
    }

    #[test]
    fn ipv6_loopback_target_is_allowed() {
        let (service, owner, peer, id) = fixture();
        let target = TunnelTarget::tcp("::1".parse().unwrap(), 3000);
        assert_eq!(
            service
                .create_tunnel(
                    id,
                    owner,
                    target,
                    peer,
                    super::unix_epoch_ms(),
                    super::unix_epoch_ms() + 60_000,
                )
                .unwrap()
                .status,
            TunnelStatus::Active
        );
    }

    #[test]
    fn expired_tunnel_rejects_new_connections() {
        let (service, owner, peer, id) = fixture();
        service
            .create_tunnel(
                id,
                owner,
                TunnelTarget::tcp("127.0.0.1".parse().unwrap(), 3000),
                peer,
                1,
                2,
            )
            .unwrap();
        assert_eq!(
            service.try_acquire_connection(id),
            Err(TunnelServiceError::Expired)
        );
    }

    #[test]
    fn duration_choices_use_expected_expiry_windows() {
        let (service, owner, peer, id) = fixture();
        let tunnel = service
            .create_tunnel_for_duration(
                id,
                owner,
                TunnelTarget::tcp("127.0.0.1".parse().unwrap(), 3000),
                peer,
                super::TunnelDuration::TenMinutes,
            )
            .unwrap();
        assert_eq!(tunnel.expires_at_ms - tunnel.created_at_ms, 600_000);
    }

    #[test]
    fn revocation_can_cancel_existing_streams() {
        let (service, owner, peer, id) = fixture();
        service
            .create_tunnel_for_duration(
                id,
                owner,
                TunnelTarget::tcp("127.0.0.1".parse().unwrap(), 3000),
                peer,
                super::TunnelDuration::UntilExit,
            )
            .unwrap();
        let token = service.cancellation_token(id).unwrap();
        assert!(!token.is_cancelled());
        service.revoke_tunnel_with_termination(id, true).unwrap();
        assert!(token.is_cancelled());
        assert_eq!(
            service.try_acquire_connection(id),
            Err(TunnelServiceError::NotFound)
        );
    }

    #[test]
    fn connection_attempt_rate_limit_is_enforced() {
        let service = TunnelService::new();
        let peer = SecretKey::generate().public();
        for _ in 0..super::MAX_CONNECTION_ATTEMPTS_PER_INTERVAL {
            assert!(service.record_connection_attempt(peer).is_ok());
        }
        assert_eq!(
            service.record_connection_attempt(peer),
            Err(TunnelServiceError::ConnectionAttemptLimitReached)
        );
    }

    #[test]
    fn shared_tunnel_limit_is_enforced() {
        let service = TunnelService::new();
        let owner = SecretKey::generate().public();
        let peer = SecretKey::generate().public();
        for index in 0..super::MAX_ACTIVE_SHARED_TUNNELS {
            assert!(service
                .create_tunnel(
                    TunnelId([index as u8; 32]),
                    owner,
                    TunnelTarget::tcp("127.0.0.1".parse().unwrap(), 3000),
                    peer,
                    super::unix_epoch_ms(),
                    super::unix_epoch_ms() + 60_000,
                )
                .is_ok());
        }
        assert_eq!(
            service.create_tunnel(
                TunnelId([255; 32]),
                owner,
                TunnelTarget::tcp("127.0.0.1".parse().unwrap(), 3000),
                peer,
                super::unix_epoch_ms(),
                super::unix_epoch_ms() + 60_000,
            ),
            Err(TunnelServiceError::TunnelLimitReached)
        );
    }

    #[test]
    fn live_info_defaults_to_connected_route_and_zero_metrics() {
        let info = TunnelLiveInfo::default();
        let snapshot = info.snapshot();
        assert_eq!(snapshot.route, TunnelRoute::Connected);
        assert_eq!(snapshot.bytes_sent, 0);
        assert_eq!(snapshot.bytes_received, 0);
        assert_eq!(snapshot.connected_at_ms, 0);
        assert_eq!(snapshot.tcp_connections, 0);
    }

    #[test]
    fn live_info_accumulates_route_bytes_and_connection_count() {
        let info = TunnelLiveInfo::default();
        info.set_route(TunnelRoute::Relay);
        info.connection_opened(1_000);
        info.add_bytes(100, 250);
        info.add_bytes(50, 75);

        let snapshot = info.snapshot();
        assert_eq!(snapshot.route, TunnelRoute::Relay);
        assert_eq!(snapshot.bytes_sent, 150);
        assert_eq!(snapshot.bytes_received, 325);
        assert_eq!(snapshot.connected_at_ms, 1_000);
        assert_eq!(snapshot.tcp_connections, 1);

        info.connection_closed();
        assert_eq!(info.snapshot().tcp_connections, 0);
    }

    #[test]
    fn live_info_keeps_first_connected_timestamp() {
        let info = TunnelLiveInfo::default();
        info.connection_opened(500);
        info.connection_opened(900);
        assert_eq!(info.snapshot().connected_at_ms, 500);
    }

    #[test]
    fn route_labels_never_invent_a_route_for_ordinary_users() {
        assert_eq!(TunnelRoute::Direct.label(), "Direct");
        assert_eq!(TunnelRoute::Relay.label(), "Relay");
        assert_eq!(TunnelRoute::Unknown.label(), "Unknown");
        assert_eq!(TunnelRoute::Connected.label(), "Connected");
    }

    #[test]
    fn service_tracks_live_info_and_clears_it_on_revoke() {
        let (service, owner, peer, id) = fixture();
        service
            .create_tunnel(
                id,
                owner,
                TunnelTarget::tcp("127.0.0.1".parse().unwrap(), 3000),
                peer,
                super::unix_epoch_ms(),
                super::unix_epoch_ms() + 60_000,
            )
            .unwrap();

        assert!(service.connection_info(id).is_none());
        service.record_route(id, TunnelRoute::Direct);
        service.record_transfer(id, 10, 20);
        service.record_connection_opened(id);

        let info = service.connection_info(id).expect("live info recorded");
        assert_eq!(info.route, TunnelRoute::Direct);
        assert_eq!(info.bytes_sent, 10);
        assert_eq!(info.bytes_received, 20);
        assert_eq!(info.tcp_connections, 1);
        assert!(info.connected_at_ms > 0);

        service.record_connection_closed(id);
        assert_eq!(service.connection_info(id).unwrap().tcp_connections, 0);

        service.revoke_tunnel(id).unwrap();
        assert!(service.connection_info(id).is_none());
    }
}
