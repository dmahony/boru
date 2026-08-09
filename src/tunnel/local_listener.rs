//! Loopback TCP listeners that open Boru tunnels for local applications.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use iroh::{endpoint::Connection, Endpoint, EndpointAddr};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::{watch, Mutex, OwnedSemaphorePermit, Semaphore},
};
use tokio_util::sync::CancellationToken;

use super::{
    forwarding, open_tunnel,
    reconnect::{run_reconnect_loop, TunnelLinkHandle, TunnelLinkStatus},
    service::{ReconnectPolicy, TunnelLiveInfo},
    TunnelCapability, TunnelId, BORU_TUNNEL_ALPN,
};

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
    live: Arc<TunnelLiveInfo>,
    /// Maximum time one routed connection may remain idle before it is closed.
    idle_timeout: Duration,
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
            live: Arc::new(TunnelLiveInfo::default()),
            idle_timeout: super::TUNNEL_IDLE_TIMEOUT,
        })
    }

    /// Configure the per-connection idle timeout, clamped to the permitted
    /// range. Any forwarded byte in either direction resets the timer.
    pub fn with_idle_timeout(mut self, idle_timeout: Duration) -> Self {
        self.idle_timeout = super::service::clamp_tunnel_idle_timeout(idle_timeout);
        self
    }

    /// Return the configured per-connection idle timeout.
    pub fn idle_timeout(&self) -> Duration {
        self.idle_timeout
    }

    /// Return the shared live connection-info handle for this listener.
    ///
    /// The transport layer updates the handle as connections open, close, and
    /// forward bytes; the GUI reads a snapshot for display.
    pub fn live_info(&self) -> Arc<TunnelLiveInfo> {
        Arc::clone(&self.live)
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
        self.route(local, permit, CancellationToken::new()).await
    }

    /// Run the listener until cancellation, routing each application
    /// connection in its own task.
    ///
    /// A link-keeper task is spawned alongside the accept loop: it dials the
    /// owner and maintains the cached peer connection with exponential
    /// backoff. When the tunnel link drops, the keeper re-establishes it
    /// automatically instead of relying on the next local connection to
    /// re-dial, and mirrors the reconnecting state into the shared live info
    /// so the GUI can reflect it.
    pub async fn run(self, cancellation: CancellationToken) -> anyhow::Result<()> {
        let keeper = spawn_link_keeper(
            self.endpoint.clone(),
            self.owner.clone(),
            self.tunnel_id,
            self.capability.clone(),
            Arc::clone(&self.peer_connection),
            Arc::clone(&self.live),
            cancellation.clone(),
        );
        let result = self.run_accept_loop(cancellation).await;
        keeper.abort();
        result
    }

    /// Accept-and-route loop without the link keeper (used by `run`).
    async fn run_accept_loop(self, cancellation: CancellationToken) -> anyhow::Result<()> {
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
            let live = Arc::clone(&self.live);
            let idle_timeout = self.idle_timeout;
            let route_cancellation = cancellation.clone();
            tokio::spawn(async move {
                let result = Self::route_with(
                    endpoint,
                    owner,
                    tunnel_id,
                    capability,
                    peer_connection,
                    live,
                    idle_timeout,
                    local,
                    permit,
                    route_cancellation,
                )
                .await;
                if let Err(error) = result {
                    tracing::debug!(%error, "local tunnel connection stopped");
                }
            });
        }
    }

    async fn route(
        &self,
        local: TcpStream,
        permit: OwnedSemaphorePermit,
        cancellation: CancellationToken,
    ) -> anyhow::Result<()> {
        Self::route_with(
            self.endpoint.clone(),
            self.owner.clone(),
            self.tunnel_id,
            self.capability.clone(),
            Arc::clone(&self.peer_connection),
            Arc::clone(&self.live),
            self.idle_timeout,
            local,
            permit,
            cancellation,
        )
        .await
    }

    async fn route_with(
        endpoint: Endpoint,
        owner: EndpointAddr,
        tunnel_id: TunnelId,
        capability: TunnelCapability,
        peer_connection: Arc<Mutex<Option<Connection>>>,
        live: Arc<TunnelLiveInfo>,
        idle_timeout: Duration,
        local: TcpStream,
        _permit: OwnedSemaphorePermit,
        cancellation: CancellationToken,
    ) -> anyhow::Result<()> {
        let connection = {
            let mut shared = peer_connection.lock().await;
            if let Some(connection) = shared.as_ref() {
                connection.clone()
            } else {
                let connection = endpoint.connect(owner, super::BORU_TUNNEL_ALPN).await?;
                tracing::info!(
                    tunnel = %super::tunnel_id_label(tunnel_id),
                    "tunnel route established"
                );
                *shared = Some(connection.clone());
                connection
            }
        };
        live.set_route(super::connection_route(&connection));
        live.connection_opened(super::unix_epoch_ms());
        let (send, recv) = match open_tunnel(&connection, tunnel_id, capability).await {
            Ok(stream) => stream,
            Err(error) => {
                // A cached QUIC connection can outlive its peer. Do not make
                // all future local connections fail against the same dead
                // transport.
                if connection.close_reason().is_some() {
                    peer_connection.lock().await.take();
                }
                live.connection_closed();
                return Err(error);
            }
        };
        match forwarding::forward_bidirectional(
            local,
            send,
            recv,
            cancellation,
            idle_timeout,
            Some(live.clone()),
        )
        .await
        {
            Ok(forwarding::ForwardEnd::Eof) => {
                tracing::debug!(tunnel = %super::tunnel_id_label(tunnel_id), "tunnel local connection closed: end of stream");
            }
            Ok(forwarding::ForwardEnd::IdleTimeout) => {
                tracing::info!(
                    tunnel = %super::tunnel_id_label(tunnel_id),
                    idle_timeout = ?idle_timeout,
                    "tunnel local connection closed: idle timeout"
                );
            }
            Ok(forwarding::ForwardEnd::Cancelled) => {
                tracing::debug!(tunnel = %super::tunnel_id_label(tunnel_id), "tunnel local connection closed: cancelled");
            }
            Err(error) => {
                tracing::warn!(tunnel = %super::tunnel_id_label(tunnel_id), %error, "tunnel local forwarding failed");
            }
        }
        if connection.close_reason().is_some() {
            peer_connection.lock().await.take();
        }
        live.connection_closed();
        tracing::debug!(tunnel = %super::tunnel_id_label(tunnel_id), "tunnel local connection closed");
        Ok(())
    }
}

/// Spawn the link-keeper task for a [`LocalTunnelListener`].
///
/// The keeper dials the owner and maintains the cached peer connection with
/// exponential backoff. When the link drops it re-dials automatically (a
/// tunnel past its expiry is never re-dialed), and mirrors the reconnecting
/// state into `live` so the GUI can display it.
fn spawn_link_keeper(
    endpoint: Endpoint,
    owner: EndpointAddr,
    tunnel_id: TunnelId,
    capability: TunnelCapability,
    peer_connection: Arc<Mutex<Option<Connection>>>,
    live: Arc<TunnelLiveInfo>,
    cancellation: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let policy = ReconnectPolicy::default();
        let expires_at_ms = capability.expires_at_ms;
        let (status_tx, status_rx) = watch::channel(TunnelLinkStatus::Idle);
        let connect = {
            let endpoint = endpoint.clone();
            let owner = owner.clone();
            let peer_connection = Arc::clone(&peer_connection);
            move || -> std::pin::Pin<
                Box<dyn std::future::Future<Output = anyhow::Result<Arc<dyn TunnelLinkHandle>>> + Send>,
            > {
                let endpoint = endpoint.clone();
                let owner = owner.clone();
                let peer_connection = Arc::clone(&peer_connection);
                Box::pin(async move {
                    let connection = endpoint.connect(owner, BORU_TUNNEL_ALPN).await?;
                    *peer_connection.lock().await = Some(connection.clone());
                    Ok(Arc::new(connection) as Arc<dyn TunnelLinkHandle>)
                })
            }
        };
        // Mirror the keeper's status into the shared live info so the GUI can
        // reflect the reconnecting state.
        let live_mirror = Arc::clone(&live);
        let mirror = tokio::spawn(async move {
            let mut status_rx = status_rx;
            while status_rx.changed().await.is_ok() {
                let status = *status_rx.borrow();
                live_mirror
                    .set_reconnecting(matches!(status, TunnelLinkStatus::Reconnecting { .. }));
            }
        });
        run_reconnect_loop(
            connect,
            move || super::unix_epoch_ms() > expires_at_ms,
            policy,
            cancellation,
            status_tx,
            None,
        )
        .await;
        mirror.abort();
        tracing::info!(tunnel = %super::tunnel_id_label(tunnel_id), "tunnel link keeper stopped");
    })
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
    async fn binds_requested_loopback_port_when_available() -> anyhow::Result<()> {
        let endpoint = Endpoint::bind(presets::Minimal).await?;
        let owner = Endpoint::bind(presets::Minimal).await?;
        // Reserve a concrete loopback port so the test asserts the exact
        // requested port is bound, not just any loopback port.
        let probe = TcpListener::bind("127.0.0.1:0").await?;
        let requested = probe.local_addr()?.port();
        drop(probe);
        let tunnel = LocalTunnelListener::bind_loopback(
            endpoint,
            EndpointAddr::new(owner.id()),
            TunnelId([11; 32]),
            TunnelCapability::sign(
                &SecretKey::generate(),
                owner.id(),
                TunnelId([11; 32]),
                0,
                u64::MAX,
            ),
            requested,
        )
        .await?;
        let addr = tunnel.local_addr()?;
        assert_eq!(addr.port(), requested, "requested port is bound");
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
