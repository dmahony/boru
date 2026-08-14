//! Screen-share invitation and session state machine.
#![allow(missing_docs)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::protocol::{ControlMessage, Hello, Permission, SCREEN_SHARE_PROTOCOL_VERSION};
use super::protocol::ScreenShareMessage;
use super::permissions::{Capability, SessionPermissions};

/// Opaque identifier for one negotiation, independent of a conversation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScreenShareSessionId([u8; 16]);

impl ScreenShareSessionId {
    /// Generate a fresh identifier using the OS CSPRNG.
    pub fn generate() -> Self { let mut bytes = [0; 16]; getrandom::fill(&mut bytes).expect("OS CSPRNG unavailable"); Self(bytes) }
    /// Construct the all-zero identifier, useful only as a test sentinel.
    pub const fn zero() -> Self { Self([0; 16]) }
    /// Construct an identifier from raw wire bytes.
    pub const fn from_bytes(bytes: [u8; 16]) -> Self { Self(bytes) }
    /// Return the wire representation.
    pub const fn as_bytes(&self) -> &[u8; 16] { &self.0 }
}
impl std::fmt::Debug for ScreenShareSessionId { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_tuple("ScreenShareSessionId").field(&hex::encode(self.0)).finish() } }

/// Lifecycle states. Streaming is only reachable after explicit Accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState { Idle, Inviting, AwaitingAcceptance, Connecting, Streaming, Paused, Ending, Ended, Failed }

/// Events exposed to the conversation/UI layer. They contain no media data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// A recipient-visible invitation that requires an explicit action.
    Invitation { session_id: ScreenShareSessionId, host_id: iroh::PublicKey, conversation_id: u64, hello: Hello },
    /// Versioned negotiation invitation (PDF Task 3.1): carries the full offer
    /// so the UI can present codecs/resolutions/fps/remote-control before the
    /// recipient decides. The recipient must accept before capture begins.
    NegotiationInvitation { session_id: ScreenShareSessionId, host_id: iroh::PublicKey, conversation_id: u64, offer: ScreenShareMessage },
    /// A session entered streaming after consent.
    Accepted { session_id: ScreenShareSessionId, peer_id: iroh::PublicKey },
    /// A peer declined or the protocol rejected the session.
    Rejected { session_id: ScreenShareSessionId, reason: String },
    /// A session ended or its connection disappeared.
    Ended { session_id: ScreenShareSessionId },
    /// Viewer requested explicit control capabilities; host UI must decide.
    ControlRequest { session_id: ScreenShareSessionId, peer_id: iroh::PublicKey, capabilities: Vec<Capability> },
    /// Control became active or was revoked while viewing continues.
    ControlChanged { session_id: ScreenShareSessionId, active: bool, capabilities: Vec<Capability> },
}

#[derive(Debug, Clone)]
struct Record { state: SessionState, host_id: iroh::PublicKey, peer_id: Option<iroh::PublicKey>, conversation_id: u64 }

/// Bounded in-memory state for active sessions.
#[derive(Debug, Default)]
pub struct SessionManager { sessions: HashMap<ScreenShareSessionId, Record>, permissions: HashMap<ScreenShareSessionId, SessionPermissions> }

pub const MAX_ACTIVE_SESSIONS: usize = 8;

impl SessionManager {
    /// Start a local invitation. The caller must send the corresponding Hello.
    /// `peer` is the invitee (the node the Hello will be dialed to); it is
    /// recorded so the eventual remote Accept can be attributed to the invitee.
    pub fn start_invitation(&mut self, id: ScreenShareSessionId, host_id: iroh::PublicKey, peer: iroh::PublicKey, conversation_id: u64) {
        if id == ScreenShareSessionId::zero() || self.sessions.len() >= MAX_ACTIVE_SESSIONS { return; }
        self.sessions.insert(id, Record { state: SessionState::AwaitingAcceptance, host_id, peer_id: Some(peer), conversation_id });
    }
    /// Return a session state, if the session is known.
    pub fn state(&self, id: ScreenShareSessionId) -> Option<SessionState> { self.sessions.get(&id).map(|record| record.state) }
    /// Return the permission record for a session, if known.
    pub fn permissions(&self, id: ScreenShareSessionId) -> Option<&SessionPermissions> { self.permissions.get(&id) }
    /// Build a default, view-only Hello for a locally initiated session.
    pub fn hello(&self, id: ScreenShareSessionId, codecs: Vec<String>, width: u16, height: u16, frame_rate: u16) -> Option<Hello> {
        let record = self.sessions.get(&id)?;
        Some(Hello { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: id, host_id: record.host_id, conversation_id: record.conversation_id, codecs, width, height, frame_rate, permission: Permission::ViewOnly })
    }
    /// Explicitly accept a pending invitation. This is the only transition to Streaming.
    pub fn accept_invitation(&mut self, id: ScreenShareSessionId, host_id: iroh::PublicKey) -> Option<ControlMessage> {
        let record = self.sessions.get_mut(&id)?;
        if record.host_id != host_id || !matches!(record.state, SessionState::Connecting | SessionState::AwaitingAcceptance) { return None; }
        record.peer_id = Some(host_id); record.state = SessionState::Streaming;
        Some(ControlMessage::Accept { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: id })
    }
    /// Explicitly decline an invitation and remove all state/resources.
    pub fn reject_invitation(&mut self, id: ScreenShareSessionId, reason: impl Into<String>) -> Option<ControlMessage> {
        if self.sessions.remove(&id).is_some() { Some(ControlMessage::Reject { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: id, reason: reason.into() }) } else { None }
    }
    /// End a session idempotently; unknown/already-ended sessions produce no wire message.
    pub fn end(&mut self, id: ScreenShareSessionId) -> Option<ControlMessage> {
        let record = self.sessions.get_mut(&id)?;
        if record.state == SessionState::Ended { return None; }
        record.state = SessionState::Ended;
        Some(ControlMessage::EndSession { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: id })
    }
    /// Host-side grant of control capabilities. Generates the fresh nonce,
    /// emits a local `ControlChanged` so the host UI shows the indicator, and
    /// returns the wire GrantControl message to send to the viewer.
    pub fn grant_control(&mut self, id: ScreenShareSessionId, capabilities: Vec<Capability>, events: &tokio::sync::mpsc::Sender<SessionEvent>) -> Option<ControlMessage> {
        let permissions = self.permissions.get_mut(&id)?;
        if !permissions.grant(capabilities.clone()) { return None; }
        let nonce = *permissions.token()?.nonce();
        let _ = events.try_send(SessionEvent::ControlChanged { session_id: id, active: true, capabilities: capabilities.clone() });
        Some(ControlMessage::GrantControl { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: id, capabilities, nonce })
    }
    /// Host-side revocation of control. Emits a local `ControlChanged` and
    /// returns the wire RevokeControl message to send to the viewer.
    pub fn revoke_control(&mut self, id: ScreenShareSessionId, events: &tokio::sync::mpsc::Sender<SessionEvent>) -> Option<ControlMessage> {
        let permissions = self.permissions.get_mut(&id)?;
        permissions.revoke_control();
        let _ = events.try_send(SessionEvent::ControlChanged { session_id: id, active: false, capabilities: vec![Capability::ViewScreen] });
        Some(ControlMessage::RevokeControl { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: id })
    }
    /// Apply one validated remote control message. Hello never grants consent.
    pub fn apply_remote(&mut self, peer_id: iroh::PublicKey, message: ControlMessage, events: &tokio::sync::mpsc::Sender<SessionEvent>) -> Option<ControlMessage> {
        match message {
            ControlMessage::Hello(hello) => {
                tracing::info!(session = ?hello.session_id, "screen-share: Hello received");
                if hello.session_id == ScreenShareSessionId::zero() || self.sessions.len() >= MAX_ACTIVE_SESSIONS {
                    return Some(ControlMessage::Reject { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: hello.session_id, reason: "session is not available".into() });
                }
                if hello.host_id != peer_id {
                    tracing::warn!(session = ?hello.session_id, "screen-share: Hello host_id does not match connected peer, rejecting");
                    return Some(ControlMessage::Reject {
                        version: SCREEN_SHARE_PROTOCOL_VERSION,
                        session_id: hello.session_id,
                        reason: "invitation identity does not match the connected peer".into(),
                    });
                }
                if hello.permission != Permission::ViewOnly { return Some(ControlMessage::Reject { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: hello.session_id, reason: "unsupported permission".into() }); }
                if self.sessions.contains_key(&hello.session_id) {
                    return Some(ControlMessage::Reject { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: hello.session_id, reason: "session already exists".into() });
                }
                self.sessions.insert(hello.session_id, Record { state: SessionState::Connecting, host_id: hello.host_id, peer_id: Some(peer_id), conversation_id: hello.conversation_id });
                self.permissions.insert(hello.session_id, SessionPermissions::view_only(hello.session_id, peer_id));
                emit_event(events, SessionEvent::Invitation { session_id: hello.session_id, host_id: hello.host_id, conversation_id: hello.conversation_id, hello });
                None
            }
            ControlMessage::Accept { session_id, .. } => {
                // The Accept always comes from the INVITEE (the peer the host
                // dialed), never from the host itself. Validate against the
                // recorded invitee: on the host record.peer_id holds the
                // viewer; on the viewer record.peer_id holds the host (set
                // when the Hello was applied). Checking host_id here would
                // make the host's own session unreachable — the check can
                // never pass because host_id is the host's own key.
                if let Some(record) = self.sessions.get_mut(&session_id) {
                    if record.peer_id == Some(peer_id)
                        && matches!(record.state, SessionState::AwaitingAcceptance | SessionState::Connecting)
                    {
                        record.state = SessionState::Streaming;
                        self.permissions.insert(session_id, SessionPermissions::view_only(session_id, peer_id));
                        tracing::info!(session = ?session_id, "screen-share: session entered Streaming");
                        emit_event(events, SessionEvent::Accepted { session_id, peer_id });
                    } else {
                        tracing::warn!(session = ?session_id, "screen-share: Accept ignored (peer or state mismatch)");
                    }
                }
                None
            }
            ControlMessage::RequestControl { session_id, capabilities, .. } => {
                if self.sessions.get(&session_id).and_then(|r| r.peer_id).is_some_and(|id| id == peer_id) {
                    let _ = events.try_send(SessionEvent::ControlRequest { session_id, peer_id, capabilities });
                }
                None
            }
            ControlMessage::GrantControl { session_id, capabilities, nonce, .. } => {
                // The viewer stores the HOST's nonce so it can echo it back in
                // every Input message; host-side validation uses that nonce.
                if let Some(permissions) = self.permissions.get_mut(&session_id) { if permissions.peer_id() == peer_id { permissions.grant_with_nonce(capabilities.clone(), nonce); let _ = events.try_send(SessionEvent::ControlChanged { session_id, active: true, capabilities }); } }
                None
            }
            ControlMessage::RevokeControl { session_id, .. } => {
                if let Some(permissions) = self.permissions.get_mut(&session_id) { if permissions.peer_id() == peer_id { permissions.revoke_control(); let _ = events.try_send(SessionEvent::ControlChanged { session_id, active: false, capabilities: vec![Capability::ViewScreen] }); } }
                None
            }
            ControlMessage::Input { .. } => None,
            ControlMessage::Reject { session_id, reason, .. } => {
                if let Some(record) = self.sessions.get(&session_id) {
                    if record.host_id != peer_id && record.peer_id != Some(peer_id) { return None; }
                }
                if self.sessions.remove(&session_id).is_some() { let _ = events.try_send(SessionEvent::Rejected { session_id, reason }); }
                None
            }
            ControlMessage::EndSession { session_id, .. } => {
                if let Some(record) = self.sessions.get_mut(&session_id) {
                    if record.host_id != peer_id && record.peer_id != Some(peer_id) { return None; }
                    if record.state != SessionState::Ended { record.state = SessionState::Ended; let _ = events.try_send(SessionEvent::Ended { session_id }); }
                }
                None
            }
        }
    }
}

/// Minimal session record retained for compatibility with the subsystem boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenShareSession { id: ScreenShareSessionId, conversation_id: u64 }
impl ScreenShareSession { pub fn new() -> Self { Self { id: ScreenShareSessionId::generate(), conversation_id: 0 } } pub fn for_conversation(conversation_id: u64) -> Self { Self { id: ScreenShareSessionId::generate(), conversation_id } } pub const fn id(&self) -> ScreenShareSessionId { self.id } pub const fn conversation_id(&self) -> u64 { self.conversation_id } }

/// Role a node plays in one negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NegotiationRole {
    /// The node that sends the offer.
    Initiator,
    /// The node that receives the offer and decides to accept or reject.
    Recipient,
}

/// Lifecycle of a negotiation before streaming starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NegotiationState {
    /// Offer sent (initiator) or received (recipient); awaiting a decision.
    Pending,
    /// The recipient explicitly accepted; capture may begin.
    Accepted,
    /// The negotiation ended without streaming (reject, cancel, timeout,
    /// peer disconnect, or protocol error).
    Closed,
}

/// The mutually supported configuration the recipient selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiatedConfig {
    /// Selected codec (one of the offer's `codecs`).
    pub codec: String,
    /// Selected capture width.
    pub width: u16,
    /// Selected capture height.
    pub height: u16,
    /// Selected frame rate.
    pub frame_rate: u16,
}

impl NegotiatedConfig {
    /// Select a mutually supported configuration from the offer given the
    /// recipient's locally supported codecs. Picks the first codec both sides
    /// support, the first offered resolution, and the highest offered frame
    /// rate. Returns `None` when there is no shared codec or the message is
    /// not a `ScreenShareOffer`.
    pub fn select(offer: &ScreenShareMessage, local_codecs: &[String]) -> Option<Self> {
        let ScreenShareMessage::ScreenShareOffer { codecs, resolutions, frame_rate_max, .. } = offer else { return None; };
        let codec = codecs
            .iter()
            .find(|offered| local_codecs.iter().any(|local| local.eq_ignore_ascii_case(offered)))?
            .clone();
        let (width, height) = resolutions.first().copied()?;
        Some(Self { codec, width, height, frame_rate: *frame_rate_max })
    }

    /// True when this configuration lies within the offer's advertised
    /// capabilities (codec list, resolution list, frame-rate range).
    pub fn within(&self, offer: &ScreenShareMessage) -> bool {
        let ScreenShareMessage::ScreenShareOffer { codecs, resolutions, frame_rate_min, frame_rate_max, .. } = offer else { return false; };
        self.codecs_contains(codecs)
            && resolutions.iter().any(|(w, h)| *w == self.width && *h == self.height)
            && self.frame_rate >= *frame_rate_min
            && self.frame_rate <= *frame_rate_max
    }

    fn codecs_contains(&self, offered: &[String]) -> bool {
        offered.iter().any(|codec| codec.eq_ignore_ascii_case(&self.codec))
    }
}

/// Stable failure for a negotiation operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NegotiationError {
    /// No negotiation is known for the session id.
    UnknownSession,
    /// A duplicate offer arrived for an already-pending session.
    DuplicateOffer,
    /// Too many concurrent negotiations.
    Capacity,
    /// The operation is not valid in the current state.
    WrongState,
    /// The remote peer is not the recorded counterparty.
    PeerMismatch,
    /// The selected configuration is not mutually supported.
    UnsupportedConfig(String),
    /// The session id is the all-zero sentinel.
    EmptySessionId,
}

#[derive(Debug, Clone)]
struct NegotiationRecord {
    role: NegotiationRole,
    state: NegotiationState,
    host_id: iroh::PublicKey,
    peer_id: iroh::PublicKey,
    conversation_id: u64,
    offer: ScreenShareMessage,
    selected: Option<NegotiatedConfig>,
    deadline: Instant,
}

/// Bounded in-memory state for negotiations (PDF Task 3.1).
///
/// The initiator records an offer before sending it; the recipient records an
/// offer when it arrives. Only an explicit accept transitions a negotiation to
/// [`NegotiationState::Accepted`], and capture is only permitted in that state
/// ([`NegotiationManager::can_start_capture`]). Duplicate offers are rejected
/// explicitly (matching the existing Hello convention), and pending
/// negotiations expire at their deadline.
#[derive(Debug, Default)]
pub struct NegotiationManager {
    negotiations: HashMap<ScreenShareSessionId, NegotiationRecord>,
}

/// Maximum concurrent negotiations tracked by one manager.
pub const MAX_ACTIVE_NEGOTIATIONS: usize = 8;

/// Extract the offer fields from a `ScreenShareMessage`, returning `None`
/// when the message is not a `ScreenShareOffer`.
fn as_offer(message: &ScreenShareMessage) -> Option<(ScreenShareSessionId, iroh::PublicKey, u64)> {
    match message {
        ScreenShareMessage::ScreenShareOffer { session_id, host_id, conversation_id, .. } => {
            Some((*session_id, *host_id, *conversation_id))
        }
        _ => None,
    }
}

impl NegotiationManager {
    /// Create an empty negotiation store.
    pub fn new() -> Self { Self::default() }

    /// Current state of a negotiation, if known.
    pub fn state(&self, id: ScreenShareSessionId) -> Option<NegotiationState> {
        self.negotiations.get(&id).map(|record| record.state)
    }

    /// The configuration selected by the recipient, once accepted.
    pub fn selected(&self, id: ScreenShareSessionId) -> Option<&NegotiatedConfig> {
        self.negotiations.get(&id).and_then(|record| record.selected.as_ref())
    }

    /// The offer being negotiated, if known.
    pub fn offer(&self, id: ScreenShareSessionId) -> Option<&ScreenShareMessage> {
        self.negotiations.get(&id).map(|record| &record.offer)
    }

    /// The role this node plays in the negotiation, if known.
    pub fn role(&self, id: ScreenShareSessionId) -> Option<NegotiationRole> {
        self.negotiations.get(&id).map(|record| record.role)
    }

    /// Number of negotiations in a state that still occupies a slot
    /// (Pending or Accepted). Closed records no longer consume capacity.
    fn active_count(&self) -> usize {
        self.negotiations
            .values()
            .filter(|record| record.state != NegotiationState::Closed)
            .count()
    }

    /// Record an offer the local node is about to send (initiator side).
    pub fn start_offer(
        &mut self,
        offer: ScreenShareMessage,
        peer: iroh::PublicKey,
        timeout: Duration,
    ) -> Result<(), NegotiationError> {
        let Some((id, host_id, conversation_id)) = as_offer(&offer) else { return Err(NegotiationError::WrongState); };
        if id == ScreenShareSessionId::zero() { return Err(NegotiationError::EmptySessionId); }
        if self.negotiations.contains_key(&id) { return Err(NegotiationError::DuplicateOffer); }
        if self.active_count() >= MAX_ACTIVE_NEGOTIATIONS { return Err(NegotiationError::Capacity); }
        self.negotiations.insert(
            id,
            NegotiationRecord {
                role: NegotiationRole::Initiator,
                state: NegotiationState::Pending,
                host_id,
                peer_id: peer,
                conversation_id,
                offer,
                selected: None,
                deadline: Instant::now() + timeout,
            },
        );
        Ok(())
    }

    /// Record an offer that arrived from `peer` (recipient side). A duplicate
    /// offer for an already-pending session is an explicit error, matching the
    /// existing Hello convention. Emits [`SessionEvent::NegotiationInvitation`]
    /// so the UI can present the capabilities.
    pub fn receive_offer(
        &mut self,
        peer: iroh::PublicKey,
        offer: ScreenShareMessage,
        timeout: Duration,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Result<(), NegotiationError> {
        let Some((id, host_id, conversation_id)) = as_offer(&offer) else { return Err(NegotiationError::WrongState); };
        if id == ScreenShareSessionId::zero() { return Err(NegotiationError::EmptySessionId); }
        if self.negotiations.contains_key(&id) { return Err(NegotiationError::DuplicateOffer); }
        if self.active_count() >= MAX_ACTIVE_NEGOTIATIONS { return Err(NegotiationError::Capacity); }
        if host_id != peer { return Err(NegotiationError::PeerMismatch); }
        self.negotiations.insert(
            id,
            NegotiationRecord {
                role: NegotiationRole::Recipient,
                state: NegotiationState::Pending,
                host_id,
                peer_id: peer,
                conversation_id,
                offer: offer.clone(),
                selected: None,
                deadline: Instant::now() + timeout,
            },
        );
        emit_event(
            events,
            SessionEvent::NegotiationInvitation {
                session_id: id,
                host_id,
                conversation_id,
                offer,
            },
        );
        Ok(())
    }

    /// Recipient-side: accept a pending negotiation with the selected
    /// configuration. Returns the wire `ScreenShareAccept` to send back and
    /// transitions the record to `Accepted` (the only state that permits
    /// capture).
    pub fn accept(
        &mut self,
        id: ScreenShareSessionId,
        config: NegotiatedConfig,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Result<ScreenShareMessage, NegotiationError> {
        let record = self.negotiations.get_mut(&id).ok_or(NegotiationError::UnknownSession)?;
        if record.role != NegotiationRole::Recipient { return Err(NegotiationError::WrongState); }
        if record.state != NegotiationState::Pending { return Err(NegotiationError::WrongState); }
        if !config.within(&record.offer) {
            return Err(NegotiationError::UnsupportedConfig(format!(
                "codec {}, {}x{} @ {}fps not offered",
                config.codec, config.width, config.height, config.frame_rate
            )));
        }
        record.selected = Some(config.clone());
        record.state = NegotiationState::Accepted;
        emit_event(events, SessionEvent::Accepted { session_id: id, peer_id: record.peer_id });
        Ok(ScreenShareMessage::ScreenShareAccept {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
            codec: config.codec,
            width: config.width,
            height: config.height,
            frame_rate: config.frame_rate,
        })
    }

    /// Recipient-side: reject a pending negotiation. Returns the wire
    /// `ScreenShareReject` to send back.
    pub fn reject(
        &mut self,
        id: ScreenShareSessionId,
        reason: impl Into<String>,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Result<ScreenShareMessage, NegotiationError> {
        let record = self.negotiations.get_mut(&id).ok_or(NegotiationError::UnknownSession)?;
        if record.role != NegotiationRole::Recipient { return Err(NegotiationError::WrongState); }
        if record.state != NegotiationState::Pending { return Err(NegotiationError::WrongState); }
        let reason = reason.into();
        record.state = NegotiationState::Closed;
        emit_event(events, SessionEvent::Rejected { session_id: id, reason: reason.clone() });
        Ok(ScreenShareMessage::ScreenShareReject {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
            reason,
        })
    }

    /// Initiator-side: cancel a pending offer before the recipient responds.
    /// Returns the wire `ScreenShareReject` to inform the peer.
    pub fn cancel(
        &mut self,
        id: ScreenShareSessionId,
        reason: impl Into<String>,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Result<ScreenShareMessage, NegotiationError> {
        let record = self.negotiations.get_mut(&id).ok_or(NegotiationError::UnknownSession)?;
        if record.role != NegotiationRole::Initiator { return Err(NegotiationError::WrongState); }
        if record.state != NegotiationState::Pending { return Err(NegotiationError::WrongState); }
        let reason = reason.into();
        record.state = NegotiationState::Closed;
        emit_event(events, SessionEvent::Rejected { session_id: id, reason: reason.clone() });
        Ok(ScreenShareMessage::ScreenShareReject {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
            reason,
        })
    }

    /// Initiator-side: the remote `Accept` arrived. Validates the selected
    /// configuration is mutually supported (within the offered capabilities),
    /// then transitions to `Accepted`. Capture may begin after this.
    pub fn handle_accept(
        &mut self,
        peer: iroh::PublicKey,
        accept: ScreenShareMessage,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Result<(), NegotiationError> {
        let ScreenShareMessage::ScreenShareAccept { session_id, codec, width, height, frame_rate, .. } = &accept else { return Err(NegotiationError::WrongState); };
        let id = *session_id;
        let record = self.negotiations.get_mut(&id).ok_or(NegotiationError::UnknownSession)?;
        if record.role != NegotiationRole::Initiator { return Err(NegotiationError::WrongState); }
        if record.state != NegotiationState::Pending { return Err(NegotiationError::WrongState); }
        if record.peer_id != peer { return Err(NegotiationError::PeerMismatch); }
        let config = NegotiatedConfig {
            codec: codec.clone(),
            width: *width,
            height: *height,
            frame_rate: *frame_rate,
        };
        if !config.within(&record.offer) {
            return Err(NegotiationError::UnsupportedConfig(format!(
                "codec {}, {}x{} @ {}fps not offered",
                config.codec, config.width, config.height, config.frame_rate
            )));
        }
        record.selected = Some(config);
        record.state = NegotiationState::Accepted;
        emit_event(events, SessionEvent::Accepted { session_id: id, peer_id: peer });
        Ok(())
    }

    /// Either side: the remote `Reject` arrived; the negotiation is closed.
    pub fn handle_reject(
        &mut self,
        peer: iroh::PublicKey,
        reject: ScreenShareMessage,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Result<(), NegotiationError> {
        let ScreenShareMessage::ScreenShareReject { session_id, reason, .. } = &reject else { return Err(NegotiationError::WrongState); };
        let id = *session_id;
        let record = self.negotiations.get_mut(&id).ok_or(NegotiationError::UnknownSession)?;
        if record.peer_id != peer { return Err(NegotiationError::PeerMismatch); }
        if record.state == NegotiationState::Closed { return Err(NegotiationError::WrongState); }
        record.state = NegotiationState::Closed;
        emit_event(events, SessionEvent::Rejected { session_id: id, reason: reason.clone() });
        Ok(())
    }

    /// Close every pending negotiation whose deadline has passed. Returns the
    /// closed session ids so the caller can notify the wire layer.
    pub fn expire_pending(&mut self, now: Instant, events: &tokio::sync::mpsc::Sender<SessionEvent>) -> Vec<ScreenShareSessionId> {
        let expired: Vec<ScreenShareSessionId> = self
            .negotiations
            .iter()
            .filter(|(_, record)| record.state == NegotiationState::Pending && record.deadline <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            if let Some(record) = self.negotiations.get_mut(id) {
                record.state = NegotiationState::Closed;
                emit_event(events, SessionEvent::Rejected { session_id: *id, reason: "negotiation timed out".into() });
            }
        }
        expired
    }

    /// Close every open negotiation involving `peer` (the peer disconnected).
    /// Returns the closed session ids.
    pub fn peer_disconnected(&mut self, peer: iroh::PublicKey, events: &tokio::sync::mpsc::Sender<SessionEvent>) -> Vec<ScreenShareSessionId> {
        let affected: Vec<ScreenShareSessionId> = self
            .negotiations
            .iter()
            .filter(|(_, record)| {
                record.state != NegotiationState::Closed
                    && (record.host_id == peer || record.peer_id == peer)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in &affected {
            if let Some(record) = self.negotiations.get_mut(id) {
                record.state = NegotiationState::Closed;
                emit_event(events, SessionEvent::Ended { session_id: *id });
            }
        }
        affected
    }

    /// Capture gate: capture must NOT begin before explicit acceptance.
    /// Returns true only for negotiations in the `Accepted` state.
    pub fn can_start_capture(&self, id: ScreenShareSessionId) -> bool {
        self.negotiations.get(&id).is_some_and(|record| record.state == NegotiationState::Accepted)
    }

    /// Number of tracked negotiations (including closed records until
    /// [`Self::prune_closed`] runs). Bounded by [`MAX_ACTIVE_NEGOTIATIONS`]
    /// for pending ones.
    pub fn len(&self) -> usize { self.negotiations.len() }
    /// True when no negotiations are tracked.
    pub fn is_empty(&self) -> bool { self.negotiations.is_empty() }
    /// Drop closed records so a long-lived manager does not grow without
    /// bound. Pending and accepted records are kept.
    pub fn prune_closed(&mut self) {
        self.negotiations.retain(|_, record| record.state != NegotiationState::Closed);
    }
}

/// Push a session event to the app-facing channel, logging a warning if the
/// bounded channel is full (a dropped Invitation/Accepted is a silent
/// negotiation failure otherwise).
fn emit_event(events: &tokio::sync::mpsc::Sender<SessionEvent>, event: SessionEvent) {
    if let Err(
        tokio::sync::mpsc::error::TrySendError::Full(ev)
        | tokio::sync::mpsc::error::TrySendError::Closed(ev),
    ) = events.try_send(event)
    {
        tracing::warn!(?ev, "screen-share: session event dropped (receiver full or closed)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn accept_requires_pending_invitation() { let key = iroh::SecretKey::generate().public(); let mut manager = SessionManager::default(); let id = ScreenShareSessionId::generate(); assert!(manager.accept_invitation(id, key).is_none()); }
    #[test] fn end_is_idempotent() { let key = iroh::SecretKey::generate().public(); let peer = iroh::SecretKey::generate().public(); let id = ScreenShareSessionId::generate(); let mut manager = SessionManager::default(); manager.start_invitation(id, key, peer, 1); assert!(manager.end(id).is_some()); assert!(manager.end(id).is_none()); assert_eq!(manager.state(id), Some(SessionState::Ended)); }
    /// Regression: the HOST's manager records the invitee and must transition
    /// to Streaming when the INVITEE's Accept arrives. (The old check compared
    /// against record.host_id — the host's own key — so the Accept was
    /// silently ignored and the host never streamed: "waiting for viewer to
    /// accept" forever, viewer sees nothing.)
    #[test]
    fn remote_accept_from_invitee_transitions_host_to_streaming() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 7);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let response = manager.apply_remote(
            viewer,
            ControlMessage::Accept { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: id },
            &tx,
        );
        assert!(response.is_none());
        assert_eq!(manager.state(id), Some(SessionState::Streaming));
        match rx.try_recv() {
            Ok(SessionEvent::Accepted { session_id, peer_id }) => {
                assert_eq!(session_id, id);
                assert_eq!(peer_id, viewer);
            }
            other => panic!("expected Accepted event, got {other:?}"),
        }
    }
    /// Mirror case: the viewer's manager (record built from the remote Hello)
    /// also accepts via the invitee check.
    #[test]
    fn remote_accept_from_invitee_transitions_viewer_to_streaming() {
        let host = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        let hello = crate::screen_share::protocol::Hello {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
            host_id: host,
            conversation_id: 7,
            codecs: vec!["h264".into()],
            width: 640,
            height: 360,
            frame_rate: 15,
            permission: Permission::ViewOnly,
        };
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        manager.apply_remote(host, ControlMessage::Hello(hello), &tx);
        assert_eq!(manager.state(id), Some(SessionState::Connecting));
        manager.apply_remote(
            host,
            ControlMessage::Accept { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: id },
            &tx,
        );
        assert_eq!(manager.state(id), Some(SessionState::Streaming));
    }
    /// A stranger's Accept must never transition the session.
    #[test]
    fn remote_accept_from_wrong_peer_is_ignored() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let stranger = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 7);
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        manager.apply_remote(
            stranger,
            ControlMessage::Accept { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: id },
            &tx,
        );
        assert_eq!(manager.state(id), Some(SessionState::AwaitingAcceptance));
    }

    // ------------------------------------------------------------------
    // Versioned negotiation (PDF Task 3.1) tests.
    // ------------------------------------------------------------------

    fn test_offer(host: iroh::PublicKey) -> ScreenShareMessage {
        ScreenShareMessage::ScreenShareOffer {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: ScreenShareSessionId::from_bytes([9; 16]),
            host_id: host,
            conversation_id: 7,
            codecs: vec!["h264".into(), "vp8".into()],
            resolutions: vec![(1920, 1080), (1280, 720)],
            frame_rate_min: 15,
            frame_rate_max: 30,
            target_bitrate_bps: 2_000_000,
            remote_control: false,
        }
    }

    fn test_offer_with_id(host: iroh::PublicKey, id: ScreenShareSessionId) -> ScreenShareMessage {
        let mut offer = test_offer(host);
        let ScreenShareMessage::ScreenShareOffer { session_id, .. } = &mut offer else { panic!("offer") };
        *session_id = id;
        offer
    }

    fn channel() -> (tokio::sync::mpsc::Sender<SessionEvent>, tokio::sync::mpsc::Receiver<SessionEvent>) {
        tokio::sync::mpsc::channel(8)
    }

    /// Extract the codec list from an offer message (test helper).
    fn offer_codecs(offer: &ScreenShareMessage) -> Vec<String> {
        let ScreenShareMessage::ScreenShareOffer { codecs, .. } = offer else { panic!("offer") };
        codecs.clone()
    }

    /// The full offer → accept → accepted round trip on one manager pair:
    /// initiator records the offer, the recipient accepts with a mutually
    /// supported configuration, and capture is only permitted after the
    /// explicit accept on both sides.
    #[test]
    fn negotiation_offer_accept_roundtrip() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let mut recipient = NegotiationManager::new();
        let (tx, mut rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);

        initiator.start_offer(test_offer(host), viewer, Duration::from_secs(30)).unwrap();
        assert_eq!(initiator.state(id), Some(NegotiationState::Pending));
        assert!(!initiator.can_start_capture(id), "no capture before acceptance");

        recipient.receive_offer(host, test_offer(host), Duration::from_secs(30), &tx).unwrap();
        assert_eq!(recipient.state(id), Some(NegotiationState::Pending));
        assert!(!recipient.can_start_capture(id), "no capture before acceptance");
        match rx.try_recv().unwrap() {
            SessionEvent::NegotiationInvitation { session_id, host_id, offer, .. } => {
                assert_eq!(session_id, id);
                assert_eq!(host_id, host);
                assert_eq!(offer_codecs(&offer), vec!["h264".to_string(), "vp8".to_string()]);
            }
            other => panic!("expected NegotiationInvitation, got {other:?}"),
        }

        // Recipient selects a mutually supported configuration and accepts.
        let selected = NegotiatedConfig::select(recipient.offer(id).unwrap(), &["h264".to_string()]).unwrap();
        assert_eq!(selected.codec, "h264");
        assert_eq!((selected.width, selected.height), (1920, 1080));
        assert_eq!(selected.frame_rate, 30);
        let accept_message = recipient.accept(id, selected, &tx).unwrap();
        assert_eq!(recipient.state(id), Some(NegotiationState::Accepted));
        assert!(recipient.can_start_capture(id), "capture allowed after explicit accept");
        // The recipient's own Accept emits Accepted naming the host.
        assert_eq!(rx.try_recv().unwrap(), SessionEvent::Accepted { session_id: id, peer_id: host });

        // Initiator applies the remote accept (the manager takes the message).
        initiator.handle_accept(viewer, accept_message, &tx).unwrap();
        assert_eq!(initiator.state(id), Some(NegotiationState::Accepted));
        assert!(initiator.can_start_capture(id), "capture allowed after remote accept");
        assert_eq!(initiator.selected(id).unwrap().codec, "h264");
        assert_eq!(rx.try_recv().unwrap(), SessionEvent::Accepted { session_id: id, peer_id: viewer });
    }

    /// A stranger cannot accept a negotiation it does not own.
    #[test]
    fn negotiation_accept_from_wrong_peer_is_rejected() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let stranger = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let (tx, _rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);
        initiator.start_offer(test_offer(host), viewer, Duration::from_secs(30)).unwrap();
        let accept = ScreenShareMessage::ScreenShareAccept { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: id, codec: "h264".into(), width: 1920, height: 1080, frame_rate: 30 };
        assert_eq!(initiator.handle_accept(stranger, accept, &tx), Err(NegotiationError::PeerMismatch));
        assert_eq!(initiator.state(id), Some(NegotiationState::Pending));
    }

    /// The recipient may only accept a configuration that is mutually
    /// supported (inside the offered capabilities).
    #[test]
    fn negotiation_accept_rejects_unsupported_config() {
        let host = iroh::SecretKey::generate().public();
        let mut recipient = NegotiationManager::new();
        let (tx, _rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);
        recipient.receive_offer(host, test_offer(host), Duration::from_secs(30), &tx).unwrap();
        let bad = NegotiatedConfig { codec: "av1".into(), width: 1920, height: 1080, frame_rate: 30 };
        assert!(matches!(recipient.accept(id, bad, &tx), Err(NegotiationError::UnsupportedConfig(_))));
        assert_eq!(recipient.state(id), Some(NegotiationState::Pending));
    }

    /// Explicit reject closes the negotiation on both sides and never permits
    /// capture.
    #[test]
    fn negotiation_reject_roundtrip() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let mut recipient = NegotiationManager::new();
        let (tx, mut rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);
        initiator.start_offer(test_offer(host), viewer, Duration::from_secs(30)).unwrap();
        recipient.receive_offer(host, test_offer(host), Duration::from_secs(30), &tx).unwrap();
        // Drain the invitation emitted by receive_offer before rejecting.
        assert!(matches!(rx.try_recv().unwrap(), SessionEvent::NegotiationInvitation { session_id, .. } if session_id == id));
        let reject_message = recipient.reject(id, "user declined", &tx).unwrap();
        assert_eq!(recipient.state(id), Some(NegotiationState::Closed));
        assert!(!recipient.can_start_capture(id));
        initiator.handle_reject(viewer, reject_message, &tx).unwrap();
        assert_eq!(initiator.state(id), Some(NegotiationState::Closed));
        assert!(!initiator.can_start_capture(id));
        assert_eq!(rx.try_recv().unwrap(), SessionEvent::Rejected { session_id: id, reason: "user declined".into() });
    }

    /// Initiator cancel withdraws a pending offer before the recipient
    /// responds; the peer is informed with a Reject message.
    #[test]
    fn negotiation_cancel_withdraws_offer() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let (tx, mut rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);
        initiator.start_offer(test_offer(host), viewer, Duration::from_secs(30)).unwrap();
        let message = initiator.cancel(id, "cancelled by initiator", &tx).unwrap();
        assert_eq!(initiator.state(id), Some(NegotiationState::Closed));
        assert!(!initiator.can_start_capture(id));
        assert!(matches!(message, ScreenShareMessage::ScreenShareReject { reason, .. } if reason == "cancelled by initiator"));
        assert_eq!(rx.try_recv().unwrap(), SessionEvent::Rejected { session_id: id, reason: "cancelled by initiator".into() });
    }

    /// A duplicate offer for an already-pending session is an explicit error,
    /// matching the existing Hello convention.
    #[test]
    fn negotiation_duplicate_offer_is_rejected() {
        let host = iroh::SecretKey::generate().public();
        let mut recipient = NegotiationManager::new();
        let (tx, _rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);
        recipient.receive_offer(host, test_offer_with_id(host, id), Duration::from_secs(30), &tx).unwrap();
        let duplicate = test_offer_with_id(host, id);
        assert_eq!(recipient.receive_offer(host, duplicate, Duration::from_secs(30), &tx), Err(NegotiationError::DuplicateOffer));
        assert_eq!(recipient.state(id), Some(NegotiationState::Pending));
    }

    /// Pending negotiations expire at their deadline and never permit capture.
    #[test]
    fn negotiation_timeout_expires_pending() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let (tx, mut rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);
        let started = std::time::Instant::now();
        initiator.start_offer(test_offer(host), viewer, Duration::from_secs(30)).unwrap();
        let before_deadline = started + Duration::from_secs(29);
        assert!(initiator.expire_pending(before_deadline, &tx).is_empty(), "not yet expired");
        assert_eq!(initiator.state(id), Some(NegotiationState::Pending));
        let after_deadline = started + Duration::from_secs(31);
        let expired = initiator.expire_pending(after_deadline, &tx);
        assert_eq!(expired, vec![id]);
        assert_eq!(initiator.state(id), Some(NegotiationState::Closed));
        assert!(!initiator.can_start_capture(id));
        assert_eq!(rx.try_recv().unwrap(), SessionEvent::Rejected { session_id: id, reason: "negotiation timed out".into() });
    }

    /// Peer disconnect closes every negotiation involving that peer.
    #[test]
    fn negotiation_peer_disconnect_closes_sessions() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let (tx, mut rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);
        let other_id = ScreenShareSessionId::from_bytes([10; 16]);
        initiator.start_offer(test_offer_with_id(host, id), viewer, Duration::from_secs(30)).unwrap();
        initiator.start_offer(test_offer_with_id(host, other_id), viewer, Duration::from_secs(30)).unwrap();
        let closed = initiator.peer_disconnected(viewer, &tx);
        assert_eq!(closed.len(), 2);
        assert_eq!(initiator.state(id), Some(NegotiationState::Closed));
        assert_eq!(initiator.state(other_id), Some(NegotiationState::Closed));
        assert!(!initiator.can_start_capture(id));
        // HashMap iteration order is unspecified: assert the set of Ended ids.
        let mut ended: Vec<ScreenShareSessionId> = (0..2)
            .map(|_| match rx.try_recv().unwrap() {
                SessionEvent::Ended { session_id } => session_id,
                other => panic!("expected Ended, got {other:?}"),
            })
            .collect();
        ended.sort_by_key(|sid| *sid.as_bytes());
        let mut expected = vec![id, other_id];
        expected.sort_by_key(|sid| *sid.as_bytes());
        assert_eq!(ended, expected);
    }

    /// An offer whose host_id does not match the connected peer is refused.
    #[test]
    fn negotiation_offer_identity_mismatch_is_rejected() {
        let host = iroh::SecretKey::generate().public();
        let stranger = iroh::SecretKey::generate().public();
        let mut recipient = NegotiationManager::new();
        let (tx, _rx) = channel();
        // The offer claims host but arrives from stranger.
        let result = recipient.receive_offer(stranger, test_offer(host), Duration::from_secs(30), &tx);
        assert_eq!(result, Err(NegotiationError::PeerMismatch));
        assert!(recipient.is_empty());
    }

    /// Closing a negotiation frees the capacity slot and a later offer with a
    /// fresh session id is accepted.
    #[test]
    fn negotiation_capacity_is_bounded_and_reusable() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let (tx, _rx) = channel();
        for i in 0..MAX_ACTIVE_NEGOTIATIONS {
            // Ids start at 1: the all-zero id is the empty-session sentinel.
            let offer = test_offer_with_id(host, ScreenShareSessionId::from_bytes([(i as u8) + 1; 16]));
            initiator.start_offer(offer, viewer, Duration::from_secs(30)).unwrap();
        }
        let overflow = test_offer_with_id(host, ScreenShareSessionId::from_bytes([0xEE; 16]));
        assert_eq!(initiator.start_offer(overflow.clone(), viewer, Duration::from_secs(30)), Err(NegotiationError::Capacity));
        // Cancel one and retry: the slot is reusable.
        initiator.cancel(ScreenShareSessionId::from_bytes([1; 16]), "cancel", &tx).unwrap();
        assert!(initiator.start_offer(overflow, viewer, Duration::from_secs(30)).is_ok());
    }

    /// The empty-session sentinel is refused by the manager before any state
    /// is created.
    #[test]
    fn negotiation_empty_session_id_is_rejected() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let mut offer = test_offer(host);
        let ScreenShareMessage::ScreenShareOffer { session_id, .. } = &mut offer else { panic!("offer") };
        *session_id = ScreenShareSessionId::zero();
        assert_eq!(initiator.start_offer(offer, viewer, Duration::from_secs(30)), Err(NegotiationError::EmptySessionId));
    }
}
