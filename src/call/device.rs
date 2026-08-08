//! Audio device access for voice calls.
//!
//! This module is the only CPAL-facing layer.  Its callbacks carry audio
//! samples and errors only; they do not know about Iroh connections, call
//! state, or protocol messages.  Higher layers can therefore move samples
//! into a queue without running network code on a real-time audio thread.

use std::fmt;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// A stable, displayable description of an audio device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    /// Backend-provided device identifier when available.
    pub id: String,
    /// Human-readable device name.
    pub name: String,
}

/// Stream parameters understood by the audio boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioStreamConfig {
    /// Number of interleaved channels.
    pub channels: u16,
    /// Samples per second.
    pub sample_rate: u32,
}

impl Default for AudioStreamConfig {
    fn default() -> Self {
        Self {
            channels: 1,
            sample_rate: 48_000,
        }
    }
}

/// Errors returned by an audio backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioDeviceError {
    /// The native backend rejected an operation.
    Backend(String),
    /// The selected device disappeared before opening its stream.
    DeviceNotFound(String),
}

impl fmt::Display for AudioDeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(message) => write!(f, "audio backend error: {message}"),
            Self::DeviceNotFound(name) => write!(f, "audio device not found: {name}"),
        }
    }
}

impl std::error::Error for AudioDeviceError {}

/// Callback invoked with a batch of mono/stereo f32 input samples.
pub type InputCallback = Box<dyn FnMut(&[f32]) + Send + 'static>;
/// Callback used to report an asynchronous stream failure.
pub type ErrorCallback = Box<dyn FnMut(String) + Send + 'static>;
/// Callback invoked when output samples should be filled.
pub type OutputCallback = Box<dyn FnMut(&mut [f32]) + Send + 'static>;

/// Narrow boundary between call audio code and a native device implementation.
pub trait AudioDeviceBackend {
    /// Native input stream handle.
    type InputHandle;
    /// Native output stream handle.
    type OutputHandle;

    /// Enumerate devices that can capture audio.
    fn enumerate_input_devices(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError>;
    /// Enumerate devices that can play audio.
    fn enumerate_output_devices(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError>;
    /// Open a capture stream. The callback must do bounded, non-blocking work.
    fn open_input(
        &self,
        device: &AudioDeviceInfo,
        config: AudioStreamConfig,
        callback: InputCallback,
        error_callback: ErrorCallback,
    ) -> Result<Self::InputHandle, AudioDeviceError>;
    /// Open a playback stream. The callback must do bounded, non-blocking work.
    fn open_output(
        &self,
        device: &AudioDeviceInfo,
        config: AudioStreamConfig,
        callback: OutputCallback,
        error_callback: ErrorCallback,
    ) -> Result<Self::OutputHandle, AudioDeviceError>;
    /// Pause an input stream and release its real-time activity.
    fn stop_input(&self, handle: &mut Self::InputHandle) -> Result<(), AudioDeviceError>;
    /// Pause an output stream and release its real-time activity.
    fn stop_output(&self, handle: &mut Self::OutputHandle) -> Result<(), AudioDeviceError>;
}

/// CPAL implementation of [`AudioDeviceBackend`].
pub struct CpalAudioDeviceBackend {
    host: cpal::Host,
}

impl fmt::Debug for CpalAudioDeviceBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CpalAudioDeviceBackend")
            .finish_non_exhaustive()
    }
}

impl Default for CpalAudioDeviceBackend {
    fn default() -> Self {
        Self {
            host: cpal::default_host(),
        }
    }
}

impl CpalAudioDeviceBackend {
    /// Construct the backend using CPAL's platform default host.
    pub fn new() -> Self {
        Self::default()
    }

    /// Select a named device, falling back to the host default and then the
    /// first enumerated device.  Missing hardware is represented by `None`.
    pub fn select_input_device(
        &self,
        preferred_name: Option<&str>,
    ) -> Result<Option<AudioDeviceInfo>, AudioDeviceError> {
        self.select_device(preferred_name, true)
    }

    /// Select a named output device with the same graceful fallback policy as
    /// [`Self::select_input_device`].
    pub fn select_output_device(
        &self,
        preferred_name: Option<&str>,
    ) -> Result<Option<AudioDeviceInfo>, AudioDeviceError> {
        self.select_device(preferred_name, false)
    }

    fn select_device(
        &self,
        preferred_name: Option<&str>,
        input: bool,
    ) -> Result<Option<AudioDeviceInfo>, AudioDeviceError> {
        let devices = if input {
            self.enumerate_input_devices()?
        } else {
            self.enumerate_output_devices()?
        };
        if let Some(name) = preferred_name {
            if let Some(device) = devices.iter().find(|device| device.name == name) {
                return Ok(Some(device.clone()));
            }
        }
        let default = if input {
            self.host.default_input_device()
        } else {
            self.host.default_output_device()
        };
        if let Some(device) = default {
            let info = Self::device_info(&device)?;
            if devices.iter().any(|candidate| candidate.id == info.id) {
                return Ok(Some(info));
            }
        }
        Ok(devices.into_iter().next())
    }

    fn device_info(device: &cpal::Device) -> Result<AudioDeviceInfo, AudioDeviceError> {
        let name = device
            .description()
            .map(|description| description.to_string())
            .or_else(|_| Ok(device.to_string()))
            .map_err(Self::backend_error)?;
        let id = device
            .id()
            .map(|id| id.to_string())
            .unwrap_or_else(|_| name.clone());
        Ok(AudioDeviceInfo { id, name })
    }

    fn backend_error(error: cpal::Error) -> AudioDeviceError {
        AudioDeviceError::Backend(error.to_string())
    }

    fn find_input(&self, info: &AudioDeviceInfo) -> Result<cpal::Device, AudioDeviceError> {
        self.host
            .input_devices()
            .map_err(Self::backend_error)?
            .find(|device| device_info_matches(device, info))
            .ok_or_else(|| AudioDeviceError::DeviceNotFound(info.name.clone()))
    }

    fn find_output(&self, info: &AudioDeviceInfo) -> Result<cpal::Device, AudioDeviceError> {
        self.host
            .output_devices()
            .map_err(Self::backend_error)?
            .find(|device| device_info_matches(device, info))
            .ok_or_else(|| AudioDeviceError::DeviceNotFound(info.name.clone()))
    }

    /// Open a capture stream backed by a bounded, lock-free SPSC queue.
    ///
    /// The returned consumer belongs to the audio worker. CPAL receives only
    /// the queue producer callback, so the real-time thread never performs
    /// work beyond a bounded non-blocking copy and drop-newest handling.
    pub fn open_input_buffered(
        &self,
        info: &AudioDeviceInfo,
        config: AudioStreamConfig,
        capacity: usize,
        error_callback: ErrorCallback,
    ) -> Result<(cpal::Stream, super::audio::AudioCaptureConsumer), AudioDeviceError> {
        let (producer, consumer) = super::audio::new_capture_buffer(capacity);
        let stream = self.open_input(info, config, producer.into_callback(), error_callback)?;
        Ok((stream, consumer))
    }
}

fn device_info_matches(device: &cpal::Device, info: &AudioDeviceInfo) -> bool {
    let id_matches = device
        .id()
        .map(|id| id.to_string() == info.id)
        .unwrap_or(false);
    id_matches || device.to_string() == info.name
}

impl AudioDeviceBackend for CpalAudioDeviceBackend {
    type InputHandle = cpal::Stream;
    type OutputHandle = cpal::Stream;

    fn enumerate_input_devices(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
        Ok(self
            .host
            .input_devices()
            .map_err(Self::backend_error)?
            .filter_map(|device| Self::device_info(&device).ok())
            .collect())
    }

    fn enumerate_output_devices(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
        Ok(self
            .host
            .output_devices()
            .map_err(Self::backend_error)?
            .filter_map(|device| Self::device_info(&device).ok())
            .collect())
    }

    fn open_input(
        &self,
        info: &AudioDeviceInfo,
        config: AudioStreamConfig,
        mut callback: InputCallback,
        mut error_callback: ErrorCallback,
    ) -> Result<Self::InputHandle, AudioDeviceError> {
        let device = self.find_input(info)?;
        let stream_config = cpal::StreamConfig {
            channels: config.channels,
            sample_rate: config.sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };
        device
            .build_input_stream::<f32, _, _>(
                stream_config,
                move |samples, _| callback(samples),
                move |error| error_callback(error.to_string()),
                Some(Duration::from_secs(2)),
            )
            .map_err(Self::backend_error)
    }

    fn open_output(
        &self,
        info: &AudioDeviceInfo,
        config: AudioStreamConfig,
        mut callback: OutputCallback,
        mut error_callback: ErrorCallback,
    ) -> Result<Self::OutputHandle, AudioDeviceError> {
        let device = self.find_output(info)?;
        let stream_config = cpal::StreamConfig {
            channels: config.channels,
            sample_rate: config.sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };
        device
            .build_output_stream::<f32, _, _>(
                stream_config,
                move |samples, _| callback(samples),
                move |error| error_callback(error.to_string()),
                Some(Duration::from_secs(2)),
            )
            .map_err(Self::backend_error)
    }

    fn stop_input(&self, handle: &mut Self::InputHandle) -> Result<(), AudioDeviceError> {
        handle.pause().map_err(Self::backend_error)
    }

    fn stop_output(&self, handle: &mut Self::OutputHandle) -> Result<(), AudioDeviceError> {
        handle.pause().map_err(Self::backend_error)
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioDeviceBackend, CpalAudioDeviceBackend};

    #[test]
    fn input_enumeration_is_safe_without_audio_hardware() {
        let backend = CpalAudioDeviceBackend::new();
        // CI runners commonly have no ALSA/PulseAudio device. Either an empty
        // list or a backend error is a valid, non-panicking result.
        let _ = backend.enumerate_input_devices();
    }

    #[test]
    fn output_enumeration_is_safe_without_audio_hardware() {
        let backend = CpalAudioDeviceBackend::new();
        let _ = backend.enumerate_output_devices();
    }

    #[test]
    fn default_selection_falls_back_without_panicking() {
        let backend = CpalAudioDeviceBackend::new();
        let _ = backend.select_input_device(Some("device-that-does-not-exist"));
        let _ = backend.select_output_device(Some("device-that-does-not-exist"));
    }
}
