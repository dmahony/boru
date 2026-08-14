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
- The GUI wiring in `examples/iced_chat/` is gated per-site with
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
    `examples/iced_chat/app/discover.rs:1468-1469`.

## 2. Module map (`src/screen_share/`)

16 files, 5,962 lines (excluding tests). `wc -l` and the definitive file list
at snapshot commit:

| Module | Lines | Role | Status |
|---|---|---|---|
| `mod.rs` | 142 | Subsystem boundary, re-exports, `ScreenShareError`, boundary unit tests | Implemented |
| `capture.rs` | 232 | `PixelFormat`, `CapturedFrame`, `FrameSink`, `ScreenCapture` trait, `TestPatternCapture` | Implemented |
| `codec.rs` | 257 | `CodecConfig`, `EncodedFrame`, `VideoEncoder`/`VideoDecoder` traits, `OpenH264Encoder`/`Decoder` | Implemented |
| `protocol.rs` | 413 | ALPN, `ControlMessage`, `Hello`, `Permission`, `ProtocolError`, `ScreenShareProtocol` (iroh handler) | Implemented |
| `transport.rs` | 190 | `MediaHeader`, `encode_media`/`decode_media`, `LatestFrameQueue`, `QuicScreenTransport`, `read_unit` | Implemented |
| `session.rs` | 288 | `ScreenShareSessionId`, `SessionState`, `SessionEvent`, `SessionManager` | Implemented |
| `host.rs` | 348 | `run_host_session` (dial → Hello → negotiate → capture/encode/send), `HostCommand` | Implemented |
| `viewer.rs` | 241 | `ViewerPipeline` (bounded receiver decode pipeline), `DecodedFrame` | Implemented |
| `permissions.rs` | 117 | `Capability`, `ControlToken`, `RequestRateLimiter`, `SessionPermissions` | Implemented |
| `remote_input.rs` | 319 | `InputEvent`, `RemoteInput` trait, Linux portal / Windows SendInput backends | Implemented |
| `adaptation.rs` | 89 | `AdaptiveQuality`, `QualityDecision` | **Implemented but UNUSED** (no production caller) |
| `stats.rs` | 121 | `ScreenShareStats`, `ScreenShareStatsSnapshot` | Implemented (internal to viewer; not surfaced to UI) |
| `platform/mod.rs` | 103 | Per-OS dispatch, `ActiveCapture`, `create_capture_source` | Implemented |
| `platform/linux.rs` | 1298 | Portal/PipeWire capture + X11 fallback + dlopen PipeWire client | Implemented |
| `platform/windows.rs` | 293 | WinRT Graphics Capture backend | Implemented |
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
(`viewer.rs:212-240`).

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

**adaptation.rs** — `AdaptiveQuality` congestion controller with 4 quality
levels (bitrate → fps → resolution, `adaptation.rs:24-60`) and hysteresis
tests. **No production caller**: `grep` for `AdaptiveQuality`/`QualityDecision`
outside `adaptation.rs`/`mod.rs` returns nothing in `src/`, `examples/`, or
`tests/`. Exported at `mod.rs:21` but never instantiated by the host or viewer
loops. This is the PDF Phase 7/adaptive-quality gap — the next chain steps can
wire it into `host.rs`.

**stats.rs** — `ScreenShareStatsSnapshot` (`stats.rs:10-25`) and
`ScreenShareStats` (`stats.rs:28-103`): monotonic counters for
capture/encode/decode/render/late-drop/bytes-in-flight, snapshot derives fps
and bitrate (`stats.rs:81-102`). Consumed by `ViewerPipeline`
(`viewer.rs:16,42,71`) and exposed via `ViewerPipeline::stats()`
(`viewer.rs:187`), but the GUI (`examples/iced_chat/app.rs`) never calls it —
no developer metrics overlay exists yet (PDF Phase 12 gap).

**platform/mod.rs** — per-OS module dispatch (`platform/mod.rs:7-19`),
`ActiveCapture` enum per OS (`platform/mod.rs:22-80`), `create_capture_source`
factory (`platform/mod.rs:83-95`), `capture_dimensions`
(`platform/mod.rs:98-100`), `CAPTURE_FPS = 15` (`platform/mod.rs:103`).

**platform/linux.rs** — the largest module. Two layers:
1. `PortalCapture` — portal state machine + bounded frame queue
   (`linux.rs:46-143`), kept for API compatibility/tests.
2. `LinuxPortalCapture` — the real backend: xdg-desktop-portal ScreenCast via
   zbus (`linux.rs:192-273`: CreateSession → SelectSources → Start with
   async Request/Response handling, `extract_stream_node_id` at
   `linux.rs:931-945`) + a **dlopen-based PipeWire client**
   (`linux.rs:358-764`: `Pw` ABI table at `linux.rs:445-513`, raw struct
   mirrors `linux.rs:363-399`, SPA pod builder/parser `linux.rs:798-924`),
   feeding CPU frames through a background `boru-pipewire-capture` thread
   (`linux.rs:633-638`).
3. `X11Capture` — direct X11 GetImage fallback via x11rb
   (`linux.rs:956-1055`, `convert_zpixmap_rgba` at `linux.rs:1063-1108`).

`ActiveCapture::{Portal,X11,TestPattern}` + `create_capture_source` selection
order (portal → X11 → test-pattern, `linux.rs:1165-1181`). 9 unit tests incl.
SPA pod round-trip and ZPixmap byte-order conversions.

**platform/windows.rs** — real WinRT `Windows.Graphics.Capture` backend:
`GraphicsCapture::try_create` builds D3D11 device + frame pool + session for
the primary monitor (`windows.rs:91-157`), `capture()` pulls GPU surfaces,
stages them to CPU (`windows.rs:213-292`). No Windows CI in this snapshot
(module compiles under `--features screen-sharing` on Windows only; verified
by release.yaml matrix).

**platform/macos.rs** — 1-line placeholder (`macos.rs:1`). No capture backend;
`ActiveCapture` on macOS is test-pattern only (`platform/mod.rs:27-30`).

## 3. Dependency usage map (within the screen-share subsystem)

| Dependency | Cargo.toml | Where used (file:line) | Purpose |
|---|---|---|---|
| `openh264` 0.9.7 | `Cargo.toml:102` | `codec.rs:110-199` (`OpenH264Encoder`, `OpenH264Decoder`) | H.264 encode/decode |
| `zbus` 5 (tokio) | `Cargo.toml:134` | `platform/linux.rs:193-270,931-945` (ScreenCast); `remote_input.rs:122-174` (RemoteDesktop) | xdg-desktop-portal D-Bus client |
| `libloading` 0.8 | `Cargo.toml:138` | `platform/linux.rs:481-513,531-532` (`Pw::load`, `Library::new(PW_LIB)`) | dlopen `libpipewire-0.3.so.0` (no PipeWire dev headers needed) |
| `windows-sys` 0.59 | `Cargo.toml:140` | `remote_input.rs:220-228` (`SendInput`, `GetSystemMetrics`) | Windows user-session input injection |
| `windows` 0.58 | `Cargo.toml:143-156` | `platform/windows.rs:11-27,91-157,213-292` | WinRT Graphics Capture |
| `x11rb` 0.13 | `Cargo.toml:160` | `platform/linux.rs:31-32,956-1055` | Direct X11 GetImage capture fallback |
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

## 4. UI entry points (`examples/iced_chat`)

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
- **Viewer panel** `view_screen_share_panel` (`app/chat.rs:354-491`),
  rendered inside the chat column (`app/chat.rs:99`). States:
  - Invitation prompt (`app/chat.rs:364-371`),
  - Host state (waiting/streaming + control consent + revoke)
    (`app/chat.rs:372-414`),
  - Viewer: decoded frame as an iced `Image` (`app/chat.rs:416-457`) with
    **Fullscreen/Inline** toggle (`app/chat.rs:459-461`), **Request Control**
    button (`app/chat.rs:468-472`), **Stop Viewing** (`app/chat.rs:474`),
    and a mouse-area that emits `ScreenSharePointerMove`/`Button` events when
    control is active (`app/chat.rs:417-447`).
- **Decode worker:** `decode_worker` (`app.rs:20483-20517`) drains inbound
  media for the session, feeds `ViewerPipeline<OpenH264Decoder>`, publishes
  newest frames to a watch channel (`app.rs:21019-21026`).
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

- **Unit tests:** 41 `#[test]` in `src/screen_share/` (codec 3, protocol 4,
  transport 3, session 5, viewer 3, permissions 2, remote_input 4,
  adaptation 2, capture 3, stats 1, mod 2, platform/linux 9) — see per-file
  table above.
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
| Remote input (Linux portal / Windows SendInput) | Implemented |
| Permissions / consent / rate limiting | Implemented |
| Viewer decode pipeline | Implemented |
| Adaptive quality controller | Implemented but **unwired** (no production caller) |
| Developer metrics/overlay | Counters implemented, **not surfaced** in UI |
| UI: start/stop/view/accept/decline/control | Implemented |
| Capability-gated peer negotiation | Implemented |

## 7. Follow-up notes (for later BORU-SS tasks — NOT fixed here)

1. **Adaptive quality is dead code.** `AdaptiveQuality`
   (`src/screen_share/adaptation.rs`) is exported but never called. PDF
   Phase 7/adaptive-quality should wire it into the `host.rs` streaming loop
   (it already consumes `ScreenShareStatsSnapshot`, which the viewer emits).
2. **Metrics not surfaced.** `ViewerPipeline::stats()` exists but the GUI
   never reads it; PDF Phase 12 (developer overlay, structured logs) needs a
   consumer.
3. **macOS backend missing** (PDF Phase 4/5 scope is Windows + Wayland first,
   so this is a known gap, not a regression).
4. **Windows backend not CI-tested** in this snapshot — compiled only on
   Windows targets (release.yaml matrix); no Windows runner evidence in repo.
5. **No monitor/source selection UI.** Capture is the primary monitor
   (Windows `MONITOR_DEFAULTTOPRIMARY`, `windows.rs:120-124`) or the portal
   default (Linux); PDF Phase 10 (multi-monitor, source switching) is open.
6. **No quality presets / manual viewer-side quality request** (PDF Phase
   7.3) — `QualityUpdate` messages are not in the protocol.
7. **VNC prototype is a separate feature** (`experimental-vnc`) and must not
   be conflated with the native subsystem; the PDF forbids tunnelling an
   external remote-desktop product into the native path.

## 8. Verification (this inventory made no code changes)

- Working tree clean at snapshot commit `8fb88527`; only this document added.
- `cargo check --features screen-sharing` (and `--all-features`) is expected
  to pass unchanged — the document records state, no behaviour changed.
