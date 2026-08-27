# BORU-NEXT-04 platform verification gates

Date: 2026-08-27
Source commit: `8075f85e48c81199bb4d85a2782f69b2efec1e25`
Required parent: `origin/wt/boru-next-3` (integrated before verification)
Decision: **Windows and macOS native runtime gates remain untested**

This record is fail-closed. It distinguishes repository and cross-build checks
from native runtime checks, and does not claim a Windows or macOS result without
the corresponding host and packaged artifact.

## Windows

### Packaged-artifact gate

**NOT RUN — no Windows packaged candidate was available in this worktree or from
the accessible release history.** There is no `.exe` or Windows `.zip` under the
repository `artifacts/` tree. The only current packaged candidate recorded by
the parent workstream is the Linux GUI artifact in
`docs/boru-next-03-gui-smoke.md` and `artifacts/seeded-peer-manifest.json`.
Consequently, GUI startup, chat/discovery round trips, screen capture, terminal,
audio, and video-call behavior were not asserted for Windows.

The intended Windows package remains the workflow-defined artifact assembled by
`scripts/package_windows.sh`: `boru.exe`, Papirus/Twemoji assets, reviewed
GStreamer runtime files and notices, and toolchain runtime DLLs. Package
assembly was not run locally because it requires a real Windows executable and
the pinned Windows GStreamer runtime tree; no substitute fixture was treated as
a release artifact.

### Host availability

The documented Windows test host (`172.16.0.17`) was checked from this Linux
runner on 2026-08-27:

| Check | Result |
|---|---|
| `ping -c 2 -W 3 172.16.0.17` | FAIL — 100% packet loss; destination host unreachable |
| `ssh -o BatchMode=yes -o ConnectTimeout=5 dan@172.16.0.17 ver` | FAIL — `No route to host` |

No credential prompt was attempted and no credentials or private runtime data
were recorded. The unreachable host is an environment limitation, not a Boru
runtime failure.

### Repository/platform checks

| Check | Result | Classification |
|---|---|---|
| `python3 scripts/check-release-feature-matrix.py` | PASS (`release feature matrix: OK`) | Repository configuration |
| `bash -n scripts/package_windows.sh scripts/package-windows.sh scripts/release-sign.sh scripts/release-checksums.sh` | PASS | Repository scripts |
| `python3 -m py_compile scripts/check-release-feature-matrix.py scripts/release-validate.py scripts/gst_windows_manifest.py` | PASS | Repository scripts |
| `rb check --target x86_64-pc-windows-gnu --bin boru --features gui,terminal,voice-calls,video-calls,screen-sharing` | FAIL — DEBSRV lacks `x86_64-w64-mingw32-g++` while building `tracy-client-sys` | Build-host toolchain limitation |
| `rb check --target x86_64-pc-windows-msvc --bin boru --features gui,terminal,voice-calls,video-calls,screen-sharing` | FAIL — DEBSRV lacks MSVC `lib.exe` and MSVC zlib environment | Build-host toolchain limitation |

The release workflow's authoritative Windows target and features remain
`x86_64-pc-windows-msvc` with `gui,terminal,voice-calls,video-calls,screen-sharing`;
`video-playback` is intentionally disabled. The failed Linux-host cross-checks
therefore do not become Windows product failures, and they do not replace a
Windows CI/native build.

## macOS arm64

**UNTESTED from this Linux environment.** The repository declares
`aarch64-apple-darwin` with the `gui` release feature set, but no native macOS
binary, desktop, peer, file-transfer, or screen-sharing run was available.
The remote DEBSRV toolchain does not provide the target: the prior target check
fails before Boru compilation with `error[E0463]: can't find crate for core`.
The local Rust target inventory includes `aarch64-apple-darwin`, but that only
establishes target metadata availability; it is not native macOS runtime
verification.

### Native screen-capture boundary

Native macOS screen sharing is **Experimental/unsupported and must not be
advertised**. `src/screen_share/platform/macos.rs` is a placeholder; the macOS
path in `src/screen_share/platform/mod.rs` uses `ActiveCapture::TestPattern` and
the synthetic 640x360 fallback. There is no ScreenCaptureKit implementation,
display/window enumeration, or Screen Recording permission flow in this
candidate. This is a product capability boundary, separate from the inability
to run macOS on the current Linux host.

The detailed capability decision is recorded in
`docs/macos-capability-decision.md`. It marks core GUI/network/file/tunnel
support as configuration-level support only, with native macOS verification
still untested, and keeps optional terminal/media features outside the macOS
release matrix.

## Gate disposition

- Linux packaged GUI and seeded-peer smoke evidence is inherited from the parent
  workstream; it is not Windows or macOS evidence.
- Windows native runtime: **UNTESTED / blocked by unavailable packaged artifact
  and unreachable host**.
- macOS arm64 native runtime: **UNTESTED / blocked by no native macOS runner**.
- macOS native screen sharing: **EXPERIMENTAL/UNSUPPORTED**, test-pattern-only;
  do not advertise it as native capture.
- No credentials, private keys, tickets, room secrets, or message bodies were
  added to the repository or this evidence record.
