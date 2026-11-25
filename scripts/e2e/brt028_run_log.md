# BRT-028 integrated suite verification log

Date: 2026-11-26 (DEBSRV, AEDT). Verified driver SHA-256: `38d61be971043f8e37b4fdcddad7a944eb8c9aba81fbadc43d96aa1130954b0b`.

## Verification command

Executed sequentially on DEBSRV; these three runs used distinct disposable profile/artifact directories and the approved no-relay mode:

```sh
xvfb-run -a python3 brt028_driver.py --artifacts-dir fixed-run-N --profile-dir fixed-profile-N --peer-relay-mode no-relay
```

The driver uses the approved SQLite seed fixture for `BRT-028 Room` and shared topic `9021bd1ed0932e4fb1dfd5477ebee17916eb431c892d44f16287962329eaf303`. Each `steps.jsonl` was parsed as six independent JSON records; all six groups passed and every record's screenshot exists.

| Run | Exit | Observed wall time | Groups 1–6 |
|---|---:|---:|---|
| fixed-run-1 | 0 | 131.221 s | PASS, PASS, PASS, PASS, PASS, PASS |
| fixed-run-2 | 0 | 118.997 s | PASS, PASS, PASS, PASS, PASS, PASS |
| fixed-run-3 | 0 | 94.760 s | PASS, PASS, PASS, PASS, PASS, PASS |

Artifacts, console output, six valid JSONL records, and per-group screenshots are on DEBSRV under `~/brt028-verify/fixed-run-{1,2,3}`. Each profile is under the corresponding `~/brt028-verify/fixed-profile-N`.

## Instrumentation defect fixed

The earlier recorded runs in this file were not counted toward stability: `Driver.assertion` appended the literal characters `\\n` instead of a newline, making `steps.jsonl` invalid as JSON Lines. Fixed `scripts/e2e/brt028_driver.py` to append a real newline; the three runs above validate the output by parsing each record. No group logic or wait behavior needed changes.

## Negative-path check

Created a temporary separate copy `brt028_driver_negative.py` on DEBSRV that replaces only group 2's call with an intentional `AssertionError("intentional negative-path probe")`. It exited 1, emitted the assertion diagnostic, and logged group 2 as failed (`~/brt028-verify/negative-fixed/`). The canonical driver was not modified for injection and its SHA-256 remained `38d61be971043f8e37b4fdcddad7a944eb8c9aba81fbadc43d96aa1130954b0b`. The failure run also emitted a teardown/readiness failure record after group 2; the suite still correctly returned nonzero.

## Sleep / wait audit and remaining harness limitations

`grep -rn 'time.sleep' scripts/e2e/brt028_driver.py` reports two occurrences: line 83 is the polling interval (`0.2s`) inside `wait_until`; line 498 is a bounded `0.25s` backoff only on Iced-peer MCP `Rate limit exceeded`. Neither is a fixed synchronization sleep outside the approved polling/backoff use. Polling is for asynchronous app/WebDriver state and readiness; explicit `wait_until` limits include `STARTUP_TIMEOUT=60s`, desktop UDP port discovery `30s`, message/delivery observations `30s` (where applicable), peer reply `45s`, and unread state `60s`, allowing process startup and gossip/UI state propagation while retaining failure diagnostics.

Harness limitations: peer reply is triggered through loopback MCP GUI-test actions, not physical keyboard input; closing the room is implemented as webview reload because no deselect affordance exists. Unresolved adjacent issue (tracked by `t_89821351`): `src/app_snapshot.rs:67` currently projects `participant_count: None`, so the desktop header renderer at `desktop/src/main.tsx:131` displays `participants` with no count even when the peer is joined. Repro: run a full suite and inspect the BRT-028 Room header after joining the peer; it shows `participants` without a number.
