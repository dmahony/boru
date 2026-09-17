//! Windows Media Foundation H.264 encoder.
//!
//! This module uses the documented IMFTransform API and the hardware-only MFT
//! enumeration flag. The input boundary is deliberately CPU NV12 because the
//! capture subsystem currently exposes `CapturedFrame` CPU pixels; this is not
//! presented as a zero-copy GPU path.
//!
//! Sources: Microsoft Media Foundation H.264 Video Encoder and IMFTransform
//! documentation (learn.microsoft.com/windows/win32/medfound).

use super::capture::{CapturedFrame, PixelFormat};
use super::codec::{
    now_micros, CodecConfig, CodecKind, CodecMetadata, EncodedPacket, VideoEncoder,
};
use super::ScreenShareError;
use windows::core::GUID;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;

fn mf_error(context: &str, error: impl std::fmt::Display) -> ScreenShareError {
    ScreenShareError::hardware_acceleration_unavailable(format!(
        "Media Foundation {context}: {error}"
    ))
}

fn media_type(config: CodecConfig, subtype: &GUID) -> Result<IMFMediaType, ScreenShareError> {
    unsafe {
        let ty = MFCreateMediaType().map_err(|e| mf_error("media type creation", e))?;
        ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| mf_error("major type", e))?;
        ty.SetGUID(&MF_MT_SUBTYPE, subtype)
            .map_err(|e| mf_error("subtype", e))?;
        ty.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|e| mf_error("interlace mode", e))?;
        ty.SetUINT64(
            &MF_MT_FRAME_SIZE,
            ((config.width as u64) << 32) | config.height as u64,
        )
        .map_err(|e| mf_error("frame size", e))?;
        ty.SetUINT64(&MF_MT_FRAME_RATE, (config.target_fps as u64) << 32 | 1)
            .map_err(|e| mf_error("frame rate", e))?;
        Ok(ty)
    }
}

fn rgb_to_nv12(frame: &CapturedFrame, config: CodecConfig) -> Result<Vec<u8>, ScreenShareError> {
    if !matches!(frame.pixel_format, PixelFormat::Bgra8 | PixelFormat::Rgba8) {
        return Err(ScreenShareError::new(
            "Media Foundation requires CPU BGRA8 or RGBA8",
        ));
    }
    let expected = frame
        .width
        .checked_mul(frame.height)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| ScreenShareError::new("frame dimensions overflow"))?
        as usize;
    if frame.pixels.len() != expected {
        return Err(ScreenShareError::new(
            "frame payload does not match dimensions",
        ));
    }
    let w = config.width as usize;
    let h = config.height as usize;
    let mut out = vec![0u8; w * h + w * h / 2];
    for y in 0..h {
        for x in 0..w {
            let sx = x * frame.width as usize / w;
            let sy = y * frame.height as usize / h;
            let i = (sy * frame.width as usize + sx) * 4;
            let (r, g, b) = if frame.pixel_format == PixelFormat::Bgra8 {
                (frame.pixels[i + 2], frame.pixels[i + 1], frame.pixels[i])
            } else {
                (frame.pixels[i], frame.pixels[i + 1], frame.pixels[i + 2])
            };
            out[y * w + x] = (0.257 * r as f32 + 0.504 * g as f32 + 0.098 * b as f32 + 16.0)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
    let uv = w * h;
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let mut u = 0.0;
            let mut v = 0.0;
            for dy in 0..2 {
                for dx in 0..2 {
                    let sx = (x + dx).min(w - 1) * frame.width as usize / w;
                    let sy = (y + dy).min(h - 1) * frame.height as usize / h;
                    let i = (sy * frame.width as usize + sx) * 4;
                    let (r, g, b) = if frame.pixel_format == PixelFormat::Bgra8 {
                        (frame.pixels[i + 2], frame.pixels[i + 1], frame.pixels[i])
                    } else {
                        (frame.pixels[i], frame.pixels[i + 1], frame.pixels[i + 2])
                    };
                    u += -0.148 * r as f32 - 0.291 * g as f32 + 0.439 * b as f32 + 128.0;
                    v += 0.439 * r as f32 - 0.368 * g as f32 - 0.071 * b as f32 + 128.0;
                }
            }
            let o = uv + (y / 2) * w + x;
            out[o] = (u / 4.0).round().clamp(0.0, 255.0) as u8;
            out[o + 1] = (v / 4.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    Ok(out)
}

pub fn hardware_available() -> bool {
    unsafe {
        if MFStartup(MF_VERSION, MFSTARTUP_FULL).is_err() {
            return false;
        }
        let result = enumerate_hardware_mft().map(|_| ()).is_ok();
        let _ = MFShutdown();
        result
    }
}

unsafe fn enumerate_hardware_mft() -> Result<IMFTransform, ScreenShareError> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0;
    MFTEnumEx(
        MFT_CATEGORY_VIDEO_ENCODER,
        MFT_ENUM_FLAG_HARDWARE,
        Some(&input),
        Some(&output),
        &mut activates,
        &mut count,
    )
    .map_err(|e| mf_error("hardware MFT enumeration", e))?;
    if activates.is_null() || count == 0 {
        return Err(ScreenShareError::hardware_acceleration_unavailable(
            "no hardware Media Foundation H.264 encoder was found",
        ));
    }
    let activation = (*activates).clone().ok_or_else(|| {
        ScreenShareError::hardware_acceleration_unavailable(
            "hardware MFT activation entry was empty",
        )
    });
    let transform = activation.and_then(|item| {
        item.ActivateObject::<IMFTransform>()
            .map_err(|e| mf_error("hardware encoder activation", e))
    });
    CoTaskMemFree(Some(activates as _));
    transform
}

pub fn create(config: CodecConfig) -> Result<Box<dyn VideoEncoder>, ScreenShareError> {
    unsafe {
        MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| mf_error("startup", e))?;
    }
    match MfEncoder::new(config) {
        Ok(encoder) => Ok(Box::new(encoder)),
        Err(error) => {
            unsafe {
                let _ = MFShutdown();
            };
            Err(error)
        }
    }
}

pub struct MfEncoder {
    transform: IMFTransform,
    config: CodecConfig,
    generation: u64,
    sequence: u64,
    frames_since_keyframe: u64,
    keyframe_requested: bool,
    shutdown: bool,
}

impl MfEncoder {
    fn new(config: CodecConfig) -> Result<Self, ScreenShareError> {
        let transform = unsafe { enumerate_hardware_mft()? };
        let input = media_type(config, &MFVideoFormat_NV12)?;
        let output = media_type(config, &MFVideoFormat_H264)?;
        unsafe {
            transform
                .SetInputType(0, &input, 0)
                .map_err(|e| mf_error("input type", e))?;
            transform
                .SetOutputType(0, &output, 0)
                .map_err(|e| mf_error("output type", e))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|e| mf_error("begin streaming", e))?;
        }
        Ok(Self {
            transform,
            config,
            generation: 0,
            sequence: 0,
            frames_since_keyframe: 0,
            keyframe_requested: true,
            shutdown: false,
        })
    }
    fn running(&self) -> Result<(), ScreenShareError> {
        if self.shutdown {
            Err(ScreenShareError::new("encoder is shut down"))
        } else {
            Ok(())
        }
    }
}

impl VideoEncoder for MfEncoder {
    fn configure(&mut self, config: CodecConfig) -> Result<(), ScreenShareError> {
        self.running()?;
        let mut replacement = Self::new(config)?;
        replacement.generation = self.generation + 1;
        self.transform = replacement.transform;
        self.config = replacement.config;
        self.generation = replacement.generation;
        self.frames_since_keyframe = 0;
        self.keyframe_requested = true;
        Ok(())
    }
    fn encode(&mut self, frame: &CapturedFrame) -> Result<EncodedPacket, ScreenShareError> {
        self.running()?;
        let nv12 = rgb_to_nv12(frame, self.config)?;
        unsafe {
            let buffer =
                MFCreateMemoryBuffer(nv12.len() as u32).map_err(|e| mf_error("input buffer", e))?;
            let mut ptr = std::ptr::null_mut();
            buffer
                .Lock(&mut ptr, None, None)
                .map_err(|e| mf_error("input lock", e))?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), ptr, nv12.len());
            buffer.Unlock().map_err(|e| mf_error("input unlock", e))?;
            buffer
                .SetCurrentLength(nv12.len() as u32)
                .map_err(|e| mf_error("input length", e))?;
            let sample = MFCreateSample().map_err(|e| mf_error("input sample", e))?;
            sample
                .AddBuffer(&buffer)
                .map_err(|e| mf_error("attach input", e))?;
            sample
                .SetSampleTime((frame.timestamp_us * 10) as i64)
                .map_err(|e| mf_error("input timestamp", e))?;
            sample
                .SetSampleDuration((10_000_000u64 / self.config.target_fps as u64) as i64)
                .map_err(|e| mf_error("input duration", e))?;
            if self.keyframe_requested
                || self.frames_since_keyframe >= self.config.keyframe_interval
            {
                self.transform
                    .ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)
                    .ok();
                self.keyframe_requested = false;
            }
            self.transform
                .ProcessInput(0, &sample, 0)
                .map_err(|e| mf_error("process input", e))?;
            let info = self
                .transform
                .GetOutputStreamInfo(0)
                .map_err(|e| mf_error("output info", e))?;
            let out_buffer =
                MFCreateMemoryBuffer(info.cbSize.max(self.config.width * self.config.height / 2))
                    .map_err(|e| mf_error("output buffer", e))?;
            let out_sample = MFCreateSample().map_err(|e| mf_error("output sample", e))?;
            out_sample
                .AddBuffer(&out_buffer)
                .map_err(|e| mf_error("attach output", e))?;
            let mut output = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(Some(out_sample)),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(None),
            }];
            let mut status = 0;
            self.transform
                .ProcessOutput(0, &mut output, &mut status)
                .map_err(|e| mf_error("process output", e))?;
            let sample = (&*output[0].pSample).as_ref().ok_or_else(|| {
                ScreenShareError::new("Media Foundation returned no output sample")
            })?;
            let result = sample
                .ConvertToContiguousBuffer()
                .map_err(|e| mf_error("output buffer access", e))?;
            let length = result
                .GetCurrentLength()
                .map_err(|e| mf_error("output length", e))? as usize;
            let mut bytes = vec![0; length];
            let mut p = std::ptr::null_mut();
            result
                .Lock(&mut p, None, None)
                .map_err(|e| mf_error("output lock", e))?;
            std::ptr::copy_nonoverlapping(p, bytes.as_mut_ptr(), length);
            result.Unlock().ok();
            let keyframe = self.keyframe_requested || self.frames_since_keyframe == 0;
            self.frames_since_keyframe = if keyframe {
                1
            } else {
                self.frames_since_keyframe + 1
            };
            let packet = EncodedPacket {
                timestamp_us: frame.timestamp_us,
                encode_timestamp_us: now_micros(),
                sequence: self.sequence,
                keyframe,
                config_generation: self.generation,
                width: self.config.width,
                height: self.config.height,
                bytes,
            };
            self.sequence += 1;
            Ok(packet)
        }
    }
    fn force_keyframe(&mut self) {
        self.keyframe_requested = true;
    }
    fn is_keyframe_pending(&self) -> bool {
        self.keyframe_requested
    }
    fn reconfigure_bitrate(&mut self, bitrate_bps: u32) -> Result<(), ScreenShareError> {
        self.running()?;
        if bitrate_bps == 0 {
            return Err(ScreenShareError::new("bitrate must be non-zero"));
        }
        self.config.target_bitrate_bps = bitrate_bps;
        self.keyframe_requested = true;
        Ok(())
    }
    fn shutdown(&mut self) -> Result<(), ScreenShareError> {
        if !self.shutdown {
            unsafe {
                let _ = self
                    .transform
                    .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
                let _ = MFShutdown();
            };
            self.shutdown = true;
        }
        Ok(())
    }
    fn metadata(&self) -> CodecMetadata {
        CodecMetadata {
            codec: CodecKind::H264Mf,
            config: self.config,
            generation: self.generation,
        }
    }
}

impl Drop for MfEncoder {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nv12_size_is_deterministic() {
        let config = CodecConfig {
            width: 4,
            height: 2,
            ..CodecConfig::default()
        };
        let frame = CapturedFrame::cpu(0, 4, 2, PixelFormat::Rgba8, vec![255; 32]).unwrap();
        assert_eq!(rgb_to_nv12(&frame, config).unwrap().len(), 12);
    }
}
