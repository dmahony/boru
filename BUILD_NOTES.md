# Boru Build Notes — Cargo features, native requirements, and cross-platform verification

Date: 2026-08-09
Scope: BORU-CALL-12 (Phase 12 of the P2P voice/video call plan) — verifies the
Cargo feature graph, documents native build requirements, and lists the
Windows/macOS verification checklist that cannot be executed on this Linux host.

## 1. Feature graph (verified against `Cargo.toml` at v0.161.0)

```
default      = ["net", "metrics"]
net          = [iroh, irpc, iroh-blobs, iroh-mdns-address-lookup, iroh-mainline-address-lookup,
                tokio, tokio-util, futures-concurrency, serde_json, ...]
gui          = [net, iced, iced_aw, iced_moving_picture, tokio, rfd, tracing-subscriber,
                tracing-appender, mimalloc, rayon, profiling, rustc-hash, regex, open, url,
                reqwest, netstat2, sysinfo]
voice-calls  = [cpal, rtrb, opus, rubato, nnnoiseless]                       # all optional
video-calls  = [voice-calls, nokhwa, openh264]                               # all optional
video-playback = [gui, iced_video_player]
terminal     = [gui, iced_term]
```

- `voice-calls` pulls in cpal (native audio I/O), rtrb + rubato (pure Rust ring
  buffer / resampler), opus (encode/decode), nnnoiseless (noise suppression).
- `video-calls` implies `voice-calls` and adds nokhwa (camera capture,
  `input-native` only — V4L2/AVFoundation/MediaFoundation) and openh264
  (H.264 encode/decode).
- `gui` includes `net`; `video-calls` does **not** include `gui`. The live-call
  video pipeline (`src/call/video/*`) is independent of the attachment
  playback system (`video-playback` / `streaming_server` / `iced_video_player`).

## 2. Isolation of media dependencies

All audio/video dependencies are declared `optional = true` and are only pulled
in through the `voice-calls` / `video-calls` features:

| Crate | Version | Optional | Pulled in by |
|---|---|---|---|
| cpal | 0.18 | yes | voice-calls |
| rtrb | 0.3 | yes | voice-calls |
| opus | 0.3 | yes | voice-calls |
| rubato | 3 | yes | voice-calls |
| nnnoiseless | 0.5.2 | yes | voice-calls |
| nokhwa | 0.10.11 | yes | video-calls |
| openh264 | 0.9.7 | yes | video-calls |

Camera/audio dependencies are NOT forced into unrelated headless/core builds:
`cargo check --no-default-features` does not even reference them, and
`src/call/` has zero imports of net-gated modules (`chat_core`,
`catalogue_model`, `friends`, `mailbox` — verified by grep).

## 3. Check matrix results (Linux host, 2026-08-09)

| Configuration | Result |
|---|---|
| `cargo check --features voice-calls` | ✅ PASS |
| `cargo check --features video-calls` | ✅ PASS |
| `cargo check --features gui,video-calls` | ✅ PASS |
| `cargo check --no-default-features` | ❌ FAIL — pre-existing, unrelated to media deps (see below) |

Known pre-existing `--no-default-features` failure (NOT introduced by the
voice/video feature work — reproduced at base commit `57bd51dc` which predates
the video-calls feature):
- `src/download.rs:11` — `use crate::chat_core::TRANSFER_TELEMETRY;` (chat_core is net-gated)
- `src/storage.rs:43,45` — `use crate::catalogue_model::...` / `use crate::friends::...` (both net-gated)
- `src/catalogue_handler.rs:20` — `use iroh::...` (iroh is net-gated)

The crate currently requires `net` (the default) to compile: several
non-gated modules unconditionally reference net-gated types. This is a
pre-existing structural issue in the file-sharing/storage subsystem, not a
regression from the call feature graph. `default = ["net", "metrics"]`
builds clean.

## 4. Native build requirements

### Linux (verified on this host)

- **voice-calls**
  - `cpal` — needs ALSA dev headers: `libasound2-dev` (Debian/Ubuntu),
    `alsa-lib` on other distros. Verify: `pkg-config --exists alsa && echo alsa-OK`
  - `opus` — links `libopus` via pkg-config (`libopus-dev`); if absent, the
    `opus-sys` build script compiles the vendored Opus source with
    configure+make (autotools).
  - `rtrb`, `rubato` — pure Rust, no system libraries.
- **video-calls** (adds to the above)
  - `nokhwa` — Linux uses V4L2 via `libclang` for bindgen
    (`libclang-dev` + `clang`). Needs a functioning `LIBCLANG_PATH` or the
    default clang install.
  - `openh264` — builds the C++ sources; needs a C++17 toolchain
    (`g++`/`clang++`).

### Windows MSVC (NOT compilable on this Linux host — CI/human must verify)

- `cpal` — WASAPI, no extra SDK.
- `nokhwa` — MediaFoundation (included in Windows SDK).
- `openh264` — MSVC C++ toolchain required; verify `cc`/`cl.exe` available in
  the build environment.
- `opus` — MSVC build via cc crate.
- Recommended CI steps:
  1. `cargo check --features gui,video-calls` on `windows-latest`
  2. `cargo test --features video-calls --lib -- call::video` on `windows-latest`
  3. Confirm no `-lgcc_s` / ALSA symbols leak into the link.

### macOS (NOT compilable on this Linux host — CI/human must verify)

- `cpal` — CoreAudio, no extra SDK.
- `nokhwa` — AVFoundation (included); macOS camera permission prompt is a
  runtime concern, not build-time.
- `openh264` — clang with C++17.
- Recommended CI steps:
  1. `cargo check --features gui,video-calls` on `macos-latest`
  2. `cargo test --features video-calls --lib -- call::video` on `macos-latest`

## 5. CI coverage in this repository

- `.github/workflows/tests.yaml` already runs a feature matrix
  (`all` / `none` / `default`), so the `--no-default-features` failure above
  should be addressed by gating `download.rs`/`storage.rs` imports (or the
  whole file-sharing subsystem) behind `net` before relying on the `none`
  leg of the matrix.
- `.github/workflows/ci.yaml` runs clippy with `--no-default-features --lib`
  and `--all-features`; the `none` leg will fail until the above is fixed.
