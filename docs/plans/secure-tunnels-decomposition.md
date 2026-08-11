# Boru Secure Tunnels — Implementation Decomposition

Source plan: `Boru Secure Tunnels Implementation Plan.pdf` (attached to kanban task `t_1bdbd90d`)
Repo: `/home/dan/iroh-gossip-chat` (https://github.com/dmahony/boru)
Decomposed by: planner (task `t_1bdbd90d`) on 2026-08-02 ~02:55 AEST
Execution: 28 kanban tasks (Phases 1–28) on board `iroh-gossip-chat`, parked in `scheduled`
status at creation and unblocked by cron job at **06:00 AEST 2026-08-02** so the dispatcher
starts the work at 6AM.

---

## Goal

Implement **Boru Secure Tunnels** natively inside Boru using its existing Iroh networking
infrastructure (single `iroh::Endpoint`, existing ALPN registration, friend/contact system,
address resolution, relay + NAT traversal, Tokio). Model the transport concepts on
n0-computer/dumbpipe, but do **not** add dumbpipe as a runtime dependency.

```
Local TCP application → Boru → encrypted Iroh QUIC stream → Boru → Remote TCP application
```

Example: Alice exposes `127.0.0.1:3000` to Bob; Bob connects through Boru and gets
`127.0.0.1:43827` on his machine pointing at Alice's dev server.

## Design invariants (apply to every phase)

1. Reuse Boru's existing Iroh endpoint — never a second networking stack.
2. Do not replace existing Boru protocols (gossip, DMs, offline inbox, backfill, files/blobs).
3. Do not expose arbitrary TCP destinations to remote peers (SSRF protection).
4. Tunnel targets are configured locally by the owner; capabilities are recipient-bound and expire.
5. Loopback-only exposure and loopback-only listeners are the default (never bind `0.0.0.0` by default).
6. No unlimited connection/task spawning; conservative limits.
7. Do not persist secrets unless absolutely necessary; never log capabilities/stream contents.
8. Every networking change requires tests; existing functionality must keep working after every stage.
9. Incremental, small, reviewable changes — no all-at-once rewrites.

## After every step (each worker must run)

1. `cargo fmt .`
2. Run the relevant existing tests.
3. `cargo check` for Boru Core.
4. `cargo check --features gui --bin boru`.
5. Fix regressions before proceeding.
6. Commit changes logically (the board's `default_workdir` is the repo).

---

## Task graph

| Phase | Task title | Assignee | Parents | Priority |
|---|---|---|---|---|
| 1  | Inspect & document existing networking architecture | boru-mcp | — | 100 |
| 2  | Dedicated tunnel protocol ALPN `/boru-tunnel/1` | boru-mcp | P1 | 99 |
| 3  | Protocol messages (authenticated handshake) | boru-mcp | P2 | 98 |
| 4  | Signed capability tokens | boru-mcp | P3 | 97 |
| 5  | TunnelService module | boru-mcp | P4 | 96 |
| 6  | Raw Iroh tunnel stream (QUIC bidi) | boru-mcp | P5 | 95 |
| 7  | Safe bidirectional forwarding | boru-mcp | P6 | 94 |
| 8  | Remote service sharing (loopback targets) | boru-mcp | P7 | 93 |
| 9  | Local tunnel listeners (auto port) | boru-mcp | P8 | 92 |
| 10 | Multiple TCP connections + concurrency limits | boru-mcp | P9 | 91 |
| 11 | Tunnel expiration & revocation | boru-mcp | P10 | 90 |
| 12 | Persistence decision (ephemeral tunnels) | boru-mcp | P11 | 89 |
| 13 | GUI: "Share local service" | coder-web | P12 | 88 |
| 14 | GUI: received shared services | coder-web | P13 | 87 |
| 15 | GUI: active tunnel management | coder-web | P14 | 86 |
| 16 | GUI: connection info (Direct/Relay/Unknown) | coder-web | P15 | 85 |
| 17 | Limits & abuse protection | boru-mcp | P10 | 84 |
| 18 | SSRF-style misuse protection | boru-mcp | P10 | 83 |
| 19 | Logging & privacy | boru-mcp | P12 | 82 |
| 20 | Network Doctor (reuse tunnel primitive) | boru-mcp | P12 | 81 |
| 21 | Optional CLI/debug interface | boru-mcp | P12 | 80 |
| 22 | Unix socket support (`#[cfg(unix)]`) | boru-mcp | P12 | 79 |
| 23 | Prepare for future use cases (doc) | planner | P12 | 78 |
| 24 | Testing (protocol/networking/security/resources) | reviewer | P12, P17, P18 | 77 |
| 25 | Documentation (README + docs) | planner | P24 | 76 |
| 26 | Final architecture review | reviewer | P24 | 75 |
| 27 | Performance review | reviewer | P24 | 74 |
| 28 | UX polish | design | P16, P24 | 73 |

Notes:

- Phases 1–12 form the strict backend chain (each depends on the previous).
- Phases 17/18 (hardening) depend only on Phase 10, so they can run while the GUI chain
  (13–16) is in flight; Phase 24 (testing) fans in on P12 + P17 + P18.
- Phases 19–23 depend on Phase 12 (backend milestone) and run in parallel.
- Final gates: Phase 25 (docs), 26 (architecture review), 27 (performance review) depend on
  Phase 24 (testing); Phase 28 (UX polish) depends on the GUI chain (P16) and the testing
  milestone (P24).

## Scheduling mechanism

- All 28 tasks were created with `initial_status=blocked` (prevents the dispatcher from
  claiming them), then moved to `scheduled` via `hermes kanban schedule <id>` — visible on
  the dashboard with a ⏱ marker, intentionally not dispatchable.
- Cron job `76a9a02dd45e` ("Unblock Boru Secure Tunnels tasks (6AM)", no_agent) fires at
  `2026-08-02T06:00:00` and runs `~/.hermes/scripts/unblock_boru_tunnels_6am.py`, which
  runs `hermes kanban unblock <all 28 ids>`; the dispatcher then promotes per the parent
  graph (Phase 1 → ready, the rest → todo until their parents complete).

## Source PDF location

`/home/dan/.hermes/kanban/boards/iroh-gossip-chat/attachments/t_1bdbd90d/Boru Secure Tunnels Implementation Plan.pdf`

Each task body references this path so the worker can read the full step text.
