//! Optional remote-input boundary.

use super::ScreenShareError;

/// A deliberately small input event placeholder for the later protocol milestone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputEvent {
    /// Opaque event code interpreted by a platform backend.
    pub code: u32,
}

/// Applies consent-authorized input events to a local desktop.
pub trait RemoteInput: Send {
    /// Apply one input event.
    fn apply(&mut self, event: InputEvent) -> Result<(), ScreenShareError>;
}
