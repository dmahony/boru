# BRT-028 integrated suite verification log

Date: 2026-11-26 (DEBSRV, AEDT). Run from a fresh clone of `origin/main` at
commit `3f68cee` ("Merge branch 'fix/brt028-driver-injection' (group-6
diagnosability)"), driver `scripts/e2e/brt028_driver.py` SHA-256
`6fb07728011145895e4ba48ce7413444f39e7b02a6588756a9788834afc24785`
(46,062 bytes).

Binaries: `~/boru-build/brt028-target/debug/{boru,boru-desktop}` (debug build,
2026-11-26 07:35). Host prerequisites: `tauri-driver`, `WebKitWebDriver`,
`xvfb-run`, Selenium, ImageMagick `import`.

## Verification command

All three runs used the approved no-relay peer configuration and distinct
disposable profile/artifact directories:

```sh
xvfb-run -a python3 scripts/e2e/brt028_driver.py \
    --artifacts-dir <root>/<run> \
    --profile-dir <root>/profile-<run> \
    --tauri-bin ~/boru-build/brt028-target/debug/boru-desktop \
    --peer-bin  ~/boru-build/brt028-target/debug/boru \
    [--inject-failure group-5|group-6]
```

`<root>` was `~/brt028-main`. Each `steps.jsonl` parsed as independent JSON
records and every record's `screenshot_path` exists.

| Run | Invocation | Exit | Wall time | Groups 1–6 |
|---|---|---:|---:|---|
| `pass` | no injection | 0 | 132 s | PASS, PASS, PASS, PASS, PASS, PASS |
| `g5` | `--inject-failure group-5` | 1 | 143 s | PASS, PASS, PASS, PASS, FAIL |
| `g6` | `--inject-failure group-6` | 1 | 161 s | PASS, PASS, PASS, PASS, PASS, FAIL |

Exit contract holds: `0` only when all six groups pass, `1` for any assertion
failure. A post-run process check (5 s after the last exit) found no
`boru-desktop`, `WebKitWebDriver`, `tauri-driver`, `Xvfb` or driver process.

## Artifact inventory

Every run preserves the same inventory under its artifacts directory:

```
steps.jsonl                    one JSON record per assertion group
desktop.log                    Tauri desktop tracing output
peer.stdout-stderr.log          Iced peer process output
participants.json              observed room participant state
group5-live-update.json        group-5 live-update observation
tauri-driver.log               WebDriver bridge log
shots/<step-id>-<status>.png   one screenshot per executed group
injection.json                 negative runs only: what was removed
```

`~/brt028-main/{pass,g5,g6}` hold the runs above; profiles are under
`~/brt028-main/profile-{pass,g5,g6}`.

## Diagnosability from artifacts alone

Both negative runs are diagnosable without re-running the app:

- `g5` — `steps.jsonl` records
  `TimeoutError: Timed out waiting for unread badge for 'BRT-028 Room' after
  peer reply 'BRT028-REPLY-…-reply'; last observed value: False`, and
  `injection.json` records `peer reply suppressed; unread badge precondition
  removed`. Groups 1–4 are recorded pass, so the failure is isolated to the
  peer-reply / unread-state assertion.
- `g6` — `steps.jsonl` records
  `TimeoutError: Timed out waiting for restored message history contains exact
  inbound body 'BRT028-REPLY-…-restart'; observed history from .messages: 'No
  messages yet. Start the conversation.'`, and `injection.json` records
  `deleted 3 persisted message row(s)`. Groups 1–5 are recorded pass, so the
  failure is isolated to restart persistence.

The `injection.json` record makes an injected failure distinguishable from a
product regression in the artifact set itself.

## Instrumentation defects fixed (earlier runs, not counted)

- `Driver.assertion` appended the literal characters `\\n` instead of a
  newline, making `steps.jsonl` invalid JSON Lines; fixed, and every run above
  validates the file by parsing each record.
- The Iced peer's loopback MCP surface enforces 10 actions/sec. `peer_mcp` now
  retries `Rate limit exceeded` errors six times with a 0.25 s backoff; every
  other error propagates unchanged. Before this retry, group 5 failed
  intermittently on that limiter.

## Sleep / wait audit

`grep -rn 'time.sleep' scripts/e2e/brt028_driver.py` reports two occurrences:
line 83 is the `0.2 s` polling interval inside `wait_until`; line 498 is the
bounded `0.25 s` MCP rate-limit backoff. No fixed synchronization sleep exists
outside those approved uses. `wait_until` limits: `STARTUP_TIMEOUT=60 s`,
desktop UDP port discovery `30 s`, message/delivery observations `30 s`, peer
reply `45 s`, unread state `60 s`.

## Harness limitations

- The peer reply is triggered through the loopback MCP GUI-test path, not
  physical keyboard input.
- Room deselection is a webview reload because the desktop UI exposes no
  deselect affordance.
- Adjacent unresolved issue tracked by `t_89821351`: `src/app_snapshot.rs:67`
  projects `participant_count: None`, so `desktop/src/main.tsx:131` renders
  `participants` with no count even when the peer is joined.

## Operational note

`tauri-driver` binds fixed ports 4444/4445. A stale bridge from an aborted run
makes the next run fail group 1 with
`ProtocolError: Remote end closed connection without response` (its
`tauri-driver.log` shows `can not listen to address: 127.0.0.1:4444`). Clear
stale `tauri-driver`/`WebKitWebDriver` processes before starting a run.
