//! OpenH264 encode CPU-usage + fps benchmark (PDF Task 7.1), plus a VA-API
//! hardware-accelerated benchmark (PDF Task 2.2 hardware path) that measures
//! the same throughput/CPU metrics for the reference profiles.
//!
//! Measures wall-clock encode throughput and an estimated single-core CPU
//! utilisation at the target frame rates for the two reference profiles
//! (720p30 and 1080p30) plus all three quality profiles. The host loop feeds
//! the encoder one frame per capture tick, so "CPU usage" is best expressed
//! as the fraction of one core the encoder occupies when sustaining the
//! target fps: `avg_encode_ms_per_frame * target_fps / 1000 * 100`.
//!
//! Run in RELEASE mode (debug `-O0` inflates the CPU-bound encode path by
//! ~40x, making the numbers meaningless and the timing assertions fail):
//! `cargo test --release --features screen-sharing --lib -- --ignored --nocapture encode_bench`
//!
//! The VA-API cases additionally require a usable render node with an H.264
//! encode entrypoint (e.g. `sudo -E cargo test ... vaapi_bench` on an Intel
//! iGPU box), mirroring the host's own fallback: on machines without one the
//! hardware case reports unavailable and the OpenH264 cases still pass.
//!
//! Results are recorded in `docs/screenshare-encode-benchmark.md`.

use std::time::Instant;

use super::capture::{CapturedFrame, PixelFormat};
use super::codec::{CodecConfig, OpenH264Encoder, QualityProfile, VideoEncoder};
#[cfg(target_os = "linux")]
use super::vaapi::VaapiEncoder;

/// Number of frames encoded per case (enough to amortise keyframe spikes and
/// scheduler noise; 90 frames ≈ 3 seconds of real-time at 30 fps).
const FRAMES_PER_CASE: u32 = 90;

/// Generate a synthetic "desktop" frame: a mostly-static gradient with a
/// moving cursor block, so the encoder sees realistic screen content (a pure
/// static frame compresses to near-zero and understates CPU usage).
fn desktop_pattern(width: u32, height: u32, tick: u32) -> CapturedFrame {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    let cursor_x = (tick * 7) % width.max(1);
    let cursor_y = (tick * 5) % height.max(1);
    for y in 0..height {
        for x in 0..width {
            let near_cursor = x.abs_diff(cursor_x) < 6 && y.abs_diff(cursor_y) < 6;
            if near_cursor {
                pixels.extend_from_slice(&[255, 0, 0, 255]);
            } else {
                let shade = ((x ^ y) >> 2) as u8;
                pixels.extend_from_slice(&[shade, shade, shade, 255]);
            }
        }
    }
    CapturedFrame::cpu(0, width, height, PixelFormat::Rgba8, pixels).unwrap()
}

/// Encode `FRAMES_PER_CASE` frames and report throughput + CPU estimate.
/// Returns `(avg_ms_per_frame, encoded_bytes_total)`.
fn run_case(
    name: &str,
    mut encoder: Box<dyn VideoEncoder>,
    width: u32,
    height: u32,
    target_fps: u32,
) -> (f64, u64) {
    // Warm-up keyframe so the timed region measures steady-state deltas.
    let _ = encoder.encode(&desktop_pattern(width, height, 0)).unwrap();

    let started = Instant::now();
    let mut bytes = 0u64;
    for tick in 1..=FRAMES_PER_CASE {
        let frame = desktop_pattern(width, height, tick);
        let packet = encoder.encode(&frame).unwrap();
        bytes += packet.bytes.len() as u64;
    }
    let elapsed = started.elapsed();
    let avg_ms = elapsed.as_secs_f64() * 1000.0 / FRAMES_PER_CASE as f64;
    let encode_fps = 1000.0 / avg_ms;
    let cpu_pct = avg_ms * target_fps as f64 / 1000.0 * 100.0;
    let bitrate_kbps = bytes as f64 * 8.0 / elapsed.as_secs_f64() / 1000.0;
    println!(
        "encode_bench: {name}: {width}x{height}@{target_fps}fps avg={avg_ms:.3}ms/frame encode_fps={encode_fps:.1} cpu%1core@{target_fps}fps={cpu_pct:.1} bytes={bytes} bitrate={bitrate_kbps:.0}kbps"
    );
    (avg_ms, bytes)
}

/// Software (OpenH264) case with the target profile's config.
fn run_software_case(name: &str, config: CodecConfig, target_fps: u32) -> (f64, u64) {
    let width = config.width;
    let height = config.height;
    run_case(name, Box::new(OpenH264Encoder::new(config).unwrap()), width, height, target_fps)
}

/// Hardware (VA-API) case; logs and skips when the local GPU cannot encode.
#[cfg(target_os = "linux")]
fn run_hardware_case(name: &str, config: CodecConfig, target_fps: u32) -> Option<(f64, u64)> {
    let width = config.width;
    let height = config.height;
    match VaapiEncoder::new(config) {
        Ok(encoder) => Some(run_case(name, Box::new(encoder), width, height, target_fps)),
        Err(error) => {
            println!(
                "encode_bench: {name}: VA-API hardware encoder unavailable, skipped ({error})"
            );
            None
        }
    }
}

#[test]
#[ignore = "perf-sensitive: must run in release mode (see module docs)"]
fn benchmark_openh264_720p30_and_1080p30() {
    // 720p30: balanced (default) profile.
    let (avg_720, _) = run_software_case("720p30-balanced", CodecConfig::profile_720p30(), 30);
    // 1080p30: balanced (default) profile.
    let (avg_1080, _) = run_software_case("1080p30-balanced", CodecConfig::profile_1080p30(), 30);
    // Quality-profile sweep on 1080p30 to show the CPU/quality knob.
    let (avg_ll, _) = run_software_case(
        "1080p30-lowlatency",
        CodecConfig { quality_profile: QualityProfile::LowLatency, ..CodecConfig::profile_1080p30() },
        30,
    );
    let (avg_hq, _) = run_software_case(
        "1080p30-highquality",
        CodecConfig { quality_profile: QualityProfile::HighQuality, ..CodecConfig::profile_1080p30() },
        30,
    );

    // Loose invariants: the encoder must sustain well above 30 fps for 720p
    // (avg < 25ms/frame → >40 fps) and at least the 30 fps target for 1080p
    // (avg < 33ms/frame → >30 fps) on any modern x86 host. Low-latency
    // profile must not be meaningfully slower than the balanced 1080p case.
    assert!(
        avg_720 < 25.0,
        "720p30 must encode faster than 25ms/frame, got {avg_720:.3}ms"
    );
    assert!(
        avg_1080 < 33.0,
        "1080p30 must sustain the 30fps target (<33ms/frame), got {avg_1080:.3}ms"
    );
    assert!(
        avg_ll <= avg_1080 + 2.0,
        "low-latency 1080p should be no slower than balanced 1080p (got {avg_ll:.3} vs {avg_1080:.3})"
    );
    // High-quality may be slower (higher complexity), but must still sustain
    // the 30 fps target (avg < 33.3ms).
    assert!(
        avg_hq < 33.0,
        "high-quality 1080p must sustain 30fps (avg {avg_hq:.3}ms)"
    );
}

/// VA-API hardware-accelerated benchmark (PDF Task 2.2). Requires a usable
/// render node with an H.264 encode entrypoint; when absent (software-only
/// host) the case is skipped rather than failed — mirroring the host's
/// fallback behaviour. Assertions are deliberately loose: hardware encode
/// should comfortably beat the 30fps target on any VA-API-capable GPU.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "perf-sensitive + hardware: must run in release mode on a VA-API-capable host (see module docs)"]
fn benchmark_vaapi_720p30_and_1080p30() {
    let Some((avg_720, _)) =
        run_hardware_case("vaapi-720p30-balanced", CodecConfig::profile_720p30(), 30)
    else {
        return; // hardware unavailable: OpenH264 cases already covered it.
    };
    let Some((avg_1080, _)) =
        run_hardware_case("vaapi-1080p30-balanced", CodecConfig::profile_1080p30(), 30)
    else {
        return;
    };
    assert!(
        avg_720 < 16.0,
        "VA-API 720p30 must be fast (avg {avg_720:.3}ms/frame)"
    );
    assert!(
        avg_1080 < 16.0,
        "VA-API 1080p30 must be fast (avg {avg_1080:.3}ms/frame)"
    );
}
