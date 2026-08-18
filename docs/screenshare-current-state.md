# Boru Screen-Sharing — Current State Inventory

Status: **inventory snapshot** (BORU-SS-03 / PDF Task 1.1). This document maps the
existing Boru screen-sharing subsystem *before* any behaviour is changed. It is
the baseline for the rest of the BORU-SS chain (phases 2-14 of
`Boru_RustDesk_Reference_Screen_Sharing_Tasks.pdf`).

Companion docs: `docs/screenshare-rustdesk-reference-policy.md` (binding
licensing/reference policy, BORU-SS-01) and `docs/experimental-vnc-tunnel.md`
(separate experimental VNC prototype, not part of the native screen-share
subsystem).

Snapshot commit: `8fb88527` (BORU-SS-02) on `origin/main`.

---

## 1. Feature definition

The subsystem is an opt-in cargo feature (`Cargo.toml:294-296`):

```toml
screen-sharing = ["net", "dep:openh264", "dep:zbus", "dep:libloading", "dep:windows-sys", "dep:windows", "dep:x11rb"]
```

- The whole `src/screen_share/` tree is gated on this feature
  (`src/lib.rs:70-72`).
- The GUI wiring in `src/bin/boru/` is gated per-site with
  `#[cfg(feature = "screen-sharing")]` (106 sites in `app.rs`, 6 in
  `app/chat.rs`, 6 in `main.rs`).
- `screen-sharing` is NOT part of `default = ["net", "metrics", "gui"]`
  (`Cargo.toml:258`); it is exercised by CI via `--all-features`
  (`.github/workflows/tests.yaml:77`, `ci.yaml:294` clippy all-features) and
  by release builds on Windows/Linux (`.github/workflows/release.yaml:34`).
- Related but SEPARATE features:
  - `video-calls = ["voice-calls", "dep:nokhwa", "dep:openh264"]`
    (`Cargo.toml:278`) — live camera calls; shares only the `openh264` crate,
    no code with screen-share.
  - `experimental-vnc = ["gui"]` (`Cargo.toml:305`) — a *different* prototype
    that tunnels an external VNC server over Boru's TCP tunnel; explicitly
    **not** the native screen-share subsystem (see
    `docs/experimental-vnc-tunnel.md`). Guarded by
    `src/bin/boru/app/discover.rs:1468-1469`.

## 2. Module map (`src/screen_share/`)

18 files, ~6,100 lines (excluding tests; `wc -l` at this revision). The
definitive file list:

| Module | Lines | Role | Status |
|---|---|---|---|
| `mod.rs` | 142 | Subsystem boundary, re-exports, `ScreenShareError`, boundary unit tests | Implemented |
| `capture.rs` | 232 | `PixelFormat`, `CapturedFrame`, `FrameSink`, `ScreenCapture` trait, `TestPatternCapture` | Implemented |
| `coords.rs` | 232 | Pure desktop↔source↔normalized coordinate mapping, DPI helpers, cursor sprite compositing (BORU-SS-12) | Implemented |
| `codec.rs` | 653 | `CodecConfig`, `QualityProfile`, `EncodedPacket`/`EncodedFrame` (capture + encode timestamps), `VideoEncoder`/`VideoDecoder` traits, `OpenH264Encoder`/`Decoder` | Implemented |
| `protocol.rs` | 413 | ALPN, `ControlMessage`, `Hello`, `Permission`, `ProtocolError`, `ScreenShareProtocol` (iroh handler) | Implemented |
| `transport.rs` | 214 | `MediaHeader` (capture + encode timestamps), `encode_media`/`decode_media`, `LatestFrameQueue`, `QuicScreenTransport`, `read_unit` | Implemented |
| `session.rs` | 288 | `ScreenShareSessionId`, `SessionState`, `SessionEvent`, `SessionManager` | Implemented |
| `host.rs` | 608 | `run_host_session` (dial → Hello → negotiate → capture/encode/send), `HostCommand`, pacing queue (BORU-SS-19) | Implemented |
| `viewer.rs` | 243 | `ViewerPipeline` (bounded receiver decode pipeline), `DecodedFrame` | Implemented |
| `permissions.rs` | 117 | `Capability`, `ControlToken`, `RequestRateLimiter`, `SessionPermissions` | Implemented |
| `remote_input.rs` | ~640 | `InputEvent`, `RemoteInput` trait, Linux RemoteDesktop portal / Windows SendInput backends + X11 XTest backend (BORU-SS-17), `device_mask_grants` + `parse_devices_mask` gates (BORU-SS-15) | Implemented |
| `adaptation.rs` | ~500 | `AdaptiveQuality` (congestion control: queue depth, throughput, RTT, encode time, drops; hysteresis; viewer `QualityUpdate` ceiling), `ViewerQualityRequest`, `QualityDecision`, `PacingController`/`PacingCounters` (latest-frame queue + drop counters, BORU-SS-19) | Implemented + wired into host.rs (BORU-SS-20) |
| `stats.rs` | 121 | `ScreenShareStats`, `ScreenShareStatsSnapshot` | Implemented (internal to viewer; not surfaced to UI) |
| `platform/mod.rs` | 103 | Per-OS dispatch, `ActiveCapture`, `create_capture_source` | Implemented |
| `platform/linux.rs` | 2544 | Portal/PipeWire capture (lifecycle machine + clean teardown) + X11 fallback (`DesktopCaptureBackend` with RandR monitor enumeration, BORU-SS-16) + dlopen PipeWire client | Implemented |
| `platform/linux_pw.rs` | ~620 | **Pure PipeWire format negotiation + CPU frame normalization (BORU-SS-14)**: SPA pod constants (verified against PipeWire headers), format advertisement pod builder, negotiated-format parser, SPA→`PixelFormat` layout mapping, stride-aware row copy with 24-bit RGB/BGR expansion, `NegotiatedFormat` with renegotiation generation counter | Implemented |
| `platform/windows.rs` | 554 | WinRT Graphics Capture backend (`DesktopCaptureBackend`) | Implemented |
| `platform/windows_common.rs` | 340 | Windows lifecycle state machine, HRESULT classification, monitor ids (Linux-tested) | Implemented |
| `platform/macos.rs` | 1 | Placeholder comment only | **Stub** |

### 2.1 Per-module detail with file:line evidence

**mod.rs** — re-exports every boundary type (`mod.rs:20-45`), defines
`ScreenShareError` (`mod.rs:48-64`), and contains a fake-boundary round-trip
test (`mod.rs:113-133`) plus a session-id uniqueness test (`mod.rs:135-141`).
Header comment states the module intentionally has no implementation
(`mod.rs:1-5`).

**capture.rs** — `PixelFormat` (`capture.rs:9-16`), `CapturedFrame` with CPU
validation (`capture.rs:19-78`), bounded `FrameSink` latest-frame queue
(`capture.rs:82-135`), `ScreenCapture` trait (`capture.rs:138-141`), and
`TestPatternCapture` synthetic source (`capture.rs:150-193`). 3 unit tests
(`capture.rs:203-231`). This is the milestone-7 capture backend used when no
real platform source is available.

**codec.rs** — codec config + constants (`codec.rs:6-11`), `CodecConfig`
validation (`codec.rs:32-42`), `EncodedFrame` (`codec.rs:51-60`),
`VideoEncoder`/`VideoDecoder`/`ScreenShareCodec` traits
(`codec.rs:62-78`), real `OpenH264Encoder` (`codec.rs:110-169`) and
`OpenH264Decoder` (`codec.rs:172-199`). Key behaviour: `skip_frames(false)`
is mandatory so a static screen still emits decodable frames
(`codec.rs:136-144`); encoder reconfigures by rebuilding
(`codec.rs:164-168`); decoder re-creates on config-generation change
(`codec.rs:185-189`). 3 unit tests incl. the static-screen regression
(`codec.rs:228-256`).

**protocol.rs** — `SCREEN_SHARE_ALPN = b"boru/screen-share/1"`
(`protocol.rs:18`), `SCREEN_SHARE_PROTOCOL_VERSION = 1` (`protocol.rs:20`),
`Permission::{ViewOnly,Capabilities}` (`protocol.rs:34-40`), `Hello`
(`protocol.rs:44-63`), `ControlMessage` (Hello/Accept/Reject/EndSession/
RequestControl/GrantControl/RevokeControl/Input) (`protocol.rs:67-88`),
`ProtocolError` (`protocol.rs:92-110`), wire `validate()` with bounds
(`protocol.rs:114-148`), postcard `encode`/`decode` with `MAX_CONTROL_FRAME`
(`protocol.rs:152-165`), `InboundMedia` (`protocol.rs:169-176`),
`ScreenShareProtocol` implementing `iroh::protocol::ProtocolHandler`
(`protocol.rs:180-280`) — keeps inbound connections per session so the app can
Accept/Reject on the same connection (`protocol.rs:263-276`). Includes the
full end-to-end QUIC invite→accept→media→decode test
(`protocol.rs:322-412`).

**transport.rs** — `ScreenTransport` sync trait (`transport.rs:17-19`),
`MAX_MEDIA_FRAME = 4 MiB` (`transport.rs:22`), `MediaHeader` with validation
(`transport.rs:48-74`), `encode_media`/`decode_media` framing
(`transport.rs:77-101`), `LatestFrameQueue` that discards stale non-keyframes
(`transport.rs:105-114`), `QuicScreenTransport` (reliable bi-stream control,
short-lived media streams; `transport.rs:118-144`), `read_unit`
(`transport.rs:147-163`). 3 unit tests (`transport.rs:171-189`).

**session.rs** — `ScreenShareSessionId` (16-byte, CSPRNG-generated,
`session.rs:13-25`), `SessionState` incl. `Streaming` only reachable after
explicit Accept (`session.rs:29`), `SessionEvent`
(Invitation/Accepted/Rejected/Ended/ControlRequest/ControlChanged,
`session.rs:33-46`), `SessionManager` state machine (`session.rs:53-190`)
with `MAX_ACTIVE_SESSIONS = 8` (`session.rs:55`). Notable security checks:
Hello host_id must match the connected peer (`session.rs:118-125`), Accept
must come from the invitee (`session.rs:135-154`), stranger Accept ignored
(test `session.rs:273-287`). 5 unit tests.

**host.rs** — `DEMO_WIDTH/HEIGHT/FPS` (640x360@15, `host.rs:29-33`),
`HostCommand::{GrantControl,RevokeControl}` (`host.rs:37-42`), and
`run_host_session` (`host.rs:48-78`) / `run_host_session_inner`
(`host.rs:81-336`): selects capture source up front (`host.rs:93-97`), dials
the viewer with the screen-share ALPN (`host.rs:109-115`), negotiates until
explicit Accept (`host.rs:157-195`), then streams capture→encode→send at
`CAPTURE_FPS` (`host.rs:216-334`) while honoring consent-gated remote input
(`host.rs:228-244`) and host commands (`host.rs:257-269`). Always emits a
final `Ended` event on silent exits so the host UI resets (`host.rs:70-77`).

**viewer.rs** — `ViewerPipeline<D: VideoDecoder>` (`viewer.rs:29-43`):
bounded queue (`viewer.rs:109-121`), ordering-watermark advance at enqueue
(`viewer.rs:117`), synchronous `process()` (decode all queued units,
`viewer.rs:125-171`), keyframe-recovery on decode error (`viewer.rs:159-167`),
`revoke()`/`end()` (`viewer.rs:76-87`), `take_frame()` (`viewer.rs:174-178`),
and counters/`stats()` (`viewer.rs:181-187`). 3 unit tests
(`viewer.rs:212-240`). **BORU-SS-21 (PDF Task 8.1, decoder pipeline):** the
pipeline now detects missing-sequence gaps at enqueue (a sequence jump counts
the lost units as dropped and requests a keyframe unless the arrival itself is
a keyframe — `viewer.rs:112-128`), recovers when a dependent frame decodes to
no picture (`Ok(None)` → keyframe request, `viewer.rs:153-163`), and surfaces
pending recovery via one-shot `take_keyframe_request()`
(`viewer.rs:218-226`); every request feeds a `keyframe_requests` counter and
the stats snapshot (`stats.rs:25-27`, `observe_keyframe_request`). The app's
decode worker (`app.rs:20498-20552`) drains the pending flag and emits
`ScreenShareMessage::KeyframeRequest` on the reliable control channel so the
host forces the next unit to be a keyframe (PDF Task 3.2 / Task 8.1). 9 unit
tests.

**permissions.rs** — `Capability::{ViewScreen,ControlPointer,ControlKeyboard,
Clipboard}` (`permissions.rs:10`), `MAX_CAPABILITIES = 4`
(`permissions.rs:11`), `RequestRateLimiter` (10s window, 4 requests,
`permissions.rs:12-26`), `ControlToken` with 15-minute TTL
(`permissions.rs:14,29-33`), `SessionPermissions` (`permissions.rs:36-90`)
with `view_only()` default, nonce-based grant (`permissions.rs:61-84`),
`revoke_control()` keeps ViewScreen (`permissions.rs:85-88`). 2 unit tests.

**remote_input.rs** — `InputEvent` (`remote_input.rs:13-20`),
`authorize_input`/`authorize_nonce` (`remote_input.rs:23-38`), `RemoteInput`
trait (`remote_input.rs:40-47`), `map_pointer`/`normalize_to_capture`
letterbox-aware coordinate mapping (`remote_input.rs:54-73`),
`UnavailableInputBackend` fail-closed default (`remote_input.rs:76-81`),
`create_platform_backend` factory (`remote_input.rs:86-102`), real Linux
xdg-desktop-portal RemoteDesktop backend via zbus (`remote_input.rs:110-175`),
real Windows `SendInput` backend via windows-sys (`remote_input.rs:184-273`,
incl. X11-keysym → virtual-key map `remote_input.rs:198-213`). 4 unit tests.

**adaptation.rs** — `AdaptiveQuality` congestion controller (PDF Task 7.3,
BORU-SS-20) with 4 quality levels (bitrate → fps → resolution,
`adaptation.rs:280-303`), hysteresis (3 congested ticks to step down, 8 stable
ticks to step up — recovery is ~3x more conservative than reduction), and
signals for send-queue depth (`send_queue_depth`, `bytes_in_flight`), measured
throughput (`measured_throughput_bps`), RTT (`rtt_us`, 0 = not available),
encode time (`encode_time_avg_us` vs frame period) and dropped frames
(delta-based `dropped_frames` / `late_drops` so an old cumulative value never
sticks forever). **Wired into `host.rs` streaming** (BORU-SS-20): a
`ScreenShareStats` collector observes capture/encode/send/pacing/media drops,
the host snapshots every 25 frames, feeds `AdaptiveQuality::update`, and
`apply_quality_config` applies the decision (resolution/fps → reconfigure with
generation bump; bitrate-only → `reconfigure_bitrate`). Viewer manual request:
`ScreenShareMessage::QualityUpdate` → `AdaptiveQuality::apply_viewer_request`
clamps the config to the requested ceiling (bitrate/fps/scale); recovery never
exceeds it; `clear_viewer_request` restores full adaptive behaviour. Also
`PacingController`/`PacingCounters` (BORU-SS-19 / PDF Task 7.2): bounded
latest-frame queue between capture and encode that drops obsolete frames over
building latency, caps queue length at `max_queue_depth`, records drop counters
(`dropped_queue_full`, `dropped_obsolete`), wired into `host.rs` streaming with
skipped-tick accounting; counters exposed for BORU-SS-28 metrics.

**stats.rs** — `ScreenShareStatsSnapshot` (`stats.rs:10-25`) and
`ScreenShareStats` (`stats.rs:28-103`): monotonic counters for
capture/encode/decode/render/late-drop/bytes-in-flight, snapshot derives fps
and bitrate (`stats.rs:81-102`). Consumed by `ViewerPipeline`
(`viewer.rs:16,42,71`) and exposed via `ViewerPipeline::stats()`
(`viewer.rs:187`), but the GUI (`src/bin/boru/app.rs`) never calls it —
no developer metrics overlay exists yet (PDF Phase 12 gap).

**platform/mod.rs** — per-OS module dispatch (`platform/mod.rs:7-19`),
`ActiveCapture` enum per OS (`platform/mod.rs:22-80`), `create_capture_source`
factory (`platform/mod.rs:83-95`), `capture_dimensions`
(`platform/mod.rs:98-100`), `CAPTURE_FPS = 15` (`platform/mod.rs:103`).

**platform/linux.rs** — the largest module. Two layers:
1. `PortalCapture` — portal state machine + bounded frame queue
   (`linux.rs:46-143`), kept for API compatibility/tests.
2. `LinuxPortalCapture` — the real backend: xdg-desktop-portal ScreenCast via
   zbus (`linux.rs:456-849`: CreateSession → SelectSources → Start with
   async Request/Response handling, `extract_stream_node_id`,
   `query_portal_version`, `detect_portal_backend`) + a **dlopen-based
   PipeWire client** (`linux.rs:859-1211`: `Pw` ABI table, raw struct
   mirrors), feeding CPU frames through a background
   `boru-pipewire-capture` thread. The SPA format negotiation and buffer
   normalization moved to the pure `linux_pw` module (BORU-SS-14, §2.4):
   `stream_param_changed` handles renegotiation (generation counter +
   `FormatChanged`), `stream_process` copies buffers row-by-row honouring
   the chunk stride and expands 24-bit RGB/BGR.
3. `PortalSessionMachine` (`linux.rs:145-357`) — pure D-Bus lifecycle state
   machine (Idle/Creating/Selecting/Starting/Streaming/Closing/Closed/Failed)
   with the D-Bus layer abstracted, plus desktop-environment detection
   (`XDG_CURRENT_DESKTOP` / `XDG_SESSION_TYPE`: GNOME, KDE Plasma 6,
   wlroots). `LinuxPortalCapture` keeps the live zbus connection + session
   object path for the whole capture lifetime and tears down cleanly:
   `close()` stops the PipeWire thread (bounded join via `PipeWireHandle`)
   and calls `org.freedesktop.portal.Session.Close`; `Drop` does the same
   best-effort. See `docs/screenshare-wayland-portal-verification.md`.
4. `X11Capture` — direct X11 GetImage fallback via x11rb
   (`linux.rs:1625-2110`, `convert_zpixmap_rgba`). Full
   `DesktopCaptureBackend` with RandR monitor enumeration and
   selected-geometry capture (BORU-SS-16, §2.6).

`ActiveCapture::{Portal,X11,TestPattern}` + `create_capture_source` selection
order is display-server aware (BORU-SS-16): Wayland/XWayland prefer the portal
first (X11 fallback after); native X11 prefers the direct backend first
(portal after); test-pattern is the last resort. 33 unit tests in
`platform/linux.rs` incl. 11 `PortalSessionMachine` lifecycle/teardown/
DE-classification tests, SPA pod round-trip, 10 BORU-SS-16 display-server /
geometry / monitor-source tests, and ZPixmap byte-order conversions.

**platform/windows.rs** — real WinRT `Windows.Graphics.Capture` backend
(BORU-SS-11 / PDF Task 4.1): `GraphicsCapture` implements the
`DesktopCaptureBackend` trait — `list_sources` enumerates monitors via
`EnumDisplayMonitors`/`GetMonitorInfoW` (`windows.rs:184-196`), `start`
builds D3D11 device + frame pool + capture session for the selected monitor
(`windows.rs:198-316`), `next_frame` pulls GPU surfaces and stages them to
CPU with a reused staging texture (`windows.rs:318-444`), and `stop` tears
down the session idempotently. Source resize is handled by recreating the
frame pool (`pool.Recreate` + fresh capture session, `windows.rs:360-390`);
monitor unplug / lock screen / permission failures are classified as typed
`CaptureFailureKind` errors (never panics). The pure lifecycle state machine,
HRESULT classification, and monitor source-id derivation live in
`platform/windows_common.rs`, which is compiled and unit-tested on every
target (10 tests on Linux). Hardware behaviour (resize/unplug/lock/consent)
is only verifiable on real Windows; the module cross-compiles for
`x86_64-pc-windows-gnu` (verified via `rb check --target
x86_64-pc-windows-gnu --no-default-features --features screen-sharing`; the
msvc target additionally needs a Windows MSVC toolchain for the C dependency
build scripts and was not checkable on debsrv).

### 2.2 Cursor strategy + coordinate model (BORU-SS-12 / PDF Task 4.2)

**Cursor strategy decision: composite into captured frames on the host.**

`Windows.Graphics.Capture` deliberately does not include the pointer in
`Direct3D11CaptureFrame` surfaces. The Windows backend therefore queries the
cursor with GDI (`GetCursorInfo` + `GetIconInfo` + `DrawIconEx`), rasterizes
it into a small BGRA sprite, and alpha-blends it into the staged frame at the
source-relative position before the encoder sees it
(`composite_system_cursor` in `windows.rs`). This was chosen over a separate
cursor stream because:

- The existing pipeline (capture → BGRA8 CPU frame → OpenH264 → protocol →
  viewer) renders frames as-is, so a composited cursor reaches every viewer
  with **zero protocol or viewer changes**.
- A separate representation (cursor shape + position messages, viewer-side
  compositing) is listed in the reference PDF Phase 14 as a *future*
  "cursor-shape optimization"; it would require new protocol messages, viewer
  rendering, and cursor lifetime management — out of proportion for the
  baseline.

The pure mapping and blending live in `src/screen_share/coords.rs`
(platform-independent, Linux-tested): `MonitorGeometry` carries a monitor's
virtual-desktop origin (physical px, may be negative for monitors left of /
above the primary), and `desktop_to_source` / `desktop_to_normalized` /
`source_to_desktop` / `normalized_to_desktop` normalize coordinates against
the **shared source** rather than the global desktop. `logical_to_physical`
and `geometry_from_logical` cover mixed-DPI and scaling-percentage layouts
(100%–200%). `CursorSprite` + `composite_cursor` perform hotspot-aware,
clipped alpha blending. `CaptureSource` now carries an optional `geometry`
field so the host knows where the shared monitor sits in the desktop.

Tests (all Linux-runnable): negative-origin monitors, mixed-DPI layouts,
scaling percentages, round-trips, cursor compositing/clipping/out-of-source
(15 `coords` tests, plus the updated `monitor_source` geometry test).

### 2.3 Portal session lifecycle + teardown (BORU-SS-13 / PDF Task 5.1)

The xdg-desktop-portal ScreenCast flow
(`src/screen_share/platform/linux.rs`) implements CreateSession →
SelectSources → Start → PipeWire node acquisition → clean teardown:

- **Lifecycle machine.** `PortalSessionMachine` is a pure state machine
  (`Idle → Creating → Selecting → Starting → Streaming`, `Closing → Closed`,
  terminal `Failed(SessionFailure)` states) with the D-Bus layer abstracted —
  unit-tested on Linux without a session bus/portal/compositor. It enforces
  call ordering, models every failure path (`NoSessionBus`,
  `CreateSessionFailed`, `SelectSourcesFailed`, `StartFailed`,
  `StartRejected(u32)`, `StartTimeout`, `ResponseStreamClosed`,
  `MissingNodeId`), teardown (`begin_close`/`on_closed`), and
  portal-initiated close (`on_portal_closed`).
- **Connection kept alive.** `LinuxPortalCapture` now stores the live zbus
  connection + session object path; previously the connection was dropped
  right after `Start`, which can make xdg-desktop-portal tear the session
  down server-side.
- **Clean teardown.** `close()` stops the PipeWire capture thread
  (`PipeWireHandle` calls `pw_main_loop_quit` — safe from any thread — and
  waits on a bounded `recv_timeout`), calls
  `org.freedesktop.portal.Session.Close`, and marks the machine `Closed`.
  `Drop` repeats this best-effort (PipeWire stop synchronous; `Session.Close`
  on a helper thread with its own tokio runtime).
- **DE handling.** `XDG_SESSION_TYPE` + `XDG_CURRENT_DESKTOP` are classified
  (GNOME / KDE Plasma 6 / wlroots-style compositors), the ScreenCast
  interface version is queried, and portal backend bus names are listed —
  all for diagnostics and actionable errors. The D-Bus flow itself is the
  same across backends; the picker is never bypassed. Real-session
  verification limits are documented in
  `docs/screenshare-wayland-portal-verification.md`.

**platform/macos.rs** — 1-line placeholder (`macos.rs:1`). No capture backend;
`ActiveCapture` on macOS is test-pattern only (`platform/mod.rs:27-30`).

### 2.4 PipeWire frame ingestion + format negotiation (BORU-SS-14 / PDF Task 5.2)

The portal's PipeWire node is consumed on the CPU-mapped path by the dlopen
client in `platform/linux.rs`, with all negotiation/copy logic factored into
the pure `platform/linux_pw.rs` module (unit-testable headless, and shared
with a future DMA-BUF path):

- **Explicit format negotiation.** The stream advertises, in preference
  order, BGRx, RGBx, BGRA, RGBA, BGR24, RGB24 (`linux_pw::build_format_pod`).
  The portal picks one and re-sends a `SPA_PARAM_Format` pod; the parser
  accepts both the advertisement shape and the real negotiated shape (plain
  Id/Rectangle or Choice-wrapped). Unknown formats (YUV, planar, 10-bit) are
  rejected rather than misinterpreted.
- **Correct SPA constants.** The original stub's SPA type/format constants
  did not match PipeWire's headers (e.g. `SPA_TYPE_Object` was 16, real is
  14; `SPA_VIDEO_FORMAT_BGRx` was 7, real is 8), which would make
  `pw_stream_connect` reject the pod and every negotiated format parse fail.
  BORU-SS-14 replaced them with the values from PipeWire's own MIT headers
  (`spa/utils/type.h`, `spa/param/format.h`, `spa/param/param-types.h`,
  `spa/param/video/raw.h`) and added a guard test pinning each value.
- **CPU-mapped normalization.** `stream_process` copies each buffer
  row-by-row honoring the chunk stride (`spa_chunk.stride`, falling back to
  tight packing), drops row padding, and expands 24-bit BGR/RGB to
  BGRA8/RGBA8 (alpha 255). The output is tightly packed, matching the
  encoder's `pixels.len() == width*height*4` requirement. Buffers that do
  not match the current negotiated geometry (stale buffers during
  renegotiation) are dropped with a debug log instead of mis-shaped.
- **Renegotiation.** The portal re-sends `SPA_PARAM_Format` when the display
  resolution changes; `stream_param_changed` updates the shared
  `NegotiatedFormat`, bumps a generation counter, logs, and emits
  `PortalEvent::FormatChanged`. The host loop reconfigures the encoder from
  the frame geometry (existing BORU-SS-03 logic at `host.rs`), so a
  resolution change re-negotiates PipeWire params → buffers → format without
  restarting the session.
- **Typed runtime errors.** `ScreenShareError` now carries a
  `ScreenShareErrorKind` (`PipeWireMissing`, `PortalMissing`,
  `PipeWireConnect`, `FormatNegotiation`, `Stream`, `Generic`). PipeWire
  library/server failures and the missing-session-bus case produce
  actionable messages (install PipeWire / is the desktop portal running?)
  with a typed kind — no panics.

### 2.5 Wayland cursor modes + RemoteDesktop input (BORU-SS-15 / PDF Task 5.3)

- **Portal cursor modes respected.** The ScreenCast `SelectSources` call now
  requests `cursor_mode` (portal interface v2+). Boru queries
  `AvailableCursorModes` first and only sends a mode the portal advertises —
  requesting an unadvertised mode closes the session. `Embedded` (2) is
  preferred: the compositor bakes the cursor into the PipeWire buffers,
  matching the composite-into-frames strategy from BORU-SS-12; when the
  portal only advertises `Hidden` (1), Boru falls back to it (no cursor in
  the stream). `Metadata` (4) is deliberately not requested (viewer-side
  cursor-sprite handling is Phase 14 future work). `CursorMode` +
  `choose_cursor_mode` + `select_sources_options` are pure and unit-tested;
  `LinuxPortalCapture::cursor_mode()` exposes the negotiated mode.
- **RemoteDesktop portal input fixed to the real spec.** The previous
  `LinuxPortalRemoteInput` fired `Start` and ignored the reply, and every
  `Notify*` call was missing the mandatory `options` vardict (`a{sv}`) — the
  portal would have rejected them. Now `connect()` awaits the async
  `Start` `Response` signal (20 s timeout), checks the response code, and
  parses the `devices` bitmask (1 = pointer, 2 = keyboard) the user actually
  granted; a denied dialog fails closed. `NotifyPointerMotion`,
  `NotifyPointerButton`, `NotifyKeyboardKeysym` pass the empty options dict
  and correct types (`i32` keycode, `0/1` `u32` state). `apply` gates each
  event on the granted device bits via the pure `device_mask_grants`
  helper; `parse_devices_mask` extracts the bitmask from the Start response.
- **Lazy input backend (explicit consent only).** `host.rs` creates the
  remote-input backend on the first explicit `GrantControl` command, not at
  streaming start. View-only shares never open a RemoteDesktop portal
  session or pop the portal dialog, and keep working when remote-input
  permission is denied (backend fails closed; input is dropped). The backend
  is shut down on `RevokeControl`, session end, and reconnect failure.
- **Explicit UI choice preserved.** The existing host-side consent flow
  (viewer `RequestControl` → host UI grant buttons → `GrantControl`) remains
  the only path to control; session tests assert Accept never grants control
  and a `ControlRequest` never changes permissions by itself.

### 2.6 X11 fallback backend (BORU-SS-16 / PDF Task 6.1)

The direct X11 backend (`src/screen_share/platform/linux.rs`) is now a full
`DesktopCaptureBackend` behind the same trait as Windows/Wayland:

- **Monitor enumeration via RandR.** `X11Capture::list_monitors()` tries the
  modern RandR 1.5 `GetMonitors` path first (physical monitor names + primary
  flag), falls back to a CRTC walk (`GetScreenResourcesCurrent` +
  `GetCrtcInfo` + `GetOutputInfo` names) for older servers, and finally to a
  single root-window "Screen" source so capture still works without RandR.
  `list_sources()` advertises each monitor as a `CaptureSource` with its
  root-window `MonitorGeometry` (negative origins supported), mirroring the
  Windows `monitor_source` shape; ids are the same FNV-1a name hash
  (`windows_common::monitor_source_id`).
- **Selected-geometry capture.** `DesktopCaptureBackend::start(source, config)`
  validates the source against the live monitor list and stores the selected
  rectangle; `next_frame()` runs `GetImage` on exactly that rectangle
  (`clip_to_root` clamps to the root bounds for monitors that moved past the
  edge). The legacy whole-root `ScreenCapture` impl is preserved for the
  `ActiveCapture::X11` fallback path, so existing host behaviour is
  unchanged.
- **Display-server detection.** `DisplayServer` (Wayland / XWayland / X11 /
  Unknown) is classified from `WAYLAND_DISPLAY`, `XDG_SESSION_TYPE` and
  `DISPLAY` (`classify_display_server` pure + `detect_display_server`
  env-reader). `create_capture_source` uses `DisplayServer::prefers_portal()`:
  under Wayland/XWayland the portal is tried first (a direct X11 capture
  would only see XWayland windows); under native X11 the direct backend is
  tried first (no portal daemon needed).
- **Correctness-first.** No XShm / damage tracking yet — documented follow-up
  (PDF Task 6.1: "optimize with SHM/damage tracking later if necessary").
  Per-frame `GetImage` + `convert_zpixmap_rgba` (LSBFirst/MSBFirst channel
  masks) is the baseline.
- **Tests.** Pure Linux-runnable tests cover display-server classification +
  portal preference, `clip_to_root` clamping (inside / partial overflow /
  fully outside / negative origin), monitor→source geometry mapping, and
  stable ids. Live tests (`x11_live_*`) are `#[ignore]`d and documented: they
  need a real X server (`$DISPLAY`, e.g. a desktop session, Xvfb, or
  Xwayland) and verify enumeration + real GetImage capture through the
  `DesktopCaptureBackend` lifecycle. Run them explicitly with
  `cargo test --features screen-sharing -- --ignored x11_live_`.

### 2.7 X11 remote input (BORU-SS-17 / PDF Task 6.2)

The X11 input backend (`X11RemoteInput` in `src/screen_share/remote_input.rs`)
injects via the XTEST extension when running under a native X11 session, so
remote control works without an xdg-desktop-portal daemon:

- **Consent-gated injection.** The backend is created only on the first
  explicit `GrantControl` (same lazy path as the portal backend), stores the
  granted device mask (pointer/keyboard bits), and `apply` re-checks the mask
  for every event. The streaming loop also runs `authorize_nonce` before
  forwarding, so injection is doubly gated on `permissions.rs` state.
- **Pure event translation.** `x11_pointer_actions` / `x11_key_action` /
  `build_keysym_to_keycode` translate normalized events into XTest actions
  (absolute motion with monitor origin, button press/release, wheel
  press+release pair, keysym → keycode via the server `GetKeyboardMapping`)
  and are unit-tested without an X server. Live XTest delivery needs a real
  X session (documented in `docs/screenshare-x11-input.md`).
- **Backend selection.** `create_platform_backend` is display-server aware:
  XTest is tried first under native X11, the RemoteDesktop portal first under
  Wayland/XWayland (XTest under XWayland only reaches XWayland windows), with
  portal ⇄ XTest fallback then `UnavailableInputBackend`.
- **Limitations documented.** Compositor/XWayland caveats (XWayland window
  coverage, synthetic-input policy, layout dependence, no secure-desktop
  injection) are in `docs/screenshare-x11-input.md`.

### 2.8 OpenH264 baseline encode + configurable quality (BORU-SS-18 / PDF Task 7.1)

The OpenH264 encoder is now configured through the capture/stream config with
a Boru-owned quality knob:

- **Quality profile.** `QualityProfile` (`codec.rs`): `Balanced` (default) /
  `LowLatency` / `HighQuality`, mapped onto the *documented* OpenH264
  `EncoderConfig` settings — usage type `SCREEN_CONTENT_REAL_TIME` (OpenH264's
  screen-sharing mode, not the camera mode used previously), complexity
  (Low/Medium/High) and QP range (45/41/36 max respectively). Wire value is a
  compact `u8` (`as_u8`/`from_u8`) carried on the versioned `StreamConfig`
  protocol message (validated; unknown values rejected).
- **Target profiles.** `CodecConfig::profile_720p30()` (1280x720 @ 30,
  2.5 Mbps) and `profile_1080p30()` (1920x1080 @ 30, 4 Mbps) plus the
  `TARGET_*` constants — the PDF Task 7.1 reference targets.
- **Config plumbing.** `CaptureConfig` now carries `target_bitrate_bps`,
  `keyframe_interval` and `quality_profile` alongside the existing capture
  fields; the host builds the encoder config via
  `CodecConfig::from_capture_config(capture, width, height)` so bitrate, fps,
  keyframe interval and quality profile all flow from one config object.
- **Fast RGB→YUV path.** `encode()` now uses
  `YUVBuffer::from_rgb8_source` (integer `write_yuv_scalar`) instead of
  `from_rgb_source` (f32 per-pixel `write_yuv_by_pixel`), a ~2x encode
  speedup at HD resolutions.
- **Benchmark.** `src/screen_share/encode_bench.rs` measures encode fps and
  single-core CPU% at 720p30/1080p30 (all profiles). `#[ignore]`d (perf
  sensitive); run in release mode. Results in
  `docs/screenshare-encode-benchmark.md`. 12 codec unit tests incl. profile
  round-trip, target-profile values, every-profile encode/decode, and
  CaptureConfig→CodecConfig application.

### 2.9 Adaptive quality (BORU-SS-20 / PDF Task 7.3)

The adaptive controller from BORU-SS-03 is now **wired into the host
streaming loop** and extended to the full PDF Task 7.3 signal set:

- **Signals tracked** (`ScreenShareStatsSnapshot` gains fields): send-queue
  depth in frames (`send_queue_depth`) and bytes (`bytes_in_flight`),
  interval measured throughput (`measured_throughput_bps`), RTT when
  available (`rtt_us`, 0 = unknown — read from the selected QUIC path),
  interval average encode time (`encode_time_avg_us`), and a monotonic total
  drop counter (`dropped_frames` = capture + pacing + media + late drops).
  The host's `ScreenShareStats` collector observes every capture/encode/send
  and pacing/media drops; `snapshot()` derives interval rates from deltas so
  the controller sees current pipeline health, not lifetime averages.
- **Hysteresis.** 3 consecutive congested ticks step quality down one level
  (bitrate 65% → bitrate 45% + fps/2 → + resolution/2); 8 clean ticks step up
  one level — recovery is ~3x more conservative than reduction. Drop signals
  are delta-based, so a burst is visible but an old counter never sticks.
- **Viewer manual request.** `ScreenShareMessage::QualityUpdate` →
  `AdaptiveQuality::apply_viewer_request` clamps the config to the requested
  ceiling (bitrate/fps/scale factor); recovery never exceeds it. The viewer
  UI has "Lower Quality" (1 Mbps / 10 fps / 60%) and "Full Quality"
  (at-or-above-base) buttons in the screen-share panel.
- **Application.** `apply_quality_config` in host.rs picks the cheapest
  path: resolution/fps changes rebuild the encoder (generation bump → decoder
  re-initialises), pure bitrate changes use `reconfigure_bitrate` (no bump,
  forced keyframe). Capture geometry changes (portal renegotiation) update the
  controller base via `set_capture_geometry` instead of fighting the adaptive
  resolution. Adaptation runs every 25 encoded frames.
- **Tests.** 7 new/updated control-loop tests: sustained pressure steps
  bitrate→fps→resolution, recovery is gradual/hysteretic, queue-depth
  pressure, RTT pressure, encode-time pressure, throughput saturation only
  with queue growth, dropped-burst pressure that does not stick, manual viewer
  request honored as a ceiling, and congestion still reducing below a viewer
  ceiling.

## 3. Dependency usage map (within the screen-share subsystem)

| Dependency | Cargo.toml | Where used (file:line) | Purpose |
|---|---|---|---|
| `openh264` 0.9.7 | `Cargo.toml:102` | `codec.rs:110-199` (`OpenH264Encoder`, `OpenH264Decoder`) | H.264 encode/decode |
| `zbus` 5 (tokio) | `Cargo.toml:134` | `platform/linux.rs:193-270,931-945` (ScreenCast); `remote_input.rs:122-174` (RemoteDesktop) | xdg-desktop-portal D-Bus client |
| `libloading` 0.8 | `Cargo.toml:138` | `platform/linux.rs:481-513,531-532` (`Pw::load`, `Library::new(PW_LIB)`) | dlopen `libpipewire-0.3.so.0` (no PipeWire dev headers needed) |
| `windows-sys` 0.59 | `Cargo.toml:140` | `remote_input.rs:220-228` (`SendInput`, `GetSystemMetrics`) | Windows user-session input injection |
| `windows` 0.58 | `Cargo.toml:143-156` | `platform/windows.rs` (WinRT Graphics Capture), `platform/windows_common.rs` (classifier, no WinRT imports) | WinRT Graphics Capture |
| `x11rb` 0.13 | `Cargo.toml:160` | `platform/linux.rs:31-32,956-1055` (GetImage capture); `remote_input.rs:414-536` (XTest fake input) | Direct X11 GetImage capture fallback + XTest remote input |
| `iroh` 1 (patched) | `Cargo.toml:111,457-463` | `protocol.rs:180-280` (ProtocolHandler/router), `transport.rs:118-163` (Connection/SendStream), `host.rs:24-25,98-155` (Endpoint::connect), `session.rs` + `permissions.rs` + `remote_input.rs` (PublicKey identity) | ALPN registration, QUIC dial/accept, control+media streams, peer identity |

Notes:
- **Shared with video-calls:** the only *dependency* shared with the
  `src/call/video/*` machinery is `openh264`. Screen-share has its own
  encoder/decoder wrapper types in `codec.rs`; `src/call/video/codec.rs` has
  separate `OpenH264Encoder`/`OpenH264Decoder` for camera calls. There is **no
  code sharing** between `screen_share/` and `call/video/` (verified: no
  `call::video` import anywhere under `src/screen_share/`). Both also use
  `postcard`/`serde`/`tokio`, which are already required by the `net` feature.
- **Not used by screen-share:** `nokhwa` (camera capture, video-calls only),
  `cpal`/`opus` (voice-calls only), `iced_video_player` (file video playback),
  `netstat2` (share-local-service dialog, unrelated).

## 4. UI entry points (`src/bin/boru`)

### Starting a share
- **Toolbar button** in the direct-conversation header:
  `app/chat.rs:1420-1450` — a "Share screen" tool button (`Icon::Monitor`)
  sends `AppMessage::StartScreenShare(key)` (`app/chat.rs:1435`). It is shown
  only for non-group, non-blocked conversations, when
  `screen_share_host_state == Idle`, and when the peer advertises the
  `screen-share` capability (`feature_offered(SCREEN_SHARE)`,
  `app/chat.rs:1421-1430`). If the peer lacks the capability, the button is
  disabled with an explanatory tooltip (`app/chat.rs:1438-1448`).
- **Handler:** `app.rs:14376` → `start_screen_share(peer)`
  (`app.rs:20780-20858`): capability-gate check (`app.rs:20788-20807`), sets
  `ScreenShareHostState::Inviting`, spawns `run_host_session` on a dedicated
  thread with its own current-thread tokio runtime (`app.rs:20839-20856`,
  with an explanatory comment about the QUIC-driver starvation bug this
  avoids), passing a `stop` AtomicBool and a `HostCommand` channel.

### Stopping / lifecycle
- **Stop Sharing** button in the host panel (`app/chat.rs:413`) →
  `AppMessage::StopScreenShare` (`app.rs:14378-14405`): sets both stop flags,
  sends `EndSession` on the viewer connection, resets all host/viewer state.
- **Accept/Decline** invitation buttons (`app/chat.rs:368-369`) →
  `AcceptScreenShare` (`app.rs:14407` → `accept_screen_share`
  `app.rs:20963-21028`: sends Accept, spawns the decode worker) /
  `DeclineScreenShare` (`app.rs:14409-14431`: sends Reject).
- **Session events** flow through `apply_screen_share_event`
  (`app.rs:21032-21094`): invitation → `screen_share_invite`, Accepted →
  `Streaming`, Rejected/Ended → reset host state, ControlRequest →
  consent prompt, ControlChanged → indicator.

### Viewing a share
- **Viewer panel** `view_screen_share_panel` (`app/chat.rs:354-507`),
  rendered inside the chat column (`app/chat.rs:99`). States:
  - Invitation prompt (`app/chat.rs:364-371`),
  - Host state (waiting/streaming + control consent + revoke)
    (`app/chat.rs:372-414`),
  - Viewer: a **dedicated scalable surface** (PDF Task 8.2) built by
    `view_screen_share_surface` (`app/screen_share_surface.rs:195-292`)
    instead of a raw fixed-box `Image`. The surface preserves the source
    aspect ratio and supports **Fit** (scale to window), **100%** (actual
    pixels), **zoom in/out** (+/− buttons and mouse-wheel, anchored at the
    cursor), **pan** (drag), and **fullscreen** (whole-window overlay
    `view_screen_share_fullscreen`, `app/chat.rs:529-597`, Esc or
    Inline exits). Geometry is pure and unit-tested:
    `SurfaceGeometry` (`app/screen_share_surface.rs:52-188`) computes
    fit scale, visible crop region, display rect, and viewport→source /
    viewport→normalized mapping under any pan/zoom.
  - A compact control row (`view_screen_share_view_controls`,
    `app/screen_share_surface.rs:299-350`) holds Fit / 100% / − / + /
    Reset / Fullscreen, then the existing **Lower Quality**, **Full
    Quality**, **Request Control** (or control-granted label), and
    **Stop Viewing** buttons.
  - Remote control input maps through the surface geometry
    (`app/screen_share_surface.rs:238-261`): a viewport point becomes a
    normalized source point, so `ScreenSharePointerMove`/`Button` events
    stay correct under pan/zoom instead of assuming the old fixed 640x360
    box.
- **Decode worker:** `decode_worker` (`app.rs:20498-20552`) drains inbound
  media for the session, feeds `ViewerPipeline<OpenH264Decoder>`, publishes
  newest frames to a watch channel (`app.rs:21103-21108`), and emits
  `ScreenShareMessage::KeyframeRequest` on the control channel when the
  pipeline reports missing/corrupt frames (BORU-SS-21).
- **Subscriptions:** `screen_share_events_subscription`
  (`app.rs:20530-20551`), `screen_share_frame_subscription`
  (`app.rs:20564-20587`), `screen_share_keyboard_subscription`
  (`app.rs:20278-20289`, forwards key presses when control is active),
  registered in `IcedChat::subscription` (`app.rs:21237-21242`).
- **Remote control consent (host side):** `ControlRequest` event →
  `screen_share_control_request` → grant buttons in the host panel
  (`app/chat.rs:380-401`) → `ScreenShareGrantControl`
  (`app.rs:14464+`).

### Wiring in main.rs
- Protocol created at `main.rs:877-882`
  (`ScreenShareProtocol::with_channels(events_tx, media_tx)`), registered on
  the router at `main.rs:1251` (`router.accept(SCREEN_SHARE_ALPN, ...)`),
  and handed to `IcedChat` at `main.rs:1955-1963`; subscriptions wired at
  `main.rs:2078-2080`.

### Capability gate (peer-version negotiation)
- `src/control_plane/extensions.rs:291-295` — `ScreenShareCapability`
  (extension 5, protocol versions only).
- `src/control_plane/capabilities.rs:75,95` — `SCREEN_SHARE_V1` /
  `SCREEN_SHARE` capability identifiers.
- The GUI gates the Share button and start path on the peer advertising a
  compatible screen-share version (`app.rs:20788-20807`, `app/chat.rs:1421`).

## 5. Test coverage summary

- **Unit tests:** 199 `#[test]` pass in `src/screen_share/` with
  `--features screen-sharing` (includes codec 3, protocol 4, transport 3,
  session 7 [incl. 2 BORU-SS-15 explicit-grant tests], viewer 9 [incl.
  BORU-SS-21 missing-gap, no-picture, one-shot, and session-isolation
  tests], permissions 2, remote_input 6 [incl. 2 BORU-SS-15 portal-gate
  tests], adaptation 2, capture 3, stats 1, mod 3 (incl. the error-kind
  mapping test added by BORU-SS-14), coords 15, platform/linux 33 [incl. 3
  BORU-SS-15 cursor-mode tests, 11 portal-lifecycle / DE-detection
  tests from BORU-SS-13, and 10 BORU-SS-16 display-server / geometry /
  monitor-source tests], platform/linux_pw 13, platform/windows_common
  10, plus the channels/reconnect/session tests added by later BORU-SS
  tasks) — see per-file table above. Plus 10 surface-geometry tests
  (`screen_share_surface.rs`, PDF Task 8.2). Two live-X11 tests
  (`x11_live_*`) are `#[ignore]`d because they need a real X server.
- **End-to-end protocol test:** `protocol.rs:322-412`
  (`end_to_end_invite_accept_media_decode`) — two real iroh endpoints, Hello →
  Invitation → Accept → media → decode through `ViewerPipeline`.
- **GUI test:** `app.rs:25641-25658` (`screen_share_blocked_when_peer_lacks_
  capability`) — gated on `screen-sharing`, asserts the capability gate blocks
  StartScreenShare for an unsupported peer.
- **Integration:** `tests/test_extensions_metadata.rs:58,180` — screen-share
  capability in control-plane extensions metadata (not media).
- No dedicated integration test runs two real instances sharing a screen in
  `tests/` (the e2e is in the unit test above).

## 6. Status summary

| Area | Status |
|---|---|
| Capture abstraction + synthetic source | Implemented |
| Real Linux capture (portal + PipeWire + X11 fallback) | Implemented |
| Real Windows capture (WinRT Graphics Capture) | Implemented |
| macOS capture | **Stub** (placeholder only) |
| H.264 encode/decode (OpenH264) | Implemented |
| QUIC transport (control + media) | Implemented |
| Session negotiation state machine | Implemented |
| Remote input (Linux RemoteDesktop portal / Windows SendInput) | Implemented |
| Permissions / consent / rate limiting | Implemented |
| Portal cursor modes (ScreenCast `cursor_mode`, BORU-SS-15) | Implemented |
| Remote-control consent gating (view-only default, explicit grant, lazy backend, BORU-SS-15) | Implemented |
| Viewer decode pipeline | Implemented |
| Scalable viewer surface: fit/100%/zoom/pan/fullscreen, aspect preserved, geometry unit-tested (PDF Task 8.2) | Implemented |
| Frame pacing: latest-frame queue, capped lengths, drop counters (PDF Task 7.2) | Implemented |
| Adaptive quality controller (queue depth, throughput, RTT, encode time, drops; hysteresis; viewer QualityUpdate ceiling) (PDF Task 7.3) | Implemented + wired into host.rs |
| Developer metrics/overlay | Counters implemented, **not surfaced** in UI |
| UI: start/stop/view/accept/decline/control | Implemented |
| Capability-gated peer negotiation | Implemented |

## 7. Follow-up notes (for later BORU-SS tasks — NOT fixed here)

1. **Metrics not surfaced.** `ViewerPipeline::stats()` exists but the GUI
   never reads it; PDF Phase 12 (developer overlay, structured logs) needs a
   consumer.
2. **macOS backend missing** (PDF Phase 4/5 scope is Windows + Wayland first,
   so this is a known gap, not a regression).
3. **Windows backend not CI-tested** in this snapshot — compiled only on
   Windows targets (release.yaml matrix); no Windows runner evidence in repo.
4. **No monitor/source selection UI.** Capture is the primary monitor
   (Windows `MONITOR_DEFAULTTOPRIMARY`, `windows.rs:120-124`) or the portal
   default (Linux); PDF Phase 10 (multi-monitor, source switching) is open.
5. **Quality presets are fixed absolute ceilings.** The viewer "Lower
   Quality" button sends a 1 Mbps / 10 fps / 60% scale `QualityUpdate`;
   "Full Quality" sends an at-or-above-base request (100 Mbps / 240 fps /
   100%). The host clamps to its own base either way, so the request is a
   ceiling, not a precise preset (PDF Phase 7.3).
6. **VNC prototype is a separate feature** (`experimental-vnc`) and must not
   be conflated with the native subsystem; the PDF forbids tunnelling an
   external remote-desktop product into the native path.

## 8. Verification (this inventory made no code changes)

- Working tree clean at snapshot commit `8fb88527`; only this document added.
- `cargo check --features screen-sharing` (and `--all-features`) is expected
  to pass unchanged — the document records state, no behaviour changed.
