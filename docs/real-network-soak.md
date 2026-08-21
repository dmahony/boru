# Real-network soak and fault injection

`scripts/soak_harness.py` is the long-haul controller for Boru. It starts 3–8
real Boru processes, gives every node a fresh `BORU_DATA_DIR`, labels stdout in
`<run-dir>/nodes/node-N.log`, samples Linux process metrics, and writes a small
`report.json` plus append-only `events.jsonl`. It does not print message bodies,
addresses, tickets, or keys. Large logs stay in the run directory and should not
be committed.

## Prerequisites

Build a debug binary with the normal remote build wrapper, then run the
controller from the repository root:

```sh
rb build --bin boru --features gui,video-playback,terminal
```

A headless host needs the same X11/software-rendering setup as the existing
`fs23_launch.sh` harness. For real GUI actions, start the Boru binary with an
Xvfb display. The controller only requires the MCP server when actions or
status snapshots are desired; `--no-mcp` is useful for a process/bootstrap
smoke run.

## Bounded smoke run

This command launches three isolated nodes for two seconds, captures one sample,
then verifies process-group cleanup. It is deterministic and returns non-zero
if a node dies unexpectedly or a child remains alive:

```sh
python3 scripts/soak_harness.py \
  --profile developer --scenario no-dht --duration-s 2 --interval-s 1 \
  --no-mcp --run-dir artifacts/soak-smoke
```

The binary must exist at `target/debug/boru`, or provide `--binary`. Use the
controller self-test when validating the script without a Boru build:

```sh
python3 scripts/soak_harness.py --self-test
```

## Network scenarios

`--scenario` records the intended topology and applies only safe Boru flags:

| Scenario | Controller behavior | Environment requirement |
|---|---|---|
| `relay-only` | disables DHT; relay remains enabled | reachable relay service |
| `same-lan` | disables DHT and relay | same broadcast domain / mDNS |
| `separate-network` | disables DHT; relay remains enabled | separate VMs, VPN, or routed networks; controller does not create VPNs |
| `no-dht` | disables DHT and relay | explicit offline/discovery-degradation profile |

The controller cannot safely emulate a relay outage or change a VM's IP from
inside the Boru process. Those faults are represented in the report as
limitations; use the VM/VPN operator profile to perform them and preserve the
same run directory schema.

## Fault schedule and actions

Repeatable faults are selected with repeatable `--fault` flags. The seeded
schedule uses the sample count and `--fault-every` to select a node:

```sh
python3 scripts/soak_harness.py --profile developer --scenario relay-only \
  --fault burst --fault restart --fault offline --fault-every 4
```

- `restart`: terminate and relaunch one isolated node with the same profile.
- `offline`: briefly stop/resume a process group, modeling an offline period.
- `burst`: submit three labeled messages through the normal GUI composer when
  MCP is enabled.

Room create/join/leave/rejoin, file transfer, and call/screen-share actions are
not guessed by the controller because they require a room fixture and platform
permissions. They are recorded as `action_skipped` unless a future fixture adds
the corresponding MCP action. Existing `boru_run_gui_message_test` can be
used in a fixture-specific wrapper without changing the report format.

## Release-candidate profile

The documented release profile is 6 nodes for 8 hours by default; run it for
8–24 hours by overriding the duration:

```sh
python3 scripts/soak_harness.py --profile release-candidate \
  --scenario separate-network --duration-s 28800 \
  --fault restart --fault burst --fault offline \
  --run-dir /var/tmp/boru-soak/rc-$(date +%Y%m%d-%H%M%S)
```

For a 24-hour candidate use `--duration-s 86400`. Put nodes on distinct VM or
VPN profiles for `separate-network`; do not reuse production data directories.

## Pass/fail interpretation

`report.json` is the compact machine-readable result (`boru-soak-report/v1`):

```json
{
  "schema": "boru-soak-report/v1",
  "status": "PASS",
  "scenario": "no-dht",
  "profile": "developer",
  "nodes": 3,
  "faults": ["restart"],
  "event_count": 12,
  "failures": [],
  "cleanup_verified": true,
  "limitations": []
}
```

A run passes only when all expected nodes remain alive through the schedule,
no controller invariant fails, and `cleanup_verified` is true after teardown.
Inspect labeled logs and `events.jsonl` for a failed seed or fault. Samples
contain RSS, file-descriptor count, thread count, and profile DB byte size;
these are the bounded-memory/worker and storage-growth signals. MCP status is
captured when reachable, including diagnostics counters exposed by the running
build.

The report is intentionally not proof of application-level delivery. For a
release decision, additionally require: bidirectional normal chat after
reconnect, transfer recovery after termination during retry, online-lease
cleanup, DHT degradation behavior, bounded RSS/workers, and no orphan process.
Record each external VM/VPN check beside the report without adding secrets or
message content.

## Cleanup and evidence

The controller sends SIGTERM to each process group, waits five seconds, then
uses SIGKILL only as a last resort. It verifies every child has exited before
writing the final report. If the shell is interrupted, rerun the process check
and remove only the run's own processes/data:

```sh
pgrep -a -f 'boru.*artifacts/soak' || true
find artifacts/soak-smoke -maxdepth 2 -type f -printf '%p %s bytes\n'
```

Never put real user data, secret keys, tickets, or unredacted network payloads
in a report or commit.
