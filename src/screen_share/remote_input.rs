//! Consent-gated remote input and platform boundaries.
#![allow(missing_docs)]
use super::permissions::{Capability, ControlToken, SessionPermissions};
use super::session::ScreenShareSessionId;
use super::ScreenShareError;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputEvent {
    pub code: u32,
    pub capability: Capability,
    pub token: Option<ControlToken>,
}
pub const MAX_INPUT_EVENT_BYTES: usize = 256;

pub fn authorize_input(permissions: &SessionPermissions, session_id: ScreenShareSessionId, peer_id: iroh::PublicKey, event: &InputEvent) -> Result<(), ScreenShareError> {
    if event.token.map_or(false, |token| permissions.allows_token(session_id, peer_id, token, event.capability, Instant::now())) {
        Ok(())
    } else {
        Err(ScreenShareError::new("remote input capability is not granted"))
    }
}

pub trait RemoteInput: Send {
    fn apply(&mut self, event: InputEvent) -> Result<(), ScreenShareError>;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizedPointer { pub x: f32, pub y: f32 }

/// Map a letterboxed viewer rectangle into capture pixels. Points outside the
/// active image are ignored, avoiding input in black bars or stale regions.
pub fn map_pointer(point: NormalizedPointer, viewer: (f32, f32), capture: (u32, u32)) -> Option<(u32, u32)> {
    if !point.x.is_finite() || !point.y.is_finite() || viewer.0 <= 0.0 || viewer.1 <= 0.0 || capture.0 == 0 || capture.1 == 0 { return None; }
    let scale = (viewer.0 / capture.0 as f32).min(viewer.1 / capture.1 as f32);
    let image = (capture.0 as f32 * scale, capture.1 as f32 * scale);
    let origin = ((viewer.0 - image.0) / 2.0, (viewer.1 - image.1) / 2.0);
    let local = (point.x * viewer.0 - origin.0, point.y * viewer.1 - origin.1);
    if local.0 < 0.0 || local.1 < 0.0 || local.0 >= image.0 || local.1 >= image.1 { return None; }
    Some(((local.0 / scale).floor() as u32, (local.1 / scale).floor() as u32))
}

#[derive(Debug, Default)]
pub struct UnavailableInputBackend;
impl RemoteInput for UnavailableInputBackend {
    fn apply(&mut self, _event: InputEvent) -> Result<(), ScreenShareError> { Err(ScreenShareError::new("remote input backend is unavailable")) }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
pub struct LinuxPortalRemoteInput { active: bool }
#[cfg(target_os = "linux")]
impl LinuxPortalRemoteInput {
    /// The real implementation must attach to an xdg-desktop-portal
    /// RemoteDesktop session. No privileged XTest/uinput fallback is allowed.
    pub fn new() -> Result<Self, ScreenShareError> { Ok(Self { active: true }) }
    pub fn revoke(&mut self) { self.active = false; }
}
#[cfg(target_os = "linux")]
impl RemoteInput for LinuxPortalRemoteInput {
    fn apply(&mut self, _event: InputEvent) -> Result<(), ScreenShareError> {
        if self.active { Err(ScreenShareError::new("desktop portal remote-desktop session is not connected")) } else { Err(ScreenShareError::new("remote-desktop session revoked")) }
    }
}

#[cfg(target_os = "windows")]
#[derive(Debug, Default)]
pub struct WindowsRemoteInput { active: bool }
#[cfg(target_os = "windows")]
impl WindowsRemoteInput {
    /// Uses the supported user-session input boundary only; UAC/secure desktop
    /// input is intentionally not attempted.
    pub fn new() -> Result<Self, ScreenShareError> { Ok(Self { active: true }) }
    pub fn revoke(&mut self) { self.active = false; }
}
#[cfg(target_os = "windows")]
impl RemoteInput for WindowsRemoteInput {
    fn apply(&mut self, _event: InputEvent) -> Result<(), ScreenShareError> {
        if self.active { Err(ScreenShareError::new("Windows input backend is not connected")) } else { Err(ScreenShareError::new("remote input revoked")) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_is_rejected_before_grant_and_after_revoke() {
        let session = ScreenShareSessionId::from_bytes([9; 16]);
        let peer = iroh::SecretKey::generate().public();
        let event = InputEvent { code: 1, capability: Capability::ControlPointer, token: None };
        let mut permissions = SessionPermissions::view_only(session, peer);
        assert!(authorize_input(&permissions, session, peer, &event).is_err());
        permissions.grant([Capability::ControlPointer]);
        let event = InputEvent { token: permissions.token(), ..event };
        assert!(authorize_input(&permissions, session, peer, &event).is_ok());
        permissions.revoke_control();
        assert!(authorize_input(&permissions, session, peer, &event).is_err());
    }
    #[test]
    fn mapping_rejects_letterbox_and_scales_capture() {
        assert_eq!(map_pointer(NormalizedPointer { x: 0.5, y: 0.5 }, (1600.0, 900.0), (1920, 1080)), Some((960, 540)));
        assert_eq!(map_pointer(NormalizedPointer { x: 0.5, y: 0.01 }, (1600.0, 1200.0), (1920, 1080)), None);
    }
}
