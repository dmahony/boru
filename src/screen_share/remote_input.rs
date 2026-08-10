//! Optional remote-input boundary.

use super::permissions::{Capability, SessionPermissions};
use super::session::ScreenShareSessionId;
use super::ScreenShareError;

/// A deliberately small input event placeholder for the later protocol milestone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputEvent {
    /// Opaque event code interpreted by a platform backend.
    pub code: u32,
    /// Capability required by this event.
    pub capability: Capability,
}

/// Maximum serialized input-event budget for a future control stream.
pub const MAX_INPUT_EVENT_BYTES: usize = 256;

/// Authorize an event without invoking a platform backend. This keeps the
/// security boundary deterministic and testable without a desktop session.
pub fn authorize_input(
    permissions: &SessionPermissions,
    session_id: ScreenShareSessionId,
    peer_id: iroh::PublicKey,
    event: &InputEvent,
) -> Result<(), ScreenShareError> {
    if permissions.allows(session_id, peer_id, event.capability) {
        Ok(())
    } else {
        Err(ScreenShareError::new(
            "remote input capability is not granted",
        ))
    }
}

/// Applies consent-authorized input events to a local desktop.
pub trait RemoteInput: Send {
    /// Apply one input event.
    fn apply(&mut self, event: InputEvent) -> Result<(), ScreenShareError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_is_rejected_before_grant_and_after_revoke() {
        let session = ScreenShareSessionId::from_bytes([9; 16]);
        let peer = iroh::SecretKey::generate().public();
        let event = InputEvent {
            code: 1,
            capability: Capability::ControlPointer,
        };
        let mut permissions = SessionPermissions::view_only(session, peer);
        assert!(authorize_input(&permissions, session, peer, &event).is_err());
        permissions.grant([Capability::ControlPointer]);
        assert!(authorize_input(&permissions, session, peer, &event).is_ok());
        permissions.revoke_control();
        assert!(authorize_input(&permissions, session, peer, &event).is_err());
    }
}
