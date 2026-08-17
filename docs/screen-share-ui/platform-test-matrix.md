# Screen-share platform test matrix (BORU-SS-41)

Task: t_0e8d9e9b — Verify/fix screen-share on X11, Wayland, Windows.
Build base: origin/main at 8c2c62a1 (v0.214.1).

## Summary

| Platform | Backend used | Result | Evidence |
|---|---|---|---|
| X11 native (VM-A host / VM-B viewer, Xvfb :98/:99) | `X11Capture` (direct X11 GetImage + XDamage, no portal) | **PASS** — live updates, source picker, source switching, window capture | log markers + screenshots below |
| Wayland (portal/PipeWire) | `LinuxPortalCapture` | **NOT TESTED** — fedora 172.16.0.21 / fedoraVM 172.16.0.23 unreachable (100% ping loss, no route); headless sway + xdg-desktop-portal-wlr stack prepared but the GUI cannot init under headless pixman EGL (`libEGL: failed to create dri2 screen`) | environment limitation, not a boru bug |
| Windows (WinRT Graphics Capture) | `GraphicsCapture` (windows.rs) | **CROSS-BUILD FIXED + VERIFIED**; runtime NOT TESTED — Windows host 172.16.0.17 unreachable, Windows11 VM SSH denied (no credentials) | cargo check exit 0, see below |

## Acceptance criteria

1. **Live screen-share verified on X11 native** — PASS, with continuous live-update
   evidence (see log markers). Wayland and Windows cannot be tested from this
   environment: fedora hosts and the Windows host are unreachable, and the local
   Windows11 VM has no usable SSH credentials. Per the task body this is the
   documented carve-out ("If a platform cannot be tested from this environment,
   say exactly which and why").
2. **Portal-missing fallback degrades gracefully** — PASS: the X11 verification
   ran with NO portal present (Xvfb), and the backend fell back to direct X11
   capture cleanly (`path=Direct`, no crash, live frames).
3. **`rb check` for the Linux build passes** — PASS:
   `rb check --bin boru --features gui,video-playback,terminal,screen-sharing`
   → exit 0, `Finished dev profile ... in 18.22s`, 0 errors.
   **Windows cross build verified via the debsrv flow** — PASS after fix:
   `cargo check --target x86_64-pc-windows-gnu --bin boru --features
   gui,terminal,voice-calls,video-calls,screen-sharing` → exit 0 (see fix below).
4. **Commit and push fixes; report matrix with concrete evidence** — this doc +
   screenshots committed with the fix.

## X11 live-verification log markers (VM-A 172.16.0.54 host, VM-B 172.16.0.55 viewer)

Host (vm-a `~/boru-test/runs/vm-a/logs/boru.log`):

```
screen-share capture backend selected backend="x11"              (18:13:07)
screen-share: capture started event="capture_start" backend="x11" codec="h264"
    width=1280 height=720 bitrate_bps=8000000 frame_rate=15 preset="lan-high" path=Direct
screen-share: host encoded first frame bytes=29140
screen-share: host frame queued on media channel sequence=0 bytes=29140
screen-share: host switched source ... title=Entire desktop: 1280x720 ... source_mode: Spanning
screen-share: X11 window capture started window=2097154 title=Boru — v0.214.1
screen-share: host switched source ... title=Boru — v0.214.1: 1024x768 ... source_mode: Single
screen-share: host skipped unchanged frame (empty dirty region) skipped_frames=8000 → 11500
    (XDamage-driven frame skip sustained across ~10 min)
```

Viewer (vm-b `~/boru-test/runs/vm-b/logs/boru.log`):

```
screen-share: viewer Accept send result error=None
screen-share: viewer source change announced ... title=screen: 1280x720 ... mode=PerDisplay
screen-share: viewer received media ... sequence=0 bytes=29140
screen-share: viewer performance metrics ... decode_fps=19-20   (sustained)
```

Live-update proof: the host root background was changed via `xsetroot`
(green→yellow→magenta); the viewer rendered the magenta strip and decode counts
rose — pixels flow live. Source picker enumerated 3 sources (Monitor, Desktop
Spanning, Window) and switching logged cleanly during the live session.

## Fix landed by this task (cross-build regression)

The Windows cross-build with `screen-sharing` failed to compile at origin/main
(6 errors, introduced by BORU-SS-36/38 multi-monitor + window-only capture work
in `src/screen_share/platform/windows.rs`):

- `E0282` windows.rs:371 — `CreateForWindow` returns a generic
  `windows::core::Result<T>`; `T` was not inferable. Added an explicit
  `windows::core::Result<GraphicsCaptureItem>` annotation.
- `E0277` windows.rs:406 — `.find(|(_, raw)| *raw == ...)` compared `&usize`
  with `usize` (iterating a `HashMap<_, usize>` gives `&usize`). Fixed with
  `**raw`.
- `never type fallback` windows.rs:409 / :432 — `CreateForMonitor::<_, T>`
  generic `T` unresolved at the unsafe call sites. Fixed with explicit
  turbofish `interop.CreateForMonitor::<_, GraphicsCaptureItem>(hmon)`.

After the fix, the exact task cross-build command passes (exit 0). The
Linux `rb check` (which does not compile windows.rs, `#[cfg(target_os =
"windows")]`) is unaffected and passes.

## Screenshots

`evidence/ss41/` (captured during the live X11 session, 2026-08-17 18:44–18:46):

- `sharer-panel.png` — host share panel (1280x720, magenta strip visible after
  xsetroot change)
- `sharer-after-mid-click.png` — host panel after accepting mid-session
- `viewer-window-capture.png` — viewer showing the window-capture source
  (1024x720)

## Remaining follow-ups (blocked on environment, not code)

1. **Wayland runtime** — needs a reachable Wayland desktop (fedora VMs
   172.16.0.21/.23, or any real Wayland session with working GPU/EGL) to
   exercise the portal ScreenCast + PipeWire path end-to-end.
2. **Windows runtime** — needs SSH access to the Windows11 VM or the Windows PC
   (172.16.0.17) to verify WinRT GraphicsCapture enumerates monitors, captures,
   and streams live frames. The cross-build now compiles with `screen-sharing`;
   only the live run is pending.
