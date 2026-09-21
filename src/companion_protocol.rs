//! Companion-device protocol on the existing authenticated Iroh endpoint.
//!
//! The protocol is intentionally small and fail-closed. `/boru/companion/1`
//! uses length-prefixed JSON frames (big-endian `u32` length followed by one
//! JSON value). Frame length is checked before allocation. Pairing is a
//! separate unauthorised phase: it can report only that approval is required.
//! Profile, contact, and message data are unavailable until the remote endpoint
//! has been explicitly approved.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

use iroh::{
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
    EndpointId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::RwLock,
    time::timeout,
};
use zeroize::Zeroize;

use crate::store::MessageStore;

/// ALPN for the companion-device protocol.
pub const COMPANION_ALPN: &[u8] = b"/boru/companion/1";
/// Current companion wire version.
pub const COMPANION_WIRE_VERSION: u16 = 1;
/// Maximum encoded JSON frame, checked before allocation.
pub const MAX_COMPANION_FRAME_BYTES: usize = 64 * 1024;
/// Deadline for each handshake/request frame.
pub const COMPANION_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

/// Capabilities exposed before approval. These are protocol names only.
pub const PUBLIC_CAPABILITIES: &[&str] = &["pairing"];

/// A companion request frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CompanionRequest {
    /// Initial negotiation. No account data is included.
    Hello {
        /// Protocol versions understood by the peer.
        versions: Vec<u16>,
        /// Protocol capability names understood by the peer.
        capabilities: Vec<String>,
    },
    /// Unauthorised request to begin pairing approval.
    PairingRequest {
        /// User-facing device label; never used as an identity.
        device_name: String,
    },
    /// Claim a QR invitation using the authenticated endpoint identity.
    InvitationClaim {
        /// Opaque encoded invitation from the desktop QR code.
        invitation: String,
        /// User-facing device label; never used as an identity.
        device_name: String,
    },
    /// Request requiring prior explicit approval.
    AuthenticatedRequest {
        /// Caller-chosen request correlation identifier.
        request_id: String,
        /// Durable registration presented by the companion.
        registration_id: Vec<u8>,
        /// Device identity bound to the registration.
        device_id: Vec<u8>,
        /// Revision captured when the request was issued.
        grant_revision: i64,
        /// Capability being exercised by this request.
        capability: String,
        /// Requested companion method.
        method: String,
        /// Method parameters, interpreted only after approval.
        params: Value,
    },
}

/// A companion response frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CompanionResponse {
    /// Version and public protocol capabilities accepted.
    HelloAccepted {
        /// Negotiated protocol version.
        version: u16,
        /// Capability names safe to expose before approval.
        capabilities: Vec<String>,
    },
    /// The peer must be approved before authenticated methods are available.
    ApprovalRequired,
    /// Pairing was accepted for later approval; no profile data is disclosed.
    PairingPending,
    /// Comparison code for the local approval screen.
    PairingCode {
        /// Six-digit transcript comparison code.
        code: String,
    },
    /// Stable protocol errors that do not disclose local state.
    Error {
        /// Stable error code.
        code: CompanionErrorCode,
    },
    /// Successful method response (currently reserved for approved extensions).
    Result {
        /// Request correlation identifier.
        request_id: String,
        /// Result payload.
        value: Value,
    },
}

/// Stable companion protocol errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompanionErrorCode {
    /// The frame could not be decoded as the expected JSON request.
    MalformedFrame,
    /// The length prefix exceeded the allocation limit.
    FrameTooLarge,
    /// No mutually supported protocol version was advertised.
    IncompatibleVersion,
    /// The requested method is not implemented.
    UnknownMethod,
    /// The endpoint has not received explicit user approval.
    NotApproved,
    /// The listener is disabled by configuration.
    Disabled,
    /// The invitation was expired, malformed, superseded, or already used.
    InvalidInvitation,
    /// The local desktop has not approved this claim.
    ApprovalPending,
    /// The invitation was explicitly rejected locally.
    Rejected,
    /// The durable registration was revoked.
    Revoked,
    /// The request used an old grant revision.
    StaleGrant,
}

/// Versioned QR payload. Its secret is never included in `Debug` output.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionInvitation {
    /// Invitation format version.
    pub version: u16,
    /// Host endpoint identity.
    pub host_endpoint_id: String,
    /// Relay/direct routing hints; hints grant no access.
    pub routing_hints: Vec<String>,
    /// Random invitation identifier.
    pub invitation_id: [u8; 16],
    /// Random bearer secret, exactly 32 bytes.
    pub secret: Vec<u8>,
    /// Absolute Unix epoch expiry in milliseconds.
    pub expires_at_ms: u64,
}

impl fmt::Debug for CompanionInvitation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompanionInvitation")
            .field("version", &self.version)
            .field("host_endpoint_id", &self.host_endpoint_id)
            .field("routing_hints", &self.routing_hints)
            .field("invitation_id", &hex::encode(self.invitation_id))
            .field("secret", &"[REDACTED]")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

impl Drop for CompanionInvitation {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl CompanionInvitation {
    /// Encode the complete invitation for QR transport without logging it.
    pub fn encode(&self) -> Result<String, CompanionLinkError> {
        if self.version != COMPANION_WIRE_VERSION || self.secret.len() != 32 {
            return Err(CompanionLinkError::InvalidInvitation);
        }
        serde_json::to_vec(self)
            .map(hex::encode)
            .map_err(|_| CompanionLinkError::InvalidInvitation)
    }

    /// Decode and structurally validate a QR payload.
    pub fn decode(encoded: &str) -> Result<Self, CompanionLinkError> {
        let bytes = hex::decode(encoded).map_err(|_| CompanionLinkError::InvalidInvitation)?;
        let invitation: Self = serde_json::from_slice(&bytes)
            .map_err(|_| CompanionLinkError::InvalidInvitation)?;
        if invitation.version != COMPANION_WIRE_VERSION || invitation.secret.len() != 32 {
            return Err(CompanionLinkError::InvalidInvitation);
        }
        Ok(invitation)
    }
}

/// Local linking failures. Remote peers receive only stable protocol codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionLinkError {
    /// Linking is disabled.
    Disabled,
    /// The token is malformed or not the active token.
    InvalidInvitation,
    /// The token has passed its expiry.
    Expired,
    /// The local user rejected the claim.
    Rejected,
    /// No claim exists for the supplied invitation id.
    UnknownClaim,
    /// The durable grant could not be committed.
    Persistence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingClaim {
    invitation_id: [u8; 16],
    device_key: EndpointId,
    comparison_code: String,
    approved: bool,
    rejected: bool,
}

#[derive(Default)]
struct LinkState {
    active: Option<CompanionInvitation>,
    claims: HashMap<[u8; 16], PendingClaim>,
}

/// Desktop-owned QR invitation and approval state.
#[derive(Clone)]
pub struct CompanionLinkManager {
    state: Arc<Mutex<LinkState>>,
    store: Option<MessageStore>,
    enabled: Arc<Mutex<bool>>,
}

impl fmt::Debug for CompanionLinkManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompanionLinkManager")
            .field("enabled", &self.enabled.lock().map(|v| *v).unwrap_or(false))
            .finish_non_exhaustive()
    }
}

impl CompanionLinkManager {
    /// Create a manager. No invitation exists until `create_invitation`.
    pub fn new(store: Option<MessageStore>, enabled: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(LinkState::default())),
            store,
            enabled: Arc::new(Mutex::new(enabled)),
        }
    }

    /// Create one active invitation; creating another invalidates the first.
    pub fn create_invitation(
        &self,
        host_endpoint_id: String,
        routing_hints: Vec<String>,
        now_ms: u64,
    ) -> Result<CompanionInvitation, CompanionLinkError> {
        if !*self.enabled.lock().unwrap() {
            return Err(CompanionLinkError::Disabled);
        }
        let mut invitation_id = [0; 16];
        let mut secret = vec![0; 32];
        getrandom::fill(&mut invitation_id).map_err(|_| CompanionLinkError::InvalidInvitation)?;
        getrandom::fill(&mut secret).map_err(|_| CompanionLinkError::InvalidInvitation)?;
        let invitation = CompanionInvitation {
            version: COMPANION_WIRE_VERSION,
            host_endpoint_id,
            routing_hints,
            invitation_id,
            secret,
            expires_at_ms: now_ms.saturating_add(120_000),
        };
        self.state.lock().unwrap().active = Some(invitation.clone());
        Ok(invitation)
    }

    /// Disable linking and erase all bearer material and pending claims.
    pub fn disable(&self) {
        *self.enabled.lock().unwrap() = false;
        let mut state = self.state.lock().unwrap();
        state.active = None;
        state.claims.clear();
    }

    /// Erase process-local invitation state, modelling a desktop restart.
    pub fn restart(&self) {
        let mut state = self.state.lock().unwrap();
        state.active = None;
        state.claims.clear();
    }

    /// Bind the first valid claim to the authenticated endpoint key.
    pub fn claim(
        &self,
        encoded: &str,
        device_key: EndpointId,
        _device_name: &str,
        now_ms: u64,
    ) -> Result<String, CompanionLinkError> {
        if !*self.enabled.lock().unwrap() {
            return Err(CompanionLinkError::Disabled);
        }
        let invitation = CompanionInvitation::decode(encoded)?;
        let mut state = self.state.lock().unwrap();
        let active = state.active.as_ref().ok_or(CompanionLinkError::InvalidInvitation)?;
        if active.invitation_id != invitation.invitation_id
            || active.secret != invitation.secret
            || now_ms >= active.expires_at_ms
        {
            return Err(if now_ms >= active.expires_at_ms {
                CompanionLinkError::Expired
            } else {
                CompanionLinkError::InvalidInvitation
            });
        }
        if let Some(existing) = state.claims.get(&invitation.invitation_id) {
            if existing.device_key != device_key {
                return Err(CompanionLinkError::InvalidInvitation);
            }
            if existing.rejected {
                return Err(CompanionLinkError::Rejected);
            }
            return Ok(existing.comparison_code.clone());
        }
        let code = comparison_code(&invitation, &device_key);
        state.claims.insert(
            invitation.invitation_id,
            PendingClaim {
                invitation_id: invitation.invitation_id,
                device_key,
                comparison_code: code.clone(),
                approved: false,
                rejected: false,
            },
        );
        Ok(code)
    }

    /// Apply the only state transition that can create a durable grant.
    pub fn approve(&self, invitation_id: [u8; 16], approve: bool) -> Result<bool, CompanionLinkError> {
        let mut state = self.state.lock().unwrap();
        let claim = state.claims.get_mut(&invitation_id).ok_or(CompanionLinkError::UnknownClaim)?;
        if claim.approved {
            return Ok(true);
        }
        if !approve {
            claim.rejected = true;
            return Ok(false);
        }
        if let Some(store) = &self.store {
            store
                .register_device(&claim.invitation_id, claim.device_key.as_bytes())
                .map_err(|_| CompanionLinkError::Persistence)?;
        }
        claim.approved = true;
        Ok(true)
    }
}

fn comparison_code(invitation: &CompanionInvitation, device_key: &EndpointId) -> String {
    let mut hasher = blake3::Hasher::new_derive_key("boru companion linking transcript v1");
    hasher.update(&invitation.version.to_be_bytes());
    hasher.update(&invitation.invitation_id);
    hasher.update(&invitation.secret);
    hasher.update(device_key.as_bytes());
    let digest = hasher.finalize();
    let number = u32::from_be_bytes(digest.as_bytes()[..4].try_into().unwrap()) % 1_000_000;
    format!("{number:06}")
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Policy/state used by the protocol handler.
#[derive(Debug, Clone, Default)]
pub struct CompanionPolicy {
    approved: Arc<RwLock<HashSet<EndpointId>>>,
    store: Option<MessageStore>,
    sessions: Arc<Mutex<HashMap<EndpointId, Vec<Connection>>>>,
}

impl CompanionPolicy {
    /// Create an empty policy. Pairing never implicitly approves a device.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach the durable grant store used for per-request authorization.
    pub fn with_store(mut self, store: MessageStore) -> Self {
        self.store = Some(store);
        self
    }

    fn register_session(&self, endpoint: EndpointId, connection: Connection) {
        self.sessions
            .lock()
            .unwrap()
            .entry(endpoint)
            .or_default()
            .push(connection);
    }

    /// Revoke durable access and immediately close tracked sessions.
    pub fn revoke(&self, registration_id: &[u8], endpoint: EndpointId) -> bool {
        let revoked = self
            .store
            .as_ref()
            .and_then(|store| store.revoke_device(registration_id).ok())
            .unwrap_or(false);
        if revoked {
            if let Some(sessions) = self.sessions.lock().unwrap().remove(&endpoint) {
                for connection in sessions {
                    connection.close(iroh::endpoint::VarInt::from_u32(1), b"companion revoked");
                }
            }
        }
        revoked
    }

    /// Mark the authenticated endpoint identity approved after user consent.
    pub async fn approve(&self, endpoint: EndpointId) {
        self.approved.write().await.insert(endpoint);
    }

    async fn is_approved(&self, endpoint: EndpointId) -> bool {
        self.approved.read().await.contains(&endpoint)
    }

    async fn authorize(
        &self,
        endpoint: EndpointId,
        registration_id: &[u8],
        device_id: &[u8],
        grant_revision: i64,
    ) -> bool {
        if endpoint.as_bytes() != device_id {
            return false;
        }
        match &self.store {
            Some(store) => store
                .authorize_companion(registration_id, device_id, grant_revision)
                .map(|grant| grant.is_some())
                .unwrap_or(false),
            None => self.is_approved(endpoint).await,
        }
    }
}

/// Handler registered on the shared Iroh router when the feature is enabled.
#[derive(Debug, Clone)]
pub struct CompanionProtocolHandler {
    policy: CompanionPolicy,
    enabled: bool,
    link_manager: Option<CompanionLinkManager>,
}

impl CompanionProtocolHandler {
    /// Build a handler. `enabled = false` rejects without registering data paths.
    pub fn new(policy: CompanionPolicy, enabled: bool) -> Self {
        Self { policy, enabled, link_manager: None }
    }

    /// Attach desktop-owned invitation and approval state.
    pub fn with_link_manager(mut self, manager: CompanionLinkManager) -> Self {
        self.link_manager = Some(manager);
        self
    }
}

impl ProtocolHandler for CompanionProtocolHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let policy = self.policy.clone();
        let enabled = self.enabled;
        let link_manager = self.link_manager.clone();
        let remote = connection.remote_id();
        let lifetime = connection.clone();
        policy.register_session(remote, connection.clone());
        loop {
            let Ok((mut send, mut recv)) = connection.accept_bi().await else {
                break;
            };
            let response = if !enabled {
                CompanionResponse::Error {
                    code: CompanionErrorCode::Disabled,
                }
            } else {
                match timeout(COMPANION_FRAME_TIMEOUT, read_frame(&mut recv)).await {
                    Ok(Ok(request)) => handle_request(request, remote, &policy, link_manager.as_ref()).await,
                    Ok(Err(FrameError::TooLarge)) => CompanionResponse::Error {
                        code: CompanionErrorCode::FrameTooLarge,
                    },
                    Ok(Err(FrameError::Malformed)) | Err(_) => CompanionResponse::Error {
                        code: CompanionErrorCode::MalformedFrame,
                    },
                    Ok(Err(FrameError::Io)) => break,
                }
            };
            if write_frame(&mut send, &response).await.is_err() {
                break;
            }
            let _ = send.finish();
        }
        let _ = lifetime.closed().await;
        Ok(())
    }
}

async fn handle_request(
    request: CompanionRequest,
    remote: EndpointId,
    policy: &CompanionPolicy,
    link_manager: Option<&CompanionLinkManager>,
) -> CompanionResponse {
    match request {
        CompanionRequest::Hello { versions, .. } if versions.contains(&COMPANION_WIRE_VERSION) => {
            CompanionResponse::HelloAccepted {
                version: COMPANION_WIRE_VERSION,
                capabilities: PUBLIC_CAPABILITIES
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect(),
            }
        }
        CompanionRequest::Hello { .. } => CompanionResponse::Error {
            code: CompanionErrorCode::IncompatibleVersion,
        },
        CompanionRequest::PairingRequest { .. } => CompanionResponse::PairingPending,
        CompanionRequest::InvitationClaim { invitation, device_name } => {
            let Some(manager) = link_manager else {
                return CompanionResponse::Error { code: CompanionErrorCode::Disabled };
            };
            match manager.claim(&invitation, remote, &device_name, unix_now_ms()) {
                Ok(code) => CompanionResponse::PairingCode { code },
                Err(CompanionLinkError::Expired | CompanionLinkError::InvalidInvitation) => {
                    CompanionResponse::Error { code: CompanionErrorCode::InvalidInvitation }
                }
                Err(CompanionLinkError::Rejected) => CompanionResponse::Error {
                    code: CompanionErrorCode::Rejected,
                },
                Err(CompanionLinkError::Disabled) => CompanionResponse::Error {
                    code: CompanionErrorCode::Disabled,
                },
                Err(_) => CompanionResponse::Error {
                    code: CompanionErrorCode::ApprovalPending,
                },
            }
        }
        CompanionRequest::AuthenticatedRequest {
            request_id,
            registration_id,
            device_id,
            grant_revision,
            capability,
            method,
            ..
        } => {
            if !policy
                .authorize(remote, &registration_id, &device_id, grant_revision)
                .await
            {
                if policy.is_approved(remote).await {
                    return CompanionResponse::Error {
                        code: CompanionErrorCode::StaleGrant,
                    };
                }
                return CompanionResponse::ApprovalRequired;
            }
            if capability != method {
                return CompanionResponse::Error {
                    code: CompanionErrorCode::UnknownMethod,
                };
            }
            let _ = (request_id, method);
            CompanionResponse::Error {
                code: CompanionErrorCode::UnknownMethod,
            }
        }
    }
}

#[derive(Debug)]
enum FrameError {
    TooLarge,
    Malformed,
    Io,
}

async fn read_frame(recv: &mut RecvStream) -> Result<CompanionRequest, FrameError> {
    let length = recv.read_u32().await.map_err(|_| FrameError::Io)? as usize;
    if length > MAX_COMPANION_FRAME_BYTES {
        return Err(FrameError::TooLarge);
    }
    let mut bytes = vec![0; length];
    recv.read_exact(&mut bytes)
        .await
        .map_err(|_| FrameError::Io)?;
    serde_json::from_slice(&bytes).map_err(|_| FrameError::Malformed)
}

async fn write_frame(send: &mut SendStream, value: &CompanionResponse) -> Result<(), FrameError> {
    let bytes = serde_json::to_vec(value).map_err(|_| FrameError::Malformed)?;
    if bytes.len() > MAX_COMPANION_FRAME_BYTES {
        return Err(FrameError::TooLarge);
    }
    send.write_u32(bytes.len() as u32)
        .await
        .map_err(|_| FrameError::Io)?;
    send.write_all(&bytes).await.map_err(|_| FrameError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::{endpoint::presets, protocol::Router, Endpoint};

    async fn endpoints(enabled: bool) -> (Endpoint, Router, CompanionPolicy) {
        let server = Endpoint::bind(presets::Minimal).await.unwrap();
        let policy = CompanionPolicy::new();
        let router = Router::builder(server.clone())
            .accept(
                COMPANION_ALPN,
                CompanionProtocolHandler::new(policy.clone(), enabled),
            )
            .spawn();
        (server, router, policy)
    }

    async fn round_trip(
        client: &Endpoint,
        router: &Router,
        request: &CompanionRequest,
    ) -> CompanionResponse {
        let conn = client
            .connect(router.endpoint().addr(), COMPANION_ALPN)
            .await
            .unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let bytes = serde_json::to_vec(request).unwrap();
        send.write_u32(bytes.len() as u32).await.unwrap();
        send.write_all(&bytes).await.unwrap();
        let len = recv.read_u32().await.unwrap() as usize;
        let mut response = vec![0; len];
        recv.read_exact(&mut response).await.unwrap();
        serde_json::from_slice(&response).unwrap()
    }

    #[tokio::test]
    async fn v1_negotiation_and_pairing_are_data_free() {
        let (server, router, _) = endpoints(true).await;
        let client = Endpoint::bind(presets::Minimal).await.unwrap();
        let response = round_trip(
            &client,
            &router,
            &CompanionRequest::Hello {
                versions: vec![1],
                capabilities: vec!["pairing".into()],
            },
        )
        .await;
        assert!(matches!(
            response,
            CompanionResponse::HelloAccepted { version: 1, .. }
        ));
        let response = round_trip(
            &client,
            &router,
            &CompanionRequest::PairingRequest {
                device_name: "phone".into(),
            },
        )
        .await;
        assert_eq!(response, CompanionResponse::PairingPending);
        client.close().await;
        server.close().await;
    }

    #[tokio::test]
    async fn incompatible_unknown_and_disabled_requests_are_rejected() {
        let (server, router, _) = endpoints(true).await;
        let client = Endpoint::bind(presets::Minimal).await.unwrap();
        assert_eq!(
            round_trip(
                &client,
                &router,
                &CompanionRequest::Hello {
                    versions: vec![99],
                    capabilities: vec![]
                }
            )
            .await,
            CompanionResponse::Error {
                code: CompanionErrorCode::IncompatibleVersion
            }
        );
        assert_eq!(
            round_trip(
                &client,
                &router,
                &CompanionRequest::AuthenticatedRequest {
                    request_id: "1".into(),
                    registration_id: vec![],
                    device_id: vec![],
                    grant_revision: 0,
                    capability: "messages.list".into(),
                    method: "messages.list".into(),
                    params: Value::Null
                }
            )
            .await,
            CompanionResponse::ApprovalRequired
        );
        client.close().await;
        server.close().await;

        let (server, router, _) = endpoints(false).await;
        let client = Endpoint::bind(presets::Minimal).await.unwrap();
        assert_eq!(
            round_trip(
                &client,
                &router,
                &CompanionRequest::Hello {
                    versions: vec![1],
                    capabilities: vec![]
                }
            )
            .await,
            CompanionResponse::Error {
                code: CompanionErrorCode::Disabled
            }
        );
        client.close().await;
        server.close().await;
    }

    #[test]
    fn wire_shape_is_stable() {
        let json = serde_json::to_string(&CompanionRequest::Hello {
            versions: vec![1],
            capabilities: vec!["pairing".into()],
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"hello","versions":[1],"capabilities":["pairing"]}"#
        );
    }

    #[test]
    fn invitation_expiry_regeneration_and_first_claim_binding() {
        let manager = CompanionLinkManager::new(None, true);
        let first = manager.create_invitation("host".into(), vec!["relay".into()], 1_000).unwrap();
        let first_wire = first.encode().unwrap();
        let device_a = iroh::SecretKey::generate().public();
        let device_b = iroh::SecretKey::generate().public();
        assert!(matches!(manager.claim(&first_wire, device_a, "trusted-looking", 121_000), Err(CompanionLinkError::Expired)));
        let second = manager.create_invitation("host".into(), vec![], 2_000).unwrap();
        let second_wire = second.encode().unwrap();
        assert!(matches!(manager.claim(&first_wire, device_a, "old", 2_001), Err(CompanionLinkError::InvalidInvitation)));
        let code = manager.claim(&second_wire, device_a, "phone", 2_001).unwrap();
        assert_eq!(code.len(), 6);
        assert_eq!(manager.claim(&second_wire, device_a, "changed label", 2_002).unwrap(), code);
        assert!(matches!(manager.claim(&second_wire, device_b, "phone", 2_002), Err(CompanionLinkError::InvalidInvitation)));
    }

    #[test]
    fn only_local_approval_persists_a_grant_and_retries_are_idempotent() {
        let store = MessageStore::memory().unwrap();
        let manager = CompanionLinkManager::new(Some(store.clone()), true);
        let invitation = manager.create_invitation("host".into(), vec![], 10).unwrap();
        let wire = invitation.encode().unwrap();
        let device = iroh::SecretKey::generate().public();
        manager.claim(&wire, device, "phone", 11).unwrap();
        assert!(store.device_registration(&invitation.invitation_id).unwrap().is_none());
        assert!(manager.approve(invitation.invitation_id, true).unwrap());
        assert!(store.device_registration(&invitation.invitation_id).unwrap().is_some());
        assert!(manager.approve(invitation.invitation_id, true).unwrap());
        assert_eq!(store.device_registration(&invitation.invitation_id).unwrap().unwrap().grant_revision, 1);
    }

    #[test]
    fn rejection_and_restart_leave_no_grant() {
        let store = MessageStore::memory().unwrap();
        let manager = CompanionLinkManager::new(Some(store.clone()), true);
        let invitation = manager.create_invitation("host".into(), vec![], 10).unwrap();
        let wire = invitation.encode().unwrap();
        let device = iroh::SecretKey::generate().public();
        manager.claim(&wire, device, "phone", 11).unwrap();
        assert!(!manager.approve(invitation.invitation_id, false).unwrap());
        assert!(store.device_registration(&invitation.invitation_id).unwrap().is_none());
        assert!(matches!(manager.claim(&wire, device, "phone", 12), Err(CompanionLinkError::Rejected)));
        manager.restart();
        assert!(matches!(manager.approve(invitation.invitation_id, true), Err(CompanionLinkError::UnknownClaim)));
    }
}
