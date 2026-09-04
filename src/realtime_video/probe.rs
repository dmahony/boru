//! Bounded, process-local probing of realtime video codec capability.
//!
//! Probing is deliberately local: it produces only a small capability result,
//! never a peer identifier or benchmark payload. Callers may use the cache to
//! avoid repeating the probe during a process lifetime.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::{CodecConfig, VideoFrame};

/// Resolution exercised by a capability probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProbeResolution {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl ProbeResolution {
    /// The two bounded realtime profiles required by the call path.
    pub const DEFAULTS: [Self; 2] = [
        Self {
            width: 320,
            height: 180,
        },
        Self {
            width: 640,
            height: 360,
        },
    ];
}

/// Limits applied to one local probe.
#[derive(Debug, Clone, Copy)]
pub struct ProbeConfig {
    /// Frames discarded to warm up an encoder/decoder pair per resolution.
    pub warmup_frames: usize,
    /// Measured frames per resolution after warm-up.
    pub sample_frames: usize,
    /// Maximum average encode/decode duration per frame.
    pub average_frame_budget: Duration,
    /// Maximum p95 encode/decode duration per frame.
    pub p95_frame_budget: Duration,
    /// Hard wall-clock limit for the complete probe.
    pub timeout: Duration,
    /// Maximum temporary bytes a backend may retain.
    pub max_memory_bytes: usize,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            warmup_frames: 2,
            sample_frames: 8,
            average_frame_budget: Duration::from_millis(50),
            p95_frame_budget: Duration::from_millis(100),
            timeout: Duration::from_secs(2),
            max_memory_bytes: 64 * 1024 * 1024,
        }
    }
}

/// The observable result of a local capability probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityProbeResult {
    /// True only when every requested profile passed all checks.
    pub supported: bool,
    /// Profiles that completed a valid encode/decode round trip.
    pub resolutions: Vec<ProbeResolution>,
    /// True when the hard wall-clock limit was reached.
    pub timed_out: bool,
}

/// Errors that prevent a probe from producing a capability result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeError {
    /// The configuration would perform no useful work.
    InvalidConfig,
    /// The backend failed to encode or decode a synthetic frame.
    Backend,
    /// The decoded frame did not match the source dimensions or pixels.
    RoundTrip,
    /// The backend exceeded the configured temporary-memory bound.
    MemoryLimit,
}

/// A codec implementation used by the probe.
pub trait ProbeBackend {
    /// Encode and decode one synthetic frame, returning its measured duration
    /// and temporary memory use. The returned frame must own its pixels.
    fn encode_decode(
        &mut self,
        input: &VideoFrame,
    ) -> Result<(VideoFrame, Duration, usize), ProbeError>;
}

/// Probe one backend against the two bounded realtime resolutions.
pub fn probe<B: ProbeBackend>(
    backend: &mut B,
    config: ProbeConfig,
) -> Result<CapabilityProbeResult, ProbeError> {
    if config.sample_frames == 0 || config.timeout.is_zero() || config.max_memory_bytes == 0 {
        return Err(ProbeError::InvalidConfig);
    }
    let started = Instant::now();
    let mut reported_time = Duration::ZERO;
    let mut passed = Vec::new();
    let mut timed_out = false;
    for resolution in ProbeResolution::DEFAULTS {
        let input = synthetic_frame(resolution)?;
        for _ in 0..config.warmup_frames {
            if started.elapsed() >= config.timeout {
                timed_out = true;
                break;
            }
            let (_, duration, _) = backend.encode_decode(&input)?;
            reported_time = reported_time.saturating_add(duration);
            if reported_time >= config.timeout {
                timed_out = true;
                break;
            }
        }
        if timed_out {
            break;
        }
        let mut durations = Vec::with_capacity(config.sample_frames);
        let mut valid = true;
        for _ in 0..config.sample_frames {
            if started.elapsed() >= config.timeout {
                timed_out = true;
                break;
            }
            let (output, duration, memory) = backend.encode_decode(&input)?;
            reported_time = reported_time.saturating_add(duration);
            if reported_time >= config.timeout {
                timed_out = true;
                break;
            }
            if memory > config.max_memory_bytes {
                return Err(ProbeError::MemoryLimit);
            }
            if output.width != input.width
                || output.height != input.height
                || output.pixels != input.pixels
            {
                return Err(ProbeError::RoundTrip);
            }
            durations.push(duration);
        }
        if timed_out {
            break;
        }
        durations.sort_unstable();
        let average = durations.iter().copied().sum::<Duration>() / durations.len() as u32;
        let p95 =
            durations[((durations.len() * 95).saturating_sub(1) / 100).min(durations.len() - 1)];
        if average <= config.average_frame_budget && p95 <= config.p95_frame_budget {
            passed.push(resolution);
        } else {
            valid = false;
        }
        if !valid {
            break;
        }
    }
    Ok(CapabilityProbeResult {
        supported: !timed_out && passed.len() == ProbeResolution::DEFAULTS.len(),
        resolutions: passed,
        timed_out,
    })
}

fn synthetic_frame(resolution: ProbeResolution) -> Result<VideoFrame, ProbeError> {
    let len = resolution.width as usize * resolution.height as usize * 4;
    let pixels = (0..len).map(|i| (i as u8).wrapping_mul(31)).collect();
    VideoFrame::packed(
        0,
        resolution.width,
        resolution.height,
        super::PixelFormat::Rgba8,
        pixels,
    )
    .map_err(|_| ProbeError::Backend)
}

/// A deterministic backend used for startup probing and unit tests.
#[derive(Debug, Default)]
pub struct SyntheticBackend;

impl ProbeBackend for SyntheticBackend {
    fn encode_decode(
        &mut self,
        input: &VideoFrame,
    ) -> Result<(VideoFrame, Duration, usize), ProbeError> {
        Ok((input.clone(), Duration::ZERO, input.pixels.len()))
    }
}

static CACHE: OnceLock<Mutex<HashMap<String, CapabilityProbeResult>>> = OnceLock::new();
const MAX_CACHE_ENTRIES: usize = 8;

/// Probe once per process and return a small, local-only capability result.
pub fn probe_cached<B: ProbeBackend>(
    key: &str,
    backend: &mut B,
    config: ProbeConfig,
) -> Result<CapabilityProbeResult, ProbeError> {
    let cache_key = format!(
        "{key}:{}:{}:{}:{}:{}:{}",
        config.warmup_frames,
        config.sample_frames,
        config.average_frame_budget.as_nanos(),
        config.p95_frame_budget.as_nanos(),
        config.timeout.as_nanos(),
        config.max_memory_bytes
    );
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(result) = cache
        .lock()
        .expect("probe cache poisoned")
        .get(&cache_key)
        .cloned()
    {
        return Ok(result);
    }
    let result = probe(backend, config)?;
    let mut cache = cache.lock().expect("probe cache poisoned");
    if cache.len() >= MAX_CACHE_ENTRIES {
        cache.clear();
    }
    cache.insert(cache_key, result.clone());
    Ok(result)
}

/// Clear the process-local cache (intended for tests and controlled restart).
pub fn clear_cache() {
    if let Some(cache) = CACHE.get() {
        cache.lock().expect("probe cache poisoned").clear();
    }
}

/// Build a probe configuration from the negotiated stream configuration.
pub fn config_for(codec: CodecConfig) -> ProbeConfig {
    let mut config = ProbeConfig::default();
    if codec.fps > 30 {
        config.average_frame_budget = Duration::from_millis(33);
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Slow(Duration);
    impl ProbeBackend for Slow {
        fn encode_decode(
            &mut self,
            input: &VideoFrame,
        ) -> Result<(VideoFrame, Duration, usize), ProbeError> {
            Ok((input.clone(), self.0, input.pixels.len()))
        }
    }

    #[test]
    fn synthetic_backend_passes_both_profiles() {
        assert!(
            probe(&mut SyntheticBackend, ProbeConfig::default())
                .unwrap()
                .supported
        );
    }

    #[test]
    fn slow_backend_fails_budget_without_exposing_timings() {
        let result = probe(
            &mut Slow(Duration::from_millis(200)),
            ProbeConfig::default(),
        )
        .unwrap();
        assert!(!result.supported);
        assert!(result.resolutions.is_empty());
    }

    #[test]
    fn reported_duration_triggers_bounded_timeout() {
        let config = ProbeConfig {
            timeout: Duration::from_millis(10),
            ..ProbeConfig::default()
        };
        let result = probe(&mut Slow(Duration::from_millis(10)), config).unwrap();
        assert!(result.timed_out);
        assert!(!result.supported);
    }

    #[test]
    fn cache_avoids_second_backend_run() {
        clear_cache();
        let mut backend = SyntheticBackend;
        let first = probe_cached("h264", &mut backend, ProbeConfig::default()).unwrap();
        let second = probe_cached(
            "h264",
            &mut Slow(Duration::from_secs(1)),
            ProbeConfig::default(),
        )
        .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn invalid_config_is_rejected() {
        let config = ProbeConfig {
            sample_frames: 0,
            ..ProbeConfig::default()
        };
        assert_eq!(
            probe(&mut SyntheticBackend, config),
            Err(ProbeError::InvalidConfig)
        );
    }
}
