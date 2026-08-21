# Boru Screen-Sharing — RustDesk-Inspired Feature Review (Phase 14)

Status: **feature review + follow-up task plan** (BORU-SS-30 / PDF Phase 14 of
`Boru_RustDesk_Reference_Screen_Sharing_Tasks.pdf`, attached to kanban task
t_2d8629a8).

Companion docs:
- `docs/screenshare-rustdesk-reference-policy.md` — binding licensing/reference
  policy (BORU-SS-01): RustDesk is **reference-only**, never copied.
- `docs/screenshare-behavioral-notes.md` — black-box behavioural study of
  RustDesk from its official documentation (BORU-SS-04).
- `docs/screenshare-current-state.md` — subsystem inventory before the
  baseline work; superseded in places by the per-task handoffs BORU-SS-01..29.
- `docs/screenshare-test-matrix.md` — per-area test results (BORU-SS-27).
- `docs/screenshare-media-path-benchmark.md`, `docs/screenshare-encode-benchmark.md`.

Scope: compare the **current** Boru screen-share baseline (BORU-SS-01..29,
pushed through `6485ca26`) against RustDesk's **documented behaviour** and
decide which Phase-14 capabilities are worth pursuing as independent follow-up
tasks. This is a review + task-creation task: **no follow-up capability is
implemented here.**

## 1. Method and source discipline

- RustDesk is used as a **behavioural reference only** (its official
  documentation and observable client behaviour — see
  `docs/screenshare-behavioral-notes.md` §1). No RustDesk source code was
  read, downloaded, or quoted.
- Every follow-up below is paired with the **public API / specification** Boru
  would implement against (portal/PipeWire/X11/Windows/OpenH264/vendor docs),
  a **licensing note** (all sources permissively usable, preserving Boru's
  MIT/Apache-2.0 flexibility), and a **rough effort**.
- Rule applied from the PDF Agent Rule: *"When RustDesk exposes an edge case
  or useful technique, write a Boru requirement, find the relevant
  platform/API documentation, and implement the behaviour independently."*

## 2. Baseline summary (BORU-SS-01..29)

| Area | Boru baseline (implemented) |
|---|---|
| Capture backends | Wayland via xdg-desktop-portal ScreenCast + dlopen PipeWire client (BORU-SS-13/14); X11 direct GetImage via x11rb (BORU-SS-16); Windows WinRT Graphics Capture (BORU-SS-11); test-pattern fallback. macOS is **Experimental/unsupported** and test-pattern-only; see [`docs/macos-capability-decision.md`](macos-capability-decision.md). |
| Monitor/source selection | Source enumeration + in-session switch (`SourcesEnumerated`, `HostCommand::SwitchSource`, `SourceChanged`) — BORU-SS-26/29 |
| Encoding | OpenH264 H.264, `VideoEncoder` trait, quality profiles, 720p30/1080p30 targets (BORU-SS-18) |
| Frame pacing | Latest-frame drop queue, obsolete-frame drops, capped queues (BORU-SS-19) |
| Adaptive quality | `AdaptiveQuality` wired into host: bitrate→fps→resolution steps, hysteresis, viewer `QualityUpdate` ceiling, RTT/throughput/encode-time/drop signals (BORU-SS-20) |
| Decoder/viewer | `ViewerPipeline` keyframe-recovery + scalable iced surface fit/100%/zoom/pan/fullscreen (BORU-SS-21, BORU-SS-22 8.2) |
| Remote control | View-only default, explicit consent, nonce-gated input, rate limiting, revoke, persistent indicator (BORU-SS-15/17) |
| Clipboard | Text-only clipboard as separate optional capability (BORU-SS-25) |
| Reconnect | Reconnecting state, keyframe-after-reconnect, view-only-after-reconnect (BORU-SS-23/24) |
| Metrics | Structured capture start/stop logs + developer metrics (BORU-SS-28) |
| Transport | Boru/Iroh QUIC, reliable control channel + media streams, `PathKind` (Direct/Relay) already detected (`transport.rs:35,129`) |

## 3. Follow-up capabilities vs current state

### 3.1 Adaptive bitrate and frame-rate tuning

- **RustDesk reference (behavioural):** adaptive bitrate toggle (default on),
  configurable 5–120 fps, quality monitor overlay — see behavioural notes
  PAC-1/PAC-2/Q-1/Q-2.
- **Current Boru state:** **already implemented** — `AdaptiveQuality`
  (BORU-SS-20) lowers bitrate → fps → resolution under sustained congestion,
  recovers conservatively after stability, honours viewer `QualityUpdate`
  ceilings, and consumes queue depth, measured throughput, RTT, encode time and
  dropped frames. `PacingController` caps queue length and drops obsolete
  frames (BORU-SS-19).
- **Public API/spec used:** OpenH264 `SEncParamExt` (bitrate/fps) —
  https://github.com/cisco/openh264; iroh connection stats/RTT —
  https://docs.iroh.computer.
- **Licensing note:** OpenH264 (BSD-2-Clause); iroh (MIT/Apache-2.0). No
  concern.
- **Rough effort:** none for the core — delivered. Residual polish (surfacing
  the current adaptation state in the UI panel, exposing the quality monitor
  overlay from BORU-SS-28 metrics) is folded into the LAN/relay presets
  follow-up (3.9) and the DoD gate.
- **Recommendation:** **do not create a dedicated task** — the capability is
  delivered; remaining UI surfacing is tracked in 3.9.

### 3.2 Dirty-region / damage-aware capture

- **RustDesk reference (behavioural):** RustDesk uses damage/dirty-region
  aware capture to avoid re-encoding unchanged screen areas, dramatically
  reducing CPU and bandwidth on mostly-static screens (reference policy §1
  lists "dirty-region / damage-aware capture" as a performance idea).
- **Current Boru state:** `DirtyRegion`/`FrameRect` types exist
  (`capture.rs:31-54`, `CapturedFrame::with_dirty_region` at `capture.rs:156`)
  but **no backend populates them and the encoder ignores them**. X11 capture
  is a full-frame `GetImage` per frame (documented follow-up in current-state
  §2.6: "No XShm / damage tracking yet"); PipeWire buffers are full frames;
  WinRT Graphics Capture does not expose dirty rects.
- **Public API/spec used:** X11 Damage extension (XDamage) + XShm —
  https://www.x.org/releases/X11R7.7/doc/damageproto/damageproto.txt (x11rb
  already provides `damage` protocol bindings, verified in the vendored
  crate); PipeWire `spa_meta_region` damage metadata —
  https://docs.pipewire.org/; Windows DXGI desktop duplication
  `IDXGIOutputDuplication::GetFrameDirtyRects` —
  https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-getframedirtyrects
  (note: the current WinRT backend cannot produce dirty rects; a DXGI
  duplication path or capture-side diffing would be needed for Windows
  damage-awareness).
- **Licensing note:** X11 damageproto (MIT-style), PipeWire (MIT), DXGI docs
  (Microsoft, no code copied). All permissive. No AGPL/GPL dependency added.
- **Rough effort:** **Medium–High** (backend plumbing on 3 platforms; deciding
  how damage maps to encoding — full-frame encode with skip, or region
  assembly; OpenH264 has no direct ROI support, so the practical first step is
  frame-level skip-on-no-damage plus X11/PipeWire region metadata).
- **Recommendation:** **create follow-up task** (BORU-SS-32).

### 3.3 Cursor-shape optimization

- **RustDesk reference (behavioural):** remote cursor is an optional overlay,
  toggleable independently of video; cursor scales/zooms with the image
  (behavioural notes CUR-1/CUR-2).
- **Current Boru state:** cursor is **composited into captured frames on the
  host** (BORU-SS-12 decision — Windows GDI cursor rasterised and blended into
  the frame; Wayland portal cursor mode `Embedded` preferred). This is correct
  but wasteful: moving the cursor re-encodes the entire frame. The portal's
  `Metadata` cursor mode (shape+position as metadata) is deliberately not
  requested yet (documented Phase 14 future work, current-state §2.5).
- **Public API/spec used:** PipeWire `spa_meta_cursor` metadata —
  https://docs.pipewire.org/; xdg-desktop-portal ScreenCast `cursor-modes`
  (`Metadata` = 4) — https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html;
  Windows `GraphicsCaptureSession.IsCursorCaptureEnabled` +
  `GetCursorShape`-style APIs — https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.iscursorcaptureenabled;
  X11 XFixes cursor notify — https://www.x.org/releases/X11R7.7/doc/fixesproto/fixesproto.txt.
  Plus Boru protocol work (cursor-shape + cursor-position control messages,
  viewer-side compositing, reuse the existing `CursorSprite`/`composite_cursor`
  from `coords.rs`).
- **Licensing note:** all platform APIs permissive; protocol messages are
  Boru-owned. No concern.
- **Rough effort:** **Medium** (protocol extension + viewer rendering; reuse
  `CursorSprite`).
- **Recommendation:** **create follow-up task** (BORU-SS-33).

### 3.4 Hardware encoder support

- **RustDesk reference (behavioural):** offers hardware encoding for smoothness
  with software fallback (behavioural notes PAC-3/Q-3).
- **Current Boru state:** `VideoEncoder` trait is codec-agnostic with
  `configure/encode/force_keyframe/reconfigure_bitrate/shutdown/metadata`
  (PDF Phase 2.2) but only the OpenH264 software implementation exists.
  `CodecKind` has a single `H264` variant (`codec.rs:146`).
- **Public API/spec used:** VA-API — https://01.org/linuxmedia/vaapi (libva,
  permissive MIT-style); Windows Media Foundation H.264/HEVC encoder
  (`IMFTransform`) — https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder;
  NVIDIA Video Codec SDK (NVENC) — https://developer.nvidia.com/video-codec-sdk
  (proprietary SDK; bindings must be permissively licensed and the SDK itself
  redistributed under NVIDIA's EULA — flag for review before adoption);
  macOS VideoToolbox — https://developer.apple.com/documentation/videotoolbox.
  Rust bindings evaluated on merit (permissive only, e.g. `windows` crate
  already in the graph for Media Foundation).
- **Licensing note:** VA-API/Media Foundation/VideoToolbox are platform APIs
  with permissive access. NVENC SDK is proprietary (free to use, redistributable
  under NVIDIA EULA) — acceptable only with a review note; never add a GPL/AGPL
  encoder binding. This is the highest-risk item on licensing review.
- **Rough effort:** **High** (per-platform encoder integration behind the
  existing trait; fallback orchestration; benchmark vs OpenH264).
- **Recommendation:** **create follow-up task** (BORU-SS-34).

### 3.5 AV1 / H.265 capability negotiation

- **RustDesk reference (behavioural):** user-selectable codec (auto / vp8 /
  vp9 / av1 / h264 / h265), hardware codecs gated on machine support
  (behavioural notes Q-3).
- **Current Boru state:** the protocol already negotiates codec **names** —
  `Hello.codecs` and `ScreenShareOffer.codecs` are preference-ordered
  `Vec<String>` (`protocol.rs:60,204`), `ScreenShareAccept.codec` selects one
  (`protocol.rs:226`), `StreamConfig.codec` re-states it (`protocol.rs:277`).
  Only `"h264"` is offered/implemented (`CodecKind::H264`). Adding a codec is a
  matter of a new `CodecKind` variant + encoder/decoder impl + advertising the
  name in the offer.
- **Public API/spec used:** AV1 spec (AOMedia) — https://aomediacodec.github.io/av1-spec/,
  dav1d (BSD-2-Clause) — https://code.videolan.org/videolan/dav1d, rav1e
  (BSD-2-Clause) — https://github.com/xiph/rav1e; H.265/HEVC (ITU-T H.265) —
  https://www.itu.int/rec/T-REC-H.265 and platform hardware encoders (Media
  Foundation / VideoToolbox) — licensing caveat below.
- **Licensing note:** AV1 is **royalty-free** and the permissive Rust
  implementations (dav1d/rav1e) keep Boru MIT/Apache-2.0-compatible — this is
  the safe path. **H.265/HEVC is patent-encumbered** (HEVC Advance / MPEG LA
  pools): Boru must not link a GPL-licensed HEVC encoder (e.g. x265), and a
  software HEVC encoder would need a patent review. Practical scope: AV1
  negotiation + decode/encode where platform support exists; H.265 only via
  hardware encoders on platforms whose vendor already covers licensing, and
  only with an explicit review gate. The existing licence gate (BORU-SS-02)
  already blocks GPL/AGPL deps.
- **Rough effort:** **High** (codec integration + negotiation matrix +
  licensing review for H.265; AV1 alone is Medium-High).
- **Recommendation:** **create follow-up task** (BORU-SS-35), scoped to AV1
  first with H.265 behind a licensing gate.

### 3.6 Window-only sharing

- **RustDesk reference (behavioural):** supports capturing a single application
  window as the source (standard remote-desktop capability).
- **Current Boru state:** `CaptureSourceKind::Window` exists in the enum
  (`capture.rs:176`) but **no backend enumerates windows**: Linux portal uses
  the portal default (monitors only), X11 lists RandR monitors, Windows lists
  `EnumDisplayMonitors`. Window capture is unimplemented end-to-end.
- **Public API/spec used:** xdg-desktop-portal ScreenCast `SelectSources`
  `types` option (monitor/window/virtual) —
  https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html;
  Windows `GraphicsCapturePicker` / `GraphicsCaptureItem.CreateForWindow` —
  https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscaptureitem.createforwindow;
  X11 window-tree traversal via x11rb (already a dependency).
- **Licensing note:** all permissive platform APIs. No concern.
- **Rough effort:** **Medium** (per-platform window enumeration + capture
  source mapping; reuse `CaptureSource`).
- **Recommendation:** **create follow-up task** (BORU-SS-36).

### 3.7 Audio / system-audio sharing

- **RustDesk reference (behavioural):** shares system audio alongside the
  screen (documented client capability).
- **Current Boru state:** **completely absent.** Screen-share transports video
  only; the `video-calls`/`voice-calls` features use `cpal`/`opus` for
  microphone audio but there is no system-audio (loopback) capture path and no
  audio channel in the screen-share protocol.
- **Public API/spec used:** PipeWire audio stream (`pw_stream` with
  `SPA_AUDIO` format) — https://docs.pipewire.org/page_tutorial4.html; Windows
  WASAPI loopback capture (`IAudioClient`/`IAudioCaptureClient`, loopback mode)
  — https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording;
  Opus codec (RFC 6716) — https://www.rfc-editor.org/rfc/rfc6716. New
  screen-share protocol messages (audio channel on a separate media stream) are
  Boru-owned.
- **Licensing note:** PipeWire (MIT), WASAPI (Microsoft API), Opus (BSD-3) —
  all permissive; the `opus` crate is already used by `voice-calls` (BSD-3).
  No concern.
- **Rough effort:** **High** (new capture path per platform, audio encode
  pipeline, protocol extension, viewer playback, sync with video is optional
  for v1).
- **Recommendation:** **create follow-up task** (BORU-SS-37).

### 3.8 Better multi-monitor switching

- **RustDesk reference (behavioural):** monitors shown individually or
  spanning-all in one session; viewer can switch viewed display without ending
  the session (behavioural notes MON-1/MON-2).
- **Current Boru state:** **partial** — BORU-SS-26/29 delivered monitor
  enumeration before share, in-session `SwitchSource`, and `SourceChanged`
  with forced keyframe. Missing: a **per-display vs spanning** source-mode
  (MON-2), **viewer-initiated** source switching (currently sharer-side only),
  and graceful **monitor unplug / dock-undock** handling (a removed monitor
  should fall back or pause, not stall).
- **Public API/spec used:** xdg-desktop-portal ScreenCast `SelectSources` for
  multiple sources — https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html;
  PipeWire format renegotiation already handled (`linux_pw.rs`); Windows
  `IDXGIOutput::GetDesc` / `EnumOutputs` — https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/dxgi-1-2-improvements;
  WinRT `GraphicsCaptureItem.Closed` for monitor unplug —
  https://learn.microsoft.com/en-us/windows/win32/api/graphicscapture/nf-graphicscapture-igraphicscaptureitem-closed.
  Protocol: extend `StreamConfig` with a `source_mode` field (Boru-owned).
- **Licensing note:** all permissive. No concern.
- **Rough effort:** **Medium** (source-mode field + viewer switch + unplug
  handling; spanning capture itself is larger and can be a later slice).
- **Recommendation:** **create follow-up task** (BORU-SS-38).

### 3.9 Quality presets for LAN vs relay connections

- **RustDesk reference (behavioural):** image-quality presets and adaptive
  quality toggles exist; LAN connections behave differently from relayed
  public connections (behavioural notes Q-1/Q-2, NAT loopback note).
- **Current Boru state:** `QualityProfile` (Balanced/LowLatency/HighQuality)
  exists and is negotiated; `AdaptiveQuality` reacts to measured conditions;
  `PathKind::{Direct,Relay}` is already detected from the iroh selected path
  (`transport.rs:35,129`). **What is missing:** choosing an **initial quality
  preset from the path kind** (e.g. higher bitrate/fps for Direct/LAN, more
  conservative ceiling for Relay), and surfacing the active preset/adaptation
  state in the screen-share panel.
- **Public API/spec used:** iroh `Connection::paths()`/`PathList` for
  Direct-vs-Relay — https://docs.iroh.computer; existing `QualityProfile` +
  `AdaptiveQuality` (`codec.rs`, `adaptation.rs`); OpenH264 `SEncParamExt`.
- **Licensing note:** all permissive. No concern.
- **Rough effort:** **Small–Medium** (initial-config selection from path kind +
  UI surfacing of current quality; mostly wiring that already exists).
- **Recommendation:** **create follow-up task** (BORU-SS-39).

## 4. Decisions and created tasks

| # | Capability | Decision | Task |
|---|---|---|---|
| 1 | Adaptive bitrate/frame-rate tuning | Already delivered (BORU-SS-20); no dedicated task — UI surfacing folded into #9 | — |
| 2 | Dirty-region / damage-aware capture | **Pursue** | BORU-SS-32 (t_b00fcc12) |
| 3 | Cursor-shape optimization | **Pursue** | BORU-SS-33 (t_e069c8e4) |
| 4 | Hardware encoder support | **Pursue** (NVENC licensing review flagged) | BORU-SS-34 (t_2d9890b4) |
| 5 | AV1/H.265 capability negotiation | **Pursue** (AV1 first; H.265 behind licensing gate) | BORU-SS-35 (t_60ad92db) |
| 6 | Window-only sharing | **Pursue** | BORU-SS-36 (t_8173baff) |
| 7 | Audio/system-audio sharing | **Pursue** | BORU-SS-37 (t_302ef6bf) |
| 8 | Better multi-monitor switching | **Pursue** (source-mode, viewer switch, unplug) | BORU-SS-38 (t_1f3110b5) |
| 9 | Quality presets for LAN vs relay | **Pursue** | BORU-SS-39 (t_2119b02f) |

Skipped: none of the nine are non-viable; #1 is simply already implemented.
Every pursued capability cites its public API/spec source and licensing note
above, satisfying the PDF Phase 14 requirement that "for every follow-up,
document the public API/specification used to implement it".

Follow-up tasks are created via `kanban_create` (assignee `deepseek-coder`,
`workspace_kind=worktree` at `/home/dan/iroh-gossip-chat`,
`parents=[t_c75c725d]`) so they queue after this review. Each task body
contains: the capability spec, the public API/spec sources, licensing notes,
scope, acceptance criteria, and the standard BORU-SS git discipline.

## 5. Cross-cutting guardrails applied to every follow-up

- RustDesk remains reference-only; no AGPL/GPL code or dependency enters the
  graph (BORU-SS-01 policy + BORU-SS-02 licence gate).
- Keep screen sharing a native Boru subsystem on Boru/Iroh transport.
- Do not alter chat/network/protocol behaviour outside the screen-share
  subsystem; preserve all existing behaviour.
- Prefer small, reviewable commits; compile/test after each area (via `rb` on
  DEBSRV for heavy builds).
- Each follow-up is independently authored against the cited public APIs.

## 6. Verification (this review made no code changes)

- Only `docs/screenshare-feature-review.md` added to the worktree; `git
  status` otherwise clean.
- `rb check --all-targets --features screen-sharing` on DEBSRV is expected to
  pass unchanged (document-only change).
- No RustDesk source text, comments, tests, or constants are reused anywhere;
  every follow-up cites an independent platform/API source.
