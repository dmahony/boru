# Boru Screen-Sharing — Wayland Portal Flow (BORU-SS-13 / PDF Task 5.1)

Status: implemented. This document describes the xdg-desktop-portal
ScreenCast flow added/extended in BORU-SS-13 (PDF Task 5.1), the desktop
environment handling, and — critically — what can and cannot be verified on
this machine (headless, no Wayland compositor, no PipeWire server).

## 1. What was implemented

The portal flow lives in `src/screen_share/platform/linux.rs`:

1. **CreateSession** (`org.freedesktop.portal.ScreenCast.CreateSession` with a
   random `session_handle_token`) → returns a session object path.
2. **SelectSources** (`types = 1` = Monitor, no `multiple` → exactly one
   stream). The desktop-environment permission dialog is NEVER bypassed: on
   Wayland the compositor shows its picker, on X11 the portal auto-selects
   the primary monitor. No fallback path bypasses consent.
3. **Start** (async Request/Response protocol): the call returns a
   `org.freedesktop.portal.Request` object path; the real result arrives on
   the `Response(u32, a{sv})` signal on that object. The `streams[0].node_id`
   is extracted from the reply body (`extract_stream_node_id`).
4. **PipeWire remote acquisition**: a dlopen-based PipeWire INPUT stream
   (`boru-screen-capture`) is connected to the returned node id on a
   dedicated thread (`boru-pipewire-capture`). Frame ingestion itself is
   BORU-SS-14 scope; the stream plumbing is in place.
5. **Clean session teardown** (new in this task):
   - The live zbus connection and the session object path are kept for the
     whole capture lifetime (previously the connection was dropped right
     after `Start`, which can make the portal tear the session down
     server-side).
   - `LinuxPortalCapture::close()` stops the PipeWire capture thread
     (`pw_main_loop_quit` is documented as callable from any thread; the
     thread then disconnects/destroys its stream, core, and context and
     signals completion through a bounded `recv_timeout`), calls
     `org.freedesktop.portal.Session.Close` on the session object, and marks
     the lifecycle machine `Closed`.
   - `Drop` performs the same cleanup best-effort: PipeWire stop is
     synchronous; `Session.Close` runs on a short-lived helper thread with
     its own current-thread tokio runtime (Drop cannot await, and the host
     session thread already has a runtime).

## 2. Lifecycle state machine

`PortalSessionMachine` is a pure state machine with the D-Bus layer kept
outside it, so the full lifecycle is unit-testable without a session bus,
portal, or compositor:

```
Idle → Creating → Selecting → Starting → Streaming
         │            │           │
         └─ on_failure/on_portal_closed/begin_close → Closing → Closed
```

- `create_session()` / `on_session_created()` / `select_sources()` /
  `start()` / `on_start_response_ok()` enforce the portal call ordering;
  invalid transitions return `MachineError::InvalidTransition`.
- Failure paths: `NoSessionBus`, `CreateSessionFailed`, `SelectSourcesFailed`,
  `StartFailed`, `StartRejected(u32)`, `StartTimeout`,
  `ResponseStreamClosed`, `MissingNodeId` — all terminal.
- Teardown: `begin_close()` (once per session) → `Closing` →
  `on_closed()` → `Closed`; `on_portal_closed()` models the portal/compositor
  ending the session while it is active.
- 11 new unit tests cover the happy path, every failure path, close from
  every active phase, close idempotency, invalid orderings, and portal-initiated
  close. Full screen-share suite: 136 tests pass on Linux (debsrv).

## 3. GNOME / KDE Plasma 6 / wlroots handling

The ScreenCast D-Bus flow is the same for every portal backend (the
`org.freedesktop.portal.Desktop` frontend normalises backend differences), so
the code takes the standard path and **detects** the environment for
diagnostics and actionable errors:

- `XDG_SESSION_TYPE` (`wayland` / `x11`) and `XDG_CURRENT_DESKTOP`
  (`GNOME`, `KDE`/`plasma`, wlroots compositors such as sway / Hyprland /
  wayfire / river / labwc / cage / gamescope / dwl) are classified and logged
  at connect time.
- The ScreenCast interface `version` property is queried
  (`org.freedesktop.DBus.Properties.Get`) and the portal backend bus names
  (`org.freedesktop.DBus.ListNames`, filtered on `impl.portal`) are reported.
- All portal error messages now carry `desktop=…, backend=…` context so a
  headless/CI failure is immediately diagnosable.

Behavioural notes per environment (documented, not code-branched):

| Environment | Behaviour relevant to this task |
|---|---|
| GNOME (xdg-desktop-portal-gnome) | Picker appears at `Start`; `SelectSources` with `types=1` + single stream works. Session must be closed via `Session.Close` or client disconnect. |
| KDE Plasma 6 (xdg-desktop-portal-kde) | KDE shows its own screen-selection dialog at `Start`. `multiple` is NOT set, so exactly one monitor is requested. Known KDE quirk: it returns multiple streams when `multiple=true`; we deliberately avoid that. |
| wlroots (xdg-desktop-portal-wlr) | No picker unless a chooser (`wlr-chooser`) is installed or the compositor has a configured output; `Start` can fail with a non-zero response. The error path surfaces the code + environment so the user sees "no output selected / chooser missing" instead of a generic failure. |

## 4. What requires a real Wayland session (cannot run headless here)

This build host is headless: no Wayland compositor, no X11 display, no
PipeWire server, no xdg-desktop-portal. The following cannot be exercised
here and are covered only by unit tests / code review:

- The actual permission dialog and source picker (GNOME/KDE/wlroots) — this
  is the whole point of the portal path and must be observed on a real
  desktop.
- `Start` returning a real PipeWire node id and `extract_stream_node_id`
  parsing a live reply body.
- PipeWire stream negotiation and buffer flow end-to-end (also BORU-SS-14
  scope).
- `Session.Close` actually releasing the portal session and PipeWire node.
- PipeWire thread teardown timing on a live stream (the bounded 2 s wait in
  `PipeWireHandle::stop`).

Suggested real-session verification (KDE Plasma 6 and/or GNOME, and a
wlroots compositor such as sway):

1. `BORU_PERF=1 cargo run --bin boru --features screen-sharing` (or the app
   with screen sharing), start a share, accept the picker.
2. Confirm the connect log line:
   `screen-share: connecting to xdg-desktop-portal ScreenCast` with the
   expected `desktop` / `backend` / `version` values.
3. Confirm the host receives frames (streaming path stats appear), then stop
   the share and confirm no `boru-pipewire-capture` thread remains
   (`ps -eLf | grep boru-pipewire-capture`).
4. Deny the picker on purpose and confirm the portal reports the rejection
   code in the error message.

## 5. Licensing note

Implemented from the xdg-desktop-portal D-Bus API documentation and PipeWire
public headers/ABI only. No RustDesk code was consulted or reproduced; the
flow follows the upstream `org.freedesktop.portal.ScreenCast` specification.
