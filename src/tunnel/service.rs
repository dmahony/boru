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
        if expires_at_ms <= created_at_ms {
            return Err(TunnelServiceError::InvalidExpiry);
        }

        let definition = TunnelDefinition {
            id,
            owner,
            target,
            allowed_peer,
            created_at_ms,
            expires_at_ms,
            status: TunnelStatus::Active,
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
        if definition.status != TunnelStatus::Connecting {
            return Err(TunnelServiceError::InvalidState);
        }
        definition.status = TunnelStatus::Connected;
        Ok(definition.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use iroh::SecretKey;

    use super::{TunnelService, TunnelStatus, TunnelTarget};
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
}
