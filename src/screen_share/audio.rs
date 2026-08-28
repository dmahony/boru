//! System-audio sharing for screen-share sessions (BORU-SS-37 / PDF Phase 14).
//!
//! RustDesk-style screen sharing carries system audio alongside the video.
//! This module implements the Boru-native audio pipeline behind the
//! `screen-sharing` feature:
//!
//! ```text
//! host:  PipeWire loopback capture ──► bounded SPSC ring ──► Opus encode
//!         ──► dedicated AUDIO_KIND stream on the screen-share QUIC path
//! viewer: Opus decode ──► bounded ring ──► cpal output stream
//! ```
//!
//! Design rules (mirroring the video pipeline):
//!
//! - **Opt-in capability.** System audio is a SEPARATE optional capability
//!   (`Capability::Audio`), never enabled automatically with the screen
//!   share; the host grants it explicitly (mirroring clipboard, PDF
//!   Task 9.3) and the viewer authorizes every packet against the grant.
//! - **Never block video on audio.** Audio rides its own stream kind and its
//!   own bounded rings; the video capture/encode loop is untouched. A slow
//!   audio producer drops samples (bounded), never backpressures video.
//! - **Bounded memory.** Every queue is a fixed-capacity SPSC ring (rtrb);
//!   there is no unbounded allocation on the real-time path.
//! - **Typed unavailable errors.** When the platform backend or an output
//!   device is missing, capture/playback fails with
//!   [`ScreenShareErrorKind::AudioUnavailable`] instead of a generic error,
//!   so the UI can tell the user exactly what is missing.
//!
//! Platform availability (v1):
//!
//! - **Linux**: PipeWire loopback capture via dlopen (`libpipewire-0.3.so.0`,
//!   a runtime dependency present on any desktop with audio). The capture
//!   stream targets the default audio sink (`target.object = audio.sink`),
//!   which records what is being played — the system-audio loopback.
//! - **Windows**: WASAPI loopback capture (`IAudioCaptureClient` loopback
//!   mode) is NOT yet implemented in this build; the backend fails with a
//!   typed [`ScreenShareErrorKind::AudioUnavailable`] and the gap is
//!   documented in `docs/screenshare-audio.md`.
//! - **Playback**: cpal output stream on the viewer (same crate as
//!   `voice-calls`). Opens the device's default output config; the callback
//!   converts f32 PCM to the device sample format and maps 1/2-channel
//!   layouts. If the device default rate differs from the 48 kHz wire rate,
//!   audio plays at the device rate without resampling (v1 gap).
//!
//! No RustDesk code was consulted; the PipeWire client follows PipeWire's
//! own tutorial (`page_tutorial4.html`) and the Boru video capture pattern in
//! `platform/linux.rs`. The Opus codec is RFC 6716 via the `opus` crate
//! (BSD-3), the same crate used by `voice-calls`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use opus::{Application, Bitrate, Channels as OpusChannels, Decoder, Encoder, FrameSize};
use rtrb::{Consumer, Producer, RingBuffer};

use cpal::traits::{DeviceTrait, HostTrait};

use super::{protocol::MAX_AUDIO_FRAME, ScreenShareError};

/// Wire sample rate for shared system audio (48 kHz, RFC 6716 §2.1.1).
pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
/// Wire channel count for shared system audio (stereo).
pub const AUDIO_CHANNELS: u16 = 2;
/// Opus frame duration used by the shared-audio profile (20 ms).
pub const AUDIO_FRAME_MS: u64 = 20;
/// Interleaved samples per Opus frame at the wire format (rate/1000 * ms * ch).
pub const AUDIO_SAMPLES_PER_FRAME: usize =
    (AUDIO_SAMPLE_RATE as usize / 1000) * AUDIO_FRAME_MS as usize * AUDIO_CHANNELS as usize;
/// Samples per channel per Opus frame (960 for 48 kHz / 20 ms).
pub const AUDIO_SAMPLES_PER_CHANNEL: usize = AUDIO_SAMPLES_PER_FRAME / AUDIO_CHANNELS as usize;
/// Target Opus bitrate for system audio (stereo music profile).
pub const AUDIO_BITRATE_BPS: i32 = 96_000;
/// Default ring capacity in samples (≈ 2 seconds of 48 kHz stereo).
pub const AUDIO_RING_SAMPLES: usize = AUDIO_SAMPLE_RATE as usize * AUDIO_CHANNELS as usize * 2;

/// A bounded SPSC sample ring between the real-time capture/playback side and
/// the worker side. Producers drop-newest when full; consumers never wait.
pub type AudioSampleProducer = Producer<f32>;
/// Consumer side of [`AudioSampleProducer`].
pub type AudioSampleConsumer = Consumer<f32>;

/// Create a bounded audio sample ring.
pub fn audio_sample_ring(capacity: usize) -> (AudioSampleProducer, AudioSampleConsumer) {
    RingBuffer::new(capacity.max(1))
}

/// Stateful Opus encoder for shared system audio (48 kHz stereo, 20 ms frames).
///
/// Opus keeps prediction state between calls, so one instance must be reused
/// for the whole session. Input must be exactly [`AUDIO_SAMPLES_PER_FRAME`]
/// interleaved f32 samples.
#[derive(Debug)]
pub struct OpusAudioEncoder {
    encoder: Encoder,
}

impl OpusAudioEncoder {
    /// Create the shared-audio profile encoder.
    pub fn new() -> Result<Self, ScreenShareError> {
        let mut encoder = Encoder::new(AUDIO_SAMPLE_RATE, OpusChannels::Stereo, Application::Audio)
            .map_err(|e| ScreenShareError::new(format!("opus encoder init failed: {e}")))?;
        encoder
            .set_bitrate(Bitrate::Bits(AUDIO_BITRATE_BPS))
            .map_err(|e| ScreenShareError::new(format!("opus bitrate failed: {e}")))?;
        encoder
            .set_vbr(true)
            .map_err(|e| ScreenShareError::new(format!("opus vbr failed: {e}")))?;
        encoder
            .set_force_channels(Some(OpusChannels::Stereo))
            .map_err(|e| ScreenShareError::new(format!("opus channel force failed: {e}")))?;
        encoder
            .set_expert_frame_duration(FrameSize::Ms20)
            .map_err(|e| ScreenShareError::new(format!("opus frame size failed: {e}")))?;
        encoder
            .set_complexity(6)
            .map_err(|e| ScreenShareError::new(format!("opus complexity failed: {e}")))?;
        Ok(Self { encoder })
    }

    /// Encode one 20 ms frame of interleaved stereo PCM. Returns `None` only
    /// for an empty DTX result; a comfort-noise packet remains `Some`.
    pub fn encode_frame(&mut self, pcm: &[f32]) -> Result<Option<Vec<u8>>, ScreenShareError> {
        if pcm.len() != AUDIO_SAMPLES_PER_FRAME {
            return Err(ScreenShareError::new(format!(
                "opus expects exactly {AUDIO_SAMPLES_PER_FRAME} interleaved samples, got {}",
                pcm.len()
            )));
        }
        let packet = self
            .encoder
            .encode_vec_float(pcm, MAX_AUDIO_FRAME)
            .map_err(|e| ScreenShareError::new(format!("opus encode failed: {e}")))?;
        Ok((!packet.is_empty()).then_some(packet))
    }

    /// The wire sample rate the encoder produces.
    pub fn sample_rate(&self) -> u32 {
        AUDIO_SAMPLE_RATE
    }

    /// The wire channel count the encoder produces.
    pub fn channels(&self) -> u16 {
        AUDIO_CHANNELS
    }
}

/// Stateful Opus decoder for received shared-audio frames.
///
/// The decoder is created with the wire sample rate/channels from the first
/// packet; later packets carrying the same rate/channels decode cleanly.
#[derive(Debug)]
pub struct OpusAudioDecoder {
    decoder: Decoder,
    sample_rate: u32,
    channels: u16,
}

impl OpusAudioDecoder {
    /// Create a decoder for `sample_rate` (8000..=48000) and `channels`
    /// (1..=2).
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, ScreenShareError> {
        if !(super::protocol::MIN_AUDIO_SAMPLE_RATE..=super::protocol::MAX_AUDIO_SAMPLE_RATE)
            .contains(&sample_rate)
        {
            return Err(ScreenShareError::new(
                "opus decoder sample rate out of range",
            ));
        }
        if channels == 0 || channels > 2 {
            return Err(ScreenShareError::new(
                "opus decoder channel count out of range",
            ));
        }
        let opus_channels = if channels == 2 {
            OpusChannels::Stereo
        } else {
            OpusChannels::Mono
        };
        let decoder = Decoder::new(sample_rate, opus_channels)
            .map_err(|e| ScreenShareError::new(format!("opus decoder init failed: {e}")))?;
        Ok(Self {
            decoder,
            sample_rate,
            channels,
        })
    }

    /// Decode one Opus packet into `out` (interleaved f32). Returns the
    /// number of interleaved samples written. `out` must hold at least the
    /// frame's sample count (rate/1000 * 20ms * channels). The `opus` crate
    /// reports samples per channel, so the returned count is scaled by the
    /// channel count to match the interleaved buffer.
    pub fn decode_frame(
        &mut self,
        packet: &[u8],
        out: &mut [f32],
    ) -> Result<usize, ScreenShareError> {
        let decoded_per_channel = self
            .decoder
            .decode_float(packet, out, false)
            .map_err(|e| ScreenShareError::new(format!("opus decode failed: {e}")))?;
        let channels = self.channels as usize;
        Ok(decoded_per_channel.saturating_mul(channels))
    }

    /// The sample rate this decoder was created with.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The channel count this decoder was created with.
    pub fn channels(&self) -> u16 {
        self.channels
    }
}

/// Platform system-audio capture backend.
///
/// `start` must be called before any capture; `stop` stops the capture
/// thread. The backend pushes interleaved f32 PCM into `producer` (bounded,
/// drop-newest) at its [`sample_rate`](Self::sample_rate) /
/// [`channels`](Self::channels).
pub trait SystemAudioCapture: Send {
    /// Start capturing system audio into the bounded ring producer.
    fn start(&mut self, producer: AudioSampleProducer) -> Result<(), ScreenShareError>;
    /// Stop capture and release the backend. Idempotent.
    fn stop(&mut self);
    /// Sample rate of the captured PCM.
    fn sample_rate(&self) -> u32;
    /// Channel count of the captured PCM.
    fn channels(&self) -> u16;
}

/// Capture backend that fails with a typed unavailable error.
///
/// Used on platforms without an implemented system-audio backend (e.g.
/// Windows WASAPI loopback in this build) and as the factory fallback when
/// the platform backend cannot initialize.
pub struct UnavailableAudioCapture {
    reason: String,
}

impl UnavailableAudioCapture {
    /// Create a backend that always fails with `reason`.
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl SystemAudioCapture for UnavailableAudioCapture {
    fn start(&mut self, _producer: AudioSampleProducer) -> Result<(), ScreenShareError> {
        Err(ScreenShareError::audio_unavailable(self.reason.clone()))
    }
    fn stop(&mut self) {}
    fn sample_rate(&self) -> u32 {
        AUDIO_SAMPLE_RATE
    }
    fn channels(&self) -> u16 {
        AUDIO_CHANNELS
    }
}

/// Synthetic capture backend for tests and the demo/CI path.
///
/// Generates a quiet 440 Hz stereo tone so the host encode → transport →
/// viewer decode pipeline can be exercised headless without PipeWire.
pub struct NullAudioCapture {
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Default for NullAudioCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl NullAudioCapture {
    /// Create a synthetic capture backend (not yet started).
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }
}

impl SystemAudioCapture for NullAudioCapture {
    fn start(&mut self, mut producer: AudioSampleProducer) -> Result<(), ScreenShareError> {
        if self.thread.is_some() {
            return Err(ScreenShareError::new("audio capture already started"));
        }
        self.running.store(true, Ordering::Release);
        let running = Arc::clone(&self.running);
        let thread = std::thread::Builder::new()
            .name("boru-null-audio-capture".into())
            .spawn(move || {
                // 440 Hz sine, 48 kHz stereo, 20 ms chunks; quiet (0.05) so
                // playback is not startling in the demo path.
                let phase_step = 2.0 * std::f32::consts::PI * 440.0 / AUDIO_SAMPLE_RATE as f32;
                let mut phase = 0.0f32;
                let mut chunk = vec![0.0f32; AUDIO_SAMPLES_PER_FRAME];
                while running.load(Ordering::Acquire) {
                    for pair in chunk.chunks_exact_mut(2) {
                        let sample = (phase.sin() * 0.05).max(-1.0).min(1.0);
                        pair[0] = sample;
                        pair[1] = sample;
                        phase += phase_step;
                        if phase > std::f32::consts::TAU {
                            phase -= std::f32::consts::TAU;
                        }
                    }
                    let _ = producer.push_partial_slice(&chunk);
                    std::thread::sleep(Duration::from_millis(AUDIO_FRAME_MS));
                }
            })
            .map_err(|e| ScreenShareError::new(format!("spawn audio capture thread: {e}")))?;
        self.thread = Some(thread);
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    fn sample_rate(&self) -> u32 {
        AUDIO_SAMPLE_RATE
    }

    fn channels(&self) -> u16 {
        AUDIO_CHANNELS
    }
}

impl Drop for NullAudioCapture {
    /// Best-effort stop on drop so a session teardown that simply drops the
    /// backend never leaks the generator thread.
    fn drop(&mut self) {
        self.stop();
    }
}

/// Create the platform's system-audio capture backend.
///
/// Linux uses the PipeWire loopback backend; other platforms (including
/// Windows WASAPI loopback, not yet implemented) return a typed-unavailable
/// backend. Callers should fall back to [`NullAudioCapture`] for the
/// demo/CI path, mirroring the video `TestPatternCapture` fallback.
pub fn create_system_audio_capture() -> Box<dyn SystemAudioCapture> {
    #[cfg(target_os = "linux")]
    {
        Box::new(PipeWireAudioCapture::new())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Box::new(UnavailableAudioCapture::new(
            "system-audio capture is only implemented on Linux (PipeWire) in this build; \
             Windows WASAPI loopback is a documented gap (docs/screenshare-audio.md)",
        ))
    }
}

/// Viewer playback sink: a bounded SPSC ring fed by the decode worker and
/// drained by a cpal output stream callback.
///
/// The output stream opens the device's default output config. The callback
/// converts f32 PCM to the device sample format and maps 1/2-channel layouts
/// (duplicating mono, averaging stereo→mono). It never allocates, locks, or
/// waits.
pub struct AudioOutput {
    _stream: cpal::Stream,
    producer: AudioSampleProducer,
    channels: u16,
}

impl std::fmt::Debug for AudioOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioOutput")
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

impl AudioOutput {
    /// Open the default output device and return a sink the decode worker
    /// pushes interleaved f32 PCM into. Fails with a typed unavailable error
    /// when there is no output device (e.g. headless sessions).
    pub fn open() -> Result<Self, ScreenShareError> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| {
                ScreenShareError::audio_unavailable(
                    "no default audio output device — system audio cannot be played back",
                )
            })?;
        let supported = device.default_output_config().map_err(|e| {
            ScreenShareError::audio_unavailable(format!("audio output config unavailable: {e}"))
        })?;
        let config: cpal::StreamConfig = supported.into();
        let channels = config.channels.max(1);
        let (producer, mut consumer) = audio_sample_ring(AUDIO_RING_SAMPLES);
        let err_fn = |e| tracing::warn!(error = %e, "screen-share: audio output stream error");
        let sample_format = supported.sample_format();
        let stream = match sample_format {
            cpal::SampleFormat::F32 => device
                .build_output_stream(
                    config,
                    move |data: &mut [f32], _| fill_output(data, &mut consumer),
                    err_fn,
                    None,
                )
                .map_err(|e| {
                    ScreenShareError::audio_unavailable(format!("audio output open failed: {e}"))
                })?,
            cpal::SampleFormat::I16 => device
                .build_output_stream(
                    config,
                    move |data: &mut [i16], _| fill_output(data, &mut consumer),
                    err_fn,
                    None,
                )
                .map_err(|e| {
                    ScreenShareError::audio_unavailable(format!("audio output open failed: {e}"))
                })?,
            cpal::SampleFormat::U16 => device
                .build_output_stream(
                    config,
                    move |data: &mut [u16], _| fill_output(data, &mut consumer),
                    err_fn,
                    None,
                )
                .map_err(|e| {
                    ScreenShareError::audio_unavailable(format!("audio output open failed: {e}"))
                })?,
            other => {
                return Err(ScreenShareError::audio_unavailable(format!(
                    "audio output sample format {other:?} is not supported"
                )));
            }
        };
        Ok(Self {
            _stream: stream,
            producer,
            channels,
        })
    }

    /// Push interleaved f32 PCM (wire format: 48 kHz stereo) into the bounded
    /// ring. Drops the newest samples when the ring is full; never waits.
    /// Returns the number of samples accepted.
    pub fn push_pcm(&mut self, samples: &[f32]) -> usize {
        let (accepted, _) = self.producer.push_partial_slice(samples);
        accepted.len()
    }

    /// Number of samples currently queued for playback (diagnostics).
    pub fn available(&self) -> usize {
        self.producer.slots()
    }

    /// Channel count of the output stream (1 or 2).
    pub fn channels(&self) -> u16 {
        self.channels
    }
}

/// Drain the playback ring into one cpal output buffer without allocating.
/// Converts f32 → `T` and maps 1/2-channel layouts to the device channels.
fn fill_output<T: cpal::Sample + cpal::FromSample<f32>>(
    data: &mut [T],
    consumer: &mut AudioSampleConsumer,
) {
    // Wire format is stereo; a mono device averages each pair, a stereo
    // device copies samples as-is. A larger channel count (rare) mirrors the
    // first channels — v1 limitation, documented.
    let mut scratch = [0.0f32; 512];
    let mut out: &mut [T] = data;
    while !out.is_empty() {
        let n = out.len().min(scratch.len());
        let (filled, _) = consumer.pop_partial_slice(&mut scratch[..n]);
        let filled = filled.len();
        let mut i = 0;
        while i < n {
            let sample = if i < filled { scratch[i] } else { 0.0 };
            out[i] = T::from_sample::<f32>(sample);
            i += 1;
        }
        out = &mut out[n..];
    }
}

// ── Linux PipeWire loopback backend ─────────────────────────────────────────

#[cfg(target_os = "linux")]
mod pipewire {
    use super::*;
    use std::ffi::{c_char, c_void, CString};

    const PW_LIB: &str = "libpipewire-0.3.so.0";
    // pw_stream direction (stream.h): PW_DIRECTION_INPUT = 1.
    const PW_DIRECTION_INPUT: u32 = 1;
    // pw_stream flags (stream.h): MAP_BUFFERS = 1, AUTOCONNECT = 4.
    const PW_STREAM_FLAG_MAP_BUFFERS: u32 = 1;
    const PW_STREAM_FLAG_AUTOCONNECT: u32 = 4;
    // pw id for "any target" (defs.h): PW_ID_ANY = 0.
    const PW_ID_ANY: u32 = 0;

    // SPA constants for the audio format pod (spa/param/format.h,
    // spa/param/audio/raw.h, spa/utils/type.h).
    const SPA_TYPE_ID: u32 = 2;
    const SPA_TYPE_INT: u32 = 3;
    const SPA_TYPE_OBJECT: u32 = 14;
    const SPA_TYPE_OBJECT_FORMAT: u32 = 0x40003;
    const SPA_PARAM_FORMAT: u32 = 4;
    const SPA_FORMAT_MEDIA_TYPE: u32 = 1;
    const SPA_FORMAT_MEDIA_SUBTYPE: u32 = 2;
    const SPA_FORMAT_AUDIO_FORMAT: u32 = 0x20001;
    const SPA_FORMAT_AUDIO_RATE: u32 = 0x20003;
    const SPA_FORMAT_AUDIO_CHANNELS: u32 = 0x20004;
    const SPA_MEDIA_TYPE_AUDIO: u32 = 1;
    const SPA_MEDIA_SUBTYPE_RAW: u32 = 1;
    const SPA_AUDIO_FORMAT_F32: u32 = 12;

    /// Minimal pw_buffer mirror (layout matches `struct pw_buffer`).
    #[repr(C)]
    struct PwBuffer {
        buffer: *mut SpaBuffer,
        user_data: *mut c_void,
        size: u64,
        requested: u64,
    }

    /// Minimal spa_buffer mirror.
    #[repr(C)]
    struct SpaBuffer {
        n_metas: u32,
        n_datas: u32,
        metas: *mut c_void,
        datas: *mut SpaData,
    }

    /// Minimal spa_data mirror.
    #[repr(C)]
    struct SpaData {
        type_: u32,
        flags: u32,
        fd: i64,
        mapoffset: u32,
        maxsize: u32,
        data: *mut c_void,
        chunk: *mut SpaChunk,
    }

    /// Minimal spa_chunk mirror.
    #[repr(C)]
    struct SpaChunk {
        offset: u32,
        size: u32,
        stride: i32,
        flags: i32,
    }

    /// PipeWire stream events table (layout matches `struct pw_stream_events`).
    #[repr(C)]
    struct PwStreamEvents {
        version: u32,
        destroy: Option<unsafe extern "C" fn(*mut c_void)>,
        state_changed: Option<unsafe extern "C" fn(*mut c_void, i32, i32, *const c_char)>,
        control_info: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void)>,
        io_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *mut c_void, u32)>,
        param_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void)>,
        add_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut PwBuffer)>,
        remove_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut PwBuffer)>,
        process: Option<unsafe extern "C" fn(*mut c_void)>,
        drained: Option<unsafe extern "C" fn(*mut c_void)>,
        command: Option<unsafe extern "C" fn(*mut c_void, *const c_void)>,
        trigger_done: Option<unsafe extern "C" fn(*mut c_void)>,
    }

    /// Owned PipeWire objects and the function table. Lives on the capture
    /// thread (raw pointers never cross threads).
    struct AudioPwCtx {
        library: libloading::Library,
        pw: AudioPw,
        main_loop: *mut c_void,
        context: *mut c_void,
        core: *mut c_void,
        stream: *mut c_void,
        params: Vec<u8>,
    }

    // SAFETY: raw pointers are only dereferenced on the thread that owns
    // `ctx`.
    unsafe impl Send for AudioPwCtx {}

    /// Per-stream callback payload; owns the bounded ring producer.
    struct AudioStreamUserData {
        ctx: *mut AudioPwCtx,
        producer: AudioSampleProducer,
    }

    // SAFETY: as for AudioPwCtx — all access happens on the capture thread.
    unsafe impl Send for AudioStreamUserData {}

    /// Function table for the PipeWire ABI we use (subset of the symbols the
    /// video capture uses, mirrored here so the audio backend is
    /// self-contained).
    struct AudioPw {
        init: unsafe extern "C" fn(*mut i32, *mut *mut *mut c_char),
        main_loop_new: unsafe extern "C" fn(props: *const c_void) -> *mut c_void,
        main_loop_get_loop: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
        main_loop_run: unsafe extern "C" fn(*mut c_void) -> i32,
        main_loop_quit: unsafe extern "C" fn(*mut c_void) -> i32,
        main_loop_destroy: unsafe extern "C" fn(*mut c_void),
        context_new: unsafe extern "C" fn(
            loop_: *mut c_void,
            props: *const c_void,
            user_data_size: usize,
        ) -> *mut c_void,
        context_connect: unsafe extern "C" fn(
            *mut c_void,
            props: *mut c_void,
            user_data_size: usize,
        ) -> *mut c_void,
        context_destroy: unsafe extern "C" fn(*mut c_void),
        core_disconnect: unsafe extern "C" fn(*mut c_void) -> i32,
        stream_new_simple: unsafe extern "C" fn(
            loop_: *mut c_void,
            name: *const c_char,
            props: *mut c_void,
            events: *const PwStreamEvents,
            data: *mut c_void,
        ) -> *mut c_void,
        stream_connect: unsafe extern "C" fn(
            stream: *mut c_void,
            direction: u32,
            target_id: u32,
            flags: u32,
            params: *const *const c_void,
            n_params: u32,
        ) -> i32,
        stream_destroy: unsafe extern "C" fn(*mut c_void),
        stream_disconnect: unsafe extern "C" fn(*mut c_void) -> i32,
        stream_dequeue_buffer: unsafe extern "C" fn(*mut c_void) -> *mut PwBuffer,
        stream_queue_buffer: unsafe extern "C" fn(*mut c_void, *mut PwBuffer) -> i32,
        properties_new: unsafe extern "C" fn(key: *const c_char, ...) -> *mut c_void,
        properties_free: unsafe extern "C" fn(*mut c_void),
    }

    impl AudioPw {
        fn load(library: &libloading::Library) -> Result<Self, ScreenShareError> {
            macro_rules! sym {
                ($name:literal) => {
                    unsafe {
                        *library
                            .get::<unsafe extern "C" fn()>(concat!($name, "\0").as_bytes())
                            .map_err(|e| {
                                ScreenShareError::new(format!("symbol {} missing: {e}", $name))
                            })?
                    }
                };
            }
            Ok(Self {
                init: unsafe { std::mem::transmute(sym!("pw_init")) },
                main_loop_new: unsafe { std::mem::transmute(sym!("pw_main_loop_new")) },
                main_loop_get_loop: unsafe { std::mem::transmute(sym!("pw_main_loop_get_loop")) },
                main_loop_run: unsafe { std::mem::transmute(sym!("pw_main_loop_run")) },
                main_loop_quit: unsafe { std::mem::transmute(sym!("pw_main_loop_quit")) },
                main_loop_destroy: unsafe { std::mem::transmute(sym!("pw_main_loop_destroy")) },
                context_new: unsafe { std::mem::transmute(sym!("pw_context_new")) },
                context_connect: unsafe { std::mem::transmute(sym!("pw_context_connect")) },
                context_destroy: unsafe { std::mem::transmute(sym!("pw_context_destroy")) },
                core_disconnect: unsafe { std::mem::transmute(sym!("pw_core_disconnect")) },
                stream_new_simple: unsafe { std::mem::transmute(sym!("pw_stream_new_simple")) },
                stream_connect: unsafe { std::mem::transmute(sym!("pw_stream_connect")) },
                stream_destroy: unsafe { std::mem::transmute(sym!("pw_stream_destroy")) },
                stream_disconnect: unsafe { std::mem::transmute(sym!("pw_stream_disconnect")) },
                stream_dequeue_buffer: unsafe {
                    std::mem::transmute(sym!("pw_stream_dequeue_buffer"))
                },
                stream_queue_buffer: unsafe { std::mem::transmute(sym!("pw_stream_queue_buffer")) },
                properties_new: unsafe { std::mem::transmute(sym!("pw_properties_new")) },
                properties_free: unsafe { std::mem::transmute(sym!("pw_properties_free")) },
            })
        }
    }

    /// Cross-thread handle that stops the PipeWire audio capture thread.
    /// `pw_main_loop_quit` is documented as callable from any thread.
    struct AudioPwHandle {
        main_loop: usize,
        main_loop_quit: unsafe extern "C" fn(*mut c_void) -> i32,
        done: std::sync::mpsc::Receiver<()>,
    }

    impl AudioPwHandle {
        fn stop(&mut self) {
            // SAFETY: pw_main_loop_quit may be called from any thread while
            // the loop object is alive; the loop stays alive until the
            // capture thread destroys it after main_loop_run returns.
            unsafe {
                (self.main_loop_quit)(self.main_loop as *mut c_void);
            }
            let _ = self.done.recv_timeout(Duration::from_secs(2));
        }
    }

    /// Connect an input stream to the default audio sink's monitor ports
    /// (system-audio loopback) and run the PipeWire main loop on a background
    /// thread. The process callback pushes interleaved f32 PCM into
    /// `producer`.
    fn spawn_pipewire_loopback(
        producer: AudioSampleProducer,
    ) -> Result<AudioPwHandle, ScreenShareError> {
        // SAFETY: every raw pointer is created and used on the spawned
        // thread; `ctx` is boxed and its pointer handed to the thread; the
        // stream events borrow the same context for their lifetime, which
        // ends when the loop quits.
        unsafe {
            let library = libloading::Library::new(PW_LIB).map_err(|e| {
                ScreenShareError::audio_unavailable(format!(
                    "cannot load {PW_LIB} — install PipeWire (e.g. `apt install pipewire`) to share system audio: {e}"
                ))
            })?;
            let pw = AudioPw::load(&library)?;
            let mut argc = 0i32;
            let mut argv: *mut *mut c_char = std::ptr::null_mut();
            (pw.init)(&mut argc, &mut argv);

            let main_loop = (pw.main_loop_new)(std::ptr::null());
            if main_loop.is_null() {
                return Err(ScreenShareError::audio_unavailable(
                    "pw_main_loop_new failed (PipeWire runtime problem)",
                ));
            }
            let loop_ = (pw.main_loop_get_loop)(main_loop);
            let context = (pw.context_new)(loop_, std::ptr::null(), 0);
            if context.is_null() {
                (pw.main_loop_destroy)(main_loop);
                return Err(ScreenShareError::audio_unavailable(
                    "pw_context_new failed (PipeWire runtime problem)",
                ));
            }
            let core = (pw.context_connect)(context, std::ptr::null_mut(), 0);
            if core.is_null() {
                (pw.context_destroy)(context);
                (pw.main_loop_destroy)(main_loop);
                return Err(ScreenShareError::audio_unavailable(
                    "pw_context_connect failed — no PipeWire server reachable (is `pipewire` running in this session?)",
                ));
            }

            let props = make_audio_stream_properties(&pw)?;
            let params = build_audio_format_pod();

            let ctx = Box::into_raw(Box::new(AudioPwCtx {
                library,
                pw,
                main_loop,
                context,
                core,
                stream: std::ptr::null_mut(),
                params,
            }));

            let user_data = Box::into_raw(Box::new(AudioStreamUserData { ctx, producer }));

            let events = PwStreamEvents {
                version: 2,
                destroy: None,
                state_changed: Some(audio_state_changed),
                control_info: None,
                io_changed: None,
                param_changed: None,
                add_buffer: None,
                remove_buffer: None,
                process: Some(audio_process),
                drained: None,
                command: None,
                trigger_done: None,
            };

            let stream_name = CString::new("boru-screen-share-audio").unwrap();
            let stream = ((*ctx).pw.stream_new_simple)(
                loop_,
                stream_name.as_ptr(),
                props,
                &events,
                user_data as *mut c_void,
            );
            if stream.is_null() {
                ((*ctx).pw.properties_free)(props);
                drop(Box::from_raw(user_data));
                drop(Box::from_raw(ctx));
                return Err(ScreenShareError::audio_unavailable(
                    "pw_stream_new_simple failed (PipeWire runtime problem)",
                ));
            }
            (*ctx).stream = stream;

            // Direction INPUT + target.object=audio.sink connects to the
            // default audio sink's monitor ports — the system-audio loopback.
            let flags = PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS;
            let result = ((*ctx).pw.stream_connect)(
                stream,
                PW_DIRECTION_INPUT,
                PW_ID_ANY,
                flags,
                [(*ctx).params.as_ptr() as *const c_void].as_ptr(),
                1,
            );
            if result < 0 {
                ((*ctx).pw.stream_destroy)(stream);
                drop(Box::from_raw(user_data));
                drop(Box::from_raw(ctx));
                return Err(ScreenShareError::audio_unavailable(format!(
                    "pw_stream_connect failed ({result}) — the default audio sink could not be linked for loopback capture"
                )));
            }

            let ctx_addr = ctx as usize;
            let user_addr = user_data as usize;
            let main_loop_addr = main_loop as usize;
            let main_loop_quit = (*ctx).pw.main_loop_quit;
            let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
            std::thread::Builder::new()
                .name("boru-pipewire-audio".into())
                .spawn(move || {
                    run_audio_pipewire_thread(
                        ctx_addr as *mut AudioPwCtx,
                        user_addr as *mut AudioStreamUserData,
                        done_tx,
                    );
                })
                .map_err(|e| ScreenShareError::new(format!("spawn pipewire audio thread: {e}")))?;

            Ok(AudioPwHandle {
                main_loop: main_loop_addr,
                main_loop_quit,
                done: done_rx,
            })
        }
    }

    /// Build the audio stream properties: an INPUT stream whose target is the
    /// default audio sink, so we capture exactly what is being played.
    unsafe fn make_audio_stream_properties(pw: &AudioPw) -> Result<*mut c_void, ScreenShareError> {
        let media_type = CString::new("media.type").unwrap();
        let audio = CString::new("Audio").unwrap();
        let category = CString::new("media.category").unwrap();
        let capture = CString::new("Capture").unwrap();
        let role = CString::new("media.role").unwrap();
        let music = CString::new("Music").unwrap();
        let node_name = CString::new("node.name").unwrap();
        let node_value = CString::new("boru-screen-share-audio").unwrap();
        let target_key = CString::new("target.object").unwrap();
        let target_value = CString::new("audio.sink").unwrap();
        let position_key = CString::new("audio.position").unwrap();
        let position_value = CString::new("FL,FR").unwrap();
        let props = (pw.properties_new)(
            media_type.as_ptr(),
            audio.as_ptr(),
            category.as_ptr(),
            capture.as_ptr(),
            role.as_ptr(),
            music.as_ptr(),
            node_name.as_ptr(),
            node_value.as_ptr(),
            target_key.as_ptr(),
            target_value.as_ptr(),
            position_key.as_ptr(),
            position_value.as_ptr(),
            std::ptr::null::<c_char>(),
        );
        if props.is_null() {
            return Err(ScreenShareError::new("pw_properties_new failed"));
        }
        Ok(props)
    }

    /// Build the SPA audio format object pod advertising F32 / 48 kHz / 2ch.
    /// Layout matches `build_format_pod` in `platform/linux_pw.rs`
    /// (all little-endian, 8-byte aligned).
    fn build_audio_format_pod() -> Vec<u8> {
        let mut pod: Vec<u8> = Vec::new();
        pod.extend_from_slice(&[0, 0, 0, 0]);
        pod.extend_from_slice(&SPA_TYPE_OBJECT.to_le_bytes());
        pod.extend_from_slice(&SPA_TYPE_OBJECT_FORMAT.to_le_bytes());
        pod.extend_from_slice(&SPA_PARAM_FORMAT.to_le_bytes());
        push_prop_id(&mut pod, SPA_FORMAT_MEDIA_TYPE, SPA_MEDIA_TYPE_AUDIO);
        push_prop_id(&mut pod, SPA_FORMAT_MEDIA_SUBTYPE, SPA_MEDIA_SUBTYPE_RAW);
        push_prop_id(&mut pod, SPA_FORMAT_AUDIO_FORMAT, SPA_AUDIO_FORMAT_F32);
        push_prop_int(&mut pod, SPA_FORMAT_AUDIO_RATE, AUDIO_SAMPLE_RATE);
        push_prop_int(&mut pod, SPA_FORMAT_AUDIO_CHANNELS, AUDIO_CHANNELS as u32);
        let body_size = pod.len() as u32 - 8;
        pod[0..4].copy_from_slice(&body_size.to_le_bytes());
        pod
    }

    fn push_prop_id(pod: &mut Vec<u8>, key: u32, value: u32) {
        pod.extend_from_slice(&key.to_le_bytes());
        pod.extend_from_slice(&0u32.to_le_bytes()); // flags
        pod.extend_from_slice(&4u32.to_le_bytes()); // value pod body size
        pod.extend_from_slice(&SPA_TYPE_ID.to_le_bytes());
        pod.extend_from_slice(&value.to_le_bytes());
        while !pod.len().is_multiple_of(8) {
            pod.push(0);
        }
    }

    fn push_prop_int(pod: &mut Vec<u8>, key: u32, value: u32) {
        pod.extend_from_slice(&key.to_le_bytes());
        pod.extend_from_slice(&0u32.to_le_bytes()); // flags
        pod.extend_from_slice(&4u32.to_le_bytes()); // value pod body size
        pod.extend_from_slice(&SPA_TYPE_INT.to_le_bytes());
        pod.extend_from_slice(&value.to_le_bytes());
        while !pod.len().is_multiple_of(8) {
            pod.push(0);
        }
    }

    /// Run the PipeWire main loop until quit; forwards audio samples, then
    /// frees every PipeWire object and signals the teardown handle.
    fn run_audio_pipewire_thread(
        ctx: *mut AudioPwCtx,
        user_data: *mut AudioStreamUserData,
        done: std::sync::mpsc::Sender<()>,
    ) {
        unsafe {
            let _ = ((*ctx).pw.main_loop_run)((*ctx).main_loop);
            let _ = ((*ctx).pw.stream_disconnect)((*ctx).stream);
            ((*ctx).pw.stream_destroy)((*ctx).stream);
            let _ = ((*ctx).pw.core_disconnect)((*ctx).core);
            ((*ctx).pw.context_destroy)((*ctx).context);
            ((*ctx).pw.main_loop_destroy)((*ctx).main_loop);
            drop(Box::from_raw(user_data));
            drop(Box::from_raw(ctx));
        }
        let _ = done.send(());
    }

    unsafe extern "C" fn audio_state_changed(
        _data: *mut c_void,
        _old: i32,
        _state: i32,
        _error: *const c_char,
    ) {
    }

    /// Dequeue one buffer and copy its f32 samples into the bounded ring.
    /// Never allocates or locks; a full ring drops the newest samples.
    unsafe extern "C" fn audio_process(data: *mut c_void) {
        let user_data = &mut *(data as *mut AudioStreamUserData);
        let pw = &(*user_data.ctx).pw;
        let stream = (*user_data.ctx).stream;
        let buffer = (pw.stream_dequeue_buffer)(stream);
        if buffer.is_null() {
            return;
        }
        let pw_buffer = &mut *buffer;
        if !pw_buffer.buffer.is_null() {
            let spa_buffer = &mut *pw_buffer.buffer;
            if spa_buffer.n_datas >= 1 && !spa_buffer.datas.is_null() {
                let data0 = &mut *spa_buffer.datas;
                if !data0.data.is_null() && !data0.chunk.is_null() {
                    let chunk = &*data0.chunk;
                    let offset = chunk.offset as usize;
                    let size = chunk.size as usize;
                    let maxsize = data0.maxsize as usize;
                    if offset <= maxsize && size <= maxsize - offset {
                        let sample_bytes = size - (size % 4);
                        let ptr = (data0.data as *const u8).add(offset) as *const f32;
                        let samples = std::slice::from_raw_parts(ptr, sample_bytes / 4);
                        let _ = user_data.producer.push_partial_slice(samples);
                    }
                }
            }
        }
        (pw.stream_queue_buffer)(stream, buffer);
    }

    /// Linux PipeWire loopback capture backend.
    pub struct PipeWireAudioCapture {
        handle: Option<AudioPwHandle>,
    }

    impl PipeWireAudioCapture {
        /// Create a PipeWire loopback capture backend (not yet started).
        pub fn new() -> Self {
            Self { handle: None }
        }
    }

    impl SystemAudioCapture for PipeWireAudioCapture {
        fn start(&mut self, producer: AudioSampleProducer) -> Result<(), ScreenShareError> {
            if self.handle.is_some() {
                return Err(ScreenShareError::new("audio capture already started"));
            }
            let handle = spawn_pipewire_loopback(producer)?;
            self.handle = Some(handle);
            Ok(())
        }

        fn stop(&mut self) {
            if let Some(mut handle) = self.handle.take() {
                handle.stop();
            }
        }

        fn sample_rate(&self) -> u32 {
            AUDIO_SAMPLE_RATE
        }

        fn channels(&self) -> u16 {
            AUDIO_CHANNELS
        }
    }

    impl Drop for PipeWireAudioCapture {
        /// Best-effort stop on drop so a session teardown that simply drops
        /// the backend never leaks the PipeWire thread or its objects.
        fn drop(&mut self) {
            self.stop();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Guard the ABI-critical SPA constants (they come from PipeWire's
        /// MIT headers; a wrong constant silently breaks the stream).
        #[test]
        fn audio_spa_constants_match_pipewire_headers() {
            assert_eq!(SPA_TYPE_Id, 2);
            assert_eq!(SPA_TYPE_Int, 3);
            assert_eq!(SPA_TYPE_Object, 14);
            assert_eq!(SPA_TYPE_OBJECT_Format, 0x40003);
            assert_eq!(SPA_PARAM_Format, 4);
            assert_eq!(SPA_FORMAT_mediaType, 1);
            assert_eq!(SPA_FORMAT_mediaSubtype, 2);
            assert_eq!(SPA_FORMAT_AUDIO_format, 0x20001);
            assert_eq!(SPA_FORMAT_AUDIO_rate, 0x20003);
            assert_eq!(SPA_FORMAT_AUDIO_channels, 0x20004);
            assert_eq!(SPA_MEDIA_TYPE_AUDIO, 1);
            assert_eq!(SPA_MEDIA_SUBTYPE_RAW, 1);
            assert_eq!(SPA_AUDIO_FORMAT_F32, 12);
        }

        #[test]
        fn audio_format_pod_is_well_formed() {
            let pod = build_audio_format_pod();
            // Header: size + SPA_TYPE_Object.
            assert!(pod.len() >= 8);
            assert_eq!(
                u32::from_le_bytes(pod[4..8].try_into().unwrap()),
                SPA_TYPE_Object
            );
            let body_size = u32::from_le_bytes(pod[0..4].try_into().unwrap()) as usize;
            assert_eq!(body_size + 8, pod.len(), "pod body size must match");
            // Every property is 8-byte aligned (no trailing garbage).
            assert!(pod.len().is_multiple_of(8));
        }
    }
}

#[cfg(target_os = "linux")]
pub use pipewire::PipeWireAudioCapture;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_bounds_and_drop_newest() {
        let (mut producer, mut consumer) = audio_sample_ring(4);
        assert_eq!(
            producer
                .push_partial_slice(&[1.0, 2.0, 3.0, 4.0, 5.0])
                .0
                .len(),
            4
        );
        let mut out = [0.0f32; 2];
        assert_eq!(consumer.pop_partial_slice(&mut out).0.len(), 2);
        assert_eq!(out, [1.0, 2.0]);
    }

    #[test]
    fn opus_round_trip_stereo_20ms() {
        let mut encoder = OpusAudioEncoder::new().expect("encoder");
        let mut decoder =
            OpusAudioDecoder::new(AUDIO_SAMPLE_RATE, AUDIO_CHANNELS).expect("decoder");
        let pcm: Vec<f32> = (0..AUDIO_SAMPLES_PER_FRAME)
            .map(|i| ((i as f32) * 0.05).sin() * 0.2)
            .collect();
        let packet = encoder.encode_frame(&pcm).expect("encode").expect("packet");
        assert!(!packet.is_empty());
        let mut out = vec![0.0f32; AUDIO_SAMPLES_PER_FRAME];
        let decoded = decoder.decode_frame(&packet, &mut out).expect("decode");
        assert_eq!(decoded, AUDIO_SAMPLES_PER_FRAME);
        // Decoded audio correlates with the input (lossy but recognizable).
        let dot: f32 = pcm.iter().zip(out.iter()).map(|(a, b)| a * b).sum();
        assert!(dot > 0.0, "decoded audio must correlate with the input");
    }

    #[test]
    fn opus_rejects_partial_and_wrong_size_frames() {
        let mut encoder = OpusAudioEncoder::new().unwrap();
        let short = vec![0.0f32; AUDIO_SAMPLES_PER_FRAME - 1];
        assert!(encoder.encode_frame(&short).is_err());
        let long = vec![0.0f32; AUDIO_SAMPLES_PER_FRAME + 1];
        assert!(encoder.encode_frame(&long).is_err());
    }

    #[test]
    fn unavailable_capture_returns_typed_error() {
        let mut capture = UnavailableAudioCapture::new("wasapi loopback not implemented");
        let (producer, _consumer) = audio_sample_ring(16);
        let error = capture.start(producer).unwrap_err();
        assert_eq!(error.kind(), ScreenShareErrorKind::AudioUnavailable);
        assert!(error.to_string().contains("wasapi"));
        capture.stop(); // idempotent
    }

    #[test]
    fn null_capture_produces_audio_until_stopped() {
        let mut capture = NullAudioCapture::new();
        let (producer, mut consumer) = audio_sample_ring(AUDIO_SAMPLES_PER_FRAME * 8);
        capture.start(producer).unwrap();
        // Wait up to ~150 ms for the 20 ms generator to fill the ring.
        let mut samples = vec![0.0f32; AUDIO_SAMPLES_PER_FRAME];
        let mut got = 0usize;
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(10));
            got = consumer.pop_partial_slice(&mut samples).0.len();
            if got > 0 {
                break;
            }
        }
        capture.stop();
        assert_eq!(capture.sample_rate(), AUDIO_SAMPLE_RATE);
        assert_eq!(capture.channels(), AUDIO_CHANNELS);
        assert!(got > 0, "null capture must produce samples");
        assert!(
            samples[..got].iter().any(|s| s.abs() > 0.001),
            "null capture must produce a non-silent tone"
        );
    }

    #[test]
    fn null_capture_rejects_double_start() {
        let mut capture = NullAudioCapture::new();
        let (producer, _consumer) = audio_sample_ring(16);
        capture.start(producer).unwrap();
        let (producer2, _consumer2) = audio_sample_ring(16);
        assert!(capture.start(producer2).is_err());
        capture.stop();
    }
}
