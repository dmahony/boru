//! Screen-share invitation and session state machine.
#![allow(missing_docs)]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::protocol::{ControlMessage, Hello, Permission, SCREEN_SHARE_PROTOCOL_VERSION};
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
    pub fn start_invitation(&mut self, id: ScreenShareSessionId, host_id: iroh::PublicKey, conversation_id: u64) {
        if id == ScreenShareSessionId::zero() || self.sessions.len() >= MAX_ACTIVE_SESSIONS { return; }
        self.sessions.insert(id, Record { state: SessionState::AwaitingAcceptance, host_id, peer_id: None, conversation_id });
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
                if hello.session_id == ScreenShareSessionId::zero() || self.sessions.len() >= MAX_ACTIVE_SESSIONS {
                    return Some(ControlMessage::Reject { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: hello.session_id, reason: "session is not available".into() });
                }
                if hello.host_id != peer_id {
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
                let _ = events.try_send(SessionEvent::Invitation { session_id: hello.session_id, host_id: hello.host_id, conversation_id: hello.conversation_id, hello });
                None
            }
            ControlMessage::Accept { session_id, .. } => {
                if let Some(record) = self.sessions.get_mut(&session_id) { if record.host_id == peer_id && matches!(record.state, SessionState::AwaitingAcceptance | SessionState::Connecting) { record.state = SessionState::Streaming; self.permissions.insert(session_id, SessionPermissions::view_only(session_id, peer_id)); let _ = events.try_send(SessionEvent::Accepted { session_id, peer_id }); } }
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn accept_requires_pending_invitation() { let key = iroh::SecretKey::generate().public(); let mut manager = SessionManager::default(); let id = ScreenShareSessionId::generate(); assert!(manager.accept_invitation(id, key).is_none()); }
    #[test] fn end_is_idempotent() { let key = iroh::SecretKey::generate().public(); let id = ScreenShareSessionId::generate(); let mut manager = SessionManager::default(); manager.start_invitation(id, key, 1); assert!(manager.end(id).is_some()); assert!(manager.end(id).is_none()); assert_eq!(manager.state(id), Some(SessionState::Ended)); }
}
