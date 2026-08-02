//! State management for configured secure tunnels.
//!
//! This service intentionally owns only tunnel metadata and lifecycle state. The
//! shared Iroh endpoint and stream forwarding remain in the networking layer;
//! this keeps GUI callers independent from the transport implementation.

use std::{collections::HashMap, net::IpAddr, sync::RwLock};

use super::TunnelId;

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
}

/// In-memory owner of configured tunnel metadata.
#[derive(Debug, Default)]
pub struct TunnelService {
    tunnels: RwLock<HashMap<TunnelId, TunnelDefinition>>,
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
            16,
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
            return Err(TunnelServiceError::NonLoopbackTarget);
        }
        if expires_at_ms <= created_at_ms {
            return Err(TunnelServiceError::InvalidExpiry);
        }
        if max_connections == 0 {
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
            return Err(TunnelServiceError::AlreadyExists);
        }
        tunnels.insert(id, definition.clone());
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
    pub fn revoke_tunnel(&self, id: TunnelId) -> Result<TunnelDefinition, TunnelServiceError> {
        let mut tunnels = self.tunnels.write().expect("tunnel service lock poisoned");
        let mut definition = tunnels.remove(&id).ok_or(TunnelServiceError::NotFound)?;
        definition.status = TunnelStatus::Revoked;
        Ok(definition)
    }

    /// Mark an active tunnel as handed to the transport for connection.
    pub fn connect_tunnel(&self, id: TunnelId) -> Result<TunnelDefinition, TunnelServiceError> {
        let mut tunnels = self.tunnels.write().expect("tunnel service lock poisoned");
        let definition = tunnels.get_mut(&id).ok_or(TunnelServiceError::NotFound)?;
        if definition.status != TunnelStatus::Active {
            return Err(TunnelServiceError::InvalidState);
        }
        definition.status = TunnelStatus::Connecting;
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
        Ok(definition.clone())
    }

    /// Reserve one connection slot, allowing multiple streams on one tunnel.
    pub fn try_acquire_connection(
        &self,
        id: TunnelId,
    ) -> Result<TunnelDefinition, TunnelServiceError> {
        let mut tunnels = self.tunnels.write().expect("tunnel service lock poisoned");
        let definition = tunnels.get_mut(&id).ok_or(TunnelServiceError::NotFound)?;
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
        }
    }
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
            .create_tunnel(id, owner, target.clone(), peer, 100, 200)
            .unwrap();

        assert_eq!(created.id, id);
        assert_eq!(created.owner, owner);
        assert_eq!(created.target, target);
        assert_eq!(created.allowed_peer, peer);
        assert_eq!(created.created_at_ms, 100);
        assert_eq!(created.expires_at_ms, 200);
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
                100,
                200,
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
                100,
                200,
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
                100,
                200,
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
                100,
                200,
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
                100,
                200,
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
                .create_tunnel(id, owner, target, peer, 100, 200)
                .unwrap()
                .status,
            TunnelStatus::Active
        );
    }
}
