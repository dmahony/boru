# Screen-Share Encode Benchmark: OpenH264 CPU + fps (720p30 / 1080p30)

Status: **BORU-SS-18 / PDF Task 7.1** — baseline encode benchmark recorded on
Linux (debsrv).

## Question

The PDF Task 7.1 asks for H.264 encoding through the existing OpenH264
dependency with 720p/30 and 1080p/30 reference targets, bitrate / fps /
keyframe-interval / quality-profile configuration, and a CPU benchmark on a
representative Linux system. How much CPU does the encode path consume at
each reference target, and can it sustain 30 fps?

## Method

`src/screen_share/encode_bench.rs` (`benchmark_openh264_720p30_and_1080p30`,
an `#[ignore]`d test) drives the real production path — `OpenH264Encoder`
implementing `VideoEncoder` — with synthetic desktop frames (a mostly-static
gradient with a small moving cursor block, so the encoder sees realistic
screen content rather than a trivially-compressible static frame).

Per case it encodes 90 frames and reports:

- **avg ms/frame** — steady-state encode latency including the full
  `encode()` path used by the host loop (RGBA→RGB, scale, RGB→YUV via the
  fast integer `write_yuv_scalar` path, H.264 encode).
- **encode fps** — how many frames/second the encoder can sustain
  back-to-back.
- **cpu%1core@targetfps** — the fraction of one core occupied when sustaining
  the target frame rate: `avg_ms_per_frame * target_fps / 1000 * 100`.

Run (release mode — debug `-O0` inflates the CPU-bound path ~40x):

```bash
cargo test --release --features screen-sharing --lib -- --ignored --nocapture encode_bench
```

## Environment

- Host: debsrv (172.16.0.59, 8 cores, Ubuntu/Debian) via `rb`.
- OpenH264 0.9.7 (crate `openh264`, C encoder via `openh264-sys2`),
  `UsageType::ScreenContentRealTime`, `RateControlMode::Bitrate`,
  `skip_frames(false)` (mandatory for the static-screen decodability fix).
- Release profile: `opt-level=3`, LTO fat, `codegen-units=1`.
- Date: 2026-08-15.

## Results

Measured 2026-08-15 on debsrv via `rb` (release):

```
encode_bench: 720p30-balanced:   1280x720@30fps avg=14.8ms/frame encode_fps=67.5 cpu%1core@30fps=44.4 bytes=24342 bitrate=146kbps
encode_bench: 1080p30-balanced:  1920x1080@30fps avg=28.1ms/frame encode_fps=35.6 cpu%1core@30fps=84.3 bytes=45285 bitrate=143kbps
encode_bench: 1080p30-lowlatency: 1920x1080@30fps avg=28.0ms/frame encode_fps=35.7 cpu%1core@30fps=84.1 bytes=45460 bitrate=144kbps
encode_bench: 1080p30-highquality: 1920x1080@30fps avg=30.1ms/frame encode_fps=33.2 cpu%1core@30fps=90.4 bytes=95276 bitrate=281kbps
```

Interpretation:

- **720p30 is cheap**: ~14.8 ms/frame (67 fps sustained) and only **44% of one
  core** to hit the 30 fps target. Plenty of headroom for capture, transport
  and GUI on the same core.
- **1080p30 is feasible but heavier**: ~28 ms/frame (35.6 fps sustained) and
  **84% of one core** at 30 fps. Sustains the target on this 8-core host with
  ~15% headroom — but leaves little room for other host-side work on the same
  core, which is exactly the scenario the later frame-dropping / adaptive
  quality tasks (PDF 7.2 / 7.3) address.
- **Quality profile knob works as intended**: LowLatency ≈ Balanced latency
  (complexity low vs medium at this bitrate), HighQuality costs ~7% more CPU
  (90% vs 84% of a core) while roughly doubling output bytes (281 vs 143 kbps
  on this synthetic content) — the expected quality/CPU trade.
- **Bitrate on the synthetic pattern is a floor, not a ceiling**: the test
  pattern is mostly-static (gradient + cursor), so deltas compress to near
  nothing; real desktop content (text, windows, motion) will produce higher
  bitrates at the same settings. The CPU numbers are the meaningful output.

## Notes / deviations

- The encoder previously used `YUVBuffer::from_rgb_source`, which routes
  through the f32 per-pixel `write_yuv_by_pixel` converter and measured
  ~40 ms extra per 1080p frame (~68 ms/frame total, below the 30 fps target).
  Switching to `from_rgb8_source` (integer `write_yuv_scalar`) brought 1080p30
  to ~28 ms/frame — above the 30 fps target. This is a Boru-owned performance
  fix using the openh264 crate's documented fast path.
- Windows benchmark: not run in this task (Windows test hardware not
  available in this environment); the benchmark test is cross-platform and can
  be run on a Windows build host when one is available.

## Acceptance

- Benchmark recorded: this document (committed with BORU-SS-18).
- 720p30 and 1080p30 reference targets both sustain 30 fps on Linux (debsrv).
- Bitrate, fps, keyframe interval and quality profile are configurable
  (`CaptureConfig` → `CodecConfig::from_capture_config`; `QualityProfile` on
  the wire `StreamConfig`).
- Unit tests for encoder lifecycle + config application pass
  (`screen_share` lib suite, 182 tests, 3 ignored: 2 live-X11 + this bench).
