# macOS arm64 capability decision

Status: **release-supported for the core GUI, with feature-specific limits**.
This is an evidence record for the `aarch64-apple-darwin` release target and is
not a claim of native runtime verification from Linux.

## Decision summary

| Capability | Status | Evidence and boundary |
|---|---|---|
| Chat and direct messages | Supported (build/configuration) | The macOS release target builds `boru` with `gui`; `gui` includes `net`, and the normal chat/direct-message paths are not gated out for macOS. Native macOS peer round-trip remains untested in this Linux environment. |
| Rooms and discovery | Supported (build/configuration) | The `gui` feature includes `net`, which contains the Iroh/gossip, discovery, and room paths. Native macOS discovery/room verification remains untested. |
| Files | Supported (build/configuration) | File sharing is part of the `net`/GUI application and has no macOS exclusion. The macOS file-reveal path uses the native `open` command (`src/bin/boru/app/files.rs:9400-9414`). Native macOS transfer verification remains untested. |
| Secure tunnels | Supported (build/configuration) | Tunnel protocol wiring is part of the GUI/network application and has no macOS-specific exclusion. Native macOS tunnel verification remains untested. |
| Voice calls | Partial / untested | `voice-calls` is available in the manifest and uses CoreAudio through `cpal`, but the macOS release artifact does not enable it. No native macOS microphone/output round trip was run here. |
| Video calls | Partial / untested | `video-calls` is available and documents AVFoundation support, but it is not enabled by the macOS release matrix. No native macOS camera/codec verification was run here. |
| Video playback | Partial | The macOS release target deliberately enables `gui` only, so inline `video-playback` is not shipped. The non-inline downloaded-file path remains available; native playback verification was not run. |
| Terminal | Not release-enabled / untested | The terminal is an opt-in `terminal` feature and is absent from the macOS release matrix. No macOS terminal runtime verification was run. |
| Native screen sharing | **Unavailable; Experimental/unsupported on macOS** | `src/screen_share/platform/macos.rs` is a one-line placeholder. The macOS `ActiveCapture` has only `TestPattern`, reports `macos-test-pattern`, and `create_capture_source` falls back to a 640x360 synthetic frame (`src/screen_share/platform/mod.rs:45-49,146-228`). There is no ScreenCaptureKit implementation, display/window enumeration, or Screen Recording permission handling. |

"Supported" above means the feature is included in the release configuration
and is not excluded by a macOS-specific source gate. It does not replace a
native macOS smoke test.

## Screen sharing decision

Do not advertise native macOS screen sharing. Mark it **Experimental/unsupported**
until a macOS worker or hardware run demonstrates all of the following:

1. ScreenCaptureKit display/window enumeration.
2. User consent and Screen Recording denial reported as an actionable status,
   without a panic or leaked capture task.
3. Frame capture and clean stop/shutdown on an actual macOS desktop.
4. Integration through the existing `ActiveCapture` abstraction without changes
   to Linux or Windows behavior.

A ScreenCaptureKit prototype was not added in this task. The current host is
Linux, the configured remote build host lacks the `aarch64-apple-darwin` target,
and adding native API bindings would require a dependency/Cargo change that
this audit explicitly excludes.

## Exact evidence

Observed from the isolated worktree at the baseline commit:

- `src/screen_share/platform/macos.rs` contains only the placeholder module comment.
- `src/screen_share/platform/mod.rs` defines the macOS backend as `ActiveCapture::TestPattern` and the non-Linux factory constructs the synthetic 640x360 capture; only the Windows branch attempts a native backend.
- `.github/workflows/release.yaml` defines macOS arm64 as `features: gui`, while Windows explicitly enables `screen-sharing`.
- `Cargo.toml` defines `video-playback`, `terminal`, `voice-calls`, `video-calls`, and `screen-sharing` as independent opt-in features; none are implied by `gui`.
- `rb check --target aarch64-apple-darwin --bin boru --features gui` failed before compiling Boru because the remote toolchain lacks the target (`error[E0463]: can't find crate for core`).

This is an evidence record, not proof of a macOS build passing or failing.
