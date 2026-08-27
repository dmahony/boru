# BORU-NEXT-10 final release audit and provenance gate

Date: 2026-08-27
Audit worktree base: `1b211d419fd43537f20272a16f15175d68357f1c`
Required parent: `origin/wt/boru-next-5` at `dcd8968f661908534b39907146a262033cd9cf68`
Decision: **NOT RELEASE-READY**

This is a fail-closed audit of BORU-NEXT-06 through BORU-NEXT-09 and the
inherited Linux, routing, room, visible-UI, and soak evidence. The audit does
not publish, tag, sign, attest, or merge a release.

## Gate disposition

| Gate | Result | Evidence and boundary |
|---|---|---|
| BORU-NEXT-06 repository quality baseline | **FAIL / release-owner disposition required** | `docs/boru-next-06-quality-baseline.md`: `cargo fmt --all --check` fails on pre-existing repository-wide drift; strict `RB_SLOTS=8 rb clippy --workspace --all-features -- -D warnings` fails on the recorded 260-warning/error baseline. No blanket waiver is granted. |
| BORU-NEXT-07 Windows packaged runtime | **BLOCKED / untested** | `docs/boru-next-07-windows-runtime-gate.md`: no packaged Windows candidate and `172.16.0.17` is unreachable; GNU/MSVC cross-toolchain checks fail before a Boru artifact is produced. No Windows runtime claim is made. |
| BORU-NEXT-08 macOS arm64 native runtime | **BLOCKED / untested** | `docs/boru-next-08-macos-arm64-gate.md`: this and DEBSRV are Linux hosts, no macOS arm64 artifact is available, and the target standard library is unavailable. Native ScreenCaptureKit is absent; screen sharing remains experimental/unsupported and test-pattern-only. |
| BORU-NEXT-09 conversation deletion persistence | **PASS (focused round trip)** | The SQLite regression `deleting_conversation_survives_sqlite_reopen` passed. It covers seed → delete → save → load/reopen, confirms the deleted topic stays absent, and preserves an unrelated topic. The GUI check also passed. |
| Linux packaged release | **PASS with warnings; publication blocked** | `docs/release-gate-report-0.227.1.md`: DEBSRV release build, package assembly, executable smoke, SPDX SBOM, validators, checksums, local provenance-subject validation, and bounded no-DHT soak passed. The build emitted the documented existing warning baseline. External attestation was not run. |
| Routing and direct-address privacy | **PASS (focused)** | `tests/test_discovery_dm_isolation.rs` and the inherited BORU-NEXT-05 synthesis cover bidirectional direct-topic isolation and discovery-topic separation. |
| Friendship/direct-conversation boundary | **PASS (focused)** | The same discovery fixture covers the direct-conversation subscription boundary; this is not a claim of a full native GUI friendship workflow. |
| Room membership | **PASS** | Inherited seeded-peer evidence records both nodes joining the explicit room and reciprocal mesh membership. |
| Visible Linux UI behavior | **PASS** | Inherited seeded-peer evidence verifies visible X11 windows, room navigation, composer interaction, bidirectional GUI messages, and a visible file-share/download round trip. Calls and screen sharing were unavailable in that fixture. |
| Bounded real-process soak / `--no-dht` | **PASS** | Inherited Linux release evidence records three isolated release processes, no-DHT startup for the bounded interval, clean shutdown, and no failures; the soak harness self-test also passed. |
| External provenance attestation/publication | **BLOCKED / human-gated** | GitHub Actions attestation and any signing/publication require release-owner approval and a publication context. They were intentionally not executed. |

## Focused verification in this audit

The following checks were run from this worktree:

- `RB_SLOTS=8 rb check --bin boru --features gui,video-playback,terminal` — **PASS**; existing warnings remain.
- `RB_SLOTS=8 rb test --lib --features gui,video-playback,terminal -- deleting_conversation_survives_sqlite_reopen` — **PASS**, 1 passed.
- `python3 scripts/check-release-feature-matrix.py` — **PASS** (`release feature matrix: OK`).
- Python compilation of release and soak helpers — **PASS**.
- `bash -n` for Windows packaging, signing, and checksum scripts — **PASS**.
- `git diff --check` — **PASS**.
- `git merge-base --is-ancestor origin/wt/boru-next-5 HEAD` — **PASS**; the required remote parent is an ancestor of this audit worktree.

The repository-wide format and strict-clippy results are inherited, independently
recorded baseline failures; this audit does not reformat or refactor unrelated
code.

## Release boundary

Technical evidence is sufficient to preserve the recorded Linux and focused
regression results, but not to approve release readiness. Before publication,
the release owner must explicitly disposition the formatting/clippy baseline,
obtain Windows and macOS gates (or approve a documented product/platform
waiver), and authorize the external attestation/signing/publication step. No
credentials, private keys, room secrets, tickets, or message bodies are stored
in this report or its provenance inputs.
