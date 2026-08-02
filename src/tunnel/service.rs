//! State management for configured secure tunnels.
//!
//! This service intentionally owns only tunnel metadata and lifecycle state. The
//! shared Iroh endpoint and stream forwarding remain in the networking layer;
//! this keeps GUI callers independent from the transport implementation.

use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    sync::RwLock,
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

    use super::{TunnelService, TunnelServiceError, TunnelStatus, TunnelTarget};
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
}
