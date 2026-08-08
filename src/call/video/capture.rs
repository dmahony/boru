//! Consent-gated camera discovery and capture ownership.
//!
//! Enumerating devices uses Nokhwa's query API only. Constructing a Nokhwa
//! [`Camera`] is deliberately deferred until [`CameraCapture::start`] is called
//! by the consent-bearing video-call flow.

use std::{fmt, time::Duration};

use nokhwa::{
    pixel_format::RgbFormat,
    utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType},
    Camera,
};

/// Requested dimensions and cadence for a live camera track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureConfig {
    /// Requested frame width in pixels.
    pub width: u32,
    /// Requested frame height in pixels.
    pub height: u32,
    /// Requested capture interval.
    pub frame_interval: Duration,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            width: 640,
            height: 480,
            frame_interval: Duration::from_millis(33),
        }
    }
}

/// One raw frame leaving the live capture boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedFrame {
    /// Monotonic capture timestamp in microseconds.
    pub timestamp_us: u64,
    /// Raw video bytes owned by the live pipeline.
    pub data: Vec<u8>,
}

/// Capture source abstraction reserved for the camera implementation task.
pub trait CaptureSource: Send {
    /// Return the next captured frame, or `None` when the source is stopped.
    fn next_frame(&mut self) -> Option<CapturedFrame>;
}

/// A camera exposed to the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraDevice {
    /// Stable identifier for this process/platform camera entry.
    pub id: String,
    /// Human-readable name supplied by the native backend.
    pub name: String,
    index: CameraIndex,
}

impl CameraDevice {
    fn from_info(info: &nokhwa::utils::CameraInfo) -> Self {
        let index = info.index().clone();
        Self {
            id: stable_camera_id(&index, &info.misc()),
            name: info.human_name(),
            index,
        }
    }
}

fn stable_camera_id(index: &CameraIndex, misc: &str) -> String {
    // Native backends provide a persistent symbolic identifier in `misc` on
    // macOS/Windows. Linux commonly has only an integer index. Avoid exposing
    // the full backend description in the UI identifier.
    if !misc.trim().is_empty() {
        format!("device-{}", blake3::hash(misc.as_bytes()).to_hex())
    } else {
        match index {
            CameraIndex::Index(index) => format!("index-{index}"),
            CameraIndex::String(index) => {
                format!("device-{}", blake3::hash(index.as_bytes()).to_hex())
            }
        }
    }
}

/// Error returned while discovering or opening a camera.
#[derive(Debug)]
pub enum CameraError {
    /// The native backend reported no camera entries.
    NoCamera,
    /// Device enumeration failed without being a permission failure.
    Enumeration(String),
    /// The operating system denied camera access.
    PermissionDenied(String),
    /// The selected camera could not be opened after consent.
    Open(String),
}

impl fmt::Display for CameraError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCamera => formatter.write_str("no camera is available"),
            Self::Enumeration(error) => write!(formatter, "camera enumeration failed: {error}"),
            Self::PermissionDenied(error) => {
                write!(formatter, "camera permission denied: {error}")
            }
            Self::Open(error) => write!(formatter, "camera could not be opened: {error}"),
        }
    }
}

impl std::error::Error for CameraError {}

/// Enumerate cameras without opening any camera device.
pub fn enumerate_cameras() -> Result<Vec<CameraDevice>, CameraError> {
    let infos = nokhwa::query(ApiBackend::Auto)
        .map_err(|error| CameraError::Enumeration(error.to_string()))?;
    Ok(infos.iter().map(CameraDevice::from_info).collect())
}

/// Select the first usable camera from an enumerated list.
pub fn select_default_camera(cameras: &[CameraDevice]) -> Result<CameraDevice, CameraError> {
    cameras.first().cloned().ok_or(CameraError::NoCamera)
}

/// A camera session that cannot open a device until explicitly started.
pub struct CameraCapture {
    selected: CameraDevice,
    camera: Option<Camera>,
}

impl fmt::Debug for CameraCapture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CameraCapture")
            .field("selected", &self.selected)
            .field("started", &self.is_started())
            .finish()
    }
}

impl CameraCapture {
    /// Create a deferred capture session. This only queries device metadata.
    pub fn new(selected: CameraDevice) -> Self {
        Self {
            selected,
            camera: None,
        }
    }

    /// The camera selected for this session.
    pub fn selected(&self) -> &CameraDevice {
        &self.selected
    }

    /// Whether the native camera has been opened.
    pub fn is_started(&self) -> bool {
        self.camera.is_some()
    }

    /// Open the camera after the caller has obtained explicit user consent.
    pub fn start(&mut self) -> Result<(), CameraError> {
        if self.camera.is_some() {
            return Ok(());
        }
        let format = RequestedFormat::new::<RgbFormat>(RequestedFormatType::None);
        let camera = Camera::new(self.selected.index.clone(), format).map_err(|error| {
            let detail = error.to_string();
            if detail.to_ascii_lowercase().contains("permission") {
                CameraError::PermissionDenied(detail)
            } else {
                CameraError::Open(detail)
            }
        })?;
        self.camera = Some(camera);
        Ok(())
    }

    /// Stop capture and release the native device.
    pub fn stop(&mut self) {
        self.camera = None;
    }

    /// Switch the selected device. Opening the replacement is deferred until
    /// the next explicit [`Self::start`] call.
    pub fn switch_camera(&mut self, selected: CameraDevice) {
        self.stop();
        self.selected = selected;
    }
}

#[cfg(test)]
mod tests {
    use super::{select_default_camera, CameraCapture, CameraDevice};
    use nokhwa::utils::CameraIndex;

    fn device(index: u32, name: &str) -> CameraDevice {
        CameraDevice {
            id: format!("index-{index}"),
            name: name.into(),
            index: CameraIndex::Index(index),
        }
    }

    #[test]
    fn empty_camera_list_is_no_camera() {
        assert!(matches!(
            select_default_camera(&[]),
            Err(super::CameraError::NoCamera)
        ));
    }

    #[test]
    fn default_camera_is_first_in_stable_enumeration_order() {
        let cameras = vec![device(2, "USB"), device(0, "Built-in")];
        assert_eq!(select_default_camera(&cameras).unwrap().id, "index-2");
    }

    #[test]
    fn constructing_capture_does_not_open_camera() {
        let capture = CameraCapture::new(device(0, "test"));
        assert!(!capture.is_started());
    }

    #[test]
    fn enumeration_without_hardware_is_empty_or_typed_failure() {
        match super::enumerate_cameras() {
            Ok(cameras) => assert!(cameras.is_empty()),
            Err(super::CameraError::Enumeration(_)) => {}
            Err(error) => panic!("unexpected camera enumeration error: {error}"),
        }
    }
}
