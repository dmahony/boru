# D16 real-process lifecycle evidence

Date: 2026-09-21
Source: commit `0e2d5cd781fcbbed13d783e28c50f9b8638340e6`

This is a fail-closed execution record. It distinguishes real Boru process evidence from the deterministic fixture workflow and does not claim delivery behavior that was not observed.

## Environment and privacy settings

- Host: Linux 6.8.0-136-generic x86_64, Ubuntu host with X11 emulation via `xvfb-run`.
- Local interfaces observed: `wlo1` and `br0` on `172.16.0.0/24`; default route through `172.16.0.1`.
- Windows host/runtime: unavailable in this environment. Existing repository gate records Windows runtime as untested; no Windows result is inferred from Linux.
- Real process runs used fresh isolated `BORU_DATA_DIR`-equivalent `--data-dir` paths under `artifacts/`; no production profile was touched.
- Privacy configuration for the real smoke/restart runs was `--no-dht --no-relay`. The two-process MCP probe used the same disabled-DHT/disabled-relay settings and loopback-only MCP binds (`127.0.0.1:18765` and `127.0.0.1:18766`). No tickets, keys, message bodies, or public-address publication were recorded.
- The normal remote build wrapper was exercised first: `RB_SLOTS=8 /home/dan/bin/rb build --features gui --bin boru` passed on DEBSRV. A local debug binary was then built only to obtain an executable for this host's real-process run.

## Real-process evidence

### Bounded process soak

Command:

```text
xvfb-run -a python3 scripts/soak_harness.py --profile developer --scenario no-dht --duration-s 8 --interval-s 1 --no-mcp --binary target/debug/boru --run-dir artifacts/d16-real-smoke
```

Result: PASS. Three real Boru processes started, stayed live for all samples, wrote isolated SQLite/profile data, and were cleaned up. The report is at `artifacts/d16-real-smoke/report.json`; the redacted summary is `artifacts/d16-real-smoke/evidence.md`.

Observed resource assertions: RSS, threads, file descriptors, CPU ticks, read/write bytes, profile database bytes, and database bytes all passed. This is process/resource evidence, not message-delivery evidence.

### Real restart with the same data directories

Command:

```text
xvfb-run -a python3 scripts/soak_harness.py --profile developer --scenario no-dht --nodes 3 --duration-s 20 --interval-s 1 --fault restart --fault-every 5 --no-mcp --binary target/debug/boru --run-dir artifacts/d16-restart-real
```

Result: PASS. The event log records scheduled restarts with new PIDs while reusing the same node data directories, followed by successful readiness and final cleanup. Examples are recorded in `artifacts/d16-restart-real/events.jsonl`; node data is under `artifacts/d16-restart-real/nodes/node-{0,1,2}`.

This proves real process startup/restart/cleanup recovery, but no logical chat message was injected in this run, so it does not prove queued-message recovery.

### Two independently persisted MCP processes

Two real processes were started with independent data directories:

```text
artifacts/d16-two-process/a
artifacts/d16-two-process/b
```

They exposed MCP on loopback ports 18765 and 18766. `boru_ping`, `boru_get_node_status`, `boru_get_gui_snapshot`, `boru_get_outbox_status`, and `boru_get_message_store` all returned successfully for both processes. The snapshots showed GUI test actions enabled, zero active rooms, zero queued outbox rows, and zero message-store rows. Discovery observed the other node and began connection attempts, but the no-DHT/no-relay profile had no resolved addresses and no room fixture was available.

The processes were terminated after the probe; no listeners remained in the final process check.

## Fixture-only and unsupported coverage

`--workflow golden-recovery` is explicitly a bounded mock workflow in `scripts/soak_harness.py`, not a real network execution. It returned 15/15 PASS assertions, including bidirectional fixture messages, offline interruption, duplicate-free fixture recovery, transfer interruption/recovery, and leave/rejoin. Its report correctly retained the limitation `fixture mode; use the real Boru binary for network-backed execution`; it is not promoted to a real transport pass.

Not exercised here:

- queued direct message while recipient is unavailable, followed by sender SIGKILL and restart without resubmission;
- recipient kill after durable receive before ACK, retry after restart, and independent receiver-row/bubble/unread counts;
- genuine peer receipt driving `Delivered` in the GUI;
- LAN room traffic in both directions, separate-internet topology, forced relay, route/network switching, or relay interruption;
- inactive direct conversations and visible UI unread/bubble evidence;
- Linux sleep/resume or Windows sleep/resume/abrupt termination.

The acceptance criteria for logical message recovery, genuine receipts, exactly-once receiver presentation, and cross-platform lifecycle behavior therefore remain **UNVERIFIED**, rather than being inferred from process-soak or fixture results. A follow-up needs a reachable seeded-peer/room fixture or two operator-controlled hosts with normal GUI controls and a way to capture durable store plus GUI evidence around the kill boundaries.
