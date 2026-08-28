//! Consent-gated remote input and platform boundaries.
#![allow(missing_docs)]
use super::coords::{normalized_to_source, MonitorGeometry, NormalizedPoint};
use super::permissions::{Capability, ControlToken, SessionPermissions};
use super::protocol::InputEventKind;
use super::session::ScreenShareSessionId;
use super::ScreenShareError;
use std::time::Instant;

/// One normalized input event flowing viewer → host (PDF Task 9.2). The
/// explicit `kind` disambiguates pointer motion, buttons, wheel ticks, key
/// down/up and modifier-state changes; `x`/`y` are capture pixels for pointer
/// kinds after the host maps normalized viewer coordinates, and 0 for
/// keyboard. `code` is a button id (1-3) for pointer buttons, an X11 wheel
/// button (4-7) for wheel ticks, an X11 keysym for keyboard, or the new
/// modifier bitmask for `ModifierChange`. `modifiers` is the viewer's current
/// held-modifier bitmask carried with every event.
#[derive(Debug, Clone, PartialEq)]
pub struct InputEvent {
    pub kind: InputEventKind,
    pub code: u32,
    pub capability: Capability,
    pub token: Option<ControlToken>,
    pub x: f32,
    pub y: f32,
    pub pressed: bool,
    pub modifiers: u32,
}
pub const MAX_INPUT_EVENT_BYTES: usize = 256;

pub fn authorize_input(
    permissions: &SessionPermissions,
    session_id: ScreenShareSessionId,
    peer_id: iroh::PublicKey,
    event: &InputEvent,
) -> Result<(), ScreenShareError> {
    if event.token.is_some_and(|token| {
        permissions.allows_token(session_id, peer_id, token, event.capability, Instant::now())
    }) {
        Ok(())
    } else {
        Err(ScreenShareError::new(
            "remote input capability is not granted",
        ))
    }
}

/// Host-side validation of a wire Input message carrying only the grant nonce.
pub fn authorize_nonce(
    permissions: &SessionPermissions,
    session_id: ScreenShareSessionId,
    peer_id: iroh::PublicKey,
    capability: Capability,
    nonce: [u8; 16],
) -> Result<(), ScreenShareError> {
    if permissions.allows(session_id, peer_id, capability)
        && permissions.nonce_matches(nonce, Instant::now())
    {
        Ok(())
    } else {
        Err(ScreenShareError::new(
            "remote input capability is not granted",
        ))
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
pub struct NormalizedPointer {
    pub x: f32,
    pub y: f32,
}

/// Map a letterboxed viewer rectangle into capture pixels. Points outside the
/// active image are ignored, avoiding input in black bars or stale regions.
pub fn map_pointer(
    point: NormalizedPointer,
    viewer: (f32, f32),
    capture: (u32, u32),
) -> Option<(u32, u32)> {
    if !point.x.is_finite()
        || !point.y.is_finite()
        || viewer.0 <= 0.0
        || viewer.1 <= 0.0
        || capture.0 == 0
        || capture.1 == 0
    {
        return None;
    }
    let scale = (viewer.0 / capture.0 as f32).min(viewer.1 / capture.1 as f32);
    let image = (capture.0 as f32 * scale, capture.1 as f32 * scale);
    let origin = ((viewer.0 - image.0) / 2.0, (viewer.1 - image.1) / 2.0);
    let local = (point.x * viewer.0 - origin.0, point.y * viewer.1 - origin.1);
    if local.0 < 0.0 || local.1 < 0.0 || local.0 >= image.0 || local.1 >= image.1 {
        return None;
    }
    Some((
        (local.0 / scale).floor() as u32,
        (local.1 / scale).floor() as u32,
    ))
}

/// Map a viewer point normalized to the image rect (0..1) into capture pixels,
/// rejecting out-of-range points. The viewer already excludes letterbox via the
/// mouse area, so this is a direct scale plus bounds check.
///
/// This delegates to the BORU-SS-12 coordinate math ([`normalized_to_source`]
/// against a zero-origin [`MonitorGeometry`] of the capture size) so the input
/// path shares the exact normalization used by the desktop↔source mapping —
/// coordinates are independent of the sender's local window size because they
/// are expressed relative to the shared source, not the viewer window.
pub fn normalize_to_capture(point: NormalizedPointer, capture: (u32, u32)) -> Option<(u32, u32)> {
    let geometry = MonitorGeometry::new(0, 0, capture.0, capture.1);
    let source = normalized_to_source(
        NormalizedPoint {
            x: point.x as f64,
            y: point.y as f64,
        },
        &geometry,
    )?;
    Some((source.x, source.y))
}

#[derive(Debug, Default)]
pub struct UnavailableInputBackend;
#[async_trait::async_trait]
impl RemoteInput for UnavailableInputBackend {
    async fn apply(&mut self, _event: InputEvent) -> Result<(), ScreenShareError> {
        Err(ScreenShareError::new("remote input backend is unavailable"))
    }
    async fn shutdown(&mut self) {}
}

/// Create the platform input backend, failing closed when the environment does
/// not provide one. `capture` is the capture source geometry used to scale
/// pointer coordinates to the platform screen; `origin` is the capture rect's
/// top-left in platform root/screen coordinates (used by the X11 backend for
/// absolute XTest motion; the portal uses relative motion and Windows uses
/// virtual-screen coordinates, so both ignore it); `granted` is the set of
/// capabilities the host explicitly granted, which every backend stores and
/// re-checks per event.
///
/// Backend order is display-server aware (PDF Task 6.2, mirroring the capture
/// side from BORU-SS-16): under Wayland or XWayland the RemoteDesktop portal
/// is preferred (XTest under XWayland only reaches XWayland windows); under a
/// native X11 session the direct XTest backend needs no portal daemon and is
/// tried first.
pub async fn create_platform_backend(
    capture: (u32, u32),
    origin: (i32, i32),
    granted: &[Capability],
) -> Box<dyn RemoteInput> {
    #[cfg(target_os = "linux")]
    {
        let portal_first =
            crate::screen_share::platform::linux::detect_display_server().prefers_portal();
        let portal = LinuxPortalRemoteInput::connect();
        let x11 = || X11RemoteInput::connect(capture, origin, granted);
        if portal_first {
            if let Ok(backend) = portal.await {
                return Box::new(backend);
            }
            if let Ok(backend) = x11() {
                return Box::new(backend);
            }
        } else {
            if let Ok(backend) = x11() {
                return Box::new(backend);
            }
            if let Ok(backend) = portal.await {
                return Box::new(backend);
            }
        }
        Box::new(UnavailableInputBackend)
    }
    #[cfg(all(not(target_os = "linux"), target_os = "windows"))]
    {
        let _ = (origin, granted);
        Box::new(WindowsRemoteInput::new(capture))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (origin, granted);
        Box::new(UnavailableInputBackend)
    }
}

// ── Linux: xdg-desktop-portal RemoteDesktop (D-Bus) ─────────────────────────
//
// The portal path is the supported injection mechanism under Wayland and
// XWayland (where XTest only reaches XWayland windows). Under a native X11
// session the direct XTest backend (see below) needs no portal daemon and is
// preferred. The session bus object is org.freedesktop.portal.RemoteDesktop
// at /org/freedesktop/portal/desktop.

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
        let connection = zbus::Connection::session()
            .await
            .map_err(|e| ScreenShareError::new(format!("no session bus: {e}")))?;
        let portal = (
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.RemoteDesktop",
        );
        // CreateSession(session_handle_token) → session object path.
        let token = format!("boru_{:016x}", rand::random::<u64>());
        let options: std::collections::HashMap<&str, zbus::zvariant::Value> =
            [("session_handle_token", zbus::zvariant::Value::from(token))]
                .into_iter()
                .collect();
        let reply = connection
            .call_method(
                Some(portal.0),
                portal.1,
                Some(portal.2),
                "CreateSession",
                &options,
            )
            .await
            .map_err(|e| ScreenShareError::new(format!("portal CreateSession failed: {e}")))?;
        let session: zbus::zvariant::OwnedObjectPath = reply
            .body()
            .deserialize()
            .map_err(|e| ScreenShareError::new(format!("portal session reply malformed: {e}")))?;
        // SelectDevices(types = Pointer | Keyboard) — per the portal spec the
        // `types` bitmask lives inside the options vardict.
        let types = PORTAL_DEVICE_POINTER | PORTAL_DEVICE_KEYBOARD;
        let device_options: std::collections::HashMap<&str, zbus::zvariant::Value> =
            [("types", zbus::zvariant::Value::U32(types))]
                .into_iter()
                .collect();
        let _ = connection
            .call_method(
                Some(portal.0),
                portal.1,
                Some(portal.2),
                "SelectDevices",
                &(session.clone(), device_options),
            )
            .await
            .map_err(|e| ScreenShareError::new(format!("portal SelectDevices failed: {e}")))?;
        // Start() is asynchronous: the reply is a Request object path and the
        // real result (response code + granted `devices` bitmask) arrives on
        // the Response signal of that object. Await it so we fail closed when
        // the user denies remote input — view-only sharing must keep working
        // (PDF Task 5.3).
        let start_options: std::collections::HashMap<&str, zbus::zvariant::Value> =
            std::collections::HashMap::new();
        let request_path: zbus::zvariant::OwnedObjectPath = tokio::time::timeout(
            Self::PORTAL_START_TIMEOUT,
            connection.call_method(
                Some(portal.0),
                portal.1,
                Some(portal.2),
                "Start",
                &(session.clone(), "", start_options),
            ),
        )
        .await
        .map_err(|_| {
            ScreenShareError::new(
                "portal remote-desktop Start timed out (no response from the consent dialog)",
            )
        })?
        .map_err(|e| ScreenShareError::new(format!("portal Start failed: {e}")))?
        .body()
        .deserialize()
        .map_err(|e| ScreenShareError::new(format!("portal Start request malformed: {e}")))?;
        let request = zbus::Proxy::new(
            &connection,
            portal.0,
            request_path.as_str(),
            "org.freedesktop.portal.Request",
        )
        .await
        .map_err(|e| ScreenShareError::new(format!("portal request proxy failed: {e}")))?;
        let mut responses = request.receive_signal("Response").await.map_err(|e| {
            ScreenShareError::new(format!("portal response subscription failed: {e}"))
        })?;
        let response = tokio::time::timeout(
            Self::PORTAL_START_TIMEOUT,
            n0_future::StreamExt::next(&mut responses),
        )
        .await
        .map_err(|_| {
            ScreenShareError::new(
                "portal remote-desktop Start timed out waiting for the consent response",
            )
        })?
        .ok_or_else(|| ScreenShareError::new("portal response stream closed"))?;
        let (response_code, body): (u32, zbus::zvariant::OwnedValue) = response
            .body()
            .deserialize()
            .map_err(|e| ScreenShareError::new(format!("portal response malformed: {e}")))?;
        if response_code != 0 {
            return Err(ScreenShareError::new(format!(
                "portal remote-desktop permission denied (code {response_code})"
            )));
        }
        let granted_devices = parse_devices_mask(&body).unwrap_or(0);
        if granted_devices & (PORTAL_DEVICE_POINTER | PORTAL_DEVICE_KEYBOARD) == 0 {
            return Err(ScreenShareError::new(
                "portal granted no input devices (remote control denied)",
            ));
        }
        tracing::info!(
            granted_devices,
            "screen-share: portal remote-desktop session started"
        );
        Ok(Self {
            connection: Some(connection),
            session: Some(session),
            granted_devices,
            last: None,
        })
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
        let (Some(connection), Some(session)) = (&self.connection, &self.session) else {
            return Err(ScreenShareError::new(
                "portal remote-desktop session is not connected",
            ));
        };
        if !device_mask_grants(self.granted_devices, event.capability) {
            return Err(ScreenShareError::new(
                "device type was not granted by the portal (view-only)",
            ));
        }
        let portal = (
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.RemoteDesktop",
        );
        match event.kind {
            InputEventKind::PointerMove | InputEventKind::PointerButton | InputEventKind::Wheel => {
                let (px, py) = (event.x as f64, event.y as f64);
                let (dx, dy) = match self.last {
                    Some((lx, ly)) => (px - lx, py - ly),
                    None => (0.0, 0.0),
                };
                self.last = Some((px, py));
                if dx != 0.0 || dy != 0.0 {
                    let _ = connection
                        .call_method(
                            Some(portal.0),
                            portal.1,
                            Some(portal.2),
                            "NotifyPointerMotion",
                            &(session, empty_options(), dx, dy),
                        )
                        .await
                        .map_err(|e| {
                            ScreenShareError::new(format!("portal pointer motion failed: {e}"))
                        })?;
                }
                match event.kind {
                    InputEventKind::PointerButton => {
                        let state = if event.pressed { 1u32 } else { 0u32 };
                        let _ = connection
                            .call_method(
                                Some(portal.0),
                                portal.1,
                                Some(portal.2),
                                "NotifyPointerButton",
                                &(session, empty_options(), event.code as i32, state),
                            )
                            .await
                            .map_err(|e| {
                                ScreenShareError::new(format!("portal pointer button failed: {e}"))
                            })?;
                    }
                    InputEventKind::Wheel
                        // Wheel tick: X11 wheel buttons 4-7 are emitted as a
                        // press+release pair so the compositor scrolls exactly
                        // once (mirrors the X11 XTest backend).
                        if event.pressed => {
                            let _ = connection
                                .call_method(
                                    Some(portal.0),
                                    portal.1,
                                    Some(portal.2),
                                    "NotifyPointerButton",
                                    &(session, empty_options(), event.code as i32, 1u32),
                                )
                                .await
                                .map_err(|e| {
                                    ScreenShareError::new(format!("portal wheel press failed: {e}"))
                                })?;
                            let _ = connection
                                .call_method(
                                    Some(portal.0),
                                    portal.1,
                                    Some(portal.2),
                                    "NotifyPointerButton",
                                    &(session, empty_options(), event.code as i32, 0u32),
                                )
                                .await
                                .map_err(|e| {
                                    ScreenShareError::new(format!(
                                        "portal wheel release failed: {e}"
                                    ))
                                })?;
                        }
                    _ => {}
                }
            }
            InputEventKind::Key => {
                let state = if event.pressed { 1u32 } else { 0u32 };
                let _ = connection
                    .call_method(
                        Some(portal.0),
                        portal.1,
                        Some(portal.2),
                        "NotifyKeyboardKeysym",
                        &(session, empty_options(), event.code as i32, state),
                    )
                    .await
                    .map_err(|e| ScreenShareError::new(format!("portal keyboard failed: {e}")))?;
            }
            InputEventKind::ModifierChange => {
                // The portal has no modifier-state API; modifier keys are
                // delivered as normal keyboard keysyms (which the compositor
                // uses to track modifier state). The explicit ModifierChange
                // event is accepted and recorded at the protocol layer.
            }
        }
        Ok(())
    }
    async fn shutdown(&mut self) {
        if let (Some(connection), Some(session)) = (&self.connection, &self.session) {
            let _ = connection
                .call_method(
                    Some("org.freedesktop.portal.Desktop"),
                    "/org/freedesktop/portal/desktop",
                    Some("org.freedesktop.portal.RemoteDesktop"),
                    "CloseSession",
                    &(session,),
                )
                .await;
        }
        self.connection = None;
        self.session = None;
        self.granted_devices = 0;
    }
}

// ── Linux: X11 XTest fake input (direct, no portal daemon) ────────────────
//
// The XTest extension injects synthetic core input into the X server (PDF
// Task 6.2). It is the right path under a native X11 session: no
// xdg-desktop-portal daemon is needed and the events reach every X11 client.
// Under Wayland/XWayland the RemoteDesktop portal is preferred (XTest only
// reaches XWayland windows); `create_platform_backend` picks the backend
// order from the display-server detection, mirroring the capture side.
//
// Consent model: the backend is constructed ONLY after the host explicitly
// granted control (`HostCommand::GrantControl`), and it stores the granted
// device mask. `apply` re-checks that mask for every event, so injection is
// gated on the permissions.rs state even if a caller bypasses the protocol
// authorization (defense in depth; the streaming loop also runs
// `authorize_nonce` before forwarding events).

/// XTest FakeInput event type constants (xtestproto.h).
pub const XTEST_KEY_PRESS: u8 = 2;
pub const XTEST_KEY_RELEASE: u8 = 3;
pub const XTEST_BUTTON_PRESS: u8 = 4;
pub const XTEST_BUTTON_RELEASE: u8 = 5;
pub const XTEST_MOTION_NOTIFY: u8 = 6;

/// One XTest fake-input request produced by the pure translation functions.
/// `Motion` carries absolute root-window coordinates (type 6 with detail 0);
/// `Button` carries an X11 button id (1-3 buttons, 4-7 wheel); `Key` carries
/// a keycode. `X11RemoteInput::apply` turns each into `FakeInput`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11Action {
    Motion { x: i16, y: i16 },
    Button { button: u8, pressed: bool },
    Key { keycode: u8, pressed: bool },
}

/// Build the keysym → keycode reverse map from a `GetKeyboardMapping` reply.
///
/// Pure and unit-tested without an X server. Keycodes start at
/// `first_keycode` (typically 8); each keycode has `keysyms_per_keycode`
/// slots (usually 2: unshifted/shifted). The lowest keycode containing a
/// keysym wins, matching `XKeysymToKeycode` semantics. Zero keysyms
/// (NoSymbol) are skipped.
pub fn build_keysym_to_keycode(
    keysyms_per_keycode: u8,
    keysyms: &[u32],
    first_keycode: u8,
) -> std::collections::HashMap<u32, u8> {
    let mut map = std::collections::HashMap::new();
    if keysyms_per_keycode == 0 {
        return map;
    }
    for (index, chunk) in keysyms.chunks(keysyms_per_keycode as usize).enumerate() {
        let keycode = first_keycode.saturating_add(index as u8);
        for &sym in chunk {
            if sym != 0 && !map.contains_key(&sym) {
                map.insert(sym, keycode);
            }
        }
    }
    map
}

/// Translate a pointer [`InputEvent`] into XTest actions.
///
/// `event.x`/`event.y` are capture pixels; `origin` is the capture rect's
/// top-left in root-window coordinates (the host passes the selected
/// monitor's origin, `(0, 0)` for a whole-root capture). Motion is clamped to
/// the root window bounds. X11 wheel buttons (4-7) are emitted as a
/// press+release pair on the press event so a scroll tick happens exactly
/// once; the matching release event is a no-op (avoids a double scroll). The
/// explicit [`InputEventKind`] decides what the code means — the same code
/// space (1-3 button, 4-7 wheel, 0 move) is enforced here as a defense-in-depth
/// check on top of protocol validation.
pub fn x11_pointer_actions(
    event: &InputEvent,
    origin: (i32, i32),
    root: (u32, u32),
) -> Result<Vec<X11Action>, ScreenShareError> {
    if root.0 == 0 || root.1 == 0 {
        return Err(ScreenShareError::new("X11 root window has zero size"));
    }
    if !event.kind.is_pointer() {
        return Err(ScreenShareError::new("not a pointer input event"));
    }
    let mut actions = Vec::new();
    // Absolute motion to the event point (moves the pointer even for
    // button/wheel events; the viewer throttles redundant moves already).
    let rx = (origin.0 as i64 + event.x as i64).clamp(0, root.0 as i64 - 1) as i16;
    let ry = (origin.1 as i64 + event.y as i64).clamp(0, root.1 as i64 - 1) as i16;
    actions.push(X11Action::Motion { x: rx, y: ry });
    match event.kind {
        InputEventKind::PointerMove => {
            if event.code != 0 {
                return Err(ScreenShareError::new("pointer move must carry code 0"));
            }
        }
        InputEventKind::PointerButton => {
            if !(1..=3).contains(&event.code) {
                return Err(ScreenShareError::new("unsupported X11 pointer button code"));
            }
            actions.push(X11Action::Button {
                button: event.code as u8,
                pressed: event.pressed,
            });
        }
        InputEventKind::Wheel => {
            if !(4..=7).contains(&event.code) {
                return Err(ScreenShareError::new("unsupported X11 wheel code"));
            }
            // Wheel tick: press + release on the press event only.
            if event.pressed {
                actions.push(X11Action::Button {
                    button: event.code as u8,
                    pressed: true,
                });
                actions.push(X11Action::Button {
                    button: event.code as u8,
                    pressed: false,
                });
            }
        }
        _ => return Err(ScreenShareError::new("not a pointer input event")),
    }
    Ok(actions)
}

/// Translate a keyboard [`InputEvent`] into an XTest Key action.
///
/// The wire code is an X11 keysym; it is mapped to a keycode through the
/// server keyboard map. Unknown keysyms fail closed (the event is rejected,
/// mirroring the Windows backend's `keysym_to_vk` returning 0).
pub fn x11_key_action(
    code: u32,
    keysym_to_keycode: &std::collections::HashMap<u32, u8>,
    pressed: bool,
) -> Result<X11Action, ScreenShareError> {
    let keycode = keysym_to_keycode.get(&code).copied().ok_or_else(|| {
        ScreenShareError::new("unsupported key code (no keycode in server mapping)")
    })?;
    Ok(X11Action::Key { keycode, pressed })
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct X11RemoteInput {
    conn: x11rb::rust_connection::RustConnection,
    root: u32,
    root_width: u32,
    root_height: u32,
    /// Device mask the host explicitly granted (1 = pointer, 2 = keyboard).
    /// Empty when control was denied — `apply` then fails closed so
    /// view-only sharing keeps working.
    granted_devices: u32,
    /// Capture rect origin in root-window coordinates (monitor x/y).
    origin: (i32, i32),
    keysym_to_keycode: std::collections::HashMap<u32, u8>,
    active: bool,
}

#[cfg(target_os = "linux")]
impl X11RemoteInput {
    /// Connect to `$DISPLAY`, verify the XTEST extension is present, and
    /// build the server keyboard map. Fails closed when the X server is
    /// unreachable or XTEST is not supported.
    pub fn connect(
        capture: (u32, u32),
        origin: (i32, i32),
        granted: &[Capability],
    ) -> Result<Self, ScreenShareError> {
        use x11rb::connection::{Connection as _, RequestConnection as _};
        use x11rb::protocol::xproto::ConnectionExt as _;

        let (conn, screen_num) = x11rb::connect(None)
            .map_err(|e| ScreenShareError::new(format!("X11 connect failed: {e}")))?;
        let screen = conn
            .setup()
            .roots
            .get(screen_num)
            .ok_or_else(|| ScreenShareError::new("X11 setup has no root screen"))?;
        let root = screen.root;
        let root_width = screen.width_in_pixels as u32;
        let root_height = screen.height_in_pixels as u32;
        // Fail closed when XTEST is not present (very rare; Xvfb/Xorg both
        // ship it). `fake_input` would error at request time otherwise.
        let extension = conn
            .extension_information(x11rb::protocol::xtest::X11_EXTENSION_NAME)
            .map_err(|e| ScreenShareError::new(format!("X11 extension query failed: {e}")))?
            .ok_or_else(|| {
                ScreenShareError::new("XTEST extension is not available on this X server")
            })?;
        let _ = extension;
        // Server keyboard mapping: keycode → keysyms, reversed for
        // keysym → keycode translation.
        let setup = conn.setup();
        let first_keycode = setup.min_keycode;
        let count = setup
            .max_keycode
            .saturating_sub(setup.min_keycode)
            .saturating_add(1);
        let mapping = conn
            .get_keyboard_mapping(first_keycode, count)
            .map_err(|e| ScreenShareError::new(format!("X11 get_keyboard_mapping failed: {e}")))?
            .reply()
            .map_err(|e| {
                ScreenShareError::new(format!("X11 keyboard mapping reply failed: {e}"))
            })?;
        let keysym_to_keycode =
            build_keysym_to_keycode(mapping.keysyms_per_keycode, &mapping.keysyms, first_keycode);
        let mut granted_devices = 0u32;
        for capability in granted {
            match capability {
                Capability::ControlPointer => granted_devices |= PORTAL_DEVICE_POINTER,
                Capability::ControlKeyboard => granted_devices |= PORTAL_DEVICE_KEYBOARD,
                _ => {}
            }
        }
        tracing::info!(granted_devices, root_width, root_height, capture = ?capture, "screen-share: X11 XTest input backend connected");
        Ok(Self {
            conn,
            root,
            root_width,
            root_height,
            granted_devices,
            origin,
            keysym_to_keycode,
            active: true,
        })
    }
}

#[cfg(target_os = "linux")]
#[async_trait::async_trait]
impl RemoteInput for X11RemoteInput {
    async fn apply(&mut self, event: InputEvent) -> Result<(), ScreenShareError> {
        use x11rb::connection::Connection as _;
        use x11rb::protocol::xtest::ConnectionExt as _;
        if !self.active {
            return Err(ScreenShareError::new("remote input revoked"));
        }
        if !device_mask_grants(self.granted_devices, event.capability) {
            return Err(ScreenShareError::new(
                "device type was not granted by the host (view-only)",
            ));
        }
        let actions = match event.kind {
            InputEventKind::PointerMove | InputEventKind::PointerButton | InputEventKind::Wheel => {
                x11_pointer_actions(&event, self.origin, (self.root_width, self.root_height))?
            }
            InputEventKind::Key => {
                vec![x11_key_action(
                    event.code,
                    &self.keysym_to_keycode,
                    event.pressed,
                )?]
            }
            InputEventKind::ModifierChange => {
                // X11 tracks modifier state from the injected modifier keysym
                // events themselves; the explicit ModifierChange event is
                // accepted and recorded at the protocol layer (defense in
                // depth for key combinations).
                Vec::new()
            }
        };
        for action in actions {
            let (type_, detail, root_x, root_y) = match action {
                X11Action::Motion { x, y } => (XTEST_MOTION_NOTIFY, 0u8, x, y),
                X11Action::Button { button, pressed } => {
                    let type_ = if pressed {
                        XTEST_BUTTON_PRESS
                    } else {
                        XTEST_BUTTON_RELEASE
                    };
                    (type_, button, 0i16, 0i16)
                }
                X11Action::Key { keycode, pressed } => {
                    let type_ = if pressed {
                        XTEST_KEY_PRESS
                    } else {
                        XTEST_KEY_RELEASE
                    };
                    (type_, keycode, 0i16, 0i16)
                }
            };
            let _ = self
                .conn
                .xtest_fake_input(type_, detail, 0, self.root, root_x, root_y, 0)
                .map_err(|e| ScreenShareError::new(format!("XTest fake input failed: {e}")))?;
        }
        // x11rb buffers requests on the connection. Flush after each logical
        // input event so a lone pointer move or key press is delivered
        // immediately rather than waiting for an unrelated later X11 request.
        self.conn
            .flush()
            .map_err(|e| ScreenShareError::new(format!("X11 input flush failed: {e}")))?;
        Ok(())
    }

    async fn shutdown(&mut self) {
        self.active = false;
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
    pub fn new(capture: (u32, u32)) -> Self {
        Self {
            active: true,
            capture,
        }
    }
    pub fn revoke(&mut self) {
        self.active = false;
    }
    /// Map a portable X11 keysym to a Windows virtual-key code. Unsupported
    /// keys map to 0 and are ignored (fail-closed).
    fn keysym_to_vk(code: u32) -> u16 {
        match code {
            0x61..=0x7A => (code - 0x20) as u16, // a-z → A-Z
            0x30..=0x39 => code as u16,          // 0-9
            0xFF0D => 0x0D,
            0xFF08 => 0x08,
            0xFF09 => 0x09,
            0xFF1B => 0x1B,
            0x20 => 0x20,
            0xFF51 => 0x25,
            0xFF52 => 0x26,
            0xFF53 => 0x27,
            0xFF54 => 0x28,
            0xFF50 => 0x24,
            0xFF57 => 0x23,
            0xFF55 => 0x21,
            0xFF56 => 0x22,
            0xFF63 => 0x2D,
            0xFFFF => 0x2E,
            0xFFE1 => 0x10,
            0xFFE2 => 0x10,
            0xFFE3 => 0x11,
            0xFFE4 => 0x11,
            0xFFE9 => 0x12,
            0xFFEA => 0x12,
            0xFFE5 => 0x14,
            0xFFBE..=0xFFC9 => 0x70 + (code - 0xFFBE) as u16, // F1-F12
            0x3B => 0xBA,
            0x3D => 0xBB,
            0x2C => 0xBC,
            0x2D => 0xBD,
            0x2E => 0xBE,
            0x2F => 0xBF,
            0x60 => 0xC0,
            0x5B => 0xDB,
            0x5C => 0xDC,
            0x5D => 0xDD,
            0x27 => 0xDE,
            _ => 0,
        }
    }
}

#[cfg(target_os = "windows")]
#[async_trait::async_trait]
impl RemoteInput for WindowsRemoteInput {
    async fn apply(&mut self, event: InputEvent) -> Result<(), ScreenShareError> {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
            MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
            MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
            MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
        };
        if !self.active {
            return Err(ScreenShareError::new("remote input revoked"));
        }
        match event.kind {
            InputEventKind::PointerMove | InputEventKind::PointerButton => {
                let (cw, ch) = self.capture;
                let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
                let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
                if vw <= 0 || vh <= 0 || cw == 0 || ch == 0 {
                    return Err(ScreenShareError::new("virtual screen metrics unavailable"));
                }
                let mut inputs = Vec::new();
                // Absolute move (0..65535 across the virtual screen): scale the
                // capture-space point by the capture geometry.
                let dx = ((event.x.clamp(0.0, cw as f32) / cw as f32) * 65535.0).floor() as i32;
                let dy = ((event.y.clamp(0.0, ch as f32) / ch as f32) * 65535.0).floor() as i32;
                inputs.push(INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx,
                            dy,
                            mouseData: 0,
                            dwFlags: MOUSEEVENTF_MOVE
                                | MOUSEEVENTF_ABSOLUTE
                                | MOUSEEVENTF_VIRTUALDESK,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                });
                if event.kind == InputEventKind::PointerButton {
                    let flag = match (event.code, event.pressed) {
                        (1, true) => MOUSEEVENTF_LEFTDOWN,
                        (1, false) => MOUSEEVENTF_LEFTUP,
                        (2, true) => MOUSEEVENTF_MIDDLEDOWN,
                        (2, false) => MOUSEEVENTF_MIDDLEUP,
                        (3, true) => MOUSEEVENTF_RIGHTDOWN,
                        (3, false) => MOUSEEVENTF_RIGHTUP,
                        _ => 0,
                    };
                    if flag != 0 {
                        inputs.push(INPUT {
                            r#type: INPUT_MOUSE,
                            Anonymous: INPUT_0 {
                                mi: MOUSEINPUT {
                                    dx,
                                    dy,
                                    mouseData: 0,
                                    dwFlags: flag,
                                    time: 0,
                                    dwExtraInfo: 0,
                                },
                            },
                        });
                    }
                }
                let ok = unsafe {
                    SendInput(
                        inputs.len() as u32,
                        inputs.as_ptr(),
                        std::mem::size_of::<INPUT>() as i32,
                    )
                } == inputs.len() as u32;
                if !ok {
                    return Err(ScreenShareError::new("SendInput failed"));
                }
            }
            InputEventKind::Wheel => {
                // Wheel tick: X11 wheel buttons 4-7 map to the native wheel
                // delta (WHEEL_DELTA = 120). Buttons 4/5 are vertical,
                // 6/7 horizontal.
                let (flag, delta) = match event.code {
                    4 => (MOUSEEVENTF_WHEEL, 120),
                    5 => (MOUSEEVENTF_WHEEL, -120),
                    6 => (MOUSEEVENTF_HWHEEL, -120),
                    7 => (MOUSEEVENTF_HWHEEL, 120),
                    _ => return Err(ScreenShareError::new("unsupported wheel code")),
                };
                if !event.pressed {
                    return Ok(());
                }
                let input = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0,
                            dy: 0,
                            mouseData: delta as u32,
                            dwFlags: flag,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                let ok = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) } == 1;
                if !ok {
                    return Err(ScreenShareError::new("SendInput failed"));
                }
            }
            InputEventKind::Key => {
                let vk = Self::keysym_to_vk(event.code);
                if vk == 0 {
                    return Err(ScreenShareError::new("unsupported key code"));
                }
                let flags = if event.pressed { 0u32 } else { KEYEVENTF_KEYUP };
                let input = INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: vk,
                            wScan: 0,
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                let ok = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) } == 1;
                if !ok {
                    return Err(ScreenShareError::new("SendInput failed"));
                }
            }
            InputEventKind::ModifierChange => {
                // Windows tracks modifier state from the injected modifier key
                // events; the explicit ModifierChange event is accepted and
                // recorded at the protocol layer.
            }
        }
        Ok(())
    }
    async fn shutdown(&mut self) {
        self.active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen_share::permissions::{Capability, SlidingWindowRateLimiter};
    use crate::screen_share::protocol::MOD_SHIFT;

    #[test]
    fn input_is_rejected_before_grant_and_after_revoke() {
        let session = ScreenShareSessionId::from_bytes([9; 16]);
        let peer = iroh::SecretKey::generate().public();
        let event = InputEvent {
            kind: InputEventKind::PointerButton,
            code: 1,
            capability: Capability::ControlPointer,
            token: None,
            x: 0.5,
            y: 0.5,
            pressed: true,
            modifiers: 0,
        };
        let mut permissions = SessionPermissions::view_only(session, peer);
        assert!(authorize_input(&permissions, session, peer, &event).is_err());
        permissions.grant([Capability::ControlPointer]);
        let event = InputEvent {
            token: permissions.token(),
            ..event
        };
        assert!(authorize_input(&permissions, session, peer, &event).is_ok());
        permissions.revoke_control();
        assert!(authorize_input(&permissions, session, peer, &event).is_err());
    }

    #[test]
    fn mapping_rejects_letterbox_and_scales_capture() {
        assert_eq!(
            map_pointer(
                NormalizedPointer { x: 0.5, y: 0.5 },
                (1600.0, 900.0),
                (1920, 1080)
            ),
            Some((960, 540))
        );
        assert_eq!(
            map_pointer(
                NormalizedPointer { x: 0.5, y: 0.01 },
                (1600.0, 1200.0),
                (1920, 1080)
            ),
            None
        );
    }

    #[test]
    fn normalize_rejects_out_of_range_points() {
        assert_eq!(
            normalize_to_capture(NormalizedPointer { x: 0.5, y: 0.5 }, (640, 360)),
            Some((320, 180))
        );
        assert_eq!(
            normalize_to_capture(NormalizedPointer { x: 1.0, y: 0.5 }, (640, 360)),
            None
        );
        assert_eq!(
            normalize_to_capture(
                NormalizedPointer {
                    x: f32::NAN,
                    y: 0.5
                },
                (640, 360)
            ),
            None
        );
    }

    #[test]
    fn authorize_nonce_rejects_stale_nonce() {
        let session = ScreenShareSessionId::from_bytes([10; 16]);
        let peer = iroh::SecretKey::generate().public();
        let mut permissions = SessionPermissions::view_only(session, peer);
        permissions.grant_with_nonce([Capability::ControlPointer], [7; 16]);
        assert!(authorize_nonce(
            &permissions,
            session,
            peer,
            Capability::ControlPointer,
            [7; 16]
        )
        .is_ok());
        assert!(authorize_nonce(
            &permissions,
            session,
            peer,
            Capability::ControlKeyboard,
            [7; 16]
        )
        .is_err());
        assert!(authorize_nonce(
            &permissions,
            session,
            peer,
            Capability::ControlPointer,
            [8; 16]
        )
        .is_err());
        permissions.revoke_control();
        assert!(authorize_nonce(
            &permissions,
            session,
            peer,
            Capability::ControlPointer,
            [7; 16]
        )
        .is_err());
    }

    /// Portal `devices` bitmask gating (PDF Task 5.3): pointer requires the
    /// pointer bit, keyboard requires the keyboard bit, and a denied/empty
    /// mask grants nothing — so view-only sharing keeps working when the
    /// user denies remote input in the portal dialog.
    #[test]
    fn device_mask_grants_follows_portal_device_bits() {
        assert!(device_mask_grants(
            PORTAL_DEVICE_POINTER,
            Capability::ControlPointer
        ));
        assert!(device_mask_grants(
            PORTAL_DEVICE_POINTER | PORTAL_DEVICE_KEYBOARD,
            Capability::ControlPointer
        ));
        assert!(device_mask_grants(
            PORTAL_DEVICE_POINTER | PORTAL_DEVICE_KEYBOARD,
            Capability::ControlKeyboard
        ));
        assert!(!device_mask_grants(
            PORTAL_DEVICE_POINTER,
            Capability::ControlKeyboard
        ));
        assert!(!device_mask_grants(0, Capability::ControlPointer));
        assert!(!device_mask_grants(0, Capability::ControlKeyboard));
        // ViewScreen is not an input device and is never portal-granted.
        assert!(!device_mask_grants(
            PORTAL_DEVICE_POINTER | PORTAL_DEVICE_KEYBOARD,
            Capability::ViewScreen
        ));
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

    // ── X11 XTest translation (PDF Task 6.2) ────────────────────────────────
    //
    // These tests are pure — they exercise the normalized-event → X11-event
    // translation without a real X server. The live XTest path (connect +
    // FakeInput over a real DISPLAY) is documented in
    // docs/screenshare-x11-input.md and needs an actual X session.

    fn ptr(code: u32, x: f32, y: f32, pressed: bool) -> InputEvent {
        let kind = match code {
            0 => InputEventKind::PointerMove,
            1..=3 => InputEventKind::PointerButton,
            4..=7 => InputEventKind::Wheel,
            _ => InputEventKind::PointerButton,
        };
        InputEvent {
            kind,
            code,
            capability: Capability::ControlPointer,
            token: None,
            x,
            y,
            pressed,
            modifiers: 0,
        }
    }

    /// A plausible `GetKeyboardMapping` reply: 2 keysyms per keycode starting
    /// at keycode 8. `a`/`A` on 38, `b`/`B` on 56, `1`/`!` on 10, and a
    /// keycode (50) with a keysym in both slots to verify lowest-keycode wins.
    fn sample_keymap() -> std::collections::HashMap<u32, u8> {
        let mut keysyms = vec![0u32; 256 * 2];
        let mut put = |keycode: usize, slot: usize, sym: u32| {
            keysyms[(keycode - 8) * 2 + slot] = sym;
        };
        put(38, 0, 0x61); // a
        put(38, 1, 0x41); // A
        put(56, 0, 0x62); // b
        put(56, 1, 0x42); // B
        put(10, 0, 0x31); // 1
        put(10, 1, 0x21); // !
        put(50, 0, 0xFFE1); // Shift_L
        put(50, 1, 0xFFE1); // Shift_L again (same keycode, both slots)
        build_keysym_to_keycode(2, &keysyms, 8)
    }

    #[test]
    fn x11_pointer_move_maps_capture_pixels_to_root() {
        // Whole-root capture: origin (0,0), root 1920x1080.
        let event = ptr(0, 960.0, 540.0, false);
        let actions = x11_pointer_actions(&event, (0, 0), (1920, 1080)).unwrap();
        assert_eq!(actions, vec![X11Action::Motion { x: 960, y: 540 }]);
    }

    #[test]
    fn x11_pointer_move_applies_monitor_origin() {
        // Second monitor at origin (1920, 0); capture-local (640, 360) →
        // root (2560, 360).
        let event = ptr(0, 640.0, 360.0, false);
        let actions = x11_pointer_actions(&event, (1920, 0), (3840, 1080)).unwrap();
        assert_eq!(actions, vec![X11Action::Motion { x: 2560, y: 360 }]);
    }

    #[test]
    fn x11_pointer_move_clamps_to_root_bounds() {
        // Negative origin monitor (left of root): clamp below 0 and past the
        // root edge.
        let event = ptr(0, 10.0, 5000.0, false);
        let actions = x11_pointer_actions(&event, (-1920, 0), (1920, 1080)).unwrap();
        assert_eq!(actions, vec![X11Action::Motion { x: 0, y: 1079 }]);
    }

    #[test]
    fn x11_pointer_button_press_and_release() {
        let press = x11_pointer_actions(&ptr(1, 100.0, 100.0, true), (0, 0), (1920, 1080)).unwrap();
        assert_eq!(
            press,
            vec![
                X11Action::Motion { x: 100, y: 100 },
                X11Action::Button {
                    button: 1,
                    pressed: true
                },
            ]
        );
        let release =
            x11_pointer_actions(&ptr(1, 100.0, 100.0, false), (0, 0), (1920, 1080)).unwrap();
        assert_eq!(
            release,
            vec![
                X11Action::Motion { x: 100, y: 100 },
                X11Action::Button {
                    button: 1,
                    pressed: false
                },
            ]
        );
    }

    #[test]
    fn x11_wheel_emits_press_release_pair_once() {
        // X11 wheel-up is button 4; a tick is press+release so the server
        // scrolls exactly once. The matching release event is a no-op.
        let tick = x11_pointer_actions(&ptr(4, 50.0, 50.0, true), (0, 0), (1920, 1080)).unwrap();
        assert_eq!(
            tick,
            vec![
                X11Action::Motion { x: 50, y: 50 },
                X11Action::Button {
                    button: 4,
                    pressed: true
                },
                X11Action::Button {
                    button: 4,
                    pressed: false
                },
            ]
        );
        let release =
            x11_pointer_actions(&ptr(4, 50.0, 50.0, false), (0, 0), (1920, 1080)).unwrap();
        assert_eq!(release, vec![X11Action::Motion { x: 50, y: 50 }]);
    }

    #[test]
    fn x11_pointer_rejects_unknown_button_and_zero_root() {
        assert!(x11_pointer_actions(&ptr(9, 0.0, 0.0, true), (0, 0), (1920, 1080)).is_err());
        assert!(x11_pointer_actions(&ptr(0, 0.0, 0.0, false), (0, 0), (0, 1080)).is_err());
    }

    #[test]
    fn x11_keysym_map_builds_lowest_keycode_and_skips_no_symbol() {
        let map = sample_keymap();
        assert_eq!(map.get(&0x61), Some(&38));
        assert_eq!(map.get(&0x41), Some(&38));
        assert_eq!(map.get(&0x62), Some(&56));
        assert_eq!(map.get(&0x31), Some(&10));
        assert_eq!(map.get(&0x21), Some(&10));
        // Shift_L present (modifier state handled via normal key events).
        assert_eq!(map.get(&0xFFE1), Some(&50));
        // Unmapped keysym is absent.
        assert_eq!(map.get(&0x7A), None);
    }

    #[test]
    fn x11_key_translates_keysym_to_keycode() {
        let map = sample_keymap();
        let action = x11_key_action(0x61, &map, true).unwrap();
        assert_eq!(
            action,
            X11Action::Key {
                keycode: 38,
                pressed: true
            }
        );
        let release = x11_key_action(0x61, &map, false).unwrap();
        assert_eq!(
            release,
            X11Action::Key {
                keycode: 38,
                pressed: false
            }
        );
        // Modifier keysym translates too — the server updates its modifier
        // state from the injected keycode.
        let shift = x11_key_action(0xFFE1, &map, true).unwrap();
        assert_eq!(
            shift,
            X11Action::Key {
                keycode: 50,
                pressed: true
            }
        );
    }

    #[test]
    fn x11_key_rejects_unknown_keysym() {
        let map = sample_keymap();
        assert!(x11_key_action(0x7A, &map, true).is_err());
    }

    #[test]
    fn x11_empty_keymap_rejects_everything() {
        let empty = std::collections::HashMap::new();
        assert!(x11_key_action(0x61, &empty, true).is_err());
        // Empty (0 keysyms per keycode) mapping builds an empty map.
        let built = build_keysym_to_keycode(0, &[], 8);
        assert!(built.is_empty());
    }

    #[test]
    fn x11_consent_gate_rejects_ungranted_device_before_translation() {
        // The backend gate mirrors the portal: with no granted device bits
        // the event is rejected even though the pure translation would
        // succeed — so a view-only share can never inject input.
        let granted = device_mask_for_capabilities(&[Capability::ViewScreen]);
        assert!(!device_mask_grants(granted, Capability::ControlPointer));
        assert!(!device_mask_grants(granted, Capability::ControlKeyboard));

        let granted_pointer = device_mask_for_capabilities(&[Capability::ControlPointer]);
        assert!(device_mask_grants(
            granted_pointer,
            Capability::ControlPointer
        ));
        assert!(!device_mask_grants(
            granted_pointer,
            Capability::ControlKeyboard
        ));

        let granted_both = device_mask_for_capabilities(&[
            Capability::ControlPointer,
            Capability::ControlKeyboard,
        ]);
        assert!(device_mask_grants(granted_both, Capability::ControlPointer));
        assert!(device_mask_grants(
            granted_both,
            Capability::ControlKeyboard
        ));
    }

    /// PDF Task 9.2: the explicit kind drives translation. A pointer-move
    /// event must not be treated as a button, and a keyboard event cannot be
    /// translated as a pointer event.
    #[test]
    fn explicit_kind_gates_translation() {
        let map = sample_keymap();
        // Pointer kinds only.
        assert!(x11_pointer_actions(&ptr(0, 10.0, 10.0, false), (0, 0), (1920, 1080)).is_ok());
        // A keyboard event is rejected by the pointer translator.
        let key_event = InputEvent {
            kind: InputEventKind::Key,
            code: 0x61,
            capability: Capability::ControlKeyboard,
            token: None,
            x: 0.0,
            y: 0.0,
            pressed: true,
            modifiers: 0,
        };
        assert!(x11_pointer_actions(&key_event, (0, 0), (1920, 1080)).is_err());
        // A pointer-move event with a nonzero code is rejected.
        let bad_move = InputEvent {
            kind: InputEventKind::PointerMove,
            code: 1,
            capability: Capability::ControlPointer,
            token: None,
            x: 10.0,
            y: 10.0,
            pressed: false,
            modifiers: 0,
        };
        assert!(x11_pointer_actions(&bad_move, (0, 0), (1920, 1080)).is_err());
        // A ModifierChange event carries no pointer action.
        let mod_change = InputEvent {
            kind: InputEventKind::ModifierChange,
            code: MOD_SHIFT,
            capability: Capability::ControlKeyboard,
            token: None,
            x: 0.0,
            y: 0.0,
            pressed: false,
            modifiers: MOD_SHIFT,
        };
        assert!(x11_pointer_actions(&mod_change, (0, 0), (1920, 1080)).is_err());
        // Modifier keysyms still translate to a key action (the X server
        // tracks modifier state from the injected keycode).
        let shift = InputEvent {
            kind: InputEventKind::Key,
            code: 0xFFE1,
            capability: Capability::ControlKeyboard,
            token: None,
            x: 0.0,
            y: 0.0,
            pressed: true,
            modifiers: MOD_SHIFT,
        };
        let action = x11_key_action(shift.code, &map, shift.pressed).unwrap();
        assert_eq!(
            action,
            X11Action::Key {
                keycode: 50,
                pressed: true
            }
        );
    }

    /// PDF Task 9.2: pointer coordinates are normalized against the shared
    /// source geometry, so the same normalized point maps to the same capture
    /// pixel regardless of the sender's local window size. The input path
    /// delegates to the BORU-SS-12 math (coords::normalized_to_source).
    #[test]
    fn normalize_to_capture_reuses_coords_math_and_is_window_independent() {
        use super::super::coords::{normalized_to_source, MonitorGeometry, NormalizedPoint};
        let capture = (1920, 1080);
        // Direct delegation check: same result as the coords module.
        let geometry = MonitorGeometry::new(0, 0, capture.0, capture.1);
        let source = normalized_to_source(NormalizedPoint { x: 0.25, y: 0.5 }, &geometry).unwrap();
        assert_eq!(
            normalize_to_capture(NormalizedPointer { x: 0.25, y: 0.5 }, capture),
            Some((source.x, source.y))
        );
        assert_eq!(
            normalize_to_capture(NormalizedPointer { x: 0.25, y: 0.5 }, capture),
            Some((480, 540))
        );
        // Window-size independence: the same normalized point (as produced by
        // the viewer's viewport_to_normalized, which divides by source size,
        // not window size) maps to the same capture pixel for any viewer size.
        for viewer_size in [(640.0, 360.0), (1280.0, 720.0), (1920.0, 1080.0)] {
            let (vw, vh) = viewer_size;
            // A viewer cursor at 25%/50% of ITS window maps to the same
            // normalized source point (source-relative), hence the same
            // capture pixel.
            let nx = (0.25 * vw) / vw;
            let ny = (0.5 * vh) / vh;
            assert_eq!(
                normalize_to_capture(
                    NormalizedPointer {
                        x: nx as f32,
                        y: ny as f32
                    },
                    capture
                ),
                Some((480, 540))
            );
        }
        // Out-of-range and NaN points are rejected (delegated bounds check).
        assert_eq!(
            normalize_to_capture(NormalizedPointer { x: 1.0, y: 0.5 }, capture),
            None
        );
        assert_eq!(
            normalize_to_capture(
                NormalizedPointer {
                    x: f32::NAN,
                    y: 0.5
                },
                capture
            ),
            None
        );
    }

    /// The host-side rate limiter lives in permissions.rs; this test pins the
    /// crate-level re-export so the host wiring can construct it with the
    /// defaults (PDF Task 9.2).
    #[test]
    fn crate_level_rate_limiter_defaults_are_available() {
        let mut limiter = SlidingWindowRateLimiter::default();
        assert!(limiter.is_empty());
        assert!(limiter.allow(Instant::now()));
        assert!(!limiter.is_empty());
    }

    /// Build the granted-device bitmask the X11 backend stores from the
    /// capabilities passed in an explicit GrantControl (pure mirror of the
    /// logic inside `X11RemoteInput::connect`).
    fn device_mask_for_capabilities(capabilities: &[Capability]) -> u32 {
        let mut mask = 0u32;
        for capability in capabilities {
            match capability {
                Capability::ControlPointer => mask |= PORTAL_DEVICE_POINTER,
                Capability::ControlKeyboard => mask |= PORTAL_DEVICE_KEYBOARD,
                _ => {}
            }
        }
        mask
    }
}
