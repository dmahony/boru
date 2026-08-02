//! Boru secure tunnel transport protocol.
//!
//! The tunnel protocol deliberately has its own ALPN while sharing Boru's
//! existing Iroh endpoint and protocol router.  Connections begin with the
//! versioned handshake messages below before later phases add capability
//! validation and stream forwarding.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use serde::{Deserialize, Serialize};

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};

/// Current version of the tunnel handshake wire messages.
pub const TUNNEL_PROTOCOL_VERSION: u16 = 1;

/// Stable identifier for a locally configured tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelId(pub [u8; 32]);

/// Opaque capability material carried by the handshake.
///
/// Capability creation and verification are intentionally deferred to the
/// capability phase.  Keeping this field in the wire format now prevents a
/// later protocol-breaking change while ensuring the handshake cannot grant
/// access based on peer identity alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelCapability(Vec<u8>);

impl TunnelCapability {
    /// Construct capability material for wire transport.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Borrow the opaque capability bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// First message sent by a tunnel initiator before application bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TunnelRequest {
    /// Request opening a previously configured tunnel.
    Open {
        /// Version of the tunnel handshake understood by the initiator.
        protocol_version: u16,
        /// Identifier of the tunnel selected by the initiator.
        tunnel_id: TunnelId,
        /// Opaque, recipient-bound capability token.
        capability: TunnelCapability,
    },
}

impl TunnelRequest {
    /// Construct an opening request using the current protocol version.
    pub fn open(tunnel_id: TunnelId, capability: TunnelCapability) -> Self {
        Self::Open {
            protocol_version: TUNNEL_PROTOCOL_VERSION,
            tunnel_id,
            capability,
        }
    }

    /// Return the protocol version advertised by this request.
    pub fn protocol_version(&self) -> u16 {
        match self {
            Self::Open {
                protocol_version, ..
            } => *protocol_version,
        }
    }
}

/// Wire-safe reason why a tunnel request was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelRejectReason {
    /// The tunnel identifier is not configured on the receiving peer.
    UnknownTunnel,
    /// The requesting peer is not authorised for this tunnel.
    NotAuthorised,
    /// The tunnel or capability is no longer valid.
    Expired,
    /// The capability could not be validated.
    InvalidCapability,
    /// The configured local target is unavailable.
    TargetUnavailable,
    /// The receiver cannot accept another tunnel at this time.
    Busy,
    /// The request uses a protocol version this peer does not support.
    ProtocolMismatch,
    /// The request could not be processed without exposing implementation details.
    InternalError,
}

/// Response to a [`TunnelRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TunnelResponse {
    /// The request passed the handshake checks; stream setup may proceed.
    Accepted,
    /// The request was rejected without disclosing local implementation data.
    Rejected(TunnelRejectReason),
}

impl TunnelResponse {
    /// Construct a rejection response.
    pub const fn rejected(reason: TunnelRejectReason) -> Self {
        Self::Rejected(reason)
    }
}

/// Map an advertised protocol version to the handshake response.
pub const fn reject_for_protocol_version(version: u16) -> TunnelResponse {
    if version == TUNNEL_PROTOCOL_VERSION {
        TunnelResponse::Accepted
    } else {
        TunnelResponse::Rejected(TunnelRejectReason::ProtocolMismatch)
    }
}

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

    use super::{
        reject_for_protocol_version, TunnelCapability, TunnelId, TunnelProtocol,
        TunnelRejectReason, TunnelRequest, TunnelResponse, BORU_TUNNEL_ALPN,
        TUNNEL_PROTOCOL_VERSION,
    };

    #[test]
    fn tunnel_alpn_is_stable() {
        assert_eq!(BORU_TUNNEL_ALPN, b"/boru-tunnel/1");
    }

    #[test]
    fn tunnel_request_round_trips_through_postcard() {
        let request = TunnelRequest::open(
            TunnelId([7; 32]),
            TunnelCapability::from_bytes(vec![1, 2, 3]),
        );
        let bytes = postcard::to_stdvec(&request).expect("serialize request");
        let decoded: TunnelRequest = postcard::from_bytes(&bytes).expect("deserialize request");
        assert_eq!(request, decoded);
        assert_eq!(request.protocol_version(), TUNNEL_PROTOCOL_VERSION);
    }

    #[test]
    fn tunnel_response_round_trips_rejection_reason() {
        let response = TunnelResponse::rejected(TunnelRejectReason::NotAuthorised);
        let bytes = postcard::to_stdvec(&response).expect("serialize response");
        let decoded: TunnelResponse = postcard::from_bytes(&bytes).expect("deserialize response");
        assert_eq!(response, decoded);
    }

    #[test]
    fn unsupported_protocol_version_maps_to_protocol_mismatch() {
        assert_eq!(
            reject_for_protocol_version(TUNNEL_PROTOCOL_VERSION + 1),
            TunnelResponse::rejected(TunnelRejectReason::ProtocolMismatch)
        );
        assert_eq!(
            reject_for_protocol_version(TUNNEL_PROTOCOL_VERSION),
            TunnelResponse::Accepted
        );
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
