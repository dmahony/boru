//! Platform-neutral logic for the Windows Graphics Capture backend.
//!
//! The WinRT/COM bindings in [`super::windows`] are gated on
//! `target_os = "windows"` and can only be exercised on real Windows
//! hardware. Everything in this module is pure Rust — the lifecycle state
//! machine, HRESULT classification, and monitor source-id derivation — so it
//! compiles and is unit-tested on every target, including Linux CI.
//!
//! The Windows backend maps its `windows::core::Error` HRESULTs onto
//! [`CaptureFailureKind`] here, and the [`GraphicsCaptureState`] transitions
//! are enforced by the same code path the hardware backend uses.

use crate::screen_share::capture::{CaptureSource, CaptureSourceId, CaptureSourceKind};
use crate::screen_share::coords::MonitorGeometry;

/// Lifecycle of a Windows Graphics Capture session.
///
/// Transitions:
/// - `Idle`/`Ended` → `Selecting` (`begin_selection`) or → `Streaming`
///   (`start`), the picker shortcut used by the monitor backend.
/// - `Selecting` → `Streaming` once a source is confirmed (`source_selected`).
/// - `Streaming` → `Ended` (`stop`/`close`). `stop` is idempotent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsCaptureState {
    Idle,
    Selecting,
    Streaming,
    Ending,
    Ended,
}

impl GraphicsCaptureState {
    /// Begin the source-picker flow. Only valid from a stopped state.
    pub fn begin_selection(self) -> Result<Self, CaptureFailureKind> {
        match self {
            Self::Idle | Self::Ended => Ok(Self::Selecting),
            _ => Err(CaptureFailureKind::AlreadyStarted),
        }
    }

    /// Confirm a picked source and enter streaming. Only valid from
    /// `Selecting` (or directly from a stopped state for the programmatic
    /// monitor-selection path).
    pub fn start(self) -> Result<Self, CaptureFailureKind> {
        match self {
            Self::Idle | Self::Ended | Self::Selecting => Ok(Self::Streaming),
            _ => Err(CaptureFailureKind::AlreadyStarted),
        }
    }

    /// Stop capture. Deliberately idempotent: stopping an already-stopped
    /// backend is a no-op so teardown paths never have to track state twice.
    pub fn stop(self) -> Self {
        Self::Ended
    }

    /// Whether frames may be pulled.
    pub fn is_streaming(self) -> bool {
        self == Self::Streaming
    }

    /// Enforce the `DesktopCaptureBackend` contract that `next_frame` is only
    /// valid while capturing.
    pub fn require_streaming(self) -> Result<(), CaptureFailureKind> {
        if self.is_streaming() {
            Ok(())
        } else {
            Err(CaptureFailureKind::NotStarted)
        }
    }
}

/// Diagnostics events surfaced by the Windows backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsCaptureEvent {
    PickerOpened,
    SourceSelected,
    FormatChanged { width: u32, height: u32 },
    SourceMinimized,
    Ended,
}

/// Typed capture failures for the Windows backend.
///
/// The task spec (PDF T4.1) requires resize, monitor unplug, lock screen,
/// minimized windows, and permission failures to surface as typed errors
/// rather than panics or raw HRESULT strings. This enum is the typed surface;
/// the backend classifies every `windows::core::Error` it sees through
/// [`CaptureFailureKind::classify`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFailureKind {
    /// Screen-capture permission was denied by Windows (`E_ACCESSDENIED`).
    PermissionDenied,
    /// The D3D11 device was lost (`DXGI_ERROR_DEVICE_REMOVED`/`RESET`).
    DeviceLost,
    /// The monitor/source is no longer available (unplugged, session ended).
    SourceUnavailable,
    /// The shared MONITOR was unplugged / the laptop was undocked — the
    /// capture item closed (WinRT `GraphicsCaptureItem.Closed`, PDF Phase 14
    /// / BORU-SS-38). Distinct from [`Self::SourceUnavailable`] (which also
    /// covers generic session-end) so the host can auto-fallback to the next
    /// available monitor without ending the session.
    MonitorLost,
    /// The workstation is locked; capture is paused (`E_CHANGED_STATE` with a
    /// still-attached monitor is treated as a lock-screen pause).
    ScreenLocked,
    /// The source is minimized (window capture only; monitors cannot
    /// minimize, kept for the future window backend).
    SourceMinimized,
    /// The source changed size; the backend recreated its frame pool.
    Resized,
    /// The requested [`CaptureSourceId`] is not a known monitor.
    UnknownSource,
    /// Capture is already running for this backend.
    AlreadyStarted,
    /// `next_frame` was called before `start` or after `stop`.
    NotStarted,
    /// Any other WinRT/COM failure, carrying the raw HRESULT.
    Api(u32),
}

impl CaptureFailureKind {
    /// Classify a raw HRESULT bit pattern into a typed failure kind.
    ///
    /// Only well-known codes are classified; anything else is preserved as
    /// [`CaptureFailureKind::Api`] so diagnostics keep the real code.
    /// `E_POINTER` is the normal "no new frame yet" result of
    /// `TryGetNextFrame` and is handled by the caller as `Ok(None)`, not as
    /// a failure — it is deliberately not classified here.
    pub fn classify(hresult: u32) -> Self {
        match hresult {
            // E_ACCESSDENIED (0x80070005): screen-capture consent missing.
            0x8007_0005 => Self::PermissionDenied,
            // DXGI_ERROR_DEVICE_REMOVED (0x887A0005) / DEVICE_RESET
            // (0x887A0007): GPU/device lost, capture cannot continue.
            0x887A_0005 | 0x887A_0007 => Self::DeviceLost,
            // E_CHANGED_STATE (0x8000000C): the capture item changed or
            // became unavailable (unplug / session ended / screen locked).
            // The caller distinguishes MonitorLost (unplug/dock-undock, PDF
            // Phase 14 / BORU-SS-38) from ScreenLocked by re-checking
            // whether the monitor is still attached; the raw code alone maps
            // to the generic SourceUnavailable.
            0x8000_000C => Self::SourceUnavailable,
            _ => Self::Api(hresult),
        }
    }

    /// Stable, user-safe description for logs and error messages.
    pub fn describe(self) -> String {
        match self {
            Self::PermissionDenied => "screen-capture permission denied".to_string(),
            Self::DeviceLost => "capture device lost (GPU removed or reset)".to_string(),
            Self::SourceUnavailable => {
                "capture source unavailable (monitor unplugged or session ended)".to_string()
            }
            Self::MonitorLost => "capture monitor lost (unplugged or undocked)".to_string(),
            Self::ScreenLocked => "capture paused: workstation locked".to_string(),
            Self::SourceMinimized => "capture source minimized".to_string(),
            Self::Resized => "capture source resized".to_string(),
            Self::UnknownSource => "unknown capture source".to_string(),
            Self::AlreadyStarted => "capture already started".to_string(),
            Self::NotStarted => "capture is not started".to_string(),
            Self::Api(code) => format!("WinRT capture error 0x{code:08X}"),
        }
    }
}

/// Derive a stable [`CaptureSourceId`] from a monitor's device name
/// (`\\.\DISPLAY1` etc.).
///
/// Monitor handles (`HMONITOR`) are opaque pointers that can change between
/// enumerations, so the public id is a stable FNV-1a hash of the device name
/// instead. Two enumerations of the same physical monitor always produce the
/// same id; two different monitors never collide in practice.
pub fn monitor_source_id(device_name: &str) -> CaptureSourceId {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in device_name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    CaptureSourceId(hash)
}

/// Derive a stable [`CaptureSourceId`] from a native `HWND` value
/// (BORU-SS-36 window capture).
///
/// `HWND` is an opaque pointer that is stable for the life of the window, so
/// the public id is a stable FNV-1a hash of the raw pointer. The value is
/// namespaced (via the `hwnd:{value}` string) so a window can never collide
/// with a monitor id in [`CaptureSourceId`] comparisons.
pub fn window_source_id(hwnd: usize) -> CaptureSourceId {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in format!("hwnd:{hwnd}").as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    CaptureSourceId(hash)
}

/// Human-readable monitor title for source pickers, e.g.
/// `\\.\DISPLAY1: 1920x1080`.
pub fn monitor_title(device_name: &str, width: u32, height: u32) -> String {
    format!("{device_name}: {width}x{height}")
}

/// Build a [`CaptureSource`] for a top-level window (BORU-SS-36).
///
/// Pure helper so the source-advertisement shape (id, kind, title, native
/// size, desktop geometry) is unit-tested without WinRT. The geometry
/// carries the window's virtual-desktop rect (from `GetWindowRect`), so the
/// host can normalize coordinates against the shared source exactly like
/// monitors.
pub fn window_source(
    hwnd: usize,
    title: &str,
    left: i32,
    top: i32,
    width: u32,
    height: u32,
) -> CaptureSource {
    CaptureSource {
        id: window_source_id(hwnd),
        kind: CaptureSourceKind::Window,
        title: format!("{title}: {width}x{height}"),
        width,
        height,
        geometry: Some(MonitorGeometry::new(left, top, width, height)),
    }
}

/// Build a [`CaptureSource`] from enumerated monitor geometry.
///
/// Pure helper so the source-advertisement shape (id, kind, title, native
/// size, desktop geometry) is unit-tested without WinRT; the Windows backend
/// calls it from its `EnumDisplayMonitors` callback. The geometry carries the
/// monitor's virtual-desktop origin (which may be negative for monitors left
/// of / above the primary) so the host can normalize coordinates against the
/// shared source.
pub fn monitor_source(device_name: &str, geometry: MonitorGeometry) -> CaptureSource {
    CaptureSource {
        id: monitor_source_id(device_name),
        kind: CaptureSourceKind::Monitor,
        title: monitor_title(device_name, geometry.width, geometry.height),
        width: geometry.width,
        height: geometry.height,
        geometry: Some(geometry),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_transitions_follow_lifecycle_contract() {
        // Idle → Selecting → Streaming is the picker path.
        let state = GraphicsCaptureState::Idle;
        let state = state.begin_selection().unwrap();
        assert_eq!(state, GraphicsCaptureState::Selecting);
        let state = state.start().unwrap();
        assert_eq!(state, GraphicsCaptureState::Streaming);
        assert!(state.is_streaming());
        // Streaming → Ended, then stop is idempotent.
        let state = state.stop();
        assert_eq!(state, GraphicsCaptureState::Ended);
        assert_eq!(state.stop(), GraphicsCaptureState::Ended);
    }

    #[test]
    fn idle_can_start_directly_for_programmatic_selection() {
        assert_eq!(
            GraphicsCaptureState::Idle.start().unwrap(),
            GraphicsCaptureState::Streaming
        );
        assert_eq!(
            GraphicsCaptureState::Ended.start().unwrap(),
            GraphicsCaptureState::Streaming
        );
    }

    #[test]
    fn start_twice_is_rejected() {
        let state = GraphicsCaptureState::Idle.start().unwrap();
        assert_eq!(state.start(), Err(CaptureFailureKind::AlreadyStarted));
    }

    #[test]
    fn begin_selection_while_streaming_is_rejected() {
        let state = GraphicsCaptureState::Idle.start().unwrap();
        assert_eq!(
            state.begin_selection(),
            Err(CaptureFailureKind::AlreadyStarted)
        );
    }

    #[test]
    fn require_streaming_rejects_unstarted_and_stopped() {
        assert_eq!(
            GraphicsCaptureState::Idle.require_streaming(),
            Err(CaptureFailureKind::NotStarted)
        );
        assert_eq!(
            GraphicsCaptureState::Ended.require_streaming(),
            Err(CaptureFailureKind::NotStarted)
        );
        assert_eq!(
            GraphicsCaptureState::Selecting.require_streaming(),
            Err(CaptureFailureKind::NotStarted)
        );
        assert!(GraphicsCaptureState::Streaming.require_streaming().is_ok());
    }

    #[test]
    fn hresult_classification_maps_known_codes() {
        // E_ACCESSDENIED
        assert_eq!(
            CaptureFailureKind::classify(0x8007_0005),
            CaptureFailureKind::PermissionDenied
        );
        // DXGI_ERROR_DEVICE_REMOVED / DEVICE_RESET
        assert_eq!(
            CaptureFailureKind::classify(0x887A_0005),
            CaptureFailureKind::DeviceLost
        );
        assert_eq!(
            CaptureFailureKind::classify(0x887A_0007),
            CaptureFailureKind::DeviceLost
        );
        // E_CHANGED_STATE
        assert_eq!(
            CaptureFailureKind::classify(0x8000_000C),
            CaptureFailureKind::SourceUnavailable
        );
    }

    #[test]
    fn hresult_classification_preserves_unknown_codes() {
        let code = 0x8000_4057; // E_FAIL (arbitrary unknown)
        assert_eq!(
            CaptureFailureKind::classify(code),
            CaptureFailureKind::Api(code as u32)
        );
        assert!(
            CaptureFailureKind::Api(code as u32)
                .describe()
                .contains("0x80004057"),
            "Api description must carry the raw code"
        );
    }

    #[test]
    fn failure_descriptions_are_user_safe() {
        for (kind, expected) in [
            (CaptureFailureKind::PermissionDenied, "permission"),
            (CaptureFailureKind::DeviceLost, "device lost"),
            (CaptureFailureKind::SourceUnavailable, "unavailable"),
            (CaptureFailureKind::ScreenLocked, "locked"),
            (CaptureFailureKind::SourceMinimized, "minimized"),
            (CaptureFailureKind::Resized, "resized"),
            (CaptureFailureKind::UnknownSource, "unknown"),
            (CaptureFailureKind::AlreadyStarted, "already started"),
            (CaptureFailureKind::NotStarted, "not started"),
        ] {
            assert!(
                kind.describe().to_lowercase().contains(expected),
                "{kind:?} description should mention {expected:?}"
            );
        }
    }

    #[test]
    fn monitor_source_id_is_stable_and_distinct() {
        let first = monitor_source_id(r"\\.\DISPLAY1");
        let again = monitor_source_id(r"\\.\DISPLAY1");
        let second = monitor_source_id(r"\\.\DISPLAY2");
        assert_eq!(first, again, "same device must map to same id");
        assert_ne!(first, second, "different devices must not collide");
    }

    #[test]
    fn window_source_id_is_stable_and_namespaced_away_from_monitors() {
        let first = window_source_id(0x0002_0001);
        let again = window_source_id(0x0002_0001);
        let other = window_source_id(0x0002_0002);
        assert_eq!(first, again, "same HWND must map to same id");
        assert_ne!(first, other, "different HWNDs must not collide");
        // Window ids must never collide with monitor ids.
        let monitor = monitor_source_id(r"\\.\DISPLAY1");
        assert_ne!(first, monitor);
    }

    #[test]
    fn window_source_advertises_kind_geometry_and_title() {
        let source = window_source(0x0002_0001, "Terminal", 100, 50, 800, 600);
        assert_eq!(source.id, window_source_id(0x0002_0001));
        assert_eq!(source.kind, CaptureSourceKind::Window);
        assert_eq!(source.title, "Terminal: 800x600");
        assert_eq!((source.width, source.height), (800, 600));
        assert_eq!(
            source.geometry,
            Some(MonitorGeometry::new(100, 50, 800, 600))
        );
        // The picker must render a distinguishable label for windows.
        assert!(source.picker_label().starts_with("[Window] "));
    }

    #[test]
    fn monitor_source_advertises_stable_geometry() {
        let geometry = MonitorGeometry::new(-1920, 0, 1920, 1080);
        let source = monitor_source(r"\\\\.\\DISPLAY1", geometry);
        assert_eq!(source.id, monitor_source_id(r"\\\\.\\DISPLAY1"));
        assert_eq!(source.kind, CaptureSourceKind::Monitor);
        assert_eq!((source.width, source.height), (1920, 1080));
        assert_eq!(source.geometry, Some(geometry));
        assert!(source.title.contains("1920x1080"));
    }
}
