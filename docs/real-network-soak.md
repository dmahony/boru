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

Before any node is launched, the controller runs a fail-fast capability
preflight. It verifies that the binary is executable and advertises the flags
needed by the selected profile, the run directory is writable and empty, the
node count is 3–8, all per-node MCP and bind/control ports are free, `DISPLAY`
is reachable (or Xvfb has been started by the operator), and Linux procfs
metrics (`/proc/self/status` and `/proc/self/fd`) are available. Use the same
checks without starting a run:

```sh
DISPLAY=:310 python3 scripts/soak_harness.py \
  --scenario no-dht --no-mcp --run-dir artifacts/preflight \
  --preflight-only
```

The command exits 0 only when every prerequisite passes and exits 2 with
actionable `errors` before the long-duration timer can start. The controller
does not install packages, start Xvfb, reserve ports, create VPNs/namespaces,
or supply relay/DHT credentials; those remain operator/environment
responsibilities.

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

## Topology profiles and responsibilities

`--scenario` records the intended topology and applies only safe Boru flags.
The controller owns process lifecycle, isolated data directories, MCP/control
port assignment, sampling, and cleanup. The operator owns network placement,
firewall/NAT/VPN setup, relay reachability, Xvfb/display setup, and any
credentials or private tickets. No profile silently changes host networking.

| Scenario | Controller behavior | Environment requirement |
|---|---|---|
| `relay-only` | `--no-dht`; relay transport remains enabled | reachable relay service; DHT is intentionally unavailable |
| `same-lan` | `--no-dht --no-relay`; direct LAN/mDNS only | same broadcast domain and working mDNS; no relay or DHT path |
| `separate-network` | `--no-dht`; relay transport remains enabled | separate VMs, VPN, or routed networks; operator provides isolation and relay reachability |
| `no-dht` | `--no-dht --no-relay` | explicit discovery-degradation/isolation profile; only direct/LAN or supplied peers |

The binary must expose the expected `--no-dht`, `--no-relay`, `--mcp`, and
`--mcp-bind` capabilities for the selected profile. A profile name is not
proof that a relay or DHT backend is reachable: external relay/DHT behavior
must be recorded as an operator check next to `report.json`. No committed
relay URL, DHT credential, ticket, or secret is required by the controller.

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

Unsupported capabilities and topology limitations are listed in
`limitations`; for example, separate-network runs state that the controller
cannot create a VPN or namespace, and room/file/call actions without a fixture
are recorded as unsupported rather than guessed. A preflight failure occurs
before `report.json` is created, so the command's stderr/JSON error list is the
authoritative prerequisite record; do not treat a missing report as a pass.

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
