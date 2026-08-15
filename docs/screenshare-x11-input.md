# Boru Screen-Share — X11 Remote Input (PDF Task 6.2)

Status: **implemented** (BORU-SS-17). Companion to
`docs/screenshare-current-state.md` and the X11 capture backend from
BORU-SS-16 (PDF Task 6.1).

## What this is

A direct X11 input-injection backend (`X11RemoteInput` in
`src/screen_share/remote_input.rs`) that translates normalized Boru input
events into X11-compatible events and sends them through the XTEST extension
(`XTestFakeInput` via `x11rb` with the `xtest` feature).

- **Consent model.** The backend is constructed only after the host user
  explicitly grants control (`HostCommand::GrantControl` in `host.rs`). It
  stores the granted device mask (pointer bit / keyboard bit, same bitmask as
  the portal backend) and `apply` re-checks that mask for **every** event —
  injection fails closed unless the current `SessionPermissions` granted the
  capability. The streaming loop additionally authorizes every wire event with
  `authorize_nonce` before it reaches the backend (defense in depth).
  View-only shares never open an input backend and never inject anything.
- **Backend selection.** `create_platform_backend` is display-server aware
  (mirroring the capture side): under a native X11 session XTest is tried
  first (no portal daemon needed); under Wayland/XWayland the RemoteDesktop
  portal is tried first (see limitations). Fallback order is
  portal ⇄ XTest, then `UnavailableInputBackend`.

## Event translation

| Normalized Boru event | X11 XTest action |
|---|---|
| Pointer move (`code == 0`) | `FakeInput(type = MotionNotify, detail = 0, root_x/root_y = absolute)` |
| Pointer button press/release (`code` 1-3: left/middle/right) | `FakeInput(type = ButtonPress/ButtonRelease, detail = button)` |
| Wheel (`code` 4-7: up/down/left/right) | press+release pair on the press event (one scroll tick; the matching release is a no-op) |
| Key press/release (keysym in `code`) | `FakeInput(type = KeyPress/KeyRelease, detail = keycode)` where keycode comes from the server's `GetKeyboardMapping` reversed (keysym → keycode) |
| Modifier keys (Shift/Control/Alt keysyms) | normal key events; the X server updates its modifier state from the injected keycode, so later key events see the correct modifier mask |

Pure translation lives in `x11_pointer_actions` / `x11_key_action` /
`build_keysym_to_keycode` and is unit-tested without an X server.

## Coordinate mapping

The host maps viewer-normalized coordinates to **capture pixels**
(`normalize_to_capture`), then the backend adds the capture rect's
root-window origin (`ActiveCapture::input_origin()`: the selected monitor's
x/y from RandR, `(0, 0)` for whole-root capture) to produce absolute
root-window coordinates for XTest. Motion is clamped to the root window
bounds, so multi-monitor layouts with negative origins and monitors past the
root edge map safely.

## Limitations under compositors and XWayland

1. **XWayland only reaches XWayland windows.** Under a Wayland session the X
   server is XWayland. XTest injects into that X server, so events are seen
   by X11/XWayland clients **only** — native Wayland windows never receive
   them. That is why the portal backend is preferred under Wayland/XWayland
   and this backend is the primary path under native X11. If XTest is forced
   under Wayland, remote control is best-effort over XWayland windows.
2. **Compositor policy on synthetic input.** Some X11 compositors (and
   window managers) treat XTEST-generated events differently from physical
   input: focus-follows-mouse, pointer confinement, and key grabs may not
   behave identically. GNOME Shell on X11 and KWin generally forward XTest
   events, but behavior is not guaranteed by any standard — verify on the
   target desktop.
3. **Keyboard layout dependence.** Keysym → keycode uses the host's *current*
   server keyboard mapping. Keys the viewer sends that the host layout cannot
   produce fail closed (rejected, not dropped silently). Modifier state is
   maintained by the server from injected modifier keycodes; there is no
   explicit modifier-mask field in the wire protocol (that is BORU-SS-24
   protocol work).
4. **No secure-desktop / lock-screen input.** XTest cannot inject into the
   secure attention / locked-screen path (same class of limitation as the
   Windows SendInput backend's UAC/secure-desktop note). A locked screen is
   not controllable remotely.
5. **XTEST availability.** Xvfb and Xorg ship XTEST by default; a server
   built without it (or a restricted X server) makes `connect` fail closed to
   `UnavailableInputBackend` — the share keeps working view-only.
6. **Absolute-only motion.** XTest fake motion is sent with absolute
   root-window coordinates (the same mechanism Xlib's `XTestFakeMotionEvent`
   uses). There is no relative-motion path, so pointer acceleration and
   relative-delta consumers are not relevant; coordinates always land at the
   requested root position, clamped to the root.
7. **No screen-change re-resolution.** The keyboard mapping and root geometry
   are captured at connect time. If the host changes layout or resolution
   mid-session, the backend is recreated on the next explicit grant/revoke
   cycle.

## What needs a real X session

The following cannot be verified headless and are covered by documentation +
manual verification (same pattern as the `x11_live_*` capture tests):

- `X11RemoteInput::connect` against a real `$DISPLAY` (Xvfb, Xorg, or
  XWayland) and successful `extension_information("XTEST")`.
- Actual `FakeInput` delivery: pointer motion, button press/release, wheel
  tick, key down/up and modifier state observed in an X11 client (e.g. `xev`
  or a terminal receiving typed keys).

Run with a live display:

```sh
cargo run --features screen-sharing --bin boru   # start a share on X11
# grant control, then verify with: xev -root -event keyboard -event mouse
```

The pure translation and consent-gating tests
(`x11_pointer_*`, `x11_key_*`, `x11_keysym_*`, `x11_consent_gate_*`,
`device_mask_grants_*`) run anywhere:
`cargo test --features screen-sharing remote_input::tests::x11_`
