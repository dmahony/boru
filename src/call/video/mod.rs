//! Video input devices and capture.

pub mod capture;

pub use capture::{
    enumerate_cameras, select_default_camera, CameraCapture, CameraDevice, CameraError,
};
