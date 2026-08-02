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

/// Version of the signed capability contract.
pub const TUNNEL_CAPABILITY_VERSION: u16 = 1;

/// A recipient-bound, expiring authorisation to open one tunnel.
///
/// The signature covers every field except `signature`.  No target address or
/// other network metadata is included: possession of this token authorises
/// only the named tunnel for the named peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelCapability {
    /// Capability contract version.
    pub version: u16,
    /// Tunnel this token authorises.
    pub tunnel_id: TunnelId,
    /// Endpoint identity of the tunnel owner and signer.
    pub owner_endpoint_id: iroh::PublicKey,
    /// The only endpoint identity permitted to present this token.
    pub allowed_peer_endpoint_id: iroh::PublicKey,
    /// Unix epoch milliseconds at which the token becomes valid.
    pub created_at_ms: u64,
    /// Unix epoch milliseconds after which the token is invalid.
    pub expires_at_ms: u64,
    /// Unpredictable per-capability nonce preventing token reuse as a forgery.
    pub nonce: [u8; 32],
    signature: Vec<u8>,
}

/// Reason a received tunnel capability was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityVerificationError {
    /// The Ed25519 signature is invalid or malformed.
    InvalidSignature,
    /// The signer is not the configured tunnel owner.
    OwnerMismatch,
    /// The requesting endpoint is not the intended recipient.
    RecipientMismatch,
    /// The capability names a different tunnel.
    TunnelMismatch,
    /// The capability expiry has passed.
    Expired,
    /// The capability is from the future.
    NotYetValid,
    /// The capability contract version is unsupported.
    UnsupportedVersion,
    /// The configured tunnel is not currently active.
    TunnelInactive,
}

impl TunnelCapability {
    /// Create and sign a fresh capability. The nonce is generated internally.
    pub fn sign(
        owner: &iroh::SecretKey,
        allowed_peer_endpoint_id: iroh::PublicKey,
        tunnel_id: TunnelId,
        created_at_ms: u64,
        expires_at_ms: u64,
    ) -> Self {
        let mut capability = Self {
            version: TUNNEL_CAPABILITY_VERSION,
            tunnel_id,
            owner_endpoint_id: owner.public(),
            allowed_peer_endpoint_id,
            created_at_ms,
            expires_at_ms,
            nonce: rand::random(),
            signature: vec![0; 64],
        };
        capability.signature = owner.sign(&capability.signing_bytes()).to_bytes().to_vec();
        capability
    }

    /// Verify a received capability against the connection and local tunnel.
    pub fn verify_for(
        &self,
        expected_owner: &iroh::PublicKey,
        requesting_peer: &iroh::PublicKey,
        expected_tunnel: TunnelId,
        now_ms: u64,
        tunnel_active: bool,
    ) -> Result<(), CapabilityVerificationError> {
        if self.version != TUNNEL_CAPABILITY_VERSION {
            return Err(CapabilityVerificationError::UnsupportedVersion);
        }
        if &self.owner_endpoint_id != expected_owner {
            return Err(CapabilityVerificationError::OwnerMismatch);
        }
        if &self.allowed_peer_endpoint_id != requesting_peer {
            return Err(CapabilityVerificationError::RecipientMismatch);
        }
        if self.tunnel_id != expected_tunnel {
            return Err(CapabilityVerificationError::TunnelMismatch);
        }
        if !tunnel_active {
            return Err(CapabilityVerificationError::TunnelInactive);
        }
        if now_ms < self.created_at_ms {
            return Err(CapabilityVerificationError::NotYetValid);
        }
        if now_ms > self.expires_at_ms {
            return Err(CapabilityVerificationError::Expired);
        }
        let signature_bytes: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| CapabilityVerificationError::InvalidSignature)?;
        let signature = iroh::Signature::from_bytes(&signature_bytes);
        self.owner_endpoint_id
            .verify(&self.signing_bytes(), &signature)
            .map_err(|_| CapabilityVerificationError::InvalidSignature)
    }

    fn signing_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(&(
            self.version,
            self.tunnel_id,
            self.owner_endpoint_id,
            self.allowed_peer_endpoint_id,
            self.created_at_ms,
            self.expires_at_ms,
            self.nonce,
        ))
        .expect("postcard capability encoding cannot fail")
    }

    #[cfg(test)]
    fn signature_mut(&mut self) -> &mut Vec<u8> {
        &mut self.signature
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
        reject_for_protocol_version, CapabilityVerificationError, TunnelCapability, TunnelId,
        TunnelProtocol, TunnelRejectReason, TunnelRequest, TunnelResponse, BORU_TUNNEL_ALPN,
        TUNNEL_PROTOCOL_VERSION,
    };

    fn capability_fixture() -> (iroh::SecretKey, iroh::SecretKey, TunnelId, TunnelCapability) {
        let owner = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let tunnel_id = TunnelId([7; 32]);
        let capability = TunnelCapability::sign(&owner, recipient.public(), tunnel_id, 100, 200);
        (owner, recipient, tunnel_id, capability)
    }

    fn verify_fixture(
        capability: &TunnelCapability,
        owner: &iroh::SecretKey,
        recipient: &iroh::SecretKey,
        tunnel_id: TunnelId,
    ) -> Result<(), CapabilityVerificationError> {
        capability.verify_for(&owner.public(), &recipient.public(), tunnel_id, 150, true)
    }

    #[test]
    fn tunnel_alpn_is_stable() {
        assert_eq!(BORU_TUNNEL_ALPN, b"/boru-tunnel/1");
    }

    #[test]
    fn tunnel_request_round_trips_through_postcard() {
        let (_owner, _recipient, tunnel_id, capability) = capability_fixture();
        let request = TunnelRequest::open(tunnel_id, capability);
        let bytes = postcard::to_stdvec(&request).expect("serialize request");
        let decoded: TunnelRequest = postcard::from_bytes(&bytes).expect("deserialize request");
        assert_eq!(request, decoded);
        assert_eq!(request.protocol_version(), TUNNEL_PROTOCOL_VERSION);
    }

    #[test]
    fn valid_capability_verifies() {
        let (owner, recipient, tunnel_id, capability) = capability_fixture();
        assert_eq!(
            verify_fixture(&capability, &owner, &recipient, tunnel_id),
            Ok(())
        );
    }

    #[test]
    fn tampered_capability_is_rejected() {
        let (owner, recipient, tunnel_id, mut capability) = capability_fixture();
        capability.expires_at_ms += 1;
        assert_eq!(
            verify_fixture(&capability, &owner, &recipient, tunnel_id),
            Err(CapabilityVerificationError::InvalidSignature)
        );
    }

    #[test]
    fn wrong_recipient_is_rejected() {
        let (owner, _recipient, tunnel_id, capability) = capability_fixture();
        let other = iroh::SecretKey::generate();
        assert_eq!(
            capability.verify_for(&owner.public(), &other.public(), tunnel_id, 150, true),
            Err(CapabilityVerificationError::RecipientMismatch)
        );
    }

    #[test]
    fn wrong_owner_is_rejected() {
        let (_owner, recipient, tunnel_id, capability) = capability_fixture();
        let other = iroh::SecretKey::generate();
        assert_eq!(
            capability.verify_for(&other.public(), &recipient.public(), tunnel_id, 150, true),
            Err(CapabilityVerificationError::OwnerMismatch)
        );
    }

    #[test]
    fn expired_capability_is_rejected() {
        let (owner, recipient, tunnel_id, capability) = capability_fixture();
        assert_eq!(
            capability.verify_for(&owner.public(), &recipient.public(), tunnel_id, 201, true),
            Err(CapabilityVerificationError::Expired)
        );
    }

    #[test]
    fn wrong_tunnel_id_is_rejected() {
        let (owner, recipient, _tunnel_id, capability) = capability_fixture();
        assert_eq!(
            capability.verify_for(
                &owner.public(),
                &recipient.public(),
                TunnelId([8; 32]),
                150,
                true
            ),
            Err(CapabilityVerificationError::TunnelMismatch)
        );
    }

    #[test]
    fn corrupted_signature_is_rejected() {
        let (owner, recipient, tunnel_id, mut capability) = capability_fixture();
        capability.signature_mut()[0] ^= 1;
        assert_eq!(
            verify_fixture(&capability, &owner, &recipient, tunnel_id),
            Err(CapabilityVerificationError::InvalidSignature)
        );
    }

    #[test]
    fn unsupported_version_and_inactive_tunnel_are_rejected() {
        let (owner, recipient, tunnel_id, mut capability) = capability_fixture();
        capability.version += 1;
        assert_eq!(
            verify_fixture(&capability, &owner, &recipient, tunnel_id),
            Err(CapabilityVerificationError::UnsupportedVersion)
        );
        let capability = TunnelCapability::sign(&owner, recipient.public(), tunnel_id, 100, 200);
        assert_eq!(
            capability.verify_for(&owner.public(), &recipient.public(), tunnel_id, 150, false),
            Err(CapabilityVerificationError::TunnelInactive)
        );
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
