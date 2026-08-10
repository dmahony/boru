//! Session-scoped permission policy for screen capture and remote input.
#![allow(missing_docs)]

use super::session::ScreenShareSessionId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Narrow capabilities. Accepting a share never implies control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability {
    ViewScreen,
    ControlPointer,
    ControlKeyboard,
    Clipboard,
}

pub const MAX_CAPABILITIES: usize = 4;
pub const REQUEST_WINDOW: Duration = Duration::from_secs(10);
pub const MAX_REQUESTS_PER_WINDOW: u32 = 4;

/// Small in-memory limiter for invitation/control prompts. It stores only
/// peer keys and timestamps, never request contents.
#[derive(Debug, Default)]
pub struct RequestRateLimiter {
    requests: HashMap<iroh::PublicKey, (Instant, u32)>,
}

impl RequestRateLimiter {
    pub fn allow(&mut self, peer_id: iroh::PublicKey, now: Instant) -> bool {
        let entry = self.requests.entry(peer_id).or_insert((now, 0));
        if now.duration_since(entry.0) >= REQUEST_WINDOW {
            *entry = (now, 0);
        }
        if entry.1 >= MAX_REQUESTS_PER_WINDOW {
            return false;
        }
        entry.1 += 1;
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPermissions {
    session_id: ScreenShareSessionId,
    peer_id: iroh::PublicKey,
    granted: Vec<Capability>,
    active: bool,
}

impl SessionPermissions {
    pub fn view_only(session_id: ScreenShareSessionId, peer_id: iroh::PublicKey) -> Self {
        Self { session_id, peer_id, granted: vec![Capability::ViewScreen], active: true }
    }

    pub fn session_id(&self) -> ScreenShareSessionId { self.session_id }
    pub fn peer_id(&self) -> iroh::PublicKey { self.peer_id }
    pub fn is_active(&self) -> bool { self.active }
    pub fn capabilities(&self) -> &[Capability] { &self.granted }

    pub fn allows(&self, session_id: ScreenShareSessionId, peer_id: iroh::PublicKey, capability: Capability) -> bool {
        self.active && self.session_id == session_id && self.peer_id == peer_id && self.granted.contains(&capability)
    }

    pub fn grant(&mut self, capabilities: impl IntoIterator<Item = Capability>) -> bool {
        if !self.active { return false; }
        for capability in capabilities {
            if !self.granted.contains(&capability) && self.granted.len() < MAX_CAPABILITIES {
                self.granted.push(capability);
            }
        }
        true
    }

    /// Revoke control while keeping view-only sharing alive.
    pub fn revoke_control(&mut self) {
        self.granted.retain(|capability| *capability == Capability::ViewScreen);
    }

    /// End the session and invalidate every capability immediately.
    pub fn end(&mut self) { self.active = false; self.granted.clear(); }
}

/// Platform capture permission state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionState {
    Unknown,
    Granted,
    Denied,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_only_does_not_authorize_input() {
        let session = ScreenShareSessionId::from_bytes([1; 16]);
        let peer = iroh::SecretKey::generate().public();
        let permissions = SessionPermissions::view_only(session, peer);
        assert!(permissions.allows(session, peer, Capability::ViewScreen));
        assert!(!permissions.allows(session, peer, Capability::ControlPointer));
    }

    #[test]
    fn revoke_is_immediate_and_session_bound() {
        let session = ScreenShareSessionId::from_bytes([2; 16]);
        let peer = iroh::SecretKey::generate().public();
        let mut permissions = SessionPermissions::view_only(session, peer);
        permissions.grant([Capability::ControlPointer, Capability::ControlKeyboard]);
        assert!(permissions.allows(session, peer, Capability::ControlPointer));
        permissions.revoke_control();
        assert!(!permissions.allows(session, peer, Capability::ControlPointer));
        assert!(!permissions.allows(ScreenShareSessionId::from_bytes([3; 16]), peer, Capability::ViewScreen));
    }
}
