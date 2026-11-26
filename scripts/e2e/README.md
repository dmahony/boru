# BRT-028 two-peer launcher

`brt028_two_peer.sh` starts one Tauri desktop process and one separate Iced/CLI
`boru` process for use by the automated two-process E2E tests. It prints one
JSON descriptor to stdout once both processes satisfy the readiness contract.
The two MCP/network listen ports are chosen dynamically from free localhost
ports; the `port` fields in the descriptor are the selected ports (the peer's
MCP bind port and a reserved dynamic Tauri descriptor port, respectively).

## Usage

From the repository root, build both binaries and run:

```sh
# Tauri desktop (its own workspace root; no `gui` feature exists on this crate)
cargo build --bin boru-desktop --manifest-path desktop/src-tauri/Cargo.toml
# Iced/CLI peer
cargo build --features gui --bin boru
scripts/e2e/brt028_two_peer.sh
```

Override paths when using binaries built elsewhere:

```sh
TAURI_BIN=/path/to/boru-desktop PEER_BIN=/path/to/boru scripts/e2e/brt028_two_peer.sh
```

The script's environment knobs are:

- `TAURI_BIN`: Tauri executable; default `desktop/src-tauri/target/debug/boru-desktop`.
- `PEER_BIN`: Iced/CLI executable; default `target/debug/boru`.
- `STARTUP_TIMEOUT`: maximum seconds for each process to become ready; default `60`.
- `ARTIFACT_DIR`: optional directory to receive copies of both logs after shutdown. Without it, the temporary run directory is removed.

Each run uses `${TMPDIR:-/tmp}/brt028-<runid>`. Tauri receives private
`XDG_DATA_HOME` and `XDG_CONFIG_HOME`, and its profile is under
`XDG_DATA_HOME/computer.iroh.boru.desktop`. The Iced peer receives its own
`--data-dir`. The script does not use or modify the user's Boru profile.

## Readiness contract and limitation

Readiness means the process is still alive, its disposable profile directory
exists, and there is a runtime-start marker: Tauri's profile database (`boru.db`)
has been created by `DesktopRuntime::start_from_config`, or the Iced peer's
tracing log `<peer-data-dir>/logs/boru.log` contains
`MCP diagnostic server listening on ...`. The peer's stdout/stderr stay empty on
a healthy start, so the marker is read from the tracing file, never from
`$PEER_LOG`. The script waits by polling these conditions until
`STARTUP_TIMEOUT`; it does not infer readiness from elapsed time alone. On any
exit, both child process groups are terminated and the temporary run root is
removed; with `ARTIFACT_DIR`, logs are copied there first.

Readiness does **not** mean the peers have discovered or connected to each
other over gossip. This launcher does not assert message delivery; that
verification belongs to the automated two-process E2E task.

# BRT-028 assertion driver

`brt028_driver.py` drives the real Tauri/WebKit UI with WebDriver and the Iced
peer over its loopback MCP GUI-test surface, then records one JSON line per
assertion group in `<artifacts>/steps.jsonl`. It must run on the host that owns
the binaries (DEBSRV has `tauri-driver`, `WebKitWebDriver`, `xvfb-run`,
ImageMagick `import` and Selenium).

Exit contract: `0` only when all six groups pass; `1` for any assertion,
startup, teardown-adjacent failure, or `--run-timeout` expiry.

Happy path, unattended:

```sh
xvfb-run -a python3 scripts/e2e/brt028_driver.py \
    --artifacts-dir /home/dan/brt028-acceptance2/pass \
    --profile-dir /home/dan/brt028-acceptance2/profile-pass
```

Deterministic negative paths (no source edits, no reseeding):

```sh
# group 5: suppress the peer reply, so the unread-badge assertion must fail
xvfb-run -a python3 scripts/e2e/brt028_driver.py \
    --artifacts-dir /home/dan/brt028-acceptance2/g5 \
    --profile-dir /home/dan/brt028-acceptance2/profile-g5 \
    --inject-failure group-5

# group 6: clear persisted message rows, so restored history must be missing
xvfb-run -a python3 scripts/e2e/brt028_driver.py \
    --artifacts-dir /home/dan/brt028-acceptance2/g6 \
    --profile-dir /home/dan/brt028-acceptance2/profile-g6 \
    --inject-failure group-6
```

Each injected run writes `injection.json` describing exactly what was removed,
so an injected failure is never mistaken for a product regression. Both
negative paths exit `1` with the failing assertion quoted in `steps.jsonl`, and
teardown still leaves no `boru-desktop`, `WebKitWebDriver`, `tauri-driver` or
`Xvfb` process behind.

The peer MCP surface enforces 10 actions/sec, so `peer_mcp` retries
`Rate limit exceeded` errors six times with a 0.25 s backoff; every other error
propagates unchanged.

