# Boru roadmap baseline (0.1)

This is the repository-grounded baseline for the roadmap work. Results below were
observed from worktree `wt/t_705cfec3` at commit `3e0e6ca7` on 2026-08-20
(UTC). A result is called pre-existing when the command ran against the
unchanged baseline and the failure is outside this baseline documentation and
smoke-test change.

## Toolchain and targets

- Package: `boru-core`, version `0.224.4`, Rust edition 2021, MSRV `1.91`.
- Canonical binary: `boru` at `src/bin/boru/main.rs`; it requires the `gui`
  feature and is the default run target. `sim` is a separate binary requiring
  `simulator`.
- Host observed by `rustc -vV`: `x86_64-unknown-linux-gnu`, Rust `1.97.1`,
  LLVM `22.1.6`; Cargo `1.97.1`; active toolchain
  `stable-x86_64-unknown-linux-gnu`.
- Installed Rust targets observed: `aarch64-apple-darwin`,
  `aarch64-linux-android`, `aarch64-pc-windows-gnullvm`,
  `aarch64-unknown-linux-gnu`, `aarch64-unknown-linux-musl`,
  `armv7-linux-androideabi`, `i686-unknown-linux-gnu`,
  `x86_64-pc-windows-gnu`, `x86_64-pc-windows-gnullvm`,
  `x86_64-unknown-freebsd`, and `x86_64-unknown-linux-gnu`.
- Product documentation and packaging cover Linux, macOS, and Windows. The
  Windows packaging script targets `x86_64-pc-windows-gnu` with the `gui`
  feature; native macOS/Windows runtime verification is not possible on this
  Linux host.

Exact inventory commands:

```sh
rustc --version
cargo --version
rustup show active-toolchain
rustup target list --installed
rustc -vV
cargo metadata --no-deps --format-version 1
```

`cargo metadata --no-deps --format-version 1` succeeded and reported one
workspace package (`boru-core`), with the manifest's explicit feature and target
inventory. `Cargo.toml` is authoritative for feature gates and `[[test]]`
`required-features`.

## Features and reusable test infrastructure

The default feature set is `net`, `metrics`, `gui`, and `wgpu-renderer`.
Important independent gates are:

- `net`: Iroh/QUIC, blobs, discovery, Tokio, and network protocol code.
- `gui`: `net` plus Iced and desktop application dependencies.
- `video-playback` and `terminal`: opt-in GUI extensions.
- `voice-calls`, `video-calls`, and `screen-sharing`: optional native media or
  capture stacks; not part of the default set.
- `test-utils`: deterministic test helpers and fixture dependencies.
- `simulator`, `dev-ui`, and `experimental-vnc`: developer/simulation gates.

Reusable helpers already present:

- `tests/support/peers.rs`: peer and node fixtures.
- `tests/support/net.rs`: endpoint/relay setup.
- `tests/support/storage.rs`: temporary storage paths.
- `tests/support/timeout.rs`: bounded polling (`wait_until`) with contextual
  timeout errors; default 30 seconds, 100 ms polling.
- `tests/support/wait.rs`: event/message wait helpers.
- `tests/support/fault.rs`: fault-injection/restart support.
- `tempfile::TempDir` appears throughout integration tests; storage tests also
  use unique paths under the process temporary directory.
- `scripts/boru-test-instance.sh`: lifecycle supervisor for headless Xvfb or
  desktop instances. It redirects per-instance logs to `<data_dir>/instance.log`
  and owns cleanup of its Xvfb child.

Logging uses `tracing` and the GUI binary's `init_logging` path in
`src/bin/boru/main.rs`. `RUST_LOG` overrides the default `EnvFilter`; logs are
written to the selected data directory and terminal filters are separately
configured. Avoid putting message bodies, keys, raw addresses, or file contents
in baseline evidence.

## Canonical verification flow

Use `rb` for all compile/test work so Cargo runs on DEBSRV. Run commands from the
repository or the target worktree; do not run long Cargo builds locally.

```sh
# formatting (local, no compilation)
cargo fmt --all -- --check

# normal development compile gate
rb check --bin boru --features gui,video-playback,terminal

# library unit tests
rb test --lib

# a targeted integration test (choose a non-relay-dependent test)
rb test --test test_simple

# optional feature-gated target checks
rb check --all-targets --features gui,video-playback,terminal
```

For broad integration coverage, run one test target per invocation with a
bounded timeout (240 seconds) and record each target separately. Several relay
suites can wait indefinitely on IPv6-first relay resolution on DEBSRV; do not
classify a timeout as a product failure without recording the environment and
suite name.

## Baseline results

| Command | Observed result |
| --- | --- |
| `cargo fmt --all -- --check` | **FAIL (pre-existing formatting drift)**; rustfmt reported diffs in existing files including `benches/compression_bench.rs`, `src/bin/boru/app/calls.rs`, `src/bin/boru/app/chat.rs`, and many existing tests. No formatter changes were applied. |
| `rb check --bin boru --features gui,video-playback,terminal` | **PASS**; finished in 36.05 seconds with 328 existing binary warnings and 5 existing library warnings. |
| `rb test --lib` | **FAIL (pre-existing)**; 2,839 passed, 4 failed, 2 ignored out of 2,845. Failures: `chat_core::tests::old_wire_format_file_share_decodes_correctly`, `chat_core::tests::old_wire_format_file_share_decodes_to_single_file_defaults`, `chat_core::tests::old_wire_format_presence_decodes_correctly` (all `DeserializeUnexpectedEnd`), and `storage::tests::docs_reference_current_schema_version` (docs omit `CURRENT_SCHEMA_VERSION: u32 = 26`). |
| `bash -n scripts/boru-test-instance.sh scripts/remote-test.sh scripts/discovery_matrix_run.sh` | **PASS**. |

The four unit failures are baseline failures: this task changed neither the
wire decoder nor `docs/message-storage-design.md`. The compile gate passed with
warnings; warning cleanup is outside this task.

## Clean-start / clean-shutdown smoke test

`scripts/clean-exit-smoke.sh` is the minimal automated lifecycle check. It uses
the existing `boru-test-instance.sh run` path, creates an isolated temporary data
directory, starts the GUI under Xvfb with MCP bound to an ephemeral loopback
port, requires the process to remain alive for five seconds, sends SIGTERM, and
requires the runner to exit within five more seconds. The script removes its
temporary directory on every exit path and prints a single PASS line.

Run it after obtaining a GUI binary:

```sh
rb build --bin boru --features gui,video-playback,terminal
# Copy the DEBSRV debug binary to target/debug/boru, or pass its local path:
./scripts/clean-exit-smoke.sh /path/to/boru
```

Prerequisites are an executable GUI-enabled `boru` binary and `xvfb-run`. A
missing binary or Xvfb is an explicit setup error (exit 2), not a test pass.
The smoke test does not use a relay or peer, so it is a startup/lifecycle check,
not a network integration test.
