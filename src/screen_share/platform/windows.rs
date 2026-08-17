//! Windows Graphics Capture backend.
//!
//! Real WinRT `Windows.Graphics.Capture` monitor capture behind the
//! platform-neutral [`DesktopCaptureBackend`] trait: source enumeration via
//! `EnumDisplayMonitors`, a D3D11 device + frame-pool + capture session for
//! the selected monitor, and GPU→CPU staging that avoids round-tripping
//! through the GPU when the frame reaches the CPU-bound OpenH264 encoder.
//!
//! Typed failure handling (PDF T4.1): resize, monitor unplug, lock screen,
//! minimized windows, and permission failures surface as
//! [`CaptureFailureKind`] errors — never panics — via the shared classifier
//! in [`super::windows_common`]. All pure logic (state machine, HRESULT
//! classification, source-id derivation) is unit-tested on Linux; only the
//! WinRT calls here require real Windows hardware.
//!
//! # Cursor strategy (PDF T4.2)
//!
//! WinRT Graphics Capture does **not** composite the pointer into captured
//! frames — `Direct3D11CaptureFrame` surfaces contain only the desktop
//! content. Boru therefore composites the cursor into the frame on the host:
//! [`composite_system_cursor`] queries the cursor shape/position with GDI
//! (`GetCursorInfo` + `GetIconInfo` + `DrawIconEx`), rasterizes it into a
//! small BGRA sprite, and alpha-blends it into the staged frame at the
//! source-relative position (see [`crate::screen_share::coords`] for the
//! decision rationale and the pure, Linux-tested mapping). This keeps the
//! wire protocol and viewer unchanged — the cursor arrives as ordinary
//! video.
//!
//! Monitor geometry (origin + physical size) is advertised with each
//! [`CaptureSource`] so the host can normalize cursor and input coordinates
//! against the shared source rather than the global desktop. Origins may be
//! negative for monitors left of / above the primary.
#![allow(missing_docs)]

use std::collections::HashMap;
use std::collections::VecDeque;
use std::ptr;

use windows::core::{factory, Interface};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors,
    GetMonitorInfoW, GetObjectW, MonitorFromWindow, SelectObject, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, HMONITOR, HGDIOBJ, MONITORINFOEXW,
    MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, EnumWindows, GetCursorInfo, GetDesktopWindow, GetIconInfo, GetWindowRect,
    GetWindowTextW, IsWindowVisible, CURSORINFO, CURSORINFO_FLAGS, CURSOR_SHOWING, DI_NORMAL,
    ICONINFO,
};

use super::windows_common::{monitor_source, window_source, CaptureFailureKind};
pub use super::windows_common::{GraphicsCaptureEvent, GraphicsCaptureState};
use crate::screen_share::capture::{
    CaptureConfig, CaptureSource, CaptureSourceId, DesktopCaptureBackend, FrameSink,
};
use crate::screen_share::coords::{composite_cursor, CursorSprite, DesktopPoint, MonitorGeometry};
use crate::screen_share::{
    CapturedFrame, PixelFormat, ScreenCapture, ScreenShareError, ScreenShareErrorKind,
};

/// Number of buffered frames in the WinRT frame pool. Two lets the compositor
/// produce a frame while the previous one is being staged to CPU.
const FRAME_POOL_BUFFERS: i32 = 2;

/// A real WinRT frame-pool capture source for one monitor.
pub struct GraphicsCapture {
    state: GraphicsCaptureState,
    sink: FrameSink,
    format: Option<(u32, u32)>,
    events: VecDeque<GraphicsCaptureEvent>,
    sources: HashMap<CaptureSourceId, usize>,
    /// Top-level windows enumerated via `EnumWindows` (BORU-SS-36), mapped
    /// from stable [`CaptureSourceId`] to raw `HWND` (`isize`).
    windows: HashMap<CaptureSourceId, isize>,
    active_source: Option<CaptureSourceId>,
    /// Virtual-desktop geometry of the active monitor, used to normalize
    /// cursor coordinates against the shared source (PDF T4.2).
    active_geometry: Option<MonitorGeometry>,
    pool: Option<Direct3D11CaptureFramePool>,
    session: Option<GraphicsCaptureSession>,
    item: Option<GraphicsCaptureItem>,
    device: Option<ID3D11Device>,
    context: Option<ID3D11DeviceContext>,
    winrt_device: Option<SendWinrtDevice>,
    staging: Option<ID3D11Texture2D>,
    staging_dimensions: Option<(u32, u32)>,
}

/// `IDirect3DDevice` is not declared `Send` by `windows-core`, but it wraps
/// the same thread-safe D3D11 device that the crate already marks `Send`
/// (`ID3D11Device`), and Graphics Capture only uses it to create/recreate the
/// frame pool. The `DesktopCaptureBackend`/`ScreenCapture` traits require
/// `Send`, so the WinRT device is held behind this wrapper.
struct SendWinrtDevice(IDirect3DDevice);
unsafe impl Send for SendWinrtDevice {}

impl std::fmt::Debug for GraphicsCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphicsCapture")
            .field("state", &self.state)
            .field("format", &self.format)
            .finish()
    }
}

impl GraphicsCapture {
    /// The source id currently being captured, when the backend is streaming
    /// a monitor (PDF Phase 10 source tracking).
    pub fn active_source_id(&self) -> Option<CaptureSourceId> {
        self.active_source
    }

    pub fn new(queue_capacity: usize) -> Result<Self, ScreenShareError> {
        Ok(Self {
            state: GraphicsCaptureState::Idle,
            sink: FrameSink::new(queue_capacity)?,
            format: None,
            events: VecDeque::new(),
            sources: HashMap::new(),
            windows: HashMap::new(),
            active_source: None,
            active_geometry: None,
            pool: None,
            session: None,
            item: None,
            device: None,
            context: None,
            winrt_device: None,
            staging: None,
            staging_dimensions: None,
        })
    }

    /// Enumerate the current monitors into the private `sources` map, plus
    /// top-level windows into the `windows` map (BORU-SS-36).
    fn refresh_sources(&mut self) -> Result<(), ScreenShareError> {
        let monitors =
            enumerate_monitors().map_err(|kind| ScreenShareError::new(kind.describe()))?;
        self.sources = monitors
            .into_iter()
            .map(|(id, hmon)| (id, hmon.0 as usize))
            .collect();
        // Window enumeration is best-effort: a failure must not break monitor
        // sharing.
        if let Ok(windows) = enumerate_windows() {
            self.windows = windows
                .into_iter()
                .map(|(id, hwnd, _title, _rect)| (id, hwnd.0 as isize))
                .collect();
        }
        Ok(())
    }

    /// Create a desktop capture for the primary monitor. Retained for the
    /// programmatic `create_capture_source` path: it enumerates monitors,
    /// picks the primary one, and starts a session with the default config.
    pub fn try_create(queue_capacity: usize) -> Result<Self, ScreenShareError> {
        let mut capture = Self::new(queue_capacity)?;
        capture.refresh_sources()?;
        let primary = unsafe { MonitorFromWindow(GetDesktopWindow(), MONITOR_DEFAULTTOPRIMARY) };
        let primary_id = capture
            .sources
            .iter()
            .find_map(|(id, hmon)| {
                if *hmon == primary.0 as usize {
                    Some(*id)
                } else {
                    None
                }
            })
            .ok_or_else(|| ScreenShareError::new("primary monitor not found in enumeration"))?;
        capture.start(primary_id, CaptureConfig::default())?;
        Ok(capture)
    }

    pub fn begin_selection(&mut self) -> Result<(), ScreenShareError> {
        self.state = self
            .state
            .begin_selection()
            .map_err(|kind| ScreenShareError::new(kind.describe()))?;
        self.events.push_back(GraphicsCaptureEvent::PickerOpened);
        Ok(())
    }
    pub fn source_selected(&mut self) -> Result<(), ScreenShareError> {
        self.state = self
            .state
            .start()
            .map_err(|kind| ScreenShareError::new(kind.describe()))?;
        self.events.push_back(GraphicsCaptureEvent::SourceSelected);
        Ok(())
    }
    pub fn push_surface(&mut self, frame: CapturedFrame) -> Result<(), ScreenShareError> {
        if !self.state.is_streaming() {
            return Err(ScreenShareError::new(
                CaptureFailureKind::NotStarted.describe(),
            ));
        }
        self.sink.push(frame);
        Ok(())
    }
    pub fn source_minimized(&mut self) {
        self.events.push_back(GraphicsCaptureEvent::SourceMinimized);
    }
    pub fn close(&mut self) {
        self.stop();
    }
    pub fn next_event(&mut self) -> Option<GraphicsCaptureEvent> {
        self.events.pop_front()
    }
    pub fn counters(&self) -> (u64, u64, u64) {
        self.sink.counters()
    }
    pub fn dimensions(&self) -> (u32, u32) {
        self.format.unwrap_or((640, 360))
    }
    pub fn state(&self) -> GraphicsCaptureState {
        self.state
    }
}

impl ScreenCapture for GraphicsCapture {
    fn capture(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
        // Preserve the pre-BORU-SS-11 contract: an idle/stopped backend
        // yields "no frame", not an error (the host loop polls and treats
        // errors as fatal). The DesktopCaptureBackend impl keeps the strict
        // lifecycle for programmatic callers.
        if !self.state.is_streaming() {
            return Ok(None);
        }
        DesktopCaptureBackend::next_frame(self)
    }
}

impl DesktopCaptureBackend for GraphicsCapture {
    fn list_sources(&self) -> Result<Vec<CaptureSource>, ScreenShareError> {
        let mut sources: Vec<CaptureSource> = enumerate_monitors()
            .map(|monitors| {
                monitors
                    .into_iter()
                    .map(|(_, hmon)| {
                        let info = monitor_info(hmon);
                        let geometry = MonitorGeometry::new(
                            info.left,
                            info.top,
                            info.rect_width,
                            info.rect_height,
                        );
                        monitor_source(&info.device_name, geometry)
                    })
                    .collect()
            })
            .map_err(|kind| ScreenShareError::new(kind.describe()))?;
        // BORU-SS-36: advertise top-level windows alongside monitors. Window
        // enumeration is best-effort — a failure must not break monitor
        // sharing.
        if let Ok(windows) = enumerate_windows() {
            sources.extend(windows.into_iter().map(|(_, hwnd, title, rect)| {
                let width = (rect.right.saturating_sub(rect.left)).max(0) as u32;
                let height = (rect.bottom.saturating_sub(rect.top)).max(0) as u32;
                window_source(hwnd.0 as usize, &title, rect.left, rect.top, width, height)
            }));
        }
        Ok(sources)
    }

    fn start(
        &mut self,
        source: CaptureSourceId,
        config: CaptureConfig,
    ) -> Result<(), ScreenShareError> {
        if config.target_fps == 0 {
            return Err(ScreenShareError::new("target fps must be non-zero"));
        }
        let next_state = self
            .state
            .start()
            .map_err(|kind| ScreenShareError::new(kind.describe()))?;
        // Ensure the monitor/window map is populated (list_sources may not
        // have been called yet).
        if self.sources.is_empty() {
            self.refresh_sources()?;
        }
        // BORU-SS-36: a Window source captures a single application window
        // via `GraphicsCaptureItem.CreateForWindow`, falling back to the
        // primary monitor if window capture fails. A Monitor source uses the
        // existing `CreateForMonitor` path.
        let window_target = self.windows.get(&source).copied();
        let hmon = if window_target.is_none() {
            self.sources.get(&source).copied()
        } else {
            None
        };
        let hmon = hmon.map(|raw| HMONITOR(raw as *mut core::ffi::c_void));

        let info = hmon.map(monitor_info);
        if let Some(info) = &info {
            if info.rect_width == 0 || info.rect_height == 0 {
                return Err(ScreenShareError::new(
                    CaptureFailureKind::SourceUnavailable.describe(),
                ));
            }
        }
        let size = info.as_ref().map(|info| windows::Graphics::SizeInt32 {
            Width: info.rect_width as i32,
            Height: info.rect_height as i32,
        });

        // D3D11 device with BGRA support (Graphics Capture requires it).
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|e| {
            ScreenShareError::new(format!(
                "{} (D3D11CreateDevice: {e})",
                CaptureFailureKind::classify(e.code().0 as u32).describe()
            ))
        })?;
        let device = device.ok_or_else(|| ScreenShareError::new("D3D11 returned no device"))?;
        let context = context.ok_or_else(|| ScreenShareError::new("D3D11 returned no context"))?;
        let dxgi: IDXGIDevice = device
            .cast()
            .map_err(|e| ScreenShareError::new(format!("D3D11 device cast: {e}")))?;
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
            .map_err(|e| ScreenShareError::new(format!("WinRT D3D device: {e}")))?;
        let winrt_device: IDirect3DDevice = inspectable
            .cast()
            .map_err(|e| ScreenShareError::new(format!("WinRT D3D interface: {e}")))?;

        let interop: IGraphicsCaptureItemInterop =
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|e| {
                ScreenShareError::new(format!(
                    "{} (GraphicsCaptureItem factory: {e})",
                    CaptureFailureKind::classify(e.code().0 as u32).describe()
                ))
            })?;
        // BORU-SS-36: window capture first; fall back to the primary monitor
        // when CreateForWindow fails (window closed between enumeration and
        // start, permission denied, etc.). The returned tuple carries the
        // ACTUAL source id captured (the monitor id after a fallback), so
        // `active_source`/`current_source()` stay truthful about what is on
        // the wire.
        let (item, active_geometry, active_source_id) = if let Some(hwnd) = window_target {
            let hwnd = HWND(hwnd as *mut core::ffi::c_void);
            let window_item: windows::core::Result<GraphicsCaptureItem> =
                unsafe { interop.CreateForWindow(hwnd) };
            match window_item {
                Ok(item) => {
                    // The capture geometry is the window's virtual-desktop
                    // rect (GetWindowRect gives the outer frame origin), but
                    // the shared source dims come from the capture item's
                    // client size so geometry matches the frames.
                    let item_size = item
                        .Size()
                        .map_err(|e| ScreenShareError::new(format!("capture item size: {e}")))?;
                    let mut rect = RECT::default();
                    let got_rect = unsafe { GetWindowRect(hwnd, &mut rect) };
                    let (left, top) = if got_rect.is_ok() {
                        (rect.left, rect.top)
                    } else {
                        (0, 0)
                    };
                    let width = item_size.Width.max(0) as u32;
                    let height = item_size.Height.max(0) as u32;
                    (
                        item,
                        MonitorGeometry::new(left, top, width, height),
                        source,
                    )
                }
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "screen-share: CreateForWindow failed; falling back to primary monitor capture"
                    );
                    let hmon = unsafe { MonitorFromWindow(GetDesktopWindow(), MONITOR_DEFAULTTOPRIMARY) };
                    let info = monitor_info(hmon);
                    let fallback_id = self
                        .sources
                        .iter()
                        .find(|(_, raw)| **raw == hmon.0 as usize)
                        .map(|(id, _)| *id)
                        .unwrap_or(source);
                    let item = unsafe {
                        interop.CreateForMonitor::<_, GraphicsCaptureItem>(hmon)
                    }
                    .map_err(|e| {
                        ScreenShareError::new(format!(
                            "{} (CreateForMonitor fallback: {e})",
                            CaptureFailureKind::classify(e.code().0 as u32).describe()
                        ))
                    })?;
                    (
                        item,
                        MonitorGeometry::new(
                            info.left,
                            info.top,
                            info.rect_width,
                            info.rect_height,
                        ),
                        fallback_id,
                    )
                }
            }
        } else {
            let hmon = hmon.ok_or_else(|| {
                ScreenShareError::new(CaptureFailureKind::UnknownSource.describe())
            })?;
            let info = monitor_info(hmon);
            let item = unsafe {
                interop.CreateForMonitor::<_, GraphicsCaptureItem>(hmon)
            }
            .map_err(|e| {
                ScreenShareError::new(format!(
                    "{} (CreateForMonitor: {e})",
                    CaptureFailureKind::classify(e.code().0 as u32).describe()
                ))
            })?;
            (
                item,
                MonitorGeometry::new(info.left, info.top, info.rect_width, info.rect_height),
                source,
            )
        };
        let item_size = item
            .Size()
            .map_err(|e| ScreenShareError::new(format!("capture item size: {e}")))?;
        if item_size.Width <= 0 || item_size.Height <= 0 {
            // The monitor exists but reports no captureable area: on a locked
            // workstation Windows stops delivering frames and the item can
            // shrink to zero.
            let kind = if let Some(hmon) = hmon {
                if monitor_attached(hmon) {
                    CaptureFailureKind::ScreenLocked
                } else {
                    CaptureFailureKind::SourceUnavailable
                }
            } else {
                CaptureFailureKind::MonitorLost
            };
            let mut error = ScreenShareError::new(kind.describe());
            if kind == CaptureFailureKind::MonitorLost {
                error = error.with_kind(ScreenShareErrorKind::MonitorLost);
            }
            return Err(error);
        }
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            FRAME_POOL_BUFFERS,
            size.unwrap_or(item_size),
        )
        .map_err(|e| ScreenShareError::new(format!("frame pool: {e}")))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|e| ScreenShareError::new(format!("capture session: {e}")))?;
        session.StartCapture().map_err(|e| {
            ScreenShareError::new(format!(
                "{} (StartCapture: {e})",
                CaptureFailureKind::classify(e.code().0 as u32).describe()
            ))
        })?;

        self.state = next_state;
        self.format = Some((item_size.Width as u32, item_size.Height as u32));
        self.active_source = Some(active_source_id);
        self.active_geometry = Some(active_geometry);
        self.pool = Some(pool);
        self.session = Some(session);
        self.item = Some(item);
        self.device = Some(device);
        self.context = Some(context);
        self.winrt_device = Some(SendWinrtDevice(winrt_device));
        self.staging = None;
        self.staging_dimensions = None;
        self.events.push_back(GraphicsCaptureEvent::SourceSelected);
        Ok(())
    }

    fn next_frame(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
        self.state
            .require_streaming()
            .map_err(|kind| ScreenShareError::new(kind.describe()))?;
        let Some(pool) = &self.pool else {
            return Err(ScreenShareError::new(
                CaptureFailureKind::NotStarted.describe(),
            ));
        };
        let frame = match pool.TryGetNextFrame() {
            Ok(frame) => frame,
            // E_POINTER (0x80004003) is the normal "no new frame yet" result
            // of TryGetNextFrame; the caller polls at CAPTURE_FPS.
            Err(e) if e.code().0 as u32 == 0x8000_4003 => return Ok(None),
            Err(e) => {
                return Err(ScreenShareError::new(
                    CaptureFailureKind::classify(e.code().0 as u32).describe(),
                ));
            }
        };
        let content = frame
            .ContentSize()
            .map_err(|e| ScreenShareError::new(format!("frame content size: {e}")))?;
        if content.Width <= 0 || content.Height <= 0 {
            // The frame reports no content: on a locked workstation Windows
            // stops delivering frames (typed ScreenLocked); if the monitor
            // itself is gone this is an unplug (typed MonitorLost, PDF Phase
            // 14 / BORU-SS-38).
            let hmon = self
                .active_source
                .and_then(|id| self.sources.get(&id).copied())
                .map(|raw| HMONITOR(raw as *mut core::ffi::c_void))
                .unwrap_or(HMONITOR(ptr::null_mut()));
            let kind = if monitor_attached(hmon) {
                CaptureFailureKind::ScreenLocked
            } else {
                CaptureFailureKind::MonitorLost
            };
            let _ = frame.Close();
            let mut error = ScreenShareError::new(kind.describe());
            if kind == CaptureFailureKind::MonitorLost {
                error = error.with_kind(ScreenShareErrorKind::MonitorLost);
            }
            return Err(error);
        }
        let width = content.Width as u32;
        let height = content.Height as u32;

        // Handle source resize: the frame pool is fixed-size, so when the
        // monitor resolution changes we must recreate the pool and start a
        // fresh capture session (Microsoft docs, Graphics Capture samples).
        if self.format != Some((width, height)) {
            let winrt_device = self
                .winrt_device
                .as_ref()
                .map(|device| &device.0)
                .ok_or_else(|| ScreenShareError::new("capture device was released"))?;
            let item = self
                .item
                .as_ref()
                .ok_or_else(|| ScreenShareError::new("capture item was released"))?;
            pool.Recreate(
                winrt_device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                FRAME_POOL_BUFFERS,
                content,
            )
            .map_err(|e| ScreenShareError::new(format!("frame pool recreate: {e}")))?;
            if let Some(old_session) = self.session.take() {
                let _ = old_session.Close();
            }
            let session = pool
                .CreateCaptureSession(item)
                .map_err(|e| ScreenShareError::new(format!("capture session recreate: {e}")))?;
            session.StartCapture().map_err(|e| {
                ScreenShareError::new(format!(
                    "{} (StartCapture after resize: {e})",
                    CaptureFailureKind::classify(e.code().0 as u32).describe()
                ))
            })?;
            self.session = Some(session);
            self.format = Some((width, height));
            self.staging = None;
            self.staging_dimensions = None;
            self.events
                .push_back(GraphicsCaptureEvent::FormatChanged { width, height });
        }

        let surface = frame
            .Surface()
            .map_err(|e| ScreenShareError::new(format!("frame surface: {e}")))?;
        let texture: ID3D11Texture2D = surface
            .cast()
            .map_err(|e| ScreenShareError::new(format!("capture surface cast: {e}")))?;
        let device = self
            .device
            .as_ref()
            .ok_or_else(|| ScreenShareError::new("capture device was released"))?;
        let context = self
            .context
            .as_ref()
            .ok_or_else(|| ScreenShareError::new("capture context was released"))?;
        let timestamp = frame
            .SystemRelativeTime()
            .map(|t| t.Duration as u64 / 10)
            .unwrap_or(0);

        if self.staging_dimensions != Some((width, height)) {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe {
                texture.GetDesc(&mut desc);
            }
            desc.Width = width;
            desc.Height = height;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            let mut staging = None;
            unsafe { device.CreateTexture2D(&desc, None, Some(&mut staging)) }
                .map_err(|e| ScreenShareError::new(format!("staging texture: {e}")))?;
            self.staging = staging;
            self.staging_dimensions = Some((width, height));
        }
        let staging = self
            .staging
            .as_ref()
            .ok_or_else(|| ScreenShareError::new("staging texture was not created"))?;
        unsafe {
            context.CopyResource(staging, &texture);
        }
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe { context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }
            .map_err(|e| ScreenShareError::new(format!("map capture frame: {e}")))?;
        let row_bytes = width as usize * 4;
        let mut pixels = vec![0u8; row_bytes * height as usize];
        unsafe {
            for row in 0..height as usize {
                let source = (mapped.pData as *const u8).add(row * mapped.RowPitch as usize);
                let target = pixels.as_mut_ptr().add(row * row_bytes);
                std::ptr::copy_nonoverlapping(source, target, row_bytes);
            }
            context.Unmap(staging, 0);
        }
        let _ = frame.Close();
        // Composite the system cursor into the staged frame (PDF T4.2). WinRT
        // Graphics Capture does not include the pointer; we draw it here at
        // the source-relative position so the viewer sees it as ordinary
        // video. Failures are non-fatal: the frame still goes out.
        if let Some(geometry) = self.active_geometry {
            let _ = composite_system_cursor(&mut pixels, width, height, &geometry);
        }
        CapturedFrame::cpu(timestamp, width, height, PixelFormat::Bgra8, pixels).map(Some)
    }

    fn stop(&mut self) {
        self.state = self.state.stop();
        if let Some(pool) = self.pool.take() {
            let _ = pool.Close();
        }
        self.session.take();
        self.item.take();
        self.staging = None;
        self.staging_dimensions = None;
        self.active_source = None;
        self.active_geometry = None;
        self.events.push_back(GraphicsCaptureEvent::Ended);
    }
}

/// Geometry + device name of one monitor, extracted from `GetMonitorInfoW`.
struct MonitorInfo {
    device_name: String,
    /// Virtual-desktop origin of the monitor (physical px; may be negative).
    left: i32,
    top: i32,
    rect_width: u32,
    rect_height: u32,
}

/// Query monitor geometry/device name, returning zeroes when the handle is
/// no longer attached (so callers can distinguish unplug from lock).
fn monitor_info(hmon: HMONITOR) -> MonitorInfo {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe { GetMonitorInfoW(hmon, (&mut info as *mut MONITORINFOEXW).cast()) };
    if ok.0 == 0 {
        return MonitorInfo {
            device_name: String::new(),
            left: 0,
            top: 0,
            rect_width: 0,
            rect_height: 0,
        };
    }
    let rect = info.monitorInfo.rcMonitor;
    let width = (rect.right.saturating_sub(rect.left)).max(0) as u32;
    let height = (rect.bottom.saturating_sub(rect.top)).max(0) as u32;
    let device_name = utf16_to_string(&info.szDevice);
    MonitorInfo {
        device_name,
        left: rect.left,
        top: rect.top,
        rect_width: width,
        rect_height: height,
    }
}

/// Whether the monitor handle still refers to an attached display.
fn monitor_attached(hmon: HMONITOR) -> bool {
    if hmon.0.is_null() {
        return false;
    }
    let info = monitor_info(hmon);
    info.rect_width != 0 && info.rect_height != 0
}

/// Query the system cursor and composite it into a staged BGRA8 frame at the
/// source-relative position (PDF T4.2).
///
/// WinRT Graphics Capture frames do not include the pointer, so the host
/// draws it here: `GetCursorInfo` returns the cursor handle, visibility flag
/// and desktop position; `GetIconInfo` returns the hotspot and bitmaps;
/// `DrawIconEx` rasterizes the shape into a small DIB section whose BGRA
/// pixels are then alpha-blended into the frame by the pure
/// [`composite_cursor`] helper. The desktop position is normalized against
/// the shared source via [`DesktopPoint`] + [`MonitorGeometry`], so monitors
/// with negative origins map correctly.
///
/// Any GDI failure is non-fatal — the frame is still delivered without the
/// cursor — matching the subsystem's "never panic on capture issues" rule.
fn composite_system_cursor(
    frame: &mut [u8],
    width: u32,
    height: u32,
    geometry: &MonitorGeometry,
) -> Result<(), ScreenShareError> {
    let mut cursor_info = CURSORINFO::default();
    cursor_info.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
    unsafe { GetCursorInfo(&mut cursor_info) }
        .map_err(|e| ScreenShareError::new(format!("GetCursorInfo: {e}")))?;
    // CURSOR_SHOWING (0x1) means the cursor is actually visible; without it
    // (hidden cursor, touch input, remote session) there is nothing to draw.
    // The flags type has no BitAnd impl in windows 0.58, so compare raw bits.
    if cursor_info.flags.0 & CURSOR_SHOWING.0 == 0 {
        return Ok(());
    }
    let mut icon_info = ICONINFO::default();
    unsafe { GetIconInfo(cursor_info.hCursor, &mut icon_info) }
        .map_err(|e| ScreenShareError::new(format!("GetIconInfo: {e}")))?;

    // Rasterize the cursor into a DIB section sized to the cursor bitmap.
    let mut bitmap = BITMAP::default();
    let hbm = if !icon_info.hbmColor.is_invalid() {
        icon_info.hbmColor
    } else {
        icon_info.hbmMask
    };
    let got = unsafe {
        GetObjectW(
            hbm,
            std::mem::size_of::<BITMAP>() as i32,
            Some((&mut bitmap as *mut BITMAP).cast()),
        )
    };
    if got == 0 || bitmap.bmWidth <= 0 || bitmap.bmHeight <= 0 {
        let _ = unsafe { DeleteObject(icon_info.hbmColor) };
        let _ = unsafe { DeleteObject(icon_info.hbmMask) };
        return Err(ScreenShareError::new(
            "cursor bitmap has no usable dimensions",
        ));
    }
    // For a monochrome cursor (hbmColor == null), hbmMask is twice the height
    // (AND mask on top, XOR mask below); the color cursor's mask is the same
    // size as the color bitmap.
    let cursor_width = bitmap.bmWidth as u32;
    let cursor_height = if icon_info.hbmColor.is_invalid() {
        (bitmap.bmHeight / 2).max(1) as u32
    } else {
        bitmap.bmHeight as u32
    };
    if cursor_width > 256 || cursor_height > 256 {
        let _ = unsafe { DeleteObject(icon_info.hbmColor) };
        let _ = unsafe { DeleteObject(icon_info.hbmMask) };
        return Err(ScreenShareError::new("cursor bitmap is unreasonably large"));
    }

    let dc = unsafe { CreateCompatibleDC(HDC::default()) };
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: cursor_width as i32,
            biHeight: -(cursor_height as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..BITMAPINFOHEADER::default()
        },
        ..BITMAPINFO::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = unsafe { CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) }
        .map_err(|e| {
            let _ = unsafe { DeleteObject(icon_info.hbmColor) };
            let _ = unsafe { DeleteObject(icon_info.hbmMask) };
            let _ = unsafe { DeleteDC(dc) };
            ScreenShareError::new(format!("CreateDIBSection: {e}"))
        })?;
    let previous = unsafe { SelectObject(dc, dib) };
    let _ = unsafe {
        DrawIconEx(
            dc,
            0,
            0,
            cursor_info.hCursor,
            cursor_width as i32,
            cursor_height as i32,
            0,
            None,
            DI_NORMAL,
        )
    };
    // Copy the DIB pixels (BGRA, top-down) into an owned sprite buffer.
    let sprite_len = (cursor_width * cursor_height * 4) as usize;
    let sprite_pixels =
        unsafe { std::slice::from_raw_parts(bits as *const u8, sprite_len) }.to_vec();
    let _ = unsafe { SelectObject(dc, previous) };
    let _ = unsafe { DeleteObject(dib) };
    let _ = unsafe { DeleteDC(dc) };
    let _ = unsafe { DeleteObject(icon_info.hbmColor) };
    let _ = unsafe { DeleteObject(icon_info.hbmMask) };

    let sprite = CursorSprite::new(
        cursor_width,
        cursor_height,
        icon_info.xHotspot,
        icon_info.yHotspot,
        sprite_pixels,
    )
    .map_err(|e| ScreenShareError::new(e.to_string()))?;
    let cursor_pos = DesktopPoint {
        x: cursor_info.ptScreenPos.x,
        y: cursor_info.ptScreenPos.y,
    };
    composite_cursor(frame, width, height, cursor_pos, geometry, &sprite);
    Ok(())
}

/// Enumerate all monitors into (stable id, handle) pairs.
fn enumerate_monitors() -> Result<Vec<(CaptureSourceId, HMONITOR)>, CaptureFailureKind> {
    let mut result: Vec<(CaptureSourceId, HMONITOR)> = Vec::new();
    let data = &mut result as *mut Vec<(CaptureSourceId, HMONITOR)>;
    let ok = unsafe {
        EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(enum_monitor_proc),
            LPARAM(data as isize),
        )
    };
    if ok.0 == 0 {
        return Err(CaptureFailureKind::Api(0));
    }
    Ok(result)
}

unsafe extern "system" fn enum_monitor_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let result = unsafe { &mut *(data.0 as *mut Vec<(CaptureSourceId, HMONITOR)>) };
    let info = monitor_info(hmon);
    if !info.device_name.is_empty() {
        let id = super::windows_common::monitor_source_id(&info.device_name);
        result.push((id, hmon));
    }
    BOOL(1)
}

/// Enumerate visible top-level windows into (stable id, HWND, title, rect)
/// tuples (BORU-SS-36).
///
/// Only windows that are visible (`IsWindowVisible`), have a non-empty title,
/// and a non-zero client rect are advertised — tooltips, popups and
/// background helper windows are noise for a sharing source picker. The raw
/// `HWND` is kept so [`DesktopCaptureBackend::start`] can call
/// `GraphicsCaptureItem.CreateForWindow`; the title/rect feed the advertised
/// [`CaptureSource`].
fn enumerate_windows() -> Result<Vec<(CaptureSourceId, HWND, String, RECT)>, CaptureFailureKind> {
    let mut result: Vec<(CaptureSourceId, HWND, String, RECT)> = Vec::new();
    let data = &mut result as *mut Vec<(CaptureSourceId, HWND, String, RECT)>;
    unsafe {
        EnumWindows(Some(enum_window_proc), LPARAM(data as isize))
            .map_err(|e| CaptureFailureKind::Api(e.code().0 as u32))?;
    }
    Ok(result)
}

unsafe extern "system" fn enum_window_proc(
    hwnd: HWND,
    data: LPARAM,
) -> BOOL {
    let result = unsafe { &mut *(data.0 as *mut Vec<(CaptureSourceId, HWND, String, RECT)>) };
    if unsafe { IsWindowVisible(hwnd) }.0 == 0 {
        return BOOL(1); // skip hidden windows
    }
    let mut title = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, &mut title) };
    if len <= 0 {
        return BOOL(1); // skip windows without a title
    }
    let title = utf16_to_string(&title[..len as usize]);
    if title.is_empty() {
        return BOOL(1);
    }
    let mut rect = RECT::default();
    let got = unsafe { GetWindowRect(hwnd, &mut rect) };
    if got.is_err() || rect.right <= rect.left || rect.bottom <= rect.top {
        return BOOL(1); // skip zero-size / invalid windows
    }
    let id = super::windows_common::window_source_id(hwnd.0 as usize);
    result.push((id, hwnd, title, rect));
    BOOL(1)
}

/// Convert a NUL-terminated UTF-16 buffer to a Rust string.
fn utf16_to_string(units: &[u16]) -> String {
    let len = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..len])
}
