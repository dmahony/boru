//! Loopback TCP listeners that open Boru tunnels for local applications.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use iroh::{Endpoint, EndpointAddr};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use super::{forwarding, open_tunnel, TunnelCapability, TunnelId};

/// A local loopback listener for one configured tunnel.
#[derive(Debug)]
pub struct LocalTunnelListener {
    listener: TcpListener,
    endpoint: Endpoint,
    owner: EndpointAddr,
    tunnel_id: TunnelId,
    capability: TunnelCapability,
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
        anyhow::ensure!(
            bind_addr.ip().is_loopback(),
            "local tunnel listeners must bind to a loopback address"
        );
        let listener = TcpListener::bind(bind_addr).await?;
        Ok(Self {
            listener,
            endpoint,
            owner,
            tunnel_id,
            capability,
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
        Self::bind(
            endpoint,
            owner,
            tunnel_id,
            capability,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
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
        self.route(local).await
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
            let endpoint = self.endpoint.clone();
            let owner = self.owner.clone();
            let tunnel_id = self.tunnel_id;
            let capability = self.capability.clone();
            tokio::spawn(async move {
                let result = Self::route_with(endpoint, owner, tunnel_id, capability, local).await;
                if let Err(error) = result {
                    tracing::debug!(%error, "local tunnel connection stopped");
                }
            });
        }
    }

    async fn route(&self, local: TcpStream) -> anyhow::Result<()> {
        Self::route_with(
            self.endpoint.clone(),
            self.owner.clone(),
            self.tunnel_id,
            self.capability.clone(),
            local,
        )
        .await
    }

    async fn route_with(
        endpoint: Endpoint,
        owner: EndpointAddr,
        tunnel_id: TunnelId,
        capability: TunnelCapability,
        local: TcpStream,
    ) -> anyhow::Result<()> {
        let connection = endpoint.connect(owner, super::BORU_TUNNEL_ALPN).await?;
        let (send, recv) = open_tunnel(&connection, tunnel_id, capability).await?;
        forwarding::forward_bidirectional(local, send, recv, CancellationToken::new()).await;
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
            let (mut socket, _) = target_listener.accept().await?;
            let mut request = [0; 4];
            socket.read_exact(&mut request).await?;
            socket.write_all(b"pong").await?;
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
        let mut app = TcpStream::connect(addr).await?;
        app.write_all(b"ping").await?;
        let mut response = [0; 4];
        timeout(Duration::from_secs(2), app.read_exact(&mut response)).await??;
        assert_eq!(&response, b"pong");
        app.shutdown().await?;
        target_task.await??;
        cancellation.cancel();
        timeout(Duration::from_secs(2), running).await???;
        router.shutdown().await?;
        recipient_endpoint.close().await;
        Ok(())
    }
}
