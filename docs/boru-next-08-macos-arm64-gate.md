# BORU-NEXT-08: macOS arm64 native gate

Status: **BLOCKED — no native macOS arm64 runner or candidate artifact was
available for this verification.** This record is fail-closed: configuration
and cross-target checks are not presented as native macOS runtime evidence.

Evidence timestamp: `2026-08-27T15:32:35+10:00`.

## Runner and artifact disposition

The worker host is Linux x86_64 (`x86_64-unknown-linux-gnu`), not macOS arm64.
The configured DEBSRV build host is also not a native macOS runner. No signed
or unsigned `boru-macos-aarch64` artifact was supplied, so package integrity,
launch, GUI rendering, chat/direct-message round trips, discovery, file
transfer, tunnel behavior, permissions, and shutdown were **NOT RUN**.

The exact remote cross-target check was attempted with:

```text
rb check --target aarch64-apple-darwin --bin boru --features gui
```

Result: **FAIL (runner/toolchain unavailable)**. Cargo stopped before compiling
Boru because the remote toolchain lacks the target standard libraries:

```text
error[E0463]: can't find crate for `std`
note: the `aarch64-apple-darwin` target may not be installed
error[E0463]: can't find crate for `core`
error: could not compile `log` (lib) due to 1 previous error
```

This is evidence that the available build host cannot perform this gate; it is
not evidence that the macOS build or runtime fails on macOS.

## Feature matrix (configured versus verified)

The release workflow (`.github/workflows/release.yaml`) selects target
`aarch64-apple-darwin` with `features: gui`. In `Cargo.toml`, `gui` includes
`net`; the other listed features are independent opt-ins.

| Capability / feature | Release configuration | Native macOS arm64 result |
|---|---|---|
| GUI, chat, direct messages, rooms/discovery, files, secure tunnels | `gui` (and its `net` dependency) | **UNTESTED** — no native runner/artifact |
| `metrics`, `wgpu-renderer` | `metrics` is in the default set; `wgpu-renderer` is declared by the release matrix | **UNTESTED** |
| `terminal` | Not enabled for macOS release | **INTENTIONALLY DISABLED** |
| `voice-calls` | Not enabled for macOS release | **NOT RELEASE-ENABLED; UNTESTED** |
| `video-calls` | Not enabled for macOS release | **NOT RELEASE-ENABLED; UNTESTED** |
| `video-playback` | Not enabled for macOS release | **NOT RELEASE-ENABLED; UNTESTED** |
| `screen-sharing` | Not enabled for macOS release | **UNSUPPORTED** (see below) |

Configuration checks performed in this worktree:

```text
python3 scripts/check-release-feature-matrix.py       PASS: release feature matrix: OK
bash -n scripts/release-sign.sh scripts/package_windows.sh   PASS
python3 -m py_compile scripts/check-release-feature-matrix.py scripts/release-validate.py   PASS
```

These checks validate repository metadata/scripts only; they do not substitute
for a native macOS run.

## ScreenCaptureKit / native screen-sharing limitation

Native macOS screen sharing is explicitly **Experimental/unsupported** and
must not be advertised as available. The current implementation is a
synthetic test-pattern backend:

- `src/screen_share/platform/macos.rs` contains only a placeholder module.
- On macOS, `ActiveCapture` has only `TestPattern`.
- `create_capture_source` constructs a 640x360 synthetic frame, and the
  backend name is `macos-test-pattern`.
- There is no ScreenCaptureKit display/window enumeration, Screen Recording
  permission flow, actual frame capture, or native clean-stop path.

Consequently, a test-pattern screenshot or successful cross-compilation would
not establish native screen-sharing support. The macOS gate remains blocked
until a real macOS desktop demonstrates source enumeration, permission denial
handling, frame capture, and clean shutdown through `ActiveCapture`.

## Required follow-up

Run the release feature set on a native Apple Silicon macOS runner, preserve
only non-sensitive machine-readable results outside the repository, and record
an artifact digest plus launch/GUI/core smoke results. Keep screen sharing
marked unsupported until a ScreenCaptureKit implementation and native desktop
verification exist.
