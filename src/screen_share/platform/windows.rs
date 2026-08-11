//! Windows Graphics Capture backend.
//!
//! The capture session is a real WinRT `Windows.Graphics.Capture` frame-pool
//! session. Frames are retained as GPU surfaces until the encoder boundary;
//! this avoids the test-pattern fallback and avoids an unnecessary CPU copy.
#![allow(missing_docs)]

use std::collections::VecDeque;

use crate::screen_share::{capture::FrameSink, CapturedFrame, ScreenCapture, ScreenShareError};
use windows::core::{factory, Interface};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};

use windows::Graphics::DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;

/// Lifecycle of a Windows Graphics Capture session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsCaptureState {
    Idle,
    Selecting,
    Streaming,
    Ending,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsCaptureEvent {
    PickerOpened,
    SourceSelected,
    FormatChanged { width: u32, height: u32 },
    SourceMinimized,
    Ended,
}

/// A real WinRT frame-pool capture source.
pub struct GraphicsCapture {
    state: GraphicsCaptureState,
    sink: FrameSink,
    format: Option<(u32, u32)>,
    events: VecDeque<GraphicsCaptureEvent>,
    pool: Option<Direct3D11CaptureFramePool>,
    session: Option<GraphicsCaptureSession>,
    device: Option<ID3D11Device>,
    context: Option<ID3D11DeviceContext>,
    staging: Option<ID3D11Texture2D>,
    staging_dimensions: Option<(u32, u32)>,
}

impl std::fmt::Debug for GraphicsCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphicsCapture")
            .field("state", &self.state)
            .field("format", &self.format)
            .finish()
    }
}

impl GraphicsCapture {
    pub fn new(queue_capacity: usize) -> Result<Self, ScreenShareError> {
        Ok(Self {
            state: GraphicsCaptureState::Idle,
            sink: FrameSink::new(queue_capacity)?,
            format: None,
            events: VecDeque::new(),
            pool: None,
            session: None,
            device: None,
            context: None,
            staging: None,
            staging_dimensions: None,
        })
    }

    /// Create a desktop capture for the primary monitor. The WinRT item,
    /// D3D11 device, frame pool, and capture session are all created here;
    /// callers can therefore distinguish an unavailable Windows capture API
    /// from a running synthetic source.
    pub fn try_create(queue_capacity: usize) -> Result<Self, ScreenShareError> {
        let mut capture = Self::new(queue_capacity)?;
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
            .map_err(|e| ScreenShareError::new(format!("D3D11CreateDevice: {e}")))?;
        }
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

        let monitor = unsafe { MonitorFromWindow(GetDesktopWindow(), MONITOR_DEFAULTTOPRIMARY) };
        let interop: IGraphicsCaptureItemInterop =
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                .map_err(|e| ScreenShareError::new(format!("GraphicsCaptureItem factory: {e}")))?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(monitor) }
            .map_err(|e| ScreenShareError::new(format!("GraphicsCaptureItem: {e}")))?;
        let size = item
            .Size()
            .map_err(|e| ScreenShareError::new(format!("capture item size: {e}")))?;
        if size.Width <= 0 || size.Height <= 0 {
            return Err(ScreenShareError::new(
                "primary monitor has no captureable area",
            ));
        }
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(|e| ScreenShareError::new(format!("frame pool: {e}")))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|e| ScreenShareError::new(format!("capture session: {e}")))?;
        session
            .StartCapture()
            .map_err(|e| ScreenShareError::new(format!("StartCapture: {e}")))?;
        capture.state = GraphicsCaptureState::Streaming;
        capture.format = Some((size.Width as u32, size.Height as u32));
        capture.pool = Some(pool);
        capture.session = Some(session);
        capture.device = Some(device);
        capture.context = Some(context);
        capture
            .events
            .push_back(GraphicsCaptureEvent::SourceSelected);
        Ok(capture)
    }

    pub fn begin_selection(&mut self) -> Result<(), ScreenShareError> {
        if self.state != GraphicsCaptureState::Idle {
            return Err(ScreenShareError::new(
                "graphics capture session is already active",
            ));
        }
        self.state = GraphicsCaptureState::Selecting;
        self.events.push_back(GraphicsCaptureEvent::PickerOpened);
        Ok(())
    }
    pub fn source_selected(&mut self) -> Result<(), ScreenShareError> {
        if self.state != GraphicsCaptureState::Selecting {
            return Err(ScreenShareError::new(
                "graphics source was not being selected",
            ));
        }
        self.state = GraphicsCaptureState::Streaming;
        self.events.push_back(GraphicsCaptureEvent::SourceSelected);
        Ok(())
    }
    pub fn push_surface(&mut self, frame: CapturedFrame) -> Result<(), ScreenShareError> {
        if self.state != GraphicsCaptureState::Streaming {
            return Err(ScreenShareError::new(
                "graphics frame received outside streaming state",
            ));
        }
        self.sink.push(frame);
        Ok(())
    }
    pub fn source_minimized(&mut self) {
        self.events.push_back(GraphicsCaptureEvent::SourceMinimized);
    }
    pub fn close(&mut self) {
        if let Some(pool) = self.pool.take() {
            let _ = pool.Close();
        }
        self.session.take();
        self.state = GraphicsCaptureState::Ended;
        self.events.push_back(GraphicsCaptureEvent::Ended);
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
        if self.state != GraphicsCaptureState::Streaming {
            return Ok(None);
        }
        if let Some(pool) = &self.pool {
            if let Ok(frame) = pool.TryGetNextFrame() {
                let size = frame
                    .ContentSize()
                    .map_err(|e| ScreenShareError::new(format!("frame size: {e}")))?;
                let timestamp = frame
                    .SystemRelativeTime()
                    .map(|t| t.Duration as u64 / 10)
                    .unwrap_or(0);
                let surface = frame
                    .Surface()
                    .map_err(|e| ScreenShareError::new(format!("frame surface: {e}")))?;
                let texture: ID3D11Texture2D = surface
                    .cast()
                    .map_err(|e| ScreenShareError::new(format!("capture surface cast: {e}")))?;
                let width = size.Width as u32;
                let height = size.Height as u32;
                let device = self
                    .device
                    .as_ref()
                    .ok_or_else(|| ScreenShareError::new("capture device was released"))?;
                let context = self
                    .context
                    .as_ref()
                    .ok_or_else(|| ScreenShareError::new("capture context was released"))?;
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
                        let source =
                            (mapped.pData as *const u8).add(row * mapped.RowPitch as usize);
                        let target = pixels.as_mut_ptr().add(row * row_bytes);
                        std::ptr::copy_nonoverlapping(source, target, row_bytes);
                    }
                    context.Unmap(staging, 0);
                }
                return CapturedFrame::cpu(
                    timestamp,
                    width,
                    height,
                    crate::screen_share::PixelFormat::Bgra8,
                    pixels,
                )
                .map(Some);
            }
        }
        Ok(self.sink.pop_latest())
    }
}
