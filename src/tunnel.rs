//! Boru secure tunnel transport protocol.
//!
//! The tunnel protocol deliberately has its own ALPN while sharing Boru's
//! existing Iroh endpoint and protocol router.  Connections begin with the
//! versioned handshake messages below before later phases add capability
//! validation and stream forwarding.

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{mpsc, Mutex},
};
use tokio_util::sync::CancellationToken;

use serde::{Deserialize, Serialize};

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};

pub(crate) mod forwarding;
mod local_listener;
pub mod service;

pub use local_listener::LocalTunnelListener;

use service::{TunnelService, TunnelStatus, TunnelTarget};

/// Current version of the tunnel handshake wire messages.
pub const TUNNEL_PROTOCOL_VERSION: u16 = 1;

/// Stable identifier for a locally configured tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// A raw bidirectional stream routed to a tunnel protocol handler.
pub type TunnelStream = (iroh::endpoint::SendStream, iroh::endpoint::RecvStream);

/// Handler for incoming Boru tunnel connections.
///
/// The handler owns no endpoint. It accepts every bidirectional stream on the
/// shared Iroh connection and exposes those streams to the tunnel service. This
/// keeps transport setup separate from the later TCP forwarding phase.
#[derive(Debug, Clone)]
pub struct TunnelProtocol {
    accepted: Arc<AtomicUsize>,
    streams: mpsc::Sender<TunnelStream>,
    incoming: Arc<Mutex<mpsc::Receiver<TunnelStream>>>,
    service: Option<Arc<TunnelService>>,
    owner: Option<iroh::PublicKey>,
}

impl Default for TunnelProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl TunnelProtocol {
    /// Construct a tunnel protocol handler.
    pub fn new() -> Self {
        let (streams, incoming) = mpsc::channel(32);
        Self {
            accepted: Arc::new(AtomicUsize::new(0)),
            streams,
            incoming: Arc::new(Mutex::new(incoming)),
            service: None,
            owner: None,
        }
    }

    /// Construct a handler that validates requests and forwards them to
    /// locally configured loopback services.
    pub fn with_service(service: Arc<TunnelService>, owner: iroh::PublicKey) -> Self {
        let mut protocol = Self::new();
        protocol.service = Some(service);
        protocol.owner = Some(owner);
        protocol
    }

    /// Return the number of incoming connections routed to this handler.
    pub fn accepted_count(&self) -> usize {
        self.accepted.load(Ordering::Acquire)
    }

    /// Wait for the next raw bidirectional stream from a remote tunnel peer.
    pub async fn accept_stream(&self) -> Option<TunnelStream> {
        self.incoming.lock().await.recv().await
    }
}

impl ProtocolHandler for TunnelProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        self.accepted.fetch_add(1, Ordering::AcqRel);
        let remote_peer = connection.remote_id();
        loop {
            let stream = connection.accept_bi().await.map_err(AcceptError::from)?;
            if let (Some(service), Some(owner)) = (&self.service, self.owner) {
                let service = Arc::clone(service);
                tokio::spawn(async move {
                    if let Err(error) =
                        handle_incoming_stream(stream, remote_peer, owner, service).await
                    {
                        tracing::debug!(%error, "tunnel stream rejected or closed");
                    }
                });
            } else if self.streams.send(stream).await.is_err() {
                return Ok(());
            }
        }
    }
}

const MAX_HANDSHAKE_SIZE: usize = 64 * 1024;

async fn write_frame<T: Serialize>(
    send: &mut iroh::endpoint::SendStream,
    value: &T,
) -> anyhow::Result<()> {
    let bytes = postcard::to_stdvec(value)?;
    anyhow::ensure!(
        bytes.len() <= MAX_HANDSHAKE_SIZE,
        "tunnel handshake is too large"
    );
    send.write_u32(bytes.len() as u32).await?;
    send.write_all(&bytes).await?;
    Ok(())
}

async fn read_frame<T: for<'de> Deserialize<'de>>(
    recv: &mut iroh::endpoint::RecvStream,
) -> anyhow::Result<T> {
    let length = recv.read_u32().await? as usize;
    anyhow::ensure!(
        length <= MAX_HANDSHAKE_SIZE,
        "tunnel handshake is too large"
    );
    let mut bytes = vec![0; length];
    recv.read_exact(&mut bytes).await?;
    Ok(postcard::from_bytes(&bytes)?)
}

/// Open an authorised tunnel stream and wait for the owner's handshake reply.
pub async fn open_tunnel(
    connection: &Connection,
    tunnel_id: TunnelId,
    capability: TunnelCapability,
) -> anyhow::Result<TunnelStream> {
    let (mut send, mut recv) = connection.open_bi().await?;
    write_frame(&mut send, &TunnelRequest::open(tunnel_id, capability)).await?;
    match read_frame::<TunnelResponse>(&mut recv).await? {
        TunnelResponse::Accepted => Ok((send, recv)),
        TunnelResponse::Rejected(reason) => anyhow::bail!("tunnel rejected: {reason:?}"),
    }
}

async fn handle_incoming_stream(
    (mut send, mut recv): TunnelStream,
    requesting_peer: iroh::PublicKey,
    owner: iroh::PublicKey,
    service: Arc<TunnelService>,
) -> anyhow::Result<()> {
    let request = read_frame::<TunnelRequest>(&mut recv).await?;
    let TunnelRequest::Open {
        protocol_version,
        tunnel_id,
        capability,
    } = request;
    if protocol_version != TUNNEL_PROTOCOL_VERSION {
        write_frame(&mut send, &reject_for_protocol_version(protocol_version)).await?;
        send.finish()?;
        anyhow::bail!("protocol mismatch");
    }
    let definition = match service.get_tunnel(tunnel_id) {
        Some(definition) => definition,
        None => {
            write_frame(
                &mut send,
                &TunnelResponse::rejected(TunnelRejectReason::UnknownTunnel),
            )
            .await?;
            send.finish()?;
            anyhow::bail!("unknown tunnel");
        }
    };
    if capability
        .verify_for(
            &owner,
            &requesting_peer,
            tunnel_id,
            unix_epoch_ms(),
            definition.status != TunnelStatus::Revoked,
        )
        .is_err()
    {
        write_frame(
            &mut send,
            &TunnelResponse::rejected(TunnelRejectReason::InvalidCapability),
        )
        .await?;
        send.finish()?;
        anyhow::bail!("invalid tunnel capability");
    }
    let _reservation = match service.try_acquire_connection(tunnel_id) {
        Ok(reservation) => reservation,
        Err(service::TunnelServiceError::ConnectionLimitReached) => {
            write_frame(
                &mut send,
                &TunnelResponse::rejected(TunnelRejectReason::Busy),
            )
            .await?;
            send.finish()?;
            anyhow::bail!("tunnel connection limit reached");
        }
        Err(_) => {
            write_frame(
                &mut send,
                &TunnelResponse::rejected(TunnelRejectReason::InvalidCapability),
            )
            .await?;
            send.finish()?;
            anyhow::bail!("tunnel is not available");
        }
    };
    let (host, port) = match definition.target {
        TunnelTarget::Tcp { host, port } => (host, port),
    };
    let local = match TcpStream::connect((host, port)).await {
        Ok(local) => local,
        Err(_) => {
            service.release_connection(tunnel_id);
            write_frame(
                &mut send,
                &TunnelResponse::rejected(TunnelRejectReason::TargetUnavailable),
            )
            .await?;
            send.finish()?;
            anyhow::bail!("local tunnel target unavailable");
        }
    };
    service
        .mark_connected(tunnel_id)
        .map_err(|_| anyhow::anyhow!("tunnel state changed"))?;
    write_frame(&mut send, &TunnelResponse::Accepted).await?;
    forwarding::forward_bidirectional(local, send, recv, CancellationToken::new()).await;
    service.release_connection(tunnel_id);
    Ok(())
}

/// Return the current Unix epoch in milliseconds for capability checks.
fn unix_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use iroh::{endpoint::presets, protocol::Router, Endpoint};
    use n0_error::{Result, StdResultExt};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time::timeout,
    };

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

    #[tokio::test]
    async fn raw_tunnel_stream_exchanges_deterministic_bytes() -> anyhow::Result<()> {
        let listener = Endpoint::bind(presets::Minimal).await?;
        let client = Endpoint::bind(presets::Minimal).await?;
        let tunnel = TunnelProtocol::new();
        let router = Router::builder(listener)
            .accept(BORU_TUNNEL_ALPN, tunnel.clone())
            .spawn();
        let connection = client
            .connect(router.endpoint().addr(), BORU_TUNNEL_ALPN)
            .await
            .std_context("connect tunnel")?;
        let (mut client_send, mut client_recv) = connection
            .open_bi()
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        client_send
            .write_all(b"hello from peer A")
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        client_send.finish().map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let (mut server_send, mut server_recv) =
            timeout(Duration::from_secs(2), tunnel.accept_stream())
                .await
                .std_context("accept raw stream")?
                .ok_or_else(|| anyhow::anyhow!("tunnel stream channel closed"))?;
        let received = server_recv
            .read_to_end(1024 * 1024)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        assert_eq!(received, b"hello from peer A");
        server_send
            .write_all(b"hello from peer B")
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        server_send.finish().map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let response = client_recv
            .read_to_end(1024 * 1024)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        assert_eq!(response, b"hello from peer B");
        router.shutdown().await.std_context("shutdown router")?;
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn raw_tunnel_stream_transfers_large_payload() -> anyhow::Result<()> {
        let listener = Endpoint::bind(presets::Minimal).await?;
        let client = Endpoint::bind(presets::Minimal).await?;
        let tunnel = TunnelProtocol::new();
        let router = Router::builder(listener)
            .accept(BORU_TUNNEL_ALPN, tunnel.clone())
            .spawn();
        let connection = client
            .connect(router.endpoint().addr(), BORU_TUNNEL_ALPN)
            .await
            .std_context("connect tunnel")?;
        let (mut send, _recv) = connection.open_bi().await?;
        let payload = (0..(2 * 1024 * 1024))
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        send.write_all(&payload[..1]).await?;
        let (_server_send, mut server_recv) =
            timeout(Duration::from_secs(2), tunnel.accept_stream())
                .await
                .std_context("accept large stream")?
                .ok_or_else(|| anyhow::anyhow!("tunnel stream channel closed"))?;
        let reader =
            tokio::spawn(async move { server_recv.read_to_end(2 * 1024 * 1024 + 1).await });
        send.write_all(&payload[1..]).await?;
        send.finish()?;
        assert_eq!(reader.await??, payload);
        router.shutdown().await.std_context("shutdown router")?;
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn raw_tunnel_stream_preserves_zero_length_finish() -> anyhow::Result<()> {
        let listener = Endpoint::bind(presets::Minimal).await?;
        let client = Endpoint::bind(presets::Minimal).await?;
        let tunnel = TunnelProtocol::new();
        let router = Router::builder(listener)
            .accept(BORU_TUNNEL_ALPN, tunnel.clone())
            .spawn();
        let connection = client
            .connect(router.endpoint().addr(), BORU_TUNNEL_ALPN)
            .await?;
        let (mut send, mut recv) = connection.open_bi().await?;
        send.finish()?;
        let (mut server_send, mut server_recv) =
            timeout(Duration::from_secs(2), tunnel.accept_stream())
                .await
                .std_context("accept zero-length stream")?
                .ok_or_else(|| anyhow::anyhow!("tunnel stream channel closed"))?;
        assert!(server_recv.read_to_end(1).await?.is_empty());
        server_send.finish()?;
        assert!(recv.read_to_end(1).await?.is_empty());
        router.shutdown().await.std_context("shutdown router")?;
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn raw_tunnel_stream_remote_disconnect_is_observable() -> anyhow::Result<()> {
        let listener = Endpoint::bind(presets::Minimal).await?;
        let client = Endpoint::bind(presets::Minimal).await?;
        let tunnel = TunnelProtocol::new();
        let router = Router::builder(listener)
            .accept(BORU_TUNNEL_ALPN, tunnel.clone())
            .spawn();
        let connection = client
            .connect(router.endpoint().addr(), BORU_TUNNEL_ALPN)
            .await?;
        let (mut send, mut recv) = connection.open_bi().await?;
        send.write_all(b"before disconnect").await?;
        let (_server_send, mut server_recv) =
            timeout(Duration::from_secs(2), tunnel.accept_stream())
                .await
                .std_context("accept disconnect stream")?
                .ok_or_else(|| anyhow::anyhow!("tunnel stream channel closed"))?;
        client.close().await;
        assert!(server_recv.read_to_end(1024).await.is_err());
        assert!(recv.read_to_end(1024).await.is_err());
        router.shutdown().await.std_context("shutdown router")?;
        Ok(())
    }

    #[tokio::test]
    async fn raw_tunnel_stream_read_can_be_cancelled() -> anyhow::Result<()> {
        let listener = Endpoint::bind(presets::Minimal).await?;
        let client = Endpoint::bind(presets::Minimal).await?;
        let tunnel = TunnelProtocol::new();
        let router = Router::builder(listener)
            .accept(BORU_TUNNEL_ALPN, tunnel.clone())
            .spawn();
        let connection = client
            .connect(router.endpoint().addr(), BORU_TUNNEL_ALPN)
            .await?;
        let (mut send, _recv) = connection.open_bi().await?;
        send.write_all(b"pending").await?;
        let (_server_send, mut server_recv) =
            timeout(Duration::from_secs(2), tunnel.accept_stream())
                .await
                .std_context("accept cancellable stream")?
                .ok_or_else(|| anyhow::anyhow!("tunnel stream channel closed"))?;
        assert!(
            timeout(Duration::from_millis(50), server_recv.read_to_end(1024))
                .await
                .is_err()
        );
        send.finish()?;
        assert_eq!(server_recv.read_to_end(1024).await?, Vec::<u8>::new());
        router.shutdown().await.std_context("shutdown router")?;
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn raw_tunnel_stream_supports_multiple_sequential_streams() -> anyhow::Result<()> {
        let listener = Endpoint::bind(presets::Minimal).await?;
        let client = Endpoint::bind(presets::Minimal).await?;
        let tunnel = TunnelProtocol::new();
        let router = Router::builder(listener)
            .accept(BORU_TUNNEL_ALPN, tunnel.clone())
            .spawn();
        let connection = client
            .connect(router.endpoint().addr(), BORU_TUNNEL_ALPN)
            .await?;
        for index in 0..4u8 {
            let (mut send, mut recv) = connection.open_bi().await?;
            send.write_all(&[index; 32]).await?;
            send.finish()?;
            let (mut server_send, mut server_recv) =
                timeout(Duration::from_secs(2), tunnel.accept_stream())
                    .await
                    .std_context("accept sequential stream")?
                    .ok_or_else(|| anyhow::anyhow!("tunnel stream channel closed"))?;
            assert_eq!(server_recv.read_to_end(64).await?, vec![index; 32]);
            server_send.write_all(&[index + 1; 16]).await?;
            server_send.finish()?;
            assert_eq!(recv.read_to_end(32).await?, vec![index + 1; 16]);
        }
        router.shutdown().await.std_context("shutdown router")?;
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn raw_tunnel_stream_supports_simultaneous_streams() -> anyhow::Result<()> {
        let listener = Endpoint::bind(presets::Minimal).await?;
        let client = Endpoint::bind(presets::Minimal).await?;
        let tunnel = TunnelProtocol::new();
        let router = Router::builder(listener)
            .accept(BORU_TUNNEL_ALPN, tunnel.clone())
            .spawn();
        let connection = client
            .connect(router.endpoint().addr(), BORU_TUNNEL_ALPN)
            .await?;
        let mut sends = Vec::new();
        for index in 0..4u8 {
            let connection = connection.clone();
            sends.push(tokio::spawn(async move {
                let (mut send, _recv) = connection.open_bi().await?;
                send.write_all(&[index; 128]).await?;
                send.finish()?;
                Ok::<_, anyhow::Error>(())
            }));
        }
        for _ in 0..4 {
            let (_send, mut recv) = timeout(Duration::from_secs(2), tunnel.accept_stream())
                .await
                .std_context("accept simultaneous stream")?
                .ok_or_else(|| anyhow::anyhow!("tunnel stream channel closed"))?;
            let bytes = recv.read_to_end(256).await?;
            assert_eq!(bytes.len(), 128);
            assert!(bytes.iter().all(|byte| *byte < 4));
        }
        for send in sends {
            send.await??;
        }
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

    #[tokio::test]
    async fn valid_capability_forwards_to_loopback_service() -> anyhow::Result<()> {
        let tcp_listener = TcpListener::bind("127.0.0.1:0").await?;
        let target_addr = tcp_listener.local_addr()?;
        let service_task = tokio::spawn(async move {
            let (mut socket, _) = tcp_listener.accept().await?;
            let mut request = [0; 4];
            socket.read_exact(&mut request).await?;
            socket.write_all(b"pong").await?;
            anyhow::Ok(())
        });

        let owner = iroh::SecretKey::generate();
        let tunnel_id = TunnelId([42; 32]);
        let listener = Endpoint::bind(presets::Minimal).await?;
        let client = Endpoint::bind(presets::Minimal).await?;
        let service = Arc::new(crate::tunnel::service::TunnelService::new());
        service
            .create_tunnel(
                tunnel_id,
                owner.public(),
                crate::tunnel::service::TunnelTarget::tcp(target_addr.ip(), target_addr.port()),
                client.id(),
                0,
                u64::MAX,
            )
            .unwrap();
        let tunnel = TunnelProtocol::with_service(Arc::clone(&service), owner.public());
        let router = Router::builder(listener)
            .accept(BORU_TUNNEL_ALPN, tunnel)
            .spawn();
        let connection = client
            .connect(router.endpoint().addr(), BORU_TUNNEL_ALPN)
            .await?;
        let capability = TunnelCapability::sign(&owner, client.id(), tunnel_id, 0, u64::MAX);
        let (mut send, mut recv) = super::open_tunnel(&connection, tunnel_id, capability).await?;
        send.write_all(b"ping").await?;
        send.finish()?;
        let mut response = [0; 4];
        recv.read_exact(&mut response).await?;
        assert_eq!(&response, b"pong");
        service_task.await??;
        router.shutdown().await?;
        client.close().await;
        Ok(())
    }
}
