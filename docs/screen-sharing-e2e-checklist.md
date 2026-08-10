# Screen-sharing manual E2E checklist (SS-M9)

This checklist is intentionally tracked separately from automated protocol tests.
Automated tests must remain desktop-session independent; update the result column
only with a reproducible manual run and record the path used.

Legend: PASS = exercised and observed; FAIL = defect with an issue/link; PENDING =
not available in the current lab.

## Required platform matrix

| Host | Viewer | Wayland/X11 or Windows | Direct | Relay | Result | Notes/date |
|---|---|---|---|---|---|---|
| Linux | Linux | Wayland |  |  | PENDING | Run on 172.16.0.54 ↔ 172.16.0.55 when available |
| Linux | Linux | Wayland |  |  | PENDING | Relay path |
| Windows | Linux | Windows → Wayland |  |  | PENDING | Manual Windows host required |
| Windows | Linux | Windows → Wayland |  |  | PENDING | Relay path |
| Linux | Windows | Wayland → Windows |  |  | PENDING | Manual Windows viewer required |
| Linux | Windows | Wayland → Windows |  |  | PENDING | Relay path |
| Windows | Windows | Windows |  |  | PENDING | Manual Windows pair required |
| Windows | Windows | Windows |  |  | PENDING | Relay path |

## Cases for every available combination

- [ ] Invitation is explicit; rejecting it does not start capture.
- [ ] Accepting grants `ViewScreen` only. No pointer, keyboard, clipboard, or
      filesystem capability is active.
- [ ] Viewer receives 1920x1080; repeat at 2560x1440 when the display supports it.
- [ ] High-DPI Windows scaling keeps the image usable and does not enable control.
- [ ] A stale/replayed message from an old session is rejected.
- [ ] Revoke Control is immediate while viewing continues; Stop Sharing ends
      viewing and clears the capture indicator.
- [ ] Closing Boru, logging out, suspending, and changing networks leave no
      capture indicator, listener, or control permission active after reconnect.
- [ ] Logs contain no frame bytes, clipboard contents, or input event payloads.
- [ ] Direct and relay paths both complete the accept, frame, revoke, and teardown
      cases without a panic.

## Automated coverage

`screen_share` unit tests cover bounded postcard round trips, version and size
validation, identity/session checks, idempotent end transitions, capability
revocation, and rejection of remote input before grant and after revoke. Run the
feature-gated tests with the repository's `rb` wrapper; no desktop session is
required.
