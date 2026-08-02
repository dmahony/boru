//! Loopback TCP listeners that open Boru tunnels for local applications.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

use iroh::{endpoint::Connection, Endpoint, EndpointAddr};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::{Mutex, OwnedSemaphorePermit, Semaphore},
};
use tokio_util::sync::CancellationToken;

use super::{forwarding, open_tunnel, TunnelCapability, TunnelId};

const DEFAULT_MAX_CONNECTIONS: usize = 16;

/// A local loopback listener for one configured tunnel.
#[derive(Debug)]
pub struct LocalTunnelListener {
    listener: TcpListener,
    endpoint: Endpoint,
    owner: EndpointAddr,
    tunnel_id: TunnelId,
    capability: TunnelCapability,
    max_connections: Arc<Semaphore>,
    peer_connection: Arc<Mutex<Option<Connection>>>,
}

impl LocalTunnelListener {
    /// Bind a listener for local applications.
    ///
    /// Only loopback addresses are accepted. In particular, `0.0.0.0` and
    /// other unspecified or LAN addresses are rejected rather than exposed by
    /// accident. Use port `0` for automatic port selection.
    pub async fn bind(
        endpoint: Endpoint,
        owner: EndpointAddr,
        tunnel_id: TunnelId,
        capability: TunnelCapability,
        bind_addr: SocketAddr,
    ) -> anyhow::Result<Self> {
        Self::bind_with_limit(
            endpoint,
            owner,
            tunnel_id,
            capability,
            bind_addr,
            DEFAULT_MAX_CONNECTIONS,
        )
        .await
    }

    /// Bind a listener with a finite per-tunnel connection limit.
    pub async fn bind_with_limit(
        endpoint: Endpoint,
        owner: EndpointAddr,
        tunnel_id: TunnelId,
        capability: TunnelCapability,
        bind_addr: SocketAddr,
        max_connections: usize,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            bind_addr.ip().is_loopback(),
            "local tunnel listeners must bind to a loopback address"
        );
        anyhow::ensure!(max_connections > 0, "maximum connections must be positive");
        let listener = TcpListener::bind(bind_addr).await?;
        Ok(Self {
            listener,
            endpoint,
            owner,
            tunnel_id,
            capability,
            max_connections: Arc::new(Semaphore::new(max_connections)),
            peer_connection: Arc::new(Mutex::new(None)),
        })
    }

    /// Bind to `127.0.0.1`, using `port` or selecting an available port when
    /// `port` is zero.
    pub async fn bind_loopback(
        endpoint: Endpoint,
        owner: EndpointAddr,
        tunnel_id: TunnelId,
        capability: TunnelCapability,
        port: u16,
    ) -> anyhow::Result<Self> {
        Self::bind_loopback_with_limit(
            endpoint,
            owner,
            tunnel_id,
            capability,
            port,
            DEFAULT_MAX_CONNECTIONS,
        )
        .await
    }

    /// Bind to loopback with a finite per-tunnel connection limit.
    pub async fn bind_loopback_with_limit(
        endpoint: Endpoint,
        owner: EndpointAddr,
        tunnel_id: TunnelId,
        capability: TunnelCapability,
        port: u16,
        max_connections: usize,
    ) -> anyhow::Result<Self> {
        Self::bind_with_limit(
            endpoint,
            owner,
            tunnel_id,
            capability,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            max_connections,
        )
        .await
    }

    /// Return the selected local address, including an automatically selected
    /// port.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accept one local application connection and route it through the
    /// configured tunnel until either side closes.
    pub async fn accept_once(&self) -> anyhow::Result<()> {
        let (local, _) = self.listener.accept().await?;
        let permit = self
            .max_connections
            .clone()
            .try_acquire_owned()
            .map_err(|_| anyhow::anyhow!("tunnel connection limit reached"))?;
        self.route(local, permit).await
    }

    /// Run the listener until cancellation, routing each application
    /// connection in its own task.
    pub async fn run(self, cancellation: CancellationToken) -> anyhow::Result<()> {
        loop {
            let accepted = tokio::select! {
                _ = cancellation.cancelled() => return Ok(()),
                accepted = self.listener.accept() => accepted?,
            };
            let (local, _) = accepted;
            let permit = match self.max_connections.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    let mut local = local;
                    let _ = local.shutdown().await;
                    tracing::debug!("local tunnel connection limit reached");
                    continue;
                }
            };
            let endpoint = self.endpoint.clone();
            let owner = self.owner.clone();
            let tunnel_id = self.tunnel_id;
            let capability = self.capability.clone();
            let peer_connection = Arc::clone(&self.peer_connection);
            tokio::spawn(async move {
                let result = Self::route_with(
                    endpoint,
                    owner,
                    tunnel_id,
                    capability,
                    peer_connection,
                    local,
                    permit,
                )
                .await;
                if let Err(error) = result {
                    tracing::debug!(%error, "local tunnel connection stopped");
                }
            });
        }
    }

    async fn route(&self, local: TcpStream, permit: OwnedSemaphorePermit) -> anyhow::Result<()> {
        Self::route_with(
            self.endpoint.clone(),
            self.owner.clone(),
            self.tunnel_id,
            self.capability.clone(),
            Arc::clone(&self.peer_connection),
            local,
            permit,
        )
        .await
    }

    async fn route_with(
        endpoint: Endpoint,
        owner: EndpointAddr,
        tunnel_id: TunnelId,
        capability: TunnelCapability,
        peer_connection: Arc<Mutex<Option<Connection>>>,
        local: TcpStream,
        _permit: OwnedSemaphorePermit,
    ) -> anyhow::Result<()> {
        let connection = {
            let mut shared = peer_connection.lock().await;
            if let Some(connection) = shared.as_ref() {
                connection.clone()
            } else {
                let route = if owner.relay_urls().next().is_some() {
                    "relay"
                } else if owner.ip_addrs().next().is_some() {
                    "direct"
                } else {
                    "unknown"
                };
                let connection = endpoint.connect(owner, super::BORU_TUNNEL_ALPN).await?;
                tracing::info!(
                    tunnel = %super::tunnel_id_label(tunnel_id),
                    route,
                    "tunnel route established"
                );
                *shared = Some(connection.clone());
                connection
            }
        };
        let (send, recv) = open_tunnel(&connection, tunnel_id, capability).await?;
        forwarding::forward_bidirectional(local, send, recv, CancellationToken::new()).await;
        tracing::debug!(tunnel = %super::tunnel_id_label(tunnel_id), "tunnel local connection closed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use iroh::{endpoint::presets, protocol::Router, Endpoint, EndpointAddr, SecretKey};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        time::timeout,
    };
    use tokio_util::sync::CancellationToken;

    use super::LocalTunnelListener;
    use crate::tunnel::{
        service::{TunnelService, TunnelTarget},
        TunnelCapability, TunnelId, BORU_TUNNEL_ALPN,
    };

    #[tokio::test]
    async fn binds_loopback_and_reports_automatic_port() -> anyhow::Result<()> {
        let endpoint = Endpoint::bind(presets::Minimal).await?;
        let owner = Endpoint::bind(presets::Minimal).await?;
        let tunnel = LocalTunnelListener::bind_loopback(
            endpoint,
            EndpointAddr::new(owner.id()),
            TunnelId([1; 32]),
            TunnelCapability::sign(
                &SecretKey::generate(),
                owner.id(),
                TunnelId([1; 32]),
                0,
                u64::MAX,
            ),
            0,
        )
        .await?;
        let addr = tunnel.local_addr()?;
        assert_eq!(addr.ip(), "127.0.0.1".parse::<std::net::IpAddr>()?);
        assert_ne!(addr.port(), 0);
        owner.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn rejects_non_loopback_bind_addresses() -> anyhow::Result<()> {
        let endpoint = Endpoint::bind(presets::Minimal).await?;
        let result = LocalTunnelListener::bind(
            endpoint,
            EndpointAddr::new(SecretKey::generate().public()),
            TunnelId([2; 32]),
            TunnelCapability::sign(
                &SecretKey::generate(),
                SecretKey::generate().public(),
                TunnelId([2; 32]),
                0,
                u64::MAX,
            ),
            "0.0.0.0:0".parse()?,
        )
        .await;
        assert!(result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn local_connection_routes_to_owner_target() -> anyhow::Result<()> {
        let target_listener = TcpListener::bind("127.0.0.1:0").await?;
        let target_addr = target_listener.local_addr()?;
        let target_task = tokio::spawn(async move {
            let mut handlers = Vec::new();
            for _ in 0..2 {
                let (mut socket, _) = target_listener.accept().await?;
                handlers.push(tokio::spawn(async move {
                    let mut request = [0; 4];
                    socket.read_exact(&mut request).await?;
                    socket.write_all(b"pong").await?;
                    anyhow::Ok(())
                }));
            }
            for handler in handlers {
                handler.await??;
            }
            anyhow::Ok(())
        });

        let owner_key = SecretKey::generate();
        let recipient_key = SecretKey::generate();
        let tunnel_id = TunnelId([3; 32]);
        let owner_endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(owner_key.clone())
            .bind()
            .await?;
        let recipient_endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(recipient_key.clone())
            .bind()
            .await?;
        let service = Arc::new(TunnelService::new());
        service
            .create_tunnel(
                tunnel_id,
                owner_key.public(),
                TunnelTarget::tcp(target_addr.ip(), target_addr.port()),
                recipient_key.public(),
                0,
                u64::MAX,
            )
            .unwrap();
        let protocol = super::super::TunnelProtocol::with_service(service, owner_key.public());
        let router = Router::builder(owner_endpoint)
            .accept(BORU_TUNNEL_ALPN, protocol)
            .spawn();
        let capability =
            TunnelCapability::sign(&owner_key, recipient_key.public(), tunnel_id, 0, u64::MAX);
        let listener = LocalTunnelListener::bind_loopback(
            recipient_endpoint.clone(),
            router.endpoint().addr(),
            tunnel_id,
            capability,
            0,
        )
        .await?;
        let addr = listener.local_addr()?;
        let cancellation = CancellationToken::new();
        let running = tokio::spawn(listener.run(cancellation.clone()));
        let mut apps = Vec::new();
        for _ in 0..2 {
            let addr = addr;
            apps.push(tokio::spawn(async move {
                let mut app = TcpStream::connect(addr).await?;
                app.write_all(b"ping").await?;
                let mut response = [0; 4];
                timeout(Duration::from_secs(2), app.read_exact(&mut response)).await??;
                anyhow::ensure!(&response == b"pong");
                app.shutdown().await?;
                anyhow::Ok(())
            }));
        }
        for (index, app) in apps.into_iter().enumerate() {
            let result = app
                .await
                .map_err(|error| anyhow::anyhow!("app task {index}: {error}"))?;
            result.map_err(|error| anyhow::anyhow!("app {index}: {error}"))?;
        }
        target_task.await??;
        cancellation.cancel();
        timeout(Duration::from_secs(2), running).await???;
        router.shutdown().await?;
        recipient_endpoint.close().await;
        Ok(())
    }
}
