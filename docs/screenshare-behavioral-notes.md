# Boru Screen-Sharing — RustDesk Black-Box Behavioural Study

Status: **behavioural requirements baseline** (BORU-SS-04 / PDF Task 1.2 of
`Boru_RustDesk_Reference_Screen_Sharing_Tasks.pdf`, attached to kanban task
t_2d8629a8).

Companion docs:
- `docs/screenshare-rustdesk-reference-policy.md` — binding licensing/reference
  policy (BORU-SS-01).
- `docs/screenshare-current-state.md` — subsystem inventory before any behaviour
  change (BORU-SS-03).
- `docs/experimental-vnc-tunnel.md` — separate VNC prototype, NOT part of the
  native subsystem.

Scope: this document records how RustDesk **behaves** (black box) in the seven
areas the PDF names — monitor selection, resolution changes, cursor behaviour,
frame pacing, reconnects, quality changes, and remote-control consent — and
converts each observation into a Boru requirement that is independently sourced
from platform/API documentation. No RustDesk source code was read, downloaded,
or quoted for this study.

---

## 1. Method and source discipline

- **Observation target.** RustDesk was **not installed** on the observation host
  (checked: no `rustdesk` binary, no snap/flatpak/dpkg package, no process), so
  direct observation was not possible in this run. Observations below therefore
  come from RustDesk's **official documentation** (the settings reference and
  platform guides) — i.e. documented, observable client behaviour — plus the
  reference PDF's own behavioural expectations where stated.
- **Black-box rule.** Every observation is phrased as *what the client does and
  under what conditions* (setting exposed → observable effect). Setting
  identifiers are referenced only as factual names; no implementation text,
  comments, or constants from RustDesk appear anywhere in this document or in
  the requirements below.
- **Independent sourcing.** Every Boru requirement cites the platform/API
  documentation Boru's implementation would be built against (xdg-desktop-portal
  ScreenCast/RemoteDesktop specs, PipeWire docs, Microsoft WinRT/Windows API
  docs, X11 docs, OpenH264 docs, Iroh docs, iced docs). These sources are
  permissively usable and keep Boru's MIT/Apache-2.0 licensing flexibility.

### Sources used (official RustDesk documentation)

| Source | URL | Behavioural content used |
|---|---|---|
| RustDesk docs index | https://rustdesk.com/docs/en/ | Codec/transport feature surface (software VP8/VP9/AV1, hardware H.264/H.265, NaCl E2E, portable/no-admin Windows) |
| Advanced settings reference | https://rustdesk.com/docs/en/self-host/client-configuration/advanced-settings/ | Permission/access-mode model, adaptive bitrate, image quality/fps/codec presets, monitor/cursor/display options, privacy mode, headless handling |
| Client overview | https://rustdesk.com/docs/en/client/ | Settings menu structure (General/Security/Network/Display), ID + one-time password model |
| Linux client guide | https://rustdesk.com/docs/en/client/linux/ | Wayland experimental status, login-screen capture limits, SELinux permission failures |
| Windows portable elevation guide | https://rustdesk.com/docs/en/client/windows/windows-portable-elevation/ | UAC/secure-desktop capture & input limits, elevation consent flow |
| NAT loopback guide | https://rustdesk.com/docs/en/self-host/nat-loopback-issues/ | LAN-vs-public-path connection failure mode |

---

## 2. Observations and requirements by area

Notation: each row is **OB** (black-box observation of RustDesk, with
conditions) → **REQ** (what Boru must do) → **SRC** (independent platform/API
source the Boru implementation would use). Current Boru state is summarised from
`docs/screenshare-current-state.md` (BORU-SS-03) and re-verified against
`src/screen_share/` in this worktree.

### 2.1 Monitor selection

| # | Black-box observation (RustDesk, official docs) | Boru requirement | Independent implementation source | Current Boru state |
|---|---|---|---|---|
| MON-1 | The client can show a **monitors toolbar** during a session so the controlling side switches between the controlled machine's displays without ending the session. | Before capture starts, enumerate capture sources (monitors/desktops) and let the sharer select one; allow the **viewer** to switch the viewed source mid-session via an explicit source-change message, with a forced keyframe after the switch. | xdg-desktop-portal `ScreenCast.SelectSources` (source enumeration + selection) — https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html; Windows: `IDXGIAdapter::EnumOutputs` / `IDXGIOutput` for monitor enumeration — https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/dxgi-1-2-improvements | **Gap.** No source-selection UI; Linux portal uses portal default, Windows captures primary monitor only (`platform/windows.rs`). PDF Phase 10/13. |
| MON-2 | Displays can be shown **as individual windows** (one window per remote display) or used **all together** for the remote session (a single session spanning every display). | Support single-monitor capture first (PDF Phase 4.1); define the protocol so a session can later express "per-display" vs "spanning" modes and re-select sources without a new peer session. | Same portal/WinRT enumeration sources as MON-1; session/negotiation extension must be Boru protocol work (control channel, `config-change` message). | **Gap.** Protocol has no source-mode field (`src/screen_share/protocol.rs` `Hello` carries initial config only). PDF Phase 10. |
| MON-3 | The controlled client can accept an incoming connection even when **no displays exist** (headless Linux), via an explicit opt-in setting that requires a desktop environment/X server; without the opt-in the machine cannot be controlled headlessly. | Handle "no capture source available" as a first-class state: surface a clear error/UI state instead of hanging or starting with a blank frame; make headless acceptance an explicit opt-in. | ScreenCast spec (no PipeWire node when no output); X11: no root window case; PDF Phase 6.1 ("enumerate monitors/screens and capture the selected geometry"). | **Partial.** Linux portal state machine handles start failures, but no explicit "no sources" UI state in the viewer/host panels. |

### 2.2 Resolution changes / source changes

| # | Black-box observation (RustDesk, official docs) | Boru requirement | Independent implementation source | Current Boru state |
|---|---|---|---|---|
| RES-1 | The viewer has a **view style**: "original" (1:1, scrollable) or "adaptive" (fit to window). When the remote resolution changes the presentation adapts without the controlling side reconnecting. | On capture resolution change, send an explicit **config-change message before media dimensions change**, force a keyframe, and have the viewer re-fit the image while preserving aspect ratio. | PipeWire stream param renegotiation (`spa_video_info`/format change events) — https://docs.pipewire.org/; WinRT `GraphicsCaptureItem.SizeChanged` — https://learn.microsoft.com/en-us/windows/win32/api/graphicscapture/nf-graphicscapture-igraphicscaptureitem-setchanged; iced `ContentFit::ScaleDown` for aspect-preserving fit. | **Gap.** `CodecConfig` rebuilds the encoder/decoder on dimension change but there is no wire-level config-change message; viewer image uses `ContentFit::Contain` already (aspect preserved). PDF Phase 8.2/10. |
| RES-2 | When CPU (software) encoding is slow, the client **scales the capture down to half resolution** above a size threshold (hardware encoding keeps full resolution); i.e. resolution is a quality lever that can change during a session. | Let the capture/encode pipeline reduce resolution under encoder/network pressure without ending the session, and communicate the change to the viewer via the config-change message (see 2.6). | OpenH264: reconfig via `SetOption`/re-init with new `SpatialLayer` size; PipeWire spa format renegotiation; PDF Phase 7.3 (resolution is the last quality lever). | **Gap.** `AdaptiveQuality` has a resolution step but is unwired; no runtime renegotiation path. PDF Phase 7.3. |
| RES-3 | On Windows, elevated UI (UAC prompts, Task Manager) cannot be captured or driven by a **non-elevated** process: the screen goes blank for those windows and the mouse is unresponsive over them; elevation (startup, controlled-end accept, or control-end request) is required to capture/interact with admin UI. | On Windows, detect and clearly report capture/input limits for elevated windows (secure desktop); do not silently send black frames; optionally support re-requesting capture after privilege elevation. | Microsoft UIPI and secure desktop: https://learn.microsoft.com/en-us/windows/win32/winstation/desktop-security-and-access-rights; WinRT Graphics Capture cannot capture the secure desktop — https://learn.microsoft.com/en-us/windows/uwp/audio-video-camera/screen-capture. | **Unknown/untested.** Windows backend exists but no Windows CI; this edge case is unhandled. PDF Phase 4.1 (handle permission failures). |

### 2.3 Cursor behaviour

| # | Black-box observation (RustDesk, official docs) | Boru requirement | Independent implementation source | Current Boru state |
|---|---|---|---|---|
| CUR-1 | The **remote cursor is an optional overlay**: the controlling side can show or hide the remote cursor independently of the video, and by default the local pointer is used while the remote cursor is hidden. | Keep the cursor **separate from the video frames** where the platform allows, and let the viewer toggle remote-cursor display without ending the session. | ScreenCast `cursor-modes` option (hidden/embedded/metadata) — https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html; PipeWire `spa_meta_cursor` metadata; WinRT `GraphicsCaptureSession.IsCursorCaptureEnabled` — https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.iscursorcaptureenabled | **Gap.** No cursor-mode option is plumbed; cursor handling is whatever the backend embeds. PDF Phase 4.2/5.3. |
| CUR-2 | The viewer can **follow the remote cursor** (view scrolls with it) and can **zoom the cursor with the image scale** (cursor scales when the view is zoomed), keeping pointer alignment at any zoom. | When the viewer zooms/pans (fit/100%/fullscreen), draw the cursor at a position and size normalized to the shared source coordinate space so pointer and image stay aligned. | Coordinate mapping already exists as `normalize_to_capture` (`src/screen_share/remote_input.rs`); scaling math is viewer-side; iced `mouse_area` for overlay input. PDF Phase 4.2 ("normalize coordinates against the shared source rather than global desktop coordinates"). | **Partial.** Pointer input is normalized; zoomed-cursor rendering is not implemented (viewer has fullscreen/inline toggle only, no zoom). PDF Phase 8.2. |
| CUR-3 | Input remapping exists on the controlling side (reverse mouse wheel, swap left/right buttons, touch-vs-mouse mode, virtual mouse/joystick on touch devices). | Expose viewer-side input remapping (wheel direction, button swap) as options; touch-mode/virtual pointer are later phases for mobile viewers (Boru is desktop-first). | Windows `SendInput`/X11 `XTEST` for the actual injection (already in `remote_input.rs`); remapping is viewer-side event transformation before sending. PDF Phase 9.2. | **Gap.** No remapping options in the input event pipeline. PDF Phase 9.2/14 (cursor-shape optimisation). |

### 2.4 Frame pacing

| # | Black-box observation (RustDesk, official docs) | Boru requirement | Independent implementation source | Current Boru state |
|---|---|---|---|---|
| PAC-1 | Session frame rate is **configurable** (range 5–120 fps, default 30) and there is an **adaptive bitrate** toggle (on by default) so the sender paces frames/bitrate to network conditions rather than queueing without limit. | Make frame rate part of the negotiated `StreamConfig` (capture fps range), defaulting to Boru's current 15 fps with 30 fps as the quality target; keep the latest-frame drop policy so latency stays bounded (PDF DoD). | OpenH264 framerate/bitrate config; portal/PipeWire spa video format fps; PDF Phase 7.1 targets (720p30/1080p30) and Phase 7.2 (drop obsolete frames over building latency). | **Partial.** `CAPTURE_FPS = 15` constant; `LatestFrameQueue` drops stale non-keyframes; fps not part of negotiation. PDF Phase 7.1/7.2. |
| PAC-2 | A **quality monitor** overlay can be shown during a session, i.e. the client continuously measures and displays stream quality — the observable signal of a pacing/adaptation loop. | Collect the PDF Phase 12 developer metrics (capture fps, encode fps, avg encode time, bytes/sec, dropped frames, queue depth, decode fps, estimated end-to-end latency) and surface them in a debug-only overlay. | `ScreenShareStats`/`ScreenShareStatsSnapshot` already exist (`src/screen_share/stats.rs`); `CapturedFrame.timestamp_us` and encode-side timestamps enable latency computation; iroh connection stats for RTT — https://docs.iroh.computer. | **Gap.** Counters exist but are never surfaced (BORU-SS-03 follow-up note 2). PDF Phase 12. |
| PAC-3 | The client offers **hardware encoding** to make the picture smoother (and can fall back to software rendering), implying the encoder choice is user-controllable and has observable smoothness effects. | Keep the encoder abstraction (PDF Phase 2.2) so hardware codecs can replace/augment OpenH264 without restarting the session; expose encoder preference in negotiation. | OpenH264 API — https://github.com/cisco/openh264; hardware codec enablement must come from platform APIs (e.g. Windows Media Foundation / NVENC via permissively licensed bindings, evaluated later). PDF Phase 2.2/7.1/14. | **Partial.** `VideoEncoder` trait exists; OpenH264 only. PDF Phase 2.2. |

### 2.5 Reconnects

| # | Black-box observation (RustDesk, official docs) | Boru requirement | Independent implementation source | Current Boru state |
|---|---|---|---|---|
| REC-1 | The device identity model is a **persistent ID + regenerating one-time password** (plus optional permanent password), so after a session drops the controlling side can reconnect to the same device without re-pairing; terminal sub-sessions can be kept **alive across a disconnect** (opt-in). | On transient media failure, keep the Boru chat/session state intact, surface a **reconnecting** UI state, re-establish the media stream, and request a fresh keyframe before resuming display (do not silently black-screen). Boru needs no ID/password pairing — iroh peer identity is persistent — so "reconnect" means a new iroh connection to the same peer. | iroh endpoint/connection lifecycle and stats — https://docs.iroh.computer (connection events, `Connection::closed()`/stats); PDF Phase 3.3 ("On transient media failure, preserve the chat/friend session. Request a fresh keyframe after media reconnection"). | **Gap.** `run_host_session` ends on connection failure; no reconnecting state in the UI; `SessionEvent` has no Reconnecting variant. PDF Phase 3.3/13. |
| REC-2 | Security-sensitive actions (e.g. requesting elevation) still require **someone at the controlled end to accept**, i.e. elevated/privileged steps are never silently resumed. | Remote-control permission must **not** auto-resume after a reconnect unless the policy explicitly allows it; a reconnected session starts view-only and requires fresh consent for control. | `SessionPermissions` defaults to view-only and control grants carry a nonce/TTL (`src/screen_share/permissions.rs`); make the "fresh session ⇒ view-only" rule explicit on reconnect. PDF Phase 3.3/9.1. | **Aligned by construction.** New sessions start view-only; make it explicit and add a test for the reconnect path. |
| REC-3 | Connection establishment has fallbacks (UDP hole punching with IPv6 option, and relay when direct path fails); a known failure mode is NAT loopback (LAN clients failing to reach a server via its public IP). | Keep iroh's existing direct/relay fallback for the screen-share session; do not introduce a separate signalling path; document that relay fallback is expected behaviour for Boru screen share. | iroh endpoint presets/relay (already used by Boru) — https://docs.iroh.computer; PDF Phase 3 (transport = Boru/Iroh encrypted P2P sessions, NAT traversal, relay fallback). | **Aligned.** Screen-share reuses the iroh endpoint. No change needed; add to the reconnect test matrix. |

### 2.6 Quality changes

| # | Black-box observation (RustDesk, official docs) | Boru requirement | Independent implementation source | Current Boru state |
|---|---|---|---|---|
| Q-1 | Image quality is a **user-facing preset** (best / balanced / low / custom) with a numeric custom quality scale and a custom fps; i.e. quality is controllable per-session and can be requested by the viewer. | Negotiate a quality profile (preset and/or explicit fps/bitrate) in `StreamConfig`, and allow the **viewer to request a lower-quality mode** mid-session via a protocol message. | PDF Phase 7.1 (expose bitrate, fps, keyframe interval, quality profile) and 7.3 ("allow the viewer to request a lower-quality mode manually"); OpenH264 `SEncParamExt` (bitrate, fps). | **Gap.** No `QualityUpdate` message in the protocol (BORU-SS-03 follow-up note 6); `CodecConfig` is fixed at session start. PDF Phase 7.3. |
| Q-2 | **Adaptive bitrate** is a toggle (default on): the client adapts to network conditions automatically; combined with the quality monitor this is a closed adaptation loop. | Wire the existing (currently dead) `AdaptiveQuality` controller into the host streaming loop: reduce bitrate → fps → resolution gradually under sustained congestion, increase conservatively after a stable recovery period, using send-queue depth, measured throughput, RTT, encode time, and dropped frames. | PDF Phase 7.3 lists exactly these inputs; `AdaptiveQuality` + `ScreenShareStatsSnapshot` already implement the decision logic (`src/screen_share/adaptation.rs`, `stats.rs`); iroh connection RTT — https://docs.iroh.computer. | **Gap.** `AdaptiveQuality` has no production caller (BORU-SS-03 follow-up note 1). PDF Phase 7.3. |
| Q-3 | The codec is **user-selectable** (auto / vp8 / vp9 / av1 / h264 / h265) with a documented caveat that hardware codecs depend on machine support; true-color (4:4:4) is an optional toggle. | Define a codec preference and colour-format support matrix in negotiation (H.264 baseline via OpenH264; 4:4:4 only if the codec and platform support it), and fail negotiation cleanly on incompatible codec choices. | OpenH264 supports I420 and I444 sampling (check per build) — https://github.com/cisco/openh264; VP8/VP9/AV1 would come from permissively licensed bindings (e.g. vpx, dav1d) evaluated later; PDF Phase 14 (AV1/H.265 negotiation "where licensing and platform support are acceptable"). | **Partial.** H.264 only, fixed; no codec negotiation field. PDF Phase 7.1/14. |
| Q-4 | There is a **privacy-mode** behaviour where the controlled side's local display is hidden (black) while the session is active — a controlled-side quality/privacy feature, distinct from the viewer's view quality. | (Later phase) Implement controlled-side "hide my screen locally while sharing" as an explicit opt-in privacy mode, never as the default, and keep it independent from remote-control permission. | ScreenCast/RemoteDesktop portals do not provide this; it is a local presentation concern (hide the portal output from local session / blank the primary surface); Windows: DXGI desktop duplication output exclusion. PDF Phase 9/14 (feature review). | **Not planned.** Record as a Phase 14 candidate; do not implement in baseline. |

### 2.7 Remote-control consent

| # | Black-box observation (RustDesk, official docs) | Boru requirement | Independent implementation source | Current Boru state |
|---|---|---|---|---|
| CON-1 | Incoming sessions are governed by an **access mode** (full / custom / view), where "view" is view-only; keyboard/mouse input, clipboard, file transfer, audio, terminal, tunnel, restart, recording, and remote-config modification are **separate permissions** that can be toggled. | Default every share to **view-only**; remote control (pointer, keyboard, clipboard) must be separately offered and explicitly accepted; no capability is granted implicitly by friendship or an existing call. | PDF Phase 9.1 (permission model); existing `Capability::{ViewScreen, ControlPointer, ControlKeyboard, Clipboard}` and `SessionPermissions` (`src/screen_share/permissions.rs`). | **Aligned.** View-only default, capability-gated control grants. PDF Phase 9.1. |
| CON-2 | The controlled side sees an **accept window before the session starts** and can change permissions there before accepting; whether the controlling side may drive that window is itself a separate opt-in. | Show an accept prompt with **per-capability choices** (view-only / +pointer / +keyboard / +clipboard) before the session starts, not just accept/reject; the sharer can change these per session. | PDF Phase 9.1/13 ("Offer monitor/source selection before capture begins. Show clear states…"); protocol `Hello` already carries `Permission`; extend the accept UI to per-capability toggles. | **Partial.** Accept/Decline only; capabilities are negotiated but the accept UI has no per-capability toggles. PDF Phase 9.1/13. |
| CON-3 | While remote control is active the controlled side can **block other local users' input** (Windows), i.e. the controlled user retains a kill switch over the session; sessions can also be recorded, and a **persistent visual indicator** accompanies active control. | Show a persistent visual indicator while remote control is active; provide a one-click **stop-control/revoke** action; stop remote input immediately when sharing ends, the peer disconnects, or consent is revoked. | PDF Phase 9.1 (persistent indicator, one-click revoke); `RevokeControl` + `SessionPermissions::revoke_control` already exist; local input blocking (later) via Windows `BlockInput`, X11 `XGrabPointer/XGrabKeyboard`, portal RemoteDesktop (not baseline). | **Aligned.** Indicator + revoke exist (`ControlChanged` event, host panel revoke button). Add disconnect-implies-revoke test. PDF Phase 9.1. |
| CON-4 | A **one-time password** regenerates for incoming sessions (plus optional permanent password), and password-based entry is one of several approve modes (click / password / both). | (Not directly applicable.) Boru uses iroh peer identities and encrypted sessions; do not introduce passwords. Record as a deliberate divergence: consent is peer-identity-based, not password-based. | PDF Phase 3 (transport = Boru/Iroh encrypted P2P sessions and existing session security). | **Aligned by design.** No change. |
| CON-5 | Input streams are gated on the granted permission and can be remapped/limited on the controlling side; remote input is only available when the permission is granted for the session. | Rate-limit pathological input streams and reject input messages unless the current session state grants remote-control permission. | PDF Phase 9.2 (rate-limit pathological streams, reject input without permission); `RequestRateLimiter` exists; add an input-event rate cap in `host.rs`/`remote_input.rs`. | **Partial.** Control-request rate limiter exists; no input-event rate cap. PDF Phase 9.2. |
| CON-6 | Clipboard sync is a **separate capability** that can be disabled independently of remote control, and can be made one-way (controlled→controlling disabled) on the controlled side. | Treat clipboard sync as a separate optional capability; do not enable it automatically with remote control; start with text-only if implemented. | PDF Phase 9.3 (clipboard = separate optional capability, text-only first); `Capability::Clipboard` already exists but has no implementation. | **Gap.** Capability enum has Clipboard but no clipboard transfer path. PDF Phase 9.3. |

---

## 3. Consolidated Boru requirements list

Cross-cutting guardrails that apply to every requirement below (from the
reference PDF and the binding policy doc):

- Implement against independent sources only (the `SRC` column); never against
  RustDesk implementation details.
- Keep screen sharing a native Boru subsystem on the existing Boru/Iroh
  transport; do not tunnel an external remote-desktop product.
- Do not alter chat/network/protocol behaviour outside the screen-share
  subsystem; preserve existing networking, chat, file transfer, video, tunnel,
  lobby, room, and persistence behaviour.
- Prefer small, reviewable commits; compile/test after each area.

### 3.1 Monitor selection and source changes (PDF Phase 10, 13)

1. **BORU-SS-REQ-01 (MON-1)** — Add capture-source enumeration and selection UI
   before a share starts; the sharer picks a monitor/desktop. Linux: drive the
   portal `SelectSources` selection explicitly (currently the portal default is
   used); Windows: enumerate outputs via DXGI and let the user pick. *Src:*
   ScreenCast spec; Microsoft DXGI docs.
2. **BORU-SS-REQ-02 (MON-1/MON-2)** — Add a protocol-level **source-change /
   config-change** control message (new `ControlMessage` variant) sent *before*
   media dimensions change; the host forces a keyframe after the change; the
   viewer re-fits without ending the session. Also carry a source-mode field
   (single / per-display / spanning) for later multi-display support. *Src:*
   PipeWire format renegotiation; WinRT `SizeChanged`; OpenH264 reconfig.
3. **BORU-SS-REQ-03 (MON-3)** — Model "no capture source" as a first-class
   state with a clear UI error (and an explicit opt-in for headless acceptance);
   never hang or stream blank frames when no output exists. *Src:* ScreenCast
   no-node case; X11 no-root-window case.
4. **BORU-SS-REQ-04 (RES-1)** — Viewer must preserve aspect ratio across
   resolution changes (fit-to-window, 100%, fullscreen; pan/zoom is a later
   phase) and re-fit on `config-change`. *Src:* iced `ContentFit`; PDF Phase 8.2.

### 3.2 Cursor (PDF Phase 4.2, 5.3, 8.2)

5. **BORU-SS-REQ-05 (CUR-1)** — Plumb a **cursor-mode** option through the
   capture backends (portal `cursor-modes`, WinRT `IsCursorCaptureEnabled`) so
   the cursor can be delivered as a separate layer or embedded; viewer gets a
   show/hide-remote-cursor toggle. *Src:* ScreenCast cursor-modes; WinRT
   GraphicsCaptureSession.
6. **BORU-SS-REQ-06 (CUR-2)** — Keep pointer coordinates normalized to the
   shared source (already done in `normalize_to_capture`) and render the cursor
   scaled/positioned for the current viewer zoom; add zoom/pan to the viewer
   surface. *Src:* existing mapping; iced rendering.
7. **BORU-SS-REQ-07 (CUR-3)** — Viewer-side input remapping (wheel direction,
   button swap) as options; extend `InputEvent` with explicit modifier state
   (currently the struct carries only code/capability/token/x/y/pressed) so key
   combinations are unambiguous at the host. *Src:* PDF Phase 9.2; platform
   injection APIs already used.

### 3.3 Frame pacing (PDF Phase 7.1, 7.2, 12)

8. **BORU-SS-REQ-08 (PAC-1)** — Move fps into `StreamConfig` negotiation
   (default 15, target 30); keep the latest-frame drop policy (stale frames are
   never queued without bound). *Src:* OpenH264 `SEncParamExt`; PDF DoD latency
   bound.
9. **BORU-SS-REQ-09 (PAC-2)** — Surface `ScreenShareStats` (capture/encode/decode
   fps, encode time, bytes/sec, dropped frames, queue depth, estimated latency)
   in a debug-only overlay behind a flag. *Src:* `stats.rs`; `timestamp_us` in
   `CapturedFrame`; iroh connection stats.
10. **BORU-SS-REQ-10 (PAC-3)** — Keep the encoder abstraction ready for
    hardware codecs; expose encoder preference in negotiation; benchmark CPU use
    of OpenH264 at 720p30/1080p30. *Src:* PDF Phase 2.2/7.1; OpenH264 API.

### 3.4 Reconnects (PDF Phase 3.3, 13)

11. **BORU-SS-REQ-11 (REC-1)** — Add a **reconnecting** session state: on
    transient media failure keep chat/session state, show "reconnecting", re-open
    the media stream, and request a fresh keyframe before resuming display. Add a
    `Reconnecting` variant to `SessionEvent`. *Src:* iroh connection lifecycle;
    PDF Phase 3.3.
12. **BORU-SS-REQ-12 (REC-2)** — A reconnected session starts **view-only**;
    remote-control permission is never silently resumed. Add a test asserting the
    reconnect path re-requires consent. *Src:* `permissions.rs` semantics; PDF
    Phase 3.3/9.1.
13. **BORU-SS-REQ-13 (REC-3)** — Keep iroh direct/relay fallback as the only
    signalling path (no separate signalling); document relay fallback as expected
    in the test matrix. *Src:* iroh endpoint docs; PDF Phase 3.

### 3.5 Quality changes (PDF Phase 7.3, 14)

14. **BORU-SS-REQ-14 (Q-1)** — Add a **`QualityUpdate` protocol message**
    (viewer-requested quality preset / lower-quality mode) and include a quality
    profile (preset, fps, bitrate) in `StreamConfig`. *Src:* PDF Phase 7.3; OpenH264
    bitrate/fps options.
15. **BORU-SS-REQ-15 (Q-2)** — Wire `AdaptiveQuality` into the host loop:
    sustained congestion lowers bitrate → fps → resolution; recovery raises
    quality conservatively; inputs are queue depth, throughput, RTT, encode time,
    dropped frames. *Src:* `adaptation.rs`/`stats.rs`; iroh RTT; PDF Phase 7.3.
16. **BORU-SS-REQ-16 (Q-3)** — Codec preference + colour-format matrix in
    negotiation (H.264 baseline; 4:4:4 only when supported); clean rejection of
    incompatible codec choices. *Src:* OpenH264 docs; PDF Phase 7.1/14.
17. **BORU-SS-REQ-17 (Q-4)** — (Phase 14 candidate, not baseline) controlled-side
    privacy mode that hides the local display during a session; explicit opt-in
    only. *Src:* local presentation concern; PDF Phase 14.

### 3.6 Remote-control consent (PDF Phase 9.1–9.3, 13)

18. **BORU-SS-REQ-18 (CON-1)** — Default every share to view-only; remote
    control offered separately and explicitly accepted; no capability implied by
    friendship/call. (Already implemented; add acceptance tests.) *Src:* PDF
    Phase 9.1.
19. **BORU-SS-REQ-19 (CON-2)** — Extend the accept UI to **per-capability
    toggles** (view-only / +pointer / +keyboard / +clipboard) chosen before the
    session starts; the sharer can change them per session. *Src:* PDF Phase
    9.1/13.
20. **BORU-SS-REQ-20 (CON-3)** — Persistent visual indicator while remote
    control is active; one-click revoke; input stops immediately on share end,
    peer disconnect, or revoke. (Already implemented; add disconnect test.) *Src:*
    PDF Phase 9.1.
21. **BORU-SS-REQ-21 (CON-5)** — Rate-limit the input event stream (beyond the
    existing control-request limiter) and reject input when the session lacks
    control permission. *Src:* PDF Phase 9.2.
22. **BORU-SS-REQ-22 (CON-6)** — Clipboard sync as a separate, non-default
    capability; text-only first. *Src:* PDF Phase 9.3; `Capability::Clipboard`
    already exists.

---

## 4. Deliberate divergences from RustDesk (recorded, not copied)

- **Identity/passwords.** RustDesk pairs devices by ID + one-time/permanent
  password with click/password approve modes. Boru deliberately does **not**
  adopt a password model: iroh peer identities and the existing encrypted
  Boru/Iroh session provide consent-by-identity. (PDF Phase 3: reuse Boru/Iroh
  encrypted P2P sessions and existing session security.)
- **Multi-display spanning.** RustDesk can span all displays in one session;
  Boru's baseline is single-monitor capture (PDF Phase 4.1) with the protocol
  extended (REQ-02) so spanning can be added later without a protocol break.
- **Hardware codec breadth.** RustDesk ships VP8/VP9/AV1 software and
  H.264/H.265 hardware codecs. Boru's baseline is OpenH264 H.264; other codecs
  are Phase 14 candidates gated on permissive licensing and platform support
  (REQ-10, REQ-16).

## 5. Verification

- This study made **no code changes**: only `docs/screenshare-behavioral-notes.md`
  was added to the worktree; `git status` is otherwise clean.
- `rb check --all-targets --features screen-sharing` on DEBSRV is expected to
  pass unchanged (document-only change).
- No RustDesk source text, comments, tests, or constants are reused anywhere in
  this document; every requirement cites an independent platform/API source.
