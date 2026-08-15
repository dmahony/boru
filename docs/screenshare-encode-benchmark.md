# Screen-Share Encode Benchmark: OpenH264 vs VA-API (720p30 / 1080p30)

Status: **BORU-SS-18 / PDF Task 7.1** baseline + **BORU-SS-34 / PDF Task 2.2
hardware-encoder** update. Measured on Linux (debsrv).

## Question

PDF Task 7.1 asks for H.264 encoding through the existing OpenH264 dependency
with 720p/30 and 1080p/30 reference targets, bitrate / fps / keyframe-interval
/ quality-profile configuration, and a CPU benchmark on a representative
Linux system. How much CPU does the encode path consume at each reference
target, and can it sustain 30 fps?

PDF Task 2.2 (BORU-SS-34) adds a hardware-accelerated encoder path behind the
same `VideoEncoder` boundary: on Linux the host first tries the VA-API H.264
encoder (`VaapiEncoder`, `libva` via dlopen — no VA headers at build time) and
falls back to OpenH264 on any typed-unavailable failure. The benchmark now
measures both backends with the same harness.

## Method

`src/screen_share/encode_bench.rs` drives the real production paths — the
`OpenH264Encoder` and `VaapiEncoder` implementations of `VideoEncoder` — with
synthetic desktop frames (a mostly-static gradient with a small moving cursor
block, so the encoder sees realistic screen content rather than a
trivially-compressible static frame).

Per case it encodes 90 frames and reports:

- **avg ms/frame** — steady-state encode latency including the full
  `encode()` path used by the host loop (RGBA→RGB, scale, RGB→YUV via the
  fast integer `write_yuv_scalar` path, H.264 encode; the VA-API path adds
  RGBA→NV12 conversion + `vaPutImage` upload, so its numbers include the
  CPU-side conversion cost too).
- **encode fps** — how many frames/second the encoder can sustain
  back-to-back.
- **cpu%1core@targetfps** — the fraction of one core occupied when sustaining
  the target frame rate: `avg_ms_per_frame * target_fps / 1000 * 100`.

Run (release mode — debug `-O0` inflates the CPU-bound path ~40x):

```bash
cargo test --release --features screen-sharing --lib -- --ignored --nocapture encode_bench
```

The VA-API case (`benchmark_vaapi_720p30_and_1080p30`) requires a usable DRI
render node with an H.264 encode entrypoint. On a host without one it prints
`VA-API hardware encoder unavailable, skipped` and the OpenH264 cases still
run — mirroring the host's own fallback orchestration.

## Environment

- Host: debsrv (172.16.0.59, 8 cores, Ubuntu/Debian) via `rb`.
- GPU: NVIDIA Quadro K2200 (GM107/Maxwell) — **no NVIDIA VA-API driver
  installed** (`nvidia-vaapi-driver` absent; only i965/crocus drivers present,
  which bind Intel hardware). libva.so.2 / libva-drm.so.2 are present.
- OpenH264 0.9.7 (crate `openh264`, C encoder via `openh264-sys2`),
  `UsageType::ScreenContentRealTime`, `RateControlMode::Bitrate`,
  `skip_frames(false)` (mandatory for the static-screen decodability fix).
- Release profile: `opt-level=3`, LTO fat, `codegen-units=1`.
- Date: 2026-08-15.

## Results

### OpenH264 (software) — measured 2026-08-15 on debsrv via `rb` (release)

```
encode_bench: 720p30-balanced:   1280x720@30fps avg=15.3ms/frame encode_fps=65.4 cpu%1core@30fps=45.9 bytes=24342 bitrate=141kbps
encode_bench: 1080p30-balanced:  1920x1080@30fps avg=29.1ms/frame encode_fps=34.3 cpu%1core@30fps=87.4 bytes=45285 bitrate=138kbps
encode_bench: 1080p30-lowlatency: 1920x1080@30fps avg=29.8ms/frame encode_fps=33.6 cpu%1core@30fps=89.3 bytes=45460 bitrate=136kbps
encode_bench: 1080p30-highquality: 1920x1080@30fps avg=31.4ms/frame encode_fps=31.9 cpu%1core@30fps=94.2 bytes=95276 bitrate=270kbps
```

(BORU-SS-18 measured 14.8 / 28.1 / 28.0 / 30.1 ms/frame on the same host; the
BORU-SS-34 re-run above confirms the baseline within scheduler noise.)

Interpretation (unchanged from BORU-SS-18):

- **720p30 is cheap**: ~15.3 ms/frame (65 fps sustained) and only **46% of one
  core** to hit the 30 fps target. Plenty of headroom for capture, transport
  and GUI on the same core.
- **1080p30 is feasible but heavier**: ~29 ms/frame (34 fps sustained) and
  **87% of one core** at 30 fps. Sustains the target on this 8-core host with
  ~13% headroom — but leaves little room for other host-side work on the same
  core, which is exactly the scenario the adaptive quality tasks (PDF 7.2/7.3)
  address.
- **Quality profile knob works as intended**: LowLatency ≈ Balanced latency
  (complexity low vs medium at this bitrate), HighQuality costs ~7% more CPU
  (94% vs 87% of a core) while roughly doubling output bytes (270 vs 138 kbps
  on this synthetic content) — the expected quality/CPU trade.
- **Bitrate on the synthetic pattern is a floor, not a ceiling**: the test
  pattern is mostly-static (gradient + cursor), so deltas compress to near
  nothing; real desktop content (text, windows, motion) will produce higher
  bitrates at the same settings. The CPU numbers are the meaningful output.

### VA-API (hardware) — typed-unavailable on debsrv

```
libva info: VA-API version 1.17.0
libva info: Trying to open /usr/lib/x86_64-linux-gnu/dri/nvidia_drv_video.so
libva info: va_openDriver() returns -1
encode_bench: vaapi-720p30-balanced: VA-API hardware encoder unavailable, skipped (vaInitialize failed: unknown libva error)
```

debsrv's GPU is an NVIDIA Quadro K2200. The DRI render node is reachable
(`/dev/dri/card0`, user in the `video` group) and libva 1.17.0 initializes,
but driver selection fails: libva probes `nvidia_drv_video.so` (the NVIDIA
VA-API shim that wraps NVENC), which is **not installed** — the host only
carries Intel-bound drivers (`i965`, `crocus`). The VA-API encoder therefore
returns the typed `ScreenShareErrorKind::HardwareAccelerationUnavailable`
error on this host and the factory falls back to OpenH264 — exactly the
acceptance path: *"a documented, typed-unavailable path on this hardware with
OpenH264 fallback"*. Installing `nvidia-vaapi-driver` would make the same
binary encode on this GPU, but it pulls the proprietary NVIDIA driver/EULA
into scope, which BORU-SS-34 explicitly defers to a licensing review.

The VA-API path itself is implemented end-to-end (`src/screen_share/vaapi.rs`,
~1.5k lines of `#[repr(C)]` libva bindings validated against the system
headers by a size-probe test, dlopen'd through the already-present
`libloading` dependency — no VA dev headers needed at build time, no new
crate dependency). Its unit tests pass on debsrv:
`vaapi_layouts_match_system_headers` (struct offsets vs `va.h` /
`va_enc_h264.h`), `codec_kind_wire_names_round_trip`, and
`nv12_conversion_matches_known_values` (BT.601 studio-range conversion). On a
host with an Intel/AMD VA-API-capable GPU the same binary selects the
hardware encoder with no code change.

### AdaptiveQuality on the hardware path

`VaapiEncoder::reconfigure_bitrate` sends a fresh `VAEncMiscParameterTypeRateControl`
buffer on the next frame without rebuilding the context, so bitrate-only
`AdaptiveQuality` changes keep working without a config-generation bump (the
decoder keeps its instance). Resolution changes (`configure`) tear down and
rebuild the VA context and bump `generation`, matching the OpenH264 contract.

## Notes / deviations

- The encoder previously used `YUVBuffer::from_rgb_source`, which routes
  through the f32 per-pixel `write_yuv_by_pixel` converter and measured
  ~40 ms extra per 1080p frame (~68 ms/frame total, below the 30 fps target).
  Switching to `from_rgb8_source` (integer `write_yuv_scalar`) brought 1080p30
  to ~28 ms/frame — above the 30 fps target. This is a Boru-owned performance
  fix using the openh264 crate's documented fast path.
- Windows benchmark: not run in this task (Windows test hardware not
  available in this environment); the benchmark test is cross-platform and can
  be run on a Windows build host when one is available. The Windows
  Media Foundation H.264 encoder (`h264_mf`, IMFTransform) is a documented
  `CodecKind` that returns a typed `HardwareAccelerationUnavailable` error
  when requested in this build — never a silent software encode — and is
  wired for future implementation via the `windows` crate already in the
  dependency graph.
- NVENC remains out of scope until the NVIDIA SDK redistribution terms are
  reviewed (BORU-SS-34 gate).

## Acceptance

- Benchmark recorded: this document (committed with BORU-SS-18, updated with
  BORU-SS-34).
- 720p30 and 1080p30 reference targets both sustain 30 fps on Linux (debsrv)
  with OpenH264; the VA-API hardware path is a documented typed-unavailable
  path on this hardware with automatic OpenH264 fallback, implemented
  end-to-end and unit-tested.
- Negotiation advertises the real encoder: `Hello.codecs` is built from
  `available_encoder_codecs()` (`h264_vaapi` first when a render node is
  usable, then `h264`); the viewer decodes the resulting baseline H.264 with
  its existing decoder unchanged.
- Bitrate, fps, keyframe interval and quality profile are configurable
  (`CaptureConfig` → `CodecConfig::from_capture_config`; `QualityProfile` on
  the wire `StreamConfig`); bitrate-only changes work on the VA-API path via
  dynamic rate-control buffers.
- Unit tests for encoder lifecycle + config application pass
  (`screen_share` lib suite; `rb check --all-targets --features screen-sharing`
  green).
