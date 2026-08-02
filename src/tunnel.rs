//! Boru secure tunnel transport protocol.
//!
//! The tunnel protocol deliberately has its own ALPN while sharing Boru's
//! existing Iroh endpoint and protocol router.  The wire handshake and stream
//! forwarding are implemented in later tunnel phases; this phase establishes
//! the routing boundary without changing any existing protocol.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};

/// ALPN for Boru's secure tunnel protocol.
pub const BORU_TUNNEL_ALPN: &[u8] = b"/boru-tunnel/1";

/// Handler for incoming Boru tunnel connections.
///
/// For now the handler records that the connection reached the tunnel
/// protocol boundary.  Later phases will perform the authenticated handshake
/// and stream forwarding using this same handler.
#[derive(Debug, Clone, Default)]
pub struct TunnelProtocol {
    accepted: Arc<AtomicUsize>,
}

impl TunnelProtocol {
    /// Construct a tunnel protocol handler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the number of incoming connections routed to this handler.
    pub fn accepted_count(&self) -> usize {
        self.accepted.load(Ordering::Acquire)
    }
}

impl ProtocolHandler for TunnelProtocol {
    async fn accept(&self, _connection: Connection) -> Result<(), AcceptError> {
        self.accepted.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use iroh::{endpoint::presets, protocol::Router, Endpoint};
    use n0_error::{Result, StdResultExt};
    use tokio::time::timeout;

    use super::{TunnelProtocol, BORU_TUNNEL_ALPN};

    #[test]
    fn tunnel_alpn_is_stable() {
        assert_eq!(BORU_TUNNEL_ALPN, b"/boru-tunnel/1");
    }

    #[tokio::test]
    async fn incoming_tunnel_connection_routes_to_tunnel_handler() -> Result {
        let listener = Endpoint::bind(presets::Minimal).await?;
        let client = Endpoint::bind(presets::Minimal).await?;
        let tunnel = TunnelProtocol::new();
        let router = Router::builder(listener)
            .accept(BORU_TUNNEL_ALPN, tunnel.clone())
            .spawn();

        client
            .connect(router.endpoint().addr(), BORU_TUNNEL_ALPN)
            .await
            .std_context("connect tunnel")?;

        timeout(Duration::from_secs(2), async {
            while tunnel.accepted_count() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .std_context("wait for tunnel handler")?;

        assert_eq!(tunnel.accepted_count(), 1);
        router.shutdown().await.std_context("shutdown router")?;
        client.close().await;
        Ok(())
    }

    #[derive(Debug, Clone, Default)]
    struct CountingHandler(std::sync::Arc<std::sync::atomic::AtomicUsize>);

    impl CountingHandler {
        fn count(&self) -> usize {
            use std::sync::atomic::Ordering;
            self.0.load(Ordering::Acquire)
        }
    }

    impl iroh::protocol::ProtocolHandler for CountingHandler {
        async fn accept(
            &self,
            _connection: iroh::endpoint::Connection,
        ) -> Result<(), iroh::protocol::AcceptError> {
            use std::sync::atomic::Ordering;
            self.0.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    #[tokio::test]
    async fn unrelated_alpn_still_routes_to_its_original_handler() -> Result {
        let listener = Endpoint::bind(presets::Minimal).await?;
        let client = Endpoint::bind(presets::Minimal).await?;
        let tunnel = TunnelProtocol::new();
        let unrelated = CountingHandler::default();
        let unrelated_alpn = b"/boru-unrelated-test/1";
        let router = Router::builder(listener)
            .accept(BORU_TUNNEL_ALPN, tunnel.clone())
            .accept(unrelated_alpn, unrelated.clone())
            .spawn();

        client
            .connect(router.endpoint().addr(), unrelated_alpn)
            .await
            .std_context("connect unrelated protocol")?;

        timeout(Duration::from_secs(2), async {
            while unrelated.count() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .std_context("wait for unrelated handler")?;

        assert_eq!(unrelated.count(), 1);
        assert_eq!(tunnel.accepted_count(), 0);
        router.shutdown().await.std_context("shutdown router")?;
        client.close().await;
        Ok(())
    }
}
