# Media rendering decision record

**Status: Linux decision measured.** The measured Linux method is the Web standard media surface inside Boru's real Tauri WebKitGTK WebView: `<img>` for still thumbnails, `<video>` for encoded playback, and Canvas 2D for synthetic decoded frames. Windows and macOS remain **NOT MEASURED**; this record makes no cross-platform performance claim.

## Decision by platform

| Platform | Status | Decision |
|---|---|---|
| Linux / Tauri / WebKitGTK | **MEASURED** (environment below) | Use `<img>` for still thumbnails, `<video>` for encoded video playback, Canvas 2D for decoded-frame drawing. The measured paths completed all five trials through the real opaque-handle media IPC. This is the final Linux decision for this application. |
| Windows / Tauri / WebView2 | **NOT MEASURED** | No reachable Windows/WebView2 runner was available. Do not infer a method or performance from Linux. Measure on a Windows runner with WebView2 before deciding. |
| macOS / Tauri / WKWebView | **NOT MEASURED** | No reachable macOS/WKWebView runner was available. Do not infer a method or performance from Linux. Measure on a macOS runner with WKWebView before deciding. |

## Linux measurement

### Environment and identity

- Host OS: Debian GNU/Linux 12 (bookworm), kernel `6.1.0-56-amd64`.
- CPU: Intel Xeon E3-1270 v5 @ 3.60 GHz; 8 logical CPUs.
- Host GPU: NVIDIA GM107GL / Quadro K2200 (`nvidia` kernel driver visible on the host).
- Actual application: Boru Tauri desktop binary, launched under `xvfb-run` on a 1440x1200x24 virtual display; the window and test surface were visibly rendered and the `<img>` preview appeared.
- Tauri crate resolved by Cargo: `tauri v2.12.7`; Wry runtime: `tauri-runtime-wry v2.12.5`.
- WebKitGTK: `2.50.6` from `pkg-config --modversion webkit2gtk-4.1`. The in-WebView user agent was `Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/60.5 Safari/605.1.15`.
- Display scale: `GDK_SCALE=1`, `GDK_DPI_SCALE=1`; WebView `devicePixelRatio=1` (viewport 1100x760).
- Rendering mode: the app was launched with `LIBGL_ALWAYS_SOFTWARE=1` under Xvfb. Hardware-accelerated rendering was **not** measured. The WebGL debug extension returned `Apple Inc.` / `Apple GPU`, which does not match this Linux host and is not treated as reliable renderer identification; the exact software GL renderer (for example, llvmpipe) was not independently verified. The host's NVIDIA device being present does not establish that WebKit used it.

### Method and scope

The workload shape and benchmark approach are **derived from prototype commit `b21cbcc9`** (`desktop/src/media-benchmark.ts` and `docs/media-rendering-benchmark.md`), but the harness was rebuilt against the landed media transport. The conditional test surface is only rendered when `VITE_MEDIA_BENCHMARK=1` is set; normal production UI paths remain unchanged.

Each measured sample uses the real Tauri media commands: upload bytes through `media_write_chunk`, retain only the opaque handle ID across commands, and read bytes via `media_read_chunk` to construct a Blob for browser decoding/rendering. No base64 payload or filesystem path is sent through IPC. `ipc_payload_bytes` counts raw upload plus read bytes for that sample. Fixture generation/fetch and upload happen before the elapsed timer; the upload bytes are included in the IPC byte count. The one warm-up is excluded from the 15 recorded samples.

- Still-image workload: 24 `<img>` thumbnails, each decoded from a Blob reconstructed from the same PNG handle; throughput is images/second.
- Encoded-video workload: a 2-second, 320x180, 24-fps WebM/VP8 fixture, played to `ended` through `<video>`; throughput is transferred bytes/second over read plus playback time.
- Synthetic-frame workload: 60 PNG-backed `ImageBitmap` decodes drawn to a 640x360 Canvas 2D surface, yielding between frames with `requestAnimationFrame`; throughput is frames/second.
- `item_p50_ms` and `item_p95_ms` are within-trial item latency percentiles. For video there is one clip per trial, so each is the whole clip trial duration. The table below reports the median of the five trials and the nearest-rank p95 across five trial results (therefore p95 is the largest observation for n=5).
- Responsiveness was measured with a `setTimeout` gap sampler. WebKitGTK reported `PerformanceObserver` long-task entries unsupported (`longtask_observer_supported: false`), so long-task counts are **NOT MEASURED**; raw JSON encodes them as `null`, not zero.
- External RSS was sampled once per second with `ps` for `boru-desktop` and `WebKitWebProcess`; the maximum aggregate RSS over the complete benchmark run was **554,036 KiB**. This is a whole-run peak, not attributable to a specific workload; per-workload RSS slices are **NOT MEASURED**. It is process RSS, not JavaScript heap.

### Results

| Workload / method | Elapsed median / p95 (ms) | Throughput median / p95 | Item p50 median / p95 (ms) | Item p95 median / p95 (ms) | IPC bytes per trial | Responsiveness max-gap median / p95 (ms) | Long tasks |
|---|---:|---:|---:|---:|---:|---:|---|
| 24 still thumbnails / `<img>` | 144 / 164 | 166.67 / 176.47 images/s | 6 / 7 | 7 / 9 | 205,875 | 7 / 10 | NOT MEASURED (API unsupported) |
| 2s WebM VP8 / `<video>` | 2,024 / 2,036 | 43,885.38 / 43,928.78 bytes/s | 2,024 / 2,036 | 2,024 / 2,036 | 177,648 | 8 / 9 | NOT MEASURED (API unsupported) |
| 60 decoded frames / Canvas 2D | 202 / 204 | 297.03 / 306.12 frames/s | 3 / 3 | 4 / 5 | 502,335 | 7 / 7 | NOT MEASURED (API unsupported) |

All three workloads completed in all five trials. Full per-trial values and the WebView runtime metadata are committed in [`media-rendering-linux-webkitgtk.json`](media-rendering-linux-webkitgtk.json).

### Native surface / texture comparison

**NOT MEASURED / unavailable in this application path.** The desktop media integration exposes opaque-handle IPC to the WebView; it does not provide a native GTK media surface or a WebKit-to-native texture interop path for these workloads. There was no like-for-like native implementation to time. The result supports the web-standard Linux surfaces only; it is not evidence that they outperform a native texture path.

## Reproduction on the recorded Linux runner

The frontend lockfile is `desktop/pnpm-lock.yaml`; the runner started with an empty `desktop/node_modules`. Dependencies were installed from the frozen lockfile:

```sh
cd desktop
corepack pnpm@10.23.0 install --frozen-lockfile
VITE_MEDIA_BENCHMARK=1 corepack pnpm@10.23.0 build
VITE_MEDIA_BENCHMARK=1 corepack pnpm@10.23.0 dev -- --host 127.0.0.1 --port 1420
```

The Tauri binary was built and run on DEBSRV, not in Chromium:

```sh
cd desktop/src-tauri
source ~/.cargo/env
cargo build

# Launch environment used by the benchmark (isolated XDG data/config dirs):
export HOME=/home/dan/boru-build/media-bench-data/home
export XDG_DATA_HOME=/home/dan/boru-build/media-bench-data/xdg-data
export XDG_CONFIG_HOME=/home/dan/boru-build/media-bench-data/xdg-config
export XDG_DOWNLOAD_DIR=/home/dan/boru-build/media-bench-data/downloads
export GDK_SCALE=1 GDK_DPI_SCALE=1 LIBGL_ALWAYS_SOFTWARE=1
xvfb-run -a -s "-screen 0 1440x1200x24" ./target/debug/boru-desktop
```

Runtime identity checks:

```sh
pkg-config --modversion webkit2gtk-4.1
cargo tree -p tauri --depth 0
cargo tree -p tauri-runtime-wry --depth 0
```

The prototype workload shape was recovered from `b21cbcc9`; fixture generation, opaque-handle transport, timing semantics, and unsupported-metric reporting above describe the rebuilt implementation used for these results.

## Verification

Commands run for this record:

```sh
rb check --bin boru --features gui
cd desktop && VITE_MEDIA_BENCHMARK=1 corepack pnpm@10.23.0 build
cd desktop/src-tauri && source ~/.cargo/env && cargo test
```

Rust check passed. The Tauri crate's current targets contain no Rust tests; Cargo reported:

```text
running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```
