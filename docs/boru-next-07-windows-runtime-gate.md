# BORU-NEXT-07 Windows packaged runtime gate

Date: 2026-08-27
Source commit: `57411ca44750bcb8c6d0d1eb3c19f86bcb189ce7`
Required parent: `origin/wt/t_bb50ecf2` (`57411ca44750bcb8c6d0d1eb3c19f86bcb189ce7`)
Decision: **UNTESTED / BLOCKED — no packaged Windows candidate and no reachable Windows host**

This record is fail-closed. Cross-compilation and packaging prerequisites are
reported separately from native Windows runtime behavior. No Windows capability
is promoted to a runtime pass based on source inspection or Linux execution.

## Candidate identity

| Item | Result |
|---|---|
| Expected release target | `x86_64-pc-windows-msvc` |
| Expected release features | `gui,terminal,voice-calls,video-calls,screen-sharing` |
| Intentionally excluded feature | `video-playback` (per `docs/release-feature-matrix.toml`) |
| Windows artifact expected from workflow | `boru-windows-x86_64.zip` containing `boru.exe`, Papirus/Twemoji assets, reviewed GStreamer runtime, notices, and target runtime DLLs |
| Candidate available in worktree | **NONE** — no Windows `.exe` or `.zip` under `artifacts/`; no `target/x86_64-pc-windows-gnu/debug/boru.exe` was produced |
| Candidate checksum / size | **NOT APPLICABLE** — no candidate exists |

The repository's release workflow is the authoritative MSVC matrix, while the
local helper `scripts/package-windows.sh` builds a GNU debug candidate and is
not equivalent to the official MSVC release artifact. Neither candidate was
available after the checks below.

## Commands and results

| Command | Result | Classification |
|---|---|---|
| `python3 scripts/check-release-feature-matrix.py` | PASS — `release feature matrix: OK` | Repository configuration |
| `bash -n scripts/package_windows.sh scripts/package-windows.sh scripts/release-sign.sh scripts/release-checksums.sh` | PASS | Repository scripts |
| `python3 -m py_compile scripts/check-release-feature-matrix.py scripts/release-validate.py scripts/gst_windows_manifest.py` | PASS | Repository scripts |
| `RB_SLOTS=8 rb check --target x86_64-pc-windows-gnu --bin boru --features gui` | FAIL before Boru compilation: `tracy-client-sys` could not execute `sccache x86_64-w64-mingw32-g++`; DEBSRV cross-build environment did not provide a usable target C++ compiler invocation | Build-host limitation |
| Local `cargo build --target x86_64-pc-windows-gnu --features gui --bin boru` with explicit POSIX MinGW compiler | FAIL at final link: `x86_64-w64-mingw32-ld: error: export ordinal too large: 189414` while linking `iced_aw` | Linux cross-linker limitation; not a Windows runtime result |
| `./scripts/package-windows.sh` with explicit POSIX MinGW compiler environment | FAIL because the build step failed; no package was assembled | Packaging blocked by build failure |
| `ping -c 2 -W 3 172.16.0.17` | FAIL — 100% packet loss; `Destination Host Unreachable` from `172.16.0.190` | Environment limitation |
| `ssh -o BatchMode=yes -o ConnectTimeout=5 dan@172.16.0.17 ver` | FAIL — `No route to host` | Environment limitation |
| `wine --version` | PASS — Wine 9.0 is installed, but no Windows executable was available to launch | Host capability only; no runtime evidence |

The local build was attempted only after the preferred remote check showed the
remote compiler limitation, and it used the repository's installed
`x86_64-pc-windows-gnu` target plus POSIX MinGW tools. The resulting linker
failure is not evidence that the Windows application itself fails.

## Runtime and artifact-integrity gate

* GUI startup: **NOT RUN** — no Windows executable and no Windows desktop host.
* Core chat/discovery behavior: **NOT RUN** — no Windows executable or peer
  fixture was available. Linux seeded-peer evidence is not Windows evidence.
* Feature-matrix behavior: **CONFIGURATION PASS ONLY** — the matrix validator
  passed; native feature behavior was not exercised.
* Package layout and notices: **NOT RUN** — `scripts/package_windows.sh` could
  not reach staging because no executable was produced; the MSVC GStreamer
  runtime tree was not available locally.
* Checksum / artifact integrity: **NOT RUN** — there is no candidate archive to
  hash or inspect.
* Screen sharing, terminal, voice calls, and video calls: **NOT RUN** on
  Windows. `video-playback` remains intentionally excluded by the declared
  release matrix; this is a product configuration decision, not a runtime test.

## Disposition and exact unblocker

Windows runtime acceptance remains **UNTESTED**, not PASS or FAIL. A subsequent
run needs either:

1. the GitHub Actions `windows-latest` artifact for the current source commit
   (the workflow's MSVC target and declared feature set), plus an accessible
   Windows desktop session for launch and interaction; or
2. a reachable Windows test host at `172.16.0.17` and a verified
   `boru-windows-x86_64.zip` candidate.

No credentials, private keys, room secrets, tickets, or message bodies were
recorded. The failure modes above are platform/toolchain availability issues
and are not repository runtime failures.
