# telepathy-audio crate evaluation for boru voice features

Date: 2026-08-02
Evaluator: deepseek-coder (kanban t_21756b98)
Source: https://github.com/chanderlud/telepathy/tree/master/rust/telepathy-audio (commit 6767fc6)
Crate version examined: v0.3.0 (workspace member of the telepathy repo)

## 1. Crate overview

`telepathy-audio` is the standalone audio-processing crate of the Telepathy
project (a Flutter + Rust + iroh voice chat app by Chander Luderman Miller).
It is a real-time audio library covering the full capture→process→encode→
network→decode→playback chain, with no UI and no transport of its own.

### What it provides

| Area | API |
|---|---|
| Device management | `devices::AudioHost` trait: `list_input_devices`, `list_output_devices`, `list_all_devices`, `input_sample_rate`, `output_sample_rate`, `open_input`, `open_output`. Concrete impls: `CpalAudioHost` (native) and `MockAudioHost` (tests). `AudioDeviceInfo { name, id }`, `AudioDeviceList`, `DeviceDirection`. |
| Capture (input) | `io::AudioInputBuilder` — per-stream config: `device(id)`, `denoise(RnnModel)` (nnnoiseless/RNNoise suppression), `volume(f32)`, `rms_threshold(f32)` (silence detection), `codec(CodecBitrateMode, residual_bits)` (SEA encode), `output_sample_rate(u32)`, `on_error(cb)`, plus shared-atomics for live control: `input_volume_shared`, `rms_threshold_shared`, `muted_shared`, `rms_shared`. Data delivery via `callback(Fn(PooledBuffer))` or any `AudioDataSink`. Returns `AudioInputHandle` (mute/unmute/set_volume/is_muted). |
| Playback (output) | `io::AudioOutputBuilder` — `device(id)`, `sample_rate(u32)` (source rate; auto-resamples to device rate), `volume(f32)`, `codec(bool)` (SEA decode), `on_error(cb)`, shared atomics: `output_volume_shared`, `deafened_shared`, `rms_shared`, `loss_shared` (underrun counter). Data from any `AudioDataSource` (e.g. `adapters::MpscSource`). Returns `AudioOutputHandle` (deafen/undeafen/set_volume/loss_receiver). |
| Channel abstraction | `io::traits::{AudioDataSink, AudioDataSource}` — trait-based, channel-agnostic. Ships `adapters::{MpscSink, MpscSource}` for `std::sync::mpsc`. Consumers can implement over tokio mpsc / flume / iroh streams. |
| Codec | Vendored **SEA codec** (`sea` module), modified from Daninet/sea-codec (MIT). Streaming, frame-based, hardcoded 480-sample frames. CBR and VBR modes, `EncoderSettings { scale_factor_bits, scale_factor_frames, residual_bits (2–8), frames_per_chunk, vbr }`. Raw (no-codec) path ships 960-byte frames of i16 @ 48 kHz; codec path ships < 960 bytes. |
| Noise suppression | `nnnoiseless` (RNNoise-derived, BSD-3-Clause) via `RnnModel` / `DenoiseState`. When enabled, input is upsampled to 48 kHz and output is always 48 kHz. |
| Resampling | `rubato` v3 `FixedSync` resampler between device rate and processing/network rate. |
| File playback | `player::{AudioPlayer, SoundHandle}` — WAV (U8/I16/I32/F32/F64) and SEA file playback with volume (dB API), fade in/out, cancellation. |
| SIMD | `internal::processing`: AVX-512 / AVX2 / WASM SIMD v128 with runtime feature detection and scalar fallback (wide_mul, i16↔f32 conversion, RMS). 480 % 16 = 0 so SIMD paths apply. |
| Errors | Rich `Error` enum: Device / Stream / Processing / Channel / Config / Task / AudioFile / Wasm, with structured sub-errors. |
| WASM | `WebAudioWrapper` (async, main-thread init for autoplay policy) + AudioWorklet capture; `wasm_thread` Web Workers for processor threads; requires SharedArrayBuffer (COOP/COEP headers) and optionally nightly rust with build-std for atomics. |

### Processing model

Each stream owns a dedicated processor thread: input runs
device → resample → volume → (denoise @48k) → RMS/silence → (SEA encode) →
`AudioDataSink`; output runs `AudioDataSource` → (SEA decode) → resample →
volume → device. Stream teardown is drop-based (handle drop joins the thread).
`FRAME_SIZE = 480` samples (10 ms @ 48 kHz), `NETWORK_FRAME = 960` bytes.

## 2. Dependency analysis

### telepathy-audio v0.3.0 direct dependencies

| Crate | Version | Purpose |
|---|---|---|
| cpal | 0.18 (features: wasm-bindgen, realtime) | cross-platform audio I/O (ALSA/WASAPI/CoreAudio/AAudio) |
| nnnoiseless | **git fork**: chanderlud/nnnoiseless @ branch `wasm-optimization`, 0.5.1, default-features=false | RNNoise denoising |
| rubato | 3 | resampling |
| rtrb | 0.3 | real-time ring buffers |
| crossbeam | 0.8 | threading/channels |
| audioadapter-buffers | 3 | zero-copy interleaved slice buffers |
| bytes | 1 | frame payloads (`Bytes`, `BytesMut`) |
| atomic_float | 1 | shared volume/RMS atomics |
| libm | 0.2 | float math for codec |
| tokio | 1 (time, sync, rt, macros) | task lifecycle |
| tracing | 0.1 | logging |
| cfg-if | 1 | target gating |
| *WASM-only*: wasm_thread 0.3, web-sys **=0.3.85 (exact pin)**, wasm-bindgen 0.2, wasm-bindgen-futures 0.4, js-sys 0.3, serde-wasm-bindgen 0.6, wasm_sync 0.1, wasmtimer 0.4 | | browser/WebAudio |

Notably: **`telepathy-audio` itself has NO iroh dependency.** iroh v1 lives in
`telepathy-core` (the Flutter bridge crate) and only on WASM targets. The
claim "same iroh v1 generation as boru" applies to the surrounding project
(telepathy-core uses iroh 1.0.3, same as boru's patched iroh), not to the
audio crate — which is transport-agnostic.

### Conflict check vs boru's tree

Boru's `Cargo.lock` contains **none** of: cpal, nnnoiseless, rubato, rtrb,
crossbeam, atomic_float, audioadapter-buffers, dasp_sample, alsa, realfft,
rustfft. Every dependency telepathy-audio brings is **new to boru** — no
version conflicts.

Shared crates align exactly with boru's lock (verified against
`Cargo.lock` at repo head):

| Crate | boru lock | telepathy lock | Verdict |
|---|---|---|---|
| bytes | 1.12.1 | 1.12.1 | aligned |
| tokio | 1.53.1 | 1.53.1 | aligned |
| tracing | 0.1.44 | 0.1.44 | aligned |
| libm | 0.2.16 | 0.2.16 | aligned |
| iroh | 1.0.3 (patched) | 1.0.3 (telepathy-core only) | aligned |

Edition: telepathy-audio uses edition 2024; boru is edition 2021 with
rust-version 1.91. Edition 2024 requires Rust ≥ 1.85 — fine for boru's MSRV.
Build verified on this machine (cargo 1.97.1, Linux): `cargo build` succeeds
(needs ALSA dev headers, which are present), **150 tests + 16 doctests pass**
(110 unit, 24 trait integration, 13 processor integration, 3 SEA codec,
16 doctests).

### Red flags

1. **NOT published on crates.io.** `crates.io/api/v1/crates/telepathy-audio`
   returns 404 ("crate does not exist") as of 2026-08-02. The crate is only
   consumable as a git/path dependency (or vendored). This is the single
   biggest supply-chain concern.
2. **Git dependency inside the crate itself.** `nnnoiseless` is pinned to a
   personal fork branch (`chanderlud/nnnoiseless`, `wasm-optimization`), not
   a crates.io release. boru currently has zero git dependencies in its lock
   (all crates.io / patched paths).
3. **web-sys exact pin** `=0.3.85` (telepathy) vs `0.3.103` (boru's current
   WASM-facing tree). Only matters if boru ever compiles for WASM; cargo
   would keep both versions. Irrelevant for native builds.
4. Single-maintainer project, young (v0.3.0; repo activity current — last
   commit 2026-08-01). No docs.rs page; docs are README + source comments.

## 3. License

- `telepathy-audio` Cargo.toml: **MIT**; repo LICENSE: MIT, Copyright 2025
  Chander Luderman Miller. ✅ Compatible with boru's MIT/Apache-2.0.
- Vendored SEA codec: MIT (© 2025 Dani Biró, Daninet/sea-codec). ✅
- Transitive `nnnoiseless`: **BSD-3-Clause** (Joe Neeman / Mozilla / Xiph /
  Jean-Marc Valin). ✅ Compatible (boru's tree already includes BSD-3-Clause
  deps).
- No copyleft (GPL/AGPL) anywhere in the tree.

## 4. Cross-platform

| Platform | Backend | Status |
|---|---|---|
| Windows | cpal WASAPI | ✅ native threads |
| macOS | cpal CoreAudio | ✅ native threads |
| Linux | cpal ALSA | ✅ native threads (build-time dep: libasound2-dev) |
| Android | cpal AAudio | ✅ native threads |
| iOS | cpal CoreAudio | ✅ native threads |
| Web/WASM | web-sys WebAudio + AudioWorklet | ✅ but requires SharedArrayBuffer (COOP/COEP headers), Web Workers, and nightly rust + build-std for atomics |

For boru (a native iced GUI app on Linux/macOS/Windows), the WASM path is
irrelevant. Native coverage is complete. The ALSA system header is a new
build-time requirement on Linux but is standard and already installed on
this machine.

## 5. Fit assessment — recommendation: **ADOPT (vendored), with conditions**

### Why adopt

- **Feature set is exactly what a boru voice MVP needs in one crate**:
  capture, denoise, silence detection, resampling, lossy codec, playback,
  volume/mute/deafen control, level meters — all behind clean builder APIs.
- **Trait-based transport seam** (`AudioDataSink`/`AudioDataSource`) maps
  directly onto boru's iroh streaming (mirrors how FileAccessHandler and the
  backfill protocol already wrap iroh QUIC streams). No audio-specific
  transport coupling.
- **Live-control atomics** (`muted_shared`, `input_volume_shared`,
  `deafened_shared`) fit boru's UI-state pattern without extra plumbing.
- **Verified healthy**: builds clean and 166 tests pass on this machine;
  core shared deps (bytes/tokio/tracing/iroh) are version-aligned with boru.
- **License clean** (MIT + BSD-3-Clause transitives only).

### Conditions / required decisions

1. **Do not consume as a crates.io dependency — it doesn't exist there.**
   Two viable paths:
   - *Vendor* (recommended, matches boru house style): copy
     `rust/telepathy-audio` into boru as `vendor/telepathy-audio` (boru
     already vendors patched iroh/mainline under `patched/`). Pin commit
     6767fc6. This also lets boru replace the `nnnoiseless` git-fork dep
     with a pinned vendored copy if desired.
   - *Git dependency*: `telepathy-audio = { git = "https://github.com/chanderlud/telepathy", rev = "<pin>", package = "telepathy-audio" }`.
     Works (cargo resolves the workspace member) but pulls the nnnoiseless
     fork-branch dep into boru's lock and depends on GitHub availability.
2. **Do not pull in `telepathy-core`** — it's the Flutter bridge crate
   (flutter_rust_bridge, jni, objc2) and has no role in a native boru.
3. **Frame/negotiation contract**: pick a fixed capture profile — 48 kHz,
   mono, 10 ms frames, SEA CBR (residual_bits 4–6) or raw 960-byte frames —
   and carry a 1-byte header (codec mode + rate) in boru's message framing so
   both ends agree. Both ends must use the same SEA settings (header is
   embedded per-frame in SEA chunks).
4. **System dep**: add `libasound2-dev` to build prerequisites on Linux.
5. **Re-evaluate before shipping to production users**: single maintainer,
   no crates.io release, and the denoiser comes from a personal fork. For an
   MVP/experimental voice feature this is acceptable; for a release it
   should be pinned+reviewed or upstreamed.

### Integration sketch

```
Capture side (per voice room/peer):
  AudioInputBuilder::new()
    .denoise(RnnModel::default())            // 48 kHz out, noise suppression
    .codec(CodecBitrateMode::Cbr, 5.0)       // SEA encode, <960 B/frame
    .muted_shared(&ui_state.muted)
    .input_volume_shared(&ui_state.volume)
    .sink(IrohQuicSink)                       // impl AudioDataSink
    .build(&host)?;

  impl AudioDataSink for IrohQuicSink {       // wraps boru's iroh QUIC stream
    fn send(&self, data: PooledBuffer) -> Result<(), ClosedOrFailed> {
      stream.send_all(&mut [Bytes::from(data.as_ref())]) ...
    }
  }

Playback side:
  AudioOutputBuilder::new()
    .sample_rate(48_000)
    .codec(true)
    .deafened_shared(&ui_state.deafened)
    .output_volume_shared(&ui_state.volume)
    .source(IrohQuicSource)                   // impl AudioDataSource
    .build(&host)?;

Transport:
  boru gossip/QUIC channel carries 960-byte (raw) or <960-byte (SEA) frames;
  loss_shared() on the output handle exposes underruns for a quality metric.
```

Steps if adopted:
1. Vendor the crate + pin the nnnoiseless fork commit; add libasound2-dev note.
2. Implement `AudioDataSink`/`AudioDataSource` over boru's iroh stream layer.
3. Add a minimal voice-frame framing/negotiation header in boru's protocol.
4. Prototype in a feature-gated example (`voice` feature) before touching the
   chat UI; measure latency with boru's existing PerfTracker.

## Summary

`telepathy-audio` v0.3.0 is a well-scoped, MIT-licensed, actively-developed
audio pipeline crate whose API, transport-agnostic trait seams, and
dependency alignment make it the most credible off-the-shelf foundation for
boru voice chat. It is not on crates.io and depends on a personal nnnoiseless
fork — acceptable for an experimental feature if **vendored and pinned**, not
if consumed loosely. **Recommendation: adopt via vendoring for a voice MVP;
re-evaluate (pin/upstream) before any production release.**
