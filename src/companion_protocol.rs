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
/// Maximum records returned in one query page.
pub const MAX_COMPANION_RECORDS: usize = 100;
/// Maximum encoded bytes returned in one history page.
pub const MAX_COMPANION_BYTES: usize = 256 * 1024;

/// V1 grant scope for conversation reads.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CompanionGrantScope {
    Accessible,
    Conversations { ids: Vec<[u8; 32]> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct ConversationView {
    pub id: String,
    pub last_activity_at_ms: u64,
    pub last_message_preview: String,
    pub unread_count: u32,
    pub muted: bool,
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct MessageView {
    pub id: String,
    pub conversation_id: String,
    pub sender_id: String,
    pub timestamp_ms: u64,
    pub kind: String,
    pub body: String,
    pub delivery_state: String,
    pub read_state: String,
}

/// Capabilities exposed before approval. These are protocol names only.
pub const PUBLIC_CAPABILITIES: &[&str] = &["pairing"];

/// A companion request frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
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
    /// Begin QR pairing using the disposable probe vocabulary.
    PairBegin {
        /// Opaque encoded invitation.
        invitation: String,
        /// User-facing device label.
        device_name: String,
    },
    /// Poll local approval state for a pairing claim.
    PairStatus {
        /// Invitation identifier being polled.
        invitation_id: Vec<u8>,
    },
    /// Authenticated host health query used by probes and mobile clients.
    HostStatus {
        /// Request correlation identifier.
        request_id: String,
        /// Durable registration identifier.
        registration_id: Vec<u8>,
        /// Endpoint identity bound to the registration.
        device_id: Vec<u8>,
        /// Current grant revision.
        grant_revision: i64,
    },
    /// Capture an authorization-bound durable snapshot watermark.
    SnapshotBegin {
        request_id: String,
        registration_id: Vec<u8>,
        device_id: Vec<u8>,
        grant_revision: i64,
    },
    /// Resume body-free change references from a durable cursor.
    ChangesResume {
        request_id: String,
        registration_id: Vec<u8>,
        device_id: Vec<u8>,
        grant_revision: i64,
        token: String,
        after_sequence: i64,
        limit: usize,
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
#[allow(missing_docs)]
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
    /// Pairing claim accepted and awaiting local approval.
    PairStarted {
        /// Invitation identifier.
        invitation_id: Vec<u8>,
        /// Transcript comparison code.
        code: String,
    },
    /// Current local approval state.
    PairStatus {
        /// Pending, approved, rejected, or unknown.
        status: String,
    },
    /// Authenticated host status response.
    HostStatus {
        /// Request correlation identifier.
        request_id: String,
        /// Host health state.
        status: String,
        /// Grant revision accepted by the host.
        grant_revision: i64,
    },
    /// Snapshot token and its immutable high-water mark.
    Snapshot {
        request_id: String,
        token: String,
        watermark: i64,
    },
    /// Body-free durable change references.
    Changes {
        request_id: String,
        changes: Vec<Value>,
        next_sequence: i64,
        end: bool,
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
    /// The operation id was reused with different semantic fields.
    OperationIdConflict,
    /// The request payload failed validation.
    InvalidRequest,
    /// No result is retained for this operation id.
    OperationNotFound,
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
        let invitation: Self =
            serde_json::from_slice(&bytes).map_err(|_| CompanionLinkError::InvalidInvitation)?;
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
        let active = state
            .active
            .as_ref()
            .ok_or(CompanionLinkError::InvalidInvitation)?;
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
    pub fn approve(
        &self,
        invitation_id: [u8; 16],
        approve: bool,
    ) -> Result<bool, CompanionLinkError> {
        let mut state = self.state.lock().unwrap();
        let claim = state
            .claims
            .get_mut(&invitation_id)
            .ok_or(CompanionLinkError::UnknownClaim)?;
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

    /// Return pairing state without exposing invitation or device secrets.
    pub fn claim_status(&self, invitation_id: &[u8]) -> &'static str {
        if invitation_id.len() != 16 {
            return "unknown";
        }
        let mut id = [0; 16];
        id.copy_from_slice(invitation_id);
        let state = self.state.lock().unwrap();
        match state.claims.get(&id) {
            Some(claim) if claim.rejected => "rejected",
            Some(claim) if claim.approved => "approved",
            Some(_) => "pending",
            None => "unknown",
        }
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
        Self {
            policy,
            enabled,
            link_manager: None,
        }
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
                    Ok(Ok(request)) => {
                        handle_request(request, remote, &policy, link_manager.as_ref()).await
                    }
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
        CompanionRequest::PairBegin {
            invitation,
            device_name,
        } => {
            let Some(manager) = link_manager else {
                return CompanionResponse::Error {
                    code: CompanionErrorCode::Disabled,
                };
            };
            match manager.claim(&invitation, remote, &device_name, unix_now_ms()) {
                Ok(code) => {
                    let invitation_id = CompanionInvitation::decode(&invitation)
                        .map(|value| value.invitation_id.to_vec())
                        .unwrap_or_default();
                    CompanionResponse::PairStarted {
                        invitation_id,
                        code,
                    }
                }
                Err(CompanionLinkError::Rejected) => CompanionResponse::Error {
                    code: CompanionErrorCode::Rejected,
                },
                Err(CompanionLinkError::Disabled) => CompanionResponse::Error {
                    code: CompanionErrorCode::Disabled,
                },
                Err(_) => CompanionResponse::Error {
                    code: CompanionErrorCode::InvalidInvitation,
                },
            }
        }
        CompanionRequest::PairStatus { invitation_id } => {
            let Some(manager) = link_manager else {
                return CompanionResponse::Error {
                    code: CompanionErrorCode::Disabled,
                };
            };
            CompanionResponse::PairStatus {
                status: manager.claim_status(&invitation_id).to_owned(),
            }
        }
        CompanionRequest::HostStatus {
            request_id,
            registration_id,
            device_id,
            grant_revision,
        } => {
            if !policy
                .authorize(remote, &registration_id, &device_id, grant_revision)
                .await
            {
                return CompanionResponse::ApprovalRequired;
            }
            CompanionResponse::HostStatus {
                request_id,
                status: "ok".into(),
                grant_revision,
            }
        }
        CompanionRequest::SnapshotBegin {
            request_id,
            registration_id,
            device_id,
            grant_revision,
        } => {
            if !policy.authorize(remote, &registration_id, &device_id, grant_revision).await {
                return CompanionResponse::ApprovalRequired;
            }
            let Some(store) = policy.store.as_ref() else {
                return CompanionResponse::Error { code: CompanionErrorCode::UnknownMethod };
            };
            match store.begin_companion_snapshot(&registration_id, &device_id, grant_revision, unix_now_ms()) {
                Ok(token) => CompanionResponse::Result {
                    request_id,
                    value: serde_json::json!({"token": token}),
                },
                Err(_) => CompanionResponse::Error { code: CompanionErrorCode::StaleGrant },
            }
        }
        CompanionRequest::ChangesResume {
            request_id,
            registration_id,
            device_id,
            grant_revision,
            token,
            after_sequence,
            limit,
        } => {
            if !policy.authorize(remote, &registration_id, &device_id, grant_revision).await {
                return CompanionResponse::ApprovalRequired;
            }
            let Some(store) = policy.store.as_ref() else {
                return CompanionResponse::Error { code: CompanionErrorCode::UnknownMethod };
            };
            match store.resume_companion_changes(&token, after_sequence, limit, unix_now_ms()) {
                Ok(page) => CompanionResponse::Result {
                    request_id,
                    value: serde_json::json!({"changes": page.changes.into_iter().map(|c| serde_json::json!({"sequence":c.sequence,"change_id":hex::encode(c.change_id),"entity_id":hex::encode(c.entity_id),"entity_revision":c.entity_revision,"kind":c.kind,"tombstone":c.tombstone})).collect::<Vec<_>>(),"next_sequence":page.next_sequence,"end":page.end}),
                },
                Err(_) => CompanionResponse::Error { code: CompanionErrorCode::StaleGrant },
            }
        }
        CompanionRequest::InvitationClaim {
            invitation,
            device_name,
        } => {
            let Some(manager) = link_manager else {
                return CompanionResponse::Error {
                    code: CompanionErrorCode::Disabled,
                };
            };
            match manager.claim(&invitation, remote, &device_name, unix_now_ms()) {
                Ok(code) => CompanionResponse::PairingCode { code },
                Err(CompanionLinkError::Expired | CompanionLinkError::InvalidInvitation) => {
                    CompanionResponse::Error {
                        code: CompanionErrorCode::InvalidInvitation,
                    }
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
            params,
        } => {
            if request_id.is_empty() || request_id.len() > 256 {
                return CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest };
            }
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
            let Some(store) = policy.store.as_ref() else {
                return CompanionResponse::Error {
                    code: CompanionErrorCode::UnknownMethod,
                };
            };
            let allowed_ids = store
                .companion_scope(&registration_id, &device_id, grant_revision)
                .ok()
                .flatten()
                .and_then(|scope| parse_scope(&scope));
            match method.as_str() {
                "operations.get" => {
                    let Some(operation_id) = params
                        .get("operation_id")
                        .and_then(Value::as_str)
                        .and_then(decode_operation_id)
                    else {
                        return CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest };
                    };
                    match store.authorized_operation_result(
                        &registration_id, &device_id, grant_revision, &operation_id,
                    ) {
                        Ok(Some(cached)) => CompanionResponse::Result {
                            request_id,
                            value: serde_json::json!({
                                "operation_id": hex::encode(operation_id),
                                "result": serde_json::from_slice::<Value>(&cached.result).unwrap_or(Value::Null),
                            }),
                        },
                        Ok(None) => CompanionResponse::Error { code: CompanionErrorCode::OperationNotFound },
                        Err(_) => CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest },
                    }
                }
                "messages.send" => {
                    let Some(operation_id) = params
                        .get("operation_id").and_then(Value::as_str).and_then(decode_operation_id)
                    else { return CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest }; };
                    let Some(conversation_id) = params
                        .get("conversation_id").and_then(Value::as_str).and_then(decode_id)
                    else { return CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest }; };
                    let Some(text) = params.get("text").and_then(Value::as_str) else {
                        return CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest };
                    };
                    let text = text.trim();
                    if text.is_empty() || text.len() > 16 * 1024 || !text.is_char_boundary(text.len()) {
                        return CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest };
                    }
                    if allowed_ids.as_ref().is_some_and(|ids| !ids.contains(&conversation_id)) {
                        return CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest };
                    }
                    let sender: [u8; 32] = match device_id.as_slice().try_into() {
                        Ok(sender) => sender,
                        Err(_) => return CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest },
                    };
                    let recipient = params.get("recipient_device_id")
                        .and_then(Value::as_str).and_then(decode_public_key);
                    if params.get("recipient_device_id").is_some() && recipient.is_none() {
                        return CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest };
                    }
                    let digest = semantic_send_digest(&conversation_id, text, recipient.as_ref());
                    match store.authorized_operation_result(
                        &registration_id, &device_id, grant_revision, &operation_id,
                    ) {
                        Ok(Some(cached)) => {
                            if cached.request_digest != digest {
                                return CompanionResponse::Error { code: CompanionErrorCode::OperationIdConflict };
                            }
                            return CompanionResponse::Result {
                                request_id,
                                value: serde_json::from_slice(&cached.result).unwrap_or(Value::Null),
                            };
                        }
                        Ok(None) => {}
                        Err(_) => return CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest },
                    }
                    let mut msg_hasher = blake3::Hasher::new_derive_key("boru companion text message v1");
                    msg_hasher.update(&conversation_id);
                    msg_hasher.update(sender.as_ref());
                    msg_hasher.update(text.as_bytes());
                    let msg_hash = *msg_hasher.finalize().as_bytes();
                    let mut change_hasher = blake3::Hasher::new_derive_key("boru companion change v1");
                    change_hasher.update(&registration_id);
                    change_hasher.update(&operation_id);
                    let change_id = change_hasher.finalize();
                    let timestamp_ms = unix_now_ms();
                    let result = serde_json::json!({
                        "operation_id": hex::encode(&operation_id),
                        "message_id": hex::encode(msg_hash),
                        "status": if recipient.is_some() { "awaiting_recipient" } else { "host_accepted" },
                    });
                    let result_bytes = serde_json::to_vec(&result).unwrap_or_default();
                    match store.commit_companion_mutation(
                        &registration_id, &device_id, grant_revision, &operation_id, &digest,
                        &result_bytes, change_id.as_bytes(), &msg_hash, &conversation_id,
                        &sender, timestamp_ms, text, recipient,
                    ) {
                        Ok(_) => CompanionResponse::Result { request_id, value: result },
                        Err(error) if error.to_string().contains("operation id reused") =>
                            CompanionResponse::Error { code: CompanionErrorCode::OperationIdConflict },
                        Err(_) => CompanionResponse::Error { code: CompanionErrorCode::StaleGrant },
                    }
                }
                "conversations.list" => {
                    let limit = params
                        .get("limit")
                        .and_then(Value::as_u64)
                        .unwrap_or(50)
                        .min(MAX_COMPANION_RECORDS as u64) as usize;
                    let (snapshot, after) = params
                        .get("after")
                        .and_then(Value::as_str)
                        .and_then(decode_conversation_cursor)
                        .map(|(snapshot, after)| (Some(snapshot), Some(after)))
                        .unwrap_or_else(|| (store.conversation_snapshot().ok(), None));
                    match store.list_conversation_meta(after, snapshot, limit.saturating_add(1)) {
                        Ok(rows) => {
                            let rows: Vec<_> = rows
                                .into_iter()
                                .filter(|row| {
                                    allowed_ids
                                        .as_ref()
                                        .map_or(true, |ids| ids.contains(&row.conversation_id))
                                })
                                .collect();
                            let mut items = Vec::new();
                            let mut truncated = false;
                            for row in rows.iter().take(limit) {
                                let candidate = ConversationView {
                                    id: hex::encode(row.conversation_id),
                                    last_activity_at_ms: row.last_activity_at_ms,
                                    last_message_preview: row.last_message_preview.clone(),
                                    unread_count: row.unread_count,
                                    muted: row.is_muted,
                                    archived: row.is_archived,
                                };
                                let mut candidate_items = items.clone();
                                candidate_items.push(candidate);
                                let candidate_response = CompanionResponse::Result {
                                    request_id: request_id.clone(),
                                    value: serde_json::json!({"snapshot":snapshot,"items":candidate_items,"next":null,"end":false}),
                                };
                                if serde_json::to_vec(&candidate_response)
                                    .map_or(true, |v| v.len() > MAX_COMPANION_FRAME_BYTES)
                                {
                                    truncated = true;
                                    break;
                                }
                                items = candidate_items;
                            }
                            let next = items.last().map(|row: &ConversationView| {
                                format!(
                                    "{}:{}:{}",
                                    snapshot.unwrap_or(0),
                                    row.last_activity_at_ms,
                                    row.id
                                )
                            });
                            let has_more = truncated || rows.len() > items.len();
                            CompanionResponse::Result {
                                request_id,
                                value: serde_json::json!({"snapshot":snapshot,"items":items,"next":next,"end":!has_more,"state":if has_more {"loading"} else {"end"}}),
                            }
                        }
                        Err(_) => CompanionResponse::Error {
                            code: CompanionErrorCode::UnknownMethod,
                        },
                    }
                }
                "messages.mark_read" => {
                    let Some(conversation_id) = params
                        .get("conversation_id")
                        .and_then(Value::as_str)
                        .and_then(decode_id)
                    else {
                        return CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest };
                    };
                    let Some(message_id) = params
                        .get("through_message_id")
                        .and_then(Value::as_str)
                        .and_then(decode_id)
                    else {
                        return CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest };
                    };
                    let Some(timestamp_ms) = params.get("through_timestamp_ms").and_then(Value::as_u64) else {
                        return CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest };
                    };
                    if allowed_ids.as_ref().is_some_and(|ids| !ids.contains(&conversation_id)) {
                        return CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest };
                    }
                    match store.mark_message_read(
                        &conversation_id,
                        &device_id,
                        timestamp_ms,
                        &message_id,
                    ) {
                        Ok(changed) => {
                            if changed {
                                let mut hasher = blake3::Hasher::new_derive_key("boru companion read change v1");
                                hasher.update(request_id.as_bytes());
                                hasher.update(&conversation_id);
                                hasher.update(&message_id);
                                let change_id = hasher.finalize();
                                let _ = store.record_companion_read_change(
                                    &registration_id,
                                    request_id.as_bytes(),
                                    change_id.as_bytes(),
                                    &conversation_id,
                                );
                            }
                            CompanionResponse::Result {
                                request_id,
                                value: serde_json::json!({
                                    "status": "seen",
                                    "changed": changed,
                                    "conversation_id": hex::encode(conversation_id),
                                    "through_message_id": hex::encode(message_id),
                                    "through_timestamp_ms": timestamp_ms,
                                }),
                            }
                        }
                        Err(_) => CompanionResponse::Error { code: CompanionErrorCode::InvalidRequest },
                    }
                }
                "messages.get" => {
                    let Some(id) = params
                        .get("conversation_id")
                        .and_then(Value::as_str)
                        .and_then(decode_id)
                    else {
                        return CompanionResponse::Error {
                            code: CompanionErrorCode::UnknownMethod,
                        };
                    };
                    let (snapshot, after) = params
                        .get("after")
                        .and_then(Value::as_str)
                        .and_then(decode_message_cursor)
                        .map(|(snapshot, ts, id)| (snapshot, Some((ts, id))))
                        .unwrap_or_else(|| {
                            (store.message_history_snapshot(&id).unwrap_or(0), None)
                        });
                    let limit = params
                        .get("limit")
                        .and_then(Value::as_u64)
                        .unwrap_or(50)
                        .min(MAX_COMPANION_RECORDS as u64) as usize;
                    let max_bytes = params
                        .get("max_bytes")
                        .and_then(Value::as_u64)
                        .unwrap_or(MAX_COMPANION_BYTES as u64)
                        .min(MAX_COMPANION_BYTES as u64)
                        as usize;
                    if allowed_ids.as_ref().is_some_and(|ids| !ids.contains(&id)) {
                        return CompanionResponse::Result {
                            request_id,
                            value: serde_json::json!({"snapshot":"local","state":"end","items":[],"next":null,"end":true}),
                        };
                    }
                    match store.get_messages_keyset(&id, after, snapshot, limit.saturating_add(1)) {
                        Ok(rows) => {
                            let mut items = Vec::new();
                            let mut truncated = false;
                            let read_marker = store
                                .conversation_read_marker(&id, &device_id)
                                .ok()
                                .flatten();
                            for row in rows.iter().take(limit) {
                                let read_state = read_marker
                                    .as_ref()
                                    .is_some_and(|(timestamp, message_id)| {
                                        (row.timestamp_ms.max(0) as u64, row.msg_hash)
                                            <= (*timestamp, *message_id)
                                    });
                                let candidate = MessageView {
                                    id: hex::encode(row.msg_hash),
                                    conversation_id: hex::encode(row.topic),
                                    sender_id: hex::encode(row.sender),
                                    timestamp_ms: row.timestamp_ms.max(0) as u64,
                                    kind: row.kind.clone(),
                                    body: row.body.clone(),
                                    delivery_state: row.delivery_state.clone(),
                                    read_state: if read_state { "read".into() } else { "unread".into() },
                                };
                                let mut candidate_items = items.clone();
                                candidate_items.push(candidate);
                                let candidate_response = CompanionResponse::Result {
                                    request_id: request_id.clone(),
                                    value: serde_json::json!({"snapshot":snapshot,"state":"loading","items":candidate_items,"next":null,"end":false}),
                                };
                                let encoded_len = serde_json::to_vec(&candidate_response)
                                    .map_or(usize::MAX, |v| v.len());
                                if encoded_len > max_bytes.min(MAX_COMPANION_FRAME_BYTES) {
                                    truncated = true;
                                    break;
                                }
                                items = candidate_items;
                            }
                            let next = items.last().and_then(|_| {
                                rows.get(items.len().saturating_sub(1)).map(|row| {
                                    format!("{}:{}:{}", snapshot, row.timestamp_ms, row.id)
                                })
                            });
                            let has_more = truncated || rows.len() > items.len();
                            let state = if items.is_empty() && truncated {
                                "unavailable"
                            } else if has_more {
                                "loading"
                            } else {
                                "end"
                            };
                            CompanionResponse::Result {
                                request_id,
                                value: serde_json::json!({"snapshot":snapshot,"state":state,"items":items,"next":next,"end":!has_more}),
                            }
                        }
                        Err(_) => CompanionResponse::Error {
                            code: CompanionErrorCode::UnknownMethod,
                        },
                    }
                }
                _ => CompanionResponse::Error {
                    code: CompanionErrorCode::UnknownMethod,
                },
            }
        }
    }
}

fn decode_id(value: &str) -> Option<[u8; 32]> {
    hex::decode(value).ok()?.try_into().ok()
}

fn decode_operation_id(value: &str) -> Option<Vec<u8>> {
    let bytes = hex::decode(value).ok()?;
    (bytes.len() == 32).then_some(bytes)
}

fn decode_public_key(value: &str) -> Option<iroh::PublicKey> {
    let bytes = hex::decode(value).ok()?;
    iroh::PublicKey::try_from(bytes.as_slice()).ok()
}

fn semantic_send_digest(
    conversation_id: &[u8; 32],
    text: &str,
    recipient: Option<&iroh::PublicKey>,
) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new_derive_key("boru companion send request v1");
    hasher.update(conversation_id);
    hasher.update(&(text.len() as u64).to_be_bytes());
    hasher.update(text.as_bytes());
    match recipient {
        Some(recipient) => {
            hasher.update(&[1]);
            hasher.update(recipient.as_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
    hasher.finalize().as_bytes().to_vec()
}

fn parse_scope(scope: &str) -> Option<Vec<[u8; 32]>> {
    if scope == "accessible" {
        return None;
    }
    let ids: Vec<String> = serde_json::from_str(scope).ok()?;
    Some(ids.into_iter().filter_map(|id| decode_id(&id)).collect())
}

fn decode_conversation_cursor(value: &str) -> Option<(u64, (u64, [u8; 32]))> {
    let mut parts = value.split(':');
    let snapshot = parts.next()?.parse().ok()?;
    let timestamp = parts.next()?.parse().ok()?;
    let id = decode_id(parts.next()?)?;
    Some((snapshot, (timestamp, id)))
}

fn decode_message_cursor(value: &str) -> Option<(i64, i64, i64)> {
    let mut parts = value.split(':');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
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

    #[tokio::test]
    async fn authenticated_history_paginates_and_hides_unauthorized_topics() {
        let store = MessageStore::memory().unwrap();
        let registration = vec![41; 16];
        let device = iroh::SecretKey::generate().public();
        store
            .register_device(&registration, device.as_bytes())
            .unwrap();
        let topic = [51; 32];
        let unauthorized = [52; 32];
        let sender = [53; 32];
        let local = [0; 32];
        store
            .insert_chat_message(
                &[61; 32], &topic, &sender, 10, "text", "one", None, None, &local,
            )
            .unwrap();
        store
            .insert_chat_message(
                &[62; 32], &topic, &sender, 20, "text", "two", None, None, &local,
            )
            .unwrap();
        let policy = CompanionPolicy::new().with_store(store);
        let request = |params| CompanionRequest::AuthenticatedRequest {
            request_id: "history".into(),
            registration_id: registration.clone(),
            device_id: device.as_bytes().to_vec(),
            grant_revision: 1,
            capability: "messages.get".into(),
            method: "messages.get".into(),
            params,
        };
        let first = handle_request(
            request(serde_json::json!({
                "conversation_id": hex::encode(topic), "limit": 1
            })),
            device,
            &policy,
            None,
        )
        .await;
        let CompanionResponse::Result { value, .. } = first else {
            panic!("history result")
        };
        assert_eq!(value["state"], "loading");
        assert_eq!(value["items"].as_array().unwrap().len(), 1);
        let cursor = value["next"].as_str().unwrap().to_owned();
        let second = handle_request(
            request(serde_json::json!({
                "conversation_id": hex::encode(topic), "limit": 1, "after": cursor
            })),
            device,
            &policy,
            None,
        )
        .await;
        let CompanionResponse::Result { value, .. } = second else {
            panic!("history result")
        };
        assert_eq!(value["state"], "end");
        let hidden = handle_request(
            request(serde_json::json!({ "conversation_id": hex::encode(unauthorized) })),
            device,
            &policy,
            None,
        )
        .await;
        let CompanionResponse::Result { value, .. } = hidden else {
            panic!("hidden result")
        };
        assert!(value["items"].as_array().unwrap().is_empty());
        assert_eq!(value["end"], true);
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
        let first = manager
            .create_invitation("host".into(), vec!["relay".into()], 1_000)
            .unwrap();
        let first_wire = first.encode().unwrap();
        let device_a = iroh::SecretKey::generate().public();
        let device_b = iroh::SecretKey::generate().public();
        assert!(matches!(
            manager.claim(&first_wire, device_a, "trusted-looking", 121_000),
            Err(CompanionLinkError::Expired)
        ));
        let second = manager
            .create_invitation("host".into(), vec![], 2_000)
            .unwrap();
        let second_wire = second.encode().unwrap();
        assert!(matches!(
            manager.claim(&first_wire, device_a, "old", 2_001),
            Err(CompanionLinkError::InvalidInvitation)
        ));
        let code = manager
            .claim(&second_wire, device_a, "phone", 2_001)
            .unwrap();
        assert_eq!(code.len(), 6);
        assert_eq!(
            manager
                .claim(&second_wire, device_a, "changed label", 2_002)
                .unwrap(),
            code
        );
        assert!(matches!(
            manager.claim(&second_wire, device_b, "phone", 2_002),
            Err(CompanionLinkError::InvalidInvitation)
        ));
    }

    #[test]
    fn only_local_approval_persists_a_grant_and_retries_are_idempotent() {
        let store = MessageStore::memory().unwrap();
        let manager = CompanionLinkManager::new(Some(store.clone()), true);
        let invitation = manager
            .create_invitation("host".into(), vec![], 10)
            .unwrap();
        let wire = invitation.encode().unwrap();
        let device = iroh::SecretKey::generate().public();
        manager.claim(&wire, device, "phone", 11).unwrap();
        assert!(store
            .device_registration(&invitation.invitation_id)
            .unwrap()
            .is_none());
        assert!(manager.approve(invitation.invitation_id, true).unwrap());
        assert!(store
            .device_registration(&invitation.invitation_id)
            .unwrap()
            .is_some());
        assert!(manager.approve(invitation.invitation_id, true).unwrap());
        assert_eq!(
            store
                .device_registration(&invitation.invitation_id)
                .unwrap()
                .unwrap()
                .grant_revision,
            1
        );
    }

    #[test]
    fn rejection_and_restart_leave_no_grant() {
        let store = MessageStore::memory().unwrap();
        let manager = CompanionLinkManager::new(Some(store.clone()), true);
        let invitation = manager
            .create_invitation("host".into(), vec![], 10)
            .unwrap();
        let wire = invitation.encode().unwrap();
        let device = iroh::SecretKey::generate().public();
        manager.claim(&wire, device, "phone", 11).unwrap();
        assert!(!manager.approve(invitation.invitation_id, false).unwrap());
        assert!(store
            .device_registration(&invitation.invitation_id)
            .unwrap()
            .is_none());
        assert!(matches!(
            manager.claim(&wire, device, "phone", 12),
            Err(CompanionLinkError::Rejected)
        ));
        manager.restart();
        assert!(matches!(
            manager.approve(invitation.invitation_id, true),
            Err(CompanionLinkError::UnknownClaim)
        ));
    }
}
