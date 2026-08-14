//! Consent-gated remote input and platform boundaries.
#![allow(missing_docs)]
use super::permissions::{Capability, ControlToken, SessionPermissions};
use super::session::ScreenShareSessionId;
use super::ScreenShareError;
use std::time::Instant;

/// One normalized input event flowing viewer → host. For pointer events `x`/`y`
/// are capture pixels after the host maps normalized viewer coordinates; for
/// keyboard events they are ignored. `code` is a button id (1-3) for pointer or
/// an X11 keysym for keyboard. `pressed` is the key/button state.
#[derive(Debug, Clone, PartialEq)]
pub struct InputEvent {
    pub code: u32,
    pub capability: Capability,
    pub token: Option<ControlToken>,
    pub x: f32,
    pub y: f32,
    pub pressed: bool,
}
pub const MAX_INPUT_EVENT_BYTES: usize = 256;

pub fn authorize_input(permissions: &SessionPermissions, session_id: ScreenShareSessionId, peer_id: iroh::PublicKey, event: &InputEvent) -> Result<(), ScreenShareError> {
    if event.token.map_or(false, |token| permissions.allows_token(session_id, peer_id, token, event.capability, Instant::now())) {
        Ok(())
    } else {
        Err(ScreenShareError::new("remote input capability is not granted"))
    }
}

/// Host-side validation of a wire Input message carrying only the grant nonce.
pub fn authorize_nonce(permissions: &SessionPermissions, session_id: ScreenShareSessionId, peer_id: iroh::PublicKey, capability: Capability, nonce: [u8; 16]) -> Result<(), ScreenShareError> {
    if permissions.allows(session_id, peer_id, capability) && permissions.nonce_matches(nonce, Instant::now()) {
        Ok(())
    } else {
        Err(ScreenShareError::new("remote input capability is not granted"))
    }
}

#[async_trait::async_trait]
pub trait RemoteInput: Send {
    /// Apply one authorized input event on the host platform. `x`/`y` are
    /// capture pixels for pointer events (the host already mapped them).
    async fn apply(&mut self, event: InputEvent) -> Result<(), ScreenShareError>;
    /// Shut the backend down immediately (portal session close / none).
    async fn shutdown(&mut self);
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

/// Map a viewer point normalized to the image rect (0..1) into capture pixels,
/// rejecting out-of-range points. The viewer already excludes letterbox via the
/// mouse area, so this is a direct scale plus bounds check.
pub fn normalize_to_capture(point: NormalizedPointer, capture: (u32, u32)) -> Option<(u32, u32)> {
    if !point.x.is_finite() || !point.y.is_finite() || capture.0 == 0 || capture.1 == 0 { return None; }
    let x = point.x * capture.0 as f32;
    let y = point.y * capture.1 as f32;
    if x < 0.0 || y < 0.0 || x >= capture.0 as f32 || y >= capture.1 as f32 { return None; }
    Some((x.floor() as u32, y.floor() as u32))
}

#[derive(Debug, Default)]
pub struct UnavailableInputBackend;
#[async_trait::async_trait]
impl RemoteInput for UnavailableInputBackend {
    async fn apply(&mut self, _event: InputEvent) -> Result<(), ScreenShareError> { Err(ScreenShareError::new("remote input backend is unavailable")) }
    async fn shutdown(&mut self) {}
}

/// Create the platform input backend, failing closed when the environment does
/// not provide one (no portal session bus on Linux). `capture` is the capture
/// source geometry used to scale pointer coordinates to the platform screen.
pub async fn create_platform_backend(capture: (u32, u32)) -> Box<dyn RemoteInput> {
    #[cfg(target_os = "linux")]
    {
        match LinuxPortalRemoteInput::connect().await {
            Ok(backend) => Box::new(backend),
            Err(_) => Box::new(UnavailableInputBackend),
        }
    }
    #[cfg(all(not(target_os = "linux"), target_os = "windows"))]
    {
        Box::new(WindowsRemoteInput::new(capture))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Box::new(UnavailableInputBackend)
    }
}

// ── Linux: xdg-desktop-portal RemoteDesktop (D-Bus) ─────────────────────────
//
// The portal path is the only supported injection mechanism on Linux; no
// privileged XTest/uinput fallback. The session bus object is
// org.freedesktop.portal.RemoteDesktop at /org/freedesktop/portal/desktop.

#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
pub struct LinuxPortalRemoteInput {
    connection: Option<zbus::Connection>,
    session: Option<zbus::zvariant::OwnedObjectPath>,
    /// Bitmask of device types the portal user actually granted
    /// (1 = pointer, 2 = keyboard). Empty when Start was denied or the
    /// portal granted nothing — `apply` then fails closed so view-only
    /// sharing keeps working (PDF Task 5.3).
    granted_devices: u32,
    last: Option<(f64, f64)>,
}

/// Portal RemoteDesktop device-type bits (org.freedesktop.portal.RemoteDesktop
/// `types` / `devices` values).
pub const PORTAL_DEVICE_POINTER: u32 = 1;
pub const PORTAL_DEVICE_KEYBOARD: u32 = 2;

/// True when a portal `devices` bitmask grants the given capability.
/// `ViewScreen` (not an input device) is never granted by the portal.
pub fn device_mask_grants(devices: u32, capability: Capability) -> bool {
    match capability {
        Capability::ControlPointer => devices & PORTAL_DEVICE_POINTER != 0,
        Capability::ControlKeyboard => devices & PORTAL_DEVICE_KEYBOARD != 0,
        _ => false,
    }
}

/// Extract the `devices` bitmask from a RemoteDesktop `Start` response body.
///
/// The reply is a dictionary `{ "devices": u32, ... }`. zvariant 5 does not
/// implement `TryFrom<&Value>` for Vec/HashMap, so walk the Value enum
/// directly (mirrors `extract_stream_node_id` in platform/linux.rs).
pub fn parse_devices_mask(body: &zbus::zvariant::Value) -> Option<u32> {
    use zbus::zvariant::Value;
    let Value::Dict(dict) = body else { return None };
    let devices_key = "devices".to_string();
    let devices = dict.get::<String, Value>(&devices_key).ok()??;
    match devices {
        Value::U32(mask) => Some(mask),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
impl LinuxPortalRemoteInput {
    /// Timeout for the interactive `Start` call (the portal shows a device
    /// consent dialog). Headless/denied environments fail closed.
    pub const PORTAL_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

    /// Create the portal RemoteDesktop session, select pointer + keyboard
    /// devices, and await the user's decision. Fails closed (Err) when no
    /// portal is reachable or the user denies the Start dialog.
    pub async fn connect() -> Result<Self, ScreenShareError> {
        let connection = zbus::Connection::session().await.map_err(|e| ScreenShareError::new(format!("no session bus: {e}")))?;
        let portal = ("org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop", "org.freedesktop.portal.RemoteDesktop");
        // CreateSession(session_handle_token) → session object path.
        let token = format!("boru_{:016x}", rand::random::<u64>());
        let options: std::collections::HashMap<&str, zbus::zvariant::Value> = [("session_handle_token", zbus::zvariant::Value::from(token))].into_iter().collect();
        let reply = connection.call_method(Some(portal.0), portal.1, Some(portal.2), "CreateSession", &options).await.map_err(|e| ScreenShareError::new(format!("portal CreateSession failed: {e}")))?;
        let session: zbus::zvariant::OwnedObjectPath = reply.body().deserialize().map_err(|e| ScreenShareError::new(format!("portal session reply malformed: {e}")))?;
        // SelectDevices(types = Pointer | Keyboard) — per the portal spec the
        // `types` bitmask lives inside the options vardict.
        let types = PORTAL_DEVICE_POINTER | PORTAL_DEVICE_KEYBOARD;
        let device_options: std::collections::HashMap<&str, zbus::zvariant::Value> = [("types", zbus::zvariant::Value::U32(types))].into_iter().collect();
        let _ = connection.call_method(Some(portal.0), portal.1, Some(portal.2), "SelectDevices", &(session.clone(), device_options)).await.map_err(|e| ScreenShareError::new(format!("portal SelectDevices failed: {e}")))?;
        // Start() is asynchronous: the reply is a Request object path and the
        // real result (response code + granted `devices` bitmask) arrives on
        // the Response signal of that object. Await it so we fail closed when
        // the user denies remote input — view-only sharing must keep working
        // (PDF Task 5.3).
        let start_options: std::collections::HashMap<&str, zbus::zvariant::Value> = std::collections::HashMap::new();
        let request_path: zbus::zvariant::OwnedObjectPath = tokio::time::timeout(
            Self::PORTAL_START_TIMEOUT,
            connection.call_method(Some(portal.0), portal.1, Some(portal.2), "Start", &(session.clone(), "", start_options)),
        )
        .await
        .map_err(|_| ScreenShareError::new("portal remote-desktop Start timed out (no response from the consent dialog)"))?
        .map_err(|e| ScreenShareError::new(format!("portal Start failed: {e}")))?
        .body()
        .deserialize()
        .map_err(|e| ScreenShareError::new(format!("portal Start request malformed: {e}")))?;
        let request = zbus::Proxy::new(&connection, portal.0, request_path.as_str(), "org.freedesktop.portal.Request")
            .await
            .map_err(|e| ScreenShareError::new(format!("portal request proxy failed: {e}")))?;
        let mut responses = request.receive_signal("Response").await.map_err(|e| ScreenShareError::new(format!("portal response subscription failed: {e}")))?;
        let response = tokio::time::timeout(Self::PORTAL_START_TIMEOUT, n0_future::StreamExt::next(&mut responses))
            .await
            .map_err(|_| ScreenShareError::new("portal remote-desktop Start timed out waiting for the consent response"))?
            .ok_or_else(|| ScreenShareError::new("portal response stream closed"))?;
        let (response_code, body): (u32, zbus::zvariant::OwnedValue) = response
            .body()
            .deserialize()
            .map_err(|e| ScreenShareError::new(format!("portal response malformed: {e}")))?;
        if response_code != 0 {
            return Err(ScreenShareError::new(format!("portal remote-desktop permission denied (code {response_code})")));
        }
        let granted_devices = parse_devices_mask(&body).unwrap_or(0);
        if granted_devices & (PORTAL_DEVICE_POINTER | PORTAL_DEVICE_KEYBOARD) == 0 {
            return Err(ScreenShareError::new("portal granted no input devices (remote control denied)"));
        }
        tracing::info!(granted_devices, "screen-share: portal remote-desktop session started");
        Ok(Self { connection: Some(connection), session: Some(session), granted_devices, last: None })
    }
}

/// Every RemoteDesktop `Notify*` method takes an `options` vardict (`a{sv}`)
/// between the session handle and the event payload (portal spec). All Boru
/// events are sent with an empty dict.
fn empty_options() -> std::collections::HashMap<&'static str, zbus::zvariant::Value<'static>> {
    std::collections::HashMap::new()
}

#[cfg(target_os = "linux")]
#[async_trait::async_trait]
impl RemoteInput for LinuxPortalRemoteInput {
    async fn apply(&mut self, event: InputEvent) -> Result<(), ScreenShareError> {
        let (Some(connection), Some(session)) = (&self.connection, &self.session) else { return Err(ScreenShareError::new("portal remote-desktop session is not connected")); };
        if !device_mask_grants(self.granted_devices, event.capability) {
            return Err(ScreenShareError::new("device type was not granted by the portal (view-only)"));
        }
        let portal = ("org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop", "org.freedesktop.portal.RemoteDesktop");
        match event.capability {
            Capability::ControlPointer => {
                let (px, py) = (event.x as f64, event.y as f64);
                let (dx, dy) = match self.last { Some((lx, ly)) => (px - lx, py - ly), None => (0.0, 0.0) };
                self.last = Some((px, py));
                if dx != 0.0 || dy != 0.0 {
                    let _ = connection.call_method(Some(portal.0), portal.1, Some(portal.2), "NotifyPointerMotion", &(session, empty_options(), dx, dy)).await.map_err(|e| ScreenShareError::new(format!("portal pointer motion failed: {e}")))?;
                }
                if event.code != 0 {
                    let state = if event.pressed { 1u32 } else { 0u32 };
                    let _ = connection.call_method(Some(portal.0), portal.1, Some(portal.2), "NotifyPointerButton", &(session, empty_options(), event.code as i32, state)).await.map_err(|e| ScreenShareError::new(format!("portal pointer button failed: {e}")))?;
                }
            }
            Capability::ControlKeyboard => {
                let state = if event.pressed { 1u32 } else { 0u32 };
                let _ = connection.call_method(Some(portal.0), portal.1, Some(portal.2), "NotifyKeyboardKeysym", &(session, empty_options(), event.code as i32, state)).await.map_err(|e| ScreenShareError::new(format!("portal keyboard failed: {e}")))?;
            }
            _ => return Err(ScreenShareError::new("capability is not supported by the portal backend")),
        }
        Ok(())
    }
    async fn shutdown(&mut self) {
        if let (Some(connection), Some(session)) = (&self.connection, &self.session) {
            let _ = connection.call_method(Some("org.freedesktop.portal.Desktop"), "/org/freedesktop/portal/desktop", Some("org.freedesktop.portal.RemoteDesktop"), "CloseSession", &(session,)).await;
        }
        self.connection = None;
        self.session = None;
        self.granted_devices = 0;
    }
}

// ── Windows: SendInput (user-session input only) ────────────────────────────
//
// Least-privileged supported mechanism: user-session injection via SendInput.
// UAC/secure-desktop input is intentionally not attempted. Coordinates are
// absolute across the virtual screen (0..65535), which keeps mapping correct
// under per-monitor DPI without querying display DPI.

#[cfg(target_os = "windows")]
#[derive(Debug, Default)]
pub struct WindowsRemoteInput {
    active: bool,
    /// Capture source geometry (scales capture pixels to the virtual screen).
    capture: (u32, u32),
}

#[cfg(target_os = "windows")]
impl WindowsRemoteInput {
    pub fn new(capture: (u32, u32)) -> Self { Self { active: true, capture } }
    pub fn revoke(&mut self) { self.active = false; }
    /// Map a portable X11 keysym to a Windows virtual-key code. Unsupported
    /// keys map to 0 and are ignored (fail-closed).
    fn keysym_to_vk(code: u32) -> u16 {
        match code {
            0x61..=0x7A => (code - 0x20) as u16, // a-z → A-Z
            0x30..=0x39 => code as u16,           // 0-9
            0xFF0D => 0x0D, 0xFF08 => 0x08, 0xFF09 => 0x09, 0xFF1B => 0x1B, 0x20 => 0x20,
            0xFF51 => 0x25, 0xFF52 => 0x26, 0xFF53 => 0x27, 0xFF54 => 0x28,
            0xFF50 => 0x24, 0xFF57 => 0x23, 0xFF55 => 0x21, 0xFF56 => 0x22,
            0xFF63 => 0x2D, 0xFFFF => 0x2E,
            0xFFE1 => 0x10, 0xFFE2 => 0x10, 0xFFE3 => 0x11, 0xFFE4 => 0x11, 0xFFE9 => 0x12, 0xFFEA => 0x12,
            0xFFE5 => 0x14,
            0xFFBE..=0xFFC9 => 0x70 + (code - 0xFFBE) as u16, // F1-F12
            0x3B => 0xBA, 0x3D => 0xBB, 0x2C => 0xBC, 0x2D => 0xBD, 0x2E => 0xBE, 0x2F => 0xBF,
            0x60 => 0xC0, 0x5B => 0xDB, 0x5C => 0xDC, 0x5D => 0xDD, 0x27 => 0xDE,
            _ => 0,
        }
    }
}

#[cfg(target_os = "windows")]
#[async_trait::async_trait]
impl RemoteInput for WindowsRemoteInput {
    async fn apply(&mut self, event: InputEvent) -> Result<(), ScreenShareError> {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, INPUT_0, KEYBDINPUT, KEYEVENTF_KEYUP,
            MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
            MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
            MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
        };
        if !self.active { return Err(ScreenShareError::new("remote input revoked")); }
        match event.capability {
            Capability::ControlPointer => {
                let (cw, ch) = self.capture;
                let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
                let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
                if vw <= 0 || vh <= 0 || cw == 0 || ch == 0 { return Err(ScreenShareError::new("virtual screen metrics unavailable")); }
                let mut inputs = Vec::new();
                // Absolute move (0..65535 across the virtual screen): scale the
                // capture-space point by the capture geometry.
                let dx = ((event.x.clamp(0.0, cw as f32) / cw as f32) * 65535.0).floor() as i32;
                let dy = ((event.y.clamp(0.0, ch as f32) / ch as f32) * 65535.0).floor() as i32;
                inputs.push(INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, time: 0, dwExtraInfo: 0 } },
                });
                if event.code != 0 {
                    let flag = match (event.code, event.pressed) {
                        (1, true) => MOUSEEVENTF_LEFTDOWN, (1, false) => MOUSEEVENTF_LEFTUP,
                        (2, true) => MOUSEEVENTF_MIDDLEDOWN, (2, false) => MOUSEEVENTF_MIDDLEUP,
                        (3, true) => MOUSEEVENTF_RIGHTDOWN, (3, false) => MOUSEEVENTF_RIGHTUP,
                        _ => 0,
                    };
                    if flag != 0 { inputs.push(INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: flag, time: 0, dwExtraInfo: 0 } } }); }
                }
                let ok = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) } == inputs.len() as u32;
                if !ok { return Err(ScreenShareError::new("SendInput failed")); }
            }
            Capability::ControlKeyboard => {
                let vk = Self::keysym_to_vk(event.code);
                if vk == 0 { return Err(ScreenShareError::new("unsupported key code")); }
                let flags = if event.pressed { 0u32 } else { KEYEVENTF_KEYUP };
                let input = INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
                };
                let ok = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) } == 1;
                if !ok { return Err(ScreenShareError::new("SendInput failed")); }
            }
            _ => return Err(ScreenShareError::new("capability is not supported by the Windows backend")),
        }
        Ok(())
    }
    async fn shutdown(&mut self) { self.active = false; }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen_share::permissions::Capability;

    #[test]
    fn input_is_rejected_before_grant_and_after_revoke() {
        let session = ScreenShareSessionId::from_bytes([9; 16]);
        let peer = iroh::SecretKey::generate().public();
        let event = InputEvent { code: 1, capability: Capability::ControlPointer, token: None, x: 0.5, y: 0.5, pressed: true };
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

    #[test]
    fn normalize_rejects_out_of_range_points() {
        assert_eq!(normalize_to_capture(NormalizedPointer { x: 0.5, y: 0.5 }, (640, 360)), Some((320, 180)));
        assert_eq!(normalize_to_capture(NormalizedPointer { x: 1.0, y: 0.5 }, (640, 360)), None);
        assert_eq!(normalize_to_capture(NormalizedPointer { x: f32::NAN, y: 0.5 }, (640, 360)), None);
    }

    #[test]
    fn authorize_nonce_rejects_stale_nonce() {
        let session = ScreenShareSessionId::from_bytes([10; 16]);
        let peer = iroh::SecretKey::generate().public();
        let mut permissions = SessionPermissions::view_only(session, peer);
        permissions.grant_with_nonce([Capability::ControlPointer], [7; 16]);
        assert!(authorize_nonce(&permissions, session, peer, Capability::ControlPointer, [7; 16]).is_ok());
        assert!(authorize_nonce(&permissions, session, peer, Capability::ControlKeyboard, [7; 16]).is_err());
        assert!(authorize_nonce(&permissions, session, peer, Capability::ControlPointer, [8; 16]).is_err());
        permissions.revoke_control();
        assert!(authorize_nonce(&permissions, session, peer, Capability::ControlPointer, [7; 16]).is_err());
    }

    /// Portal `devices` bitmask gating (PDF Task 5.3): pointer requires the
    /// pointer bit, keyboard requires the keyboard bit, and a denied/empty
    /// mask grants nothing — so view-only sharing keeps working when the
    /// user denies remote input in the portal dialog.
    #[test]
    fn device_mask_grants_follows_portal_device_bits() {
        assert!(device_mask_grants(PORTAL_DEVICE_POINTER, Capability::ControlPointer));
        assert!(device_mask_grants(PORTAL_DEVICE_POINTER | PORTAL_DEVICE_KEYBOARD, Capability::ControlPointer));
        assert!(device_mask_grants(PORTAL_DEVICE_POINTER | PORTAL_DEVICE_KEYBOARD, Capability::ControlKeyboard));
        assert!(!device_mask_grants(PORTAL_DEVICE_POINTER, Capability::ControlKeyboard));
        assert!(!device_mask_grants(0, Capability::ControlPointer));
        assert!(!device_mask_grants(0, Capability::ControlKeyboard));
        // ViewScreen is not an input device and is never portal-granted.
        assert!(!device_mask_grants(PORTAL_DEVICE_POINTER | PORTAL_DEVICE_KEYBOARD, Capability::ViewScreen));
        assert!(!device_mask_grants(0, Capability::ViewScreen));
    }

    /// Parse the `devices` bitmask out of a RemoteDesktop Start response body
    /// (a `{ devices: u32 }` vardict). Missing/wrong-typed keys are None.
    #[test]
    fn parse_devices_mask_reads_start_response_body() {
        use zbus::zvariant::{Dict, Signature, Value};
        let mut dict = Dict::new(
            &Signature::try_from("s").unwrap(),
            &Signature::try_from("u").unwrap(),
        );
        dict.add("devices", 3u32).unwrap();
        assert_eq!(parse_devices_mask(&Value::Dict(dict)), Some(3));

        let mut only_pointer = Dict::new(
            &Signature::try_from("s").unwrap(),
            &Signature::try_from("u").unwrap(),
        );
        only_pointer.add("devices", 1u32).unwrap();
        assert_eq!(parse_devices_mask(&Value::Dict(only_pointer)), Some(1));

        let empty = Dict::new(
            &Signature::try_from("s").unwrap(),
            &Signature::try_from("u").unwrap(),
        );
        assert_eq!(parse_devices_mask(&Value::Dict(empty)), None);

        let mut wrong_type = Dict::new(
            &Signature::try_from("s").unwrap(),
            &Signature::try_from("s").unwrap(),
        );
        wrong_type.add("devices", "denied").unwrap();
        assert_eq!(parse_devices_mask(&Value::Dict(wrong_type)), None);

        // Not a dict at all.
        assert_eq!(parse_devices_mask(&Value::U32(3)), None);
    }
}
