# BORU-CP-15 — Developer networking health view (PDF Phase 5, Task 5.3)

## Objective

Provide one place to inspect the control plane and data plane
independently. A debug-only surface shows peer state and recent
transitions, with **separate indicators** for Discovery, Endpoint, Direct
Topic, Inbound Delivery, Outbound Delivery, and Path — plus a
**copy-diagnostics** output with stable labels so agent/debug sessions can
compare two machines side by side.

This is a **developer/debug surface only**. It is not wired into the chat
UI and the format is not stable for user-facing use yet (PDF Task 5.3
step 4).

## Surface

* `src/control_plane/health.rs` — the health row model, the two renderers
  (`render_health_view` human table, `render_copy_diagnostics` stable
  block), and the direct-topic probe helper (`probe_direct_topic`).
* `examples/doctor.rs` → `cargo run --example doctor -- health [...]` — the
  live command: boots a real node, joins the internal discovery topic,
  probes each discovered peer's deterministic direct topic, and prints the
  view.
* `tests/test_health_view.rs` — in-process two-node comparison proving the
  PDF 5.3 acceptance criteria.

## Command

```bash
# one machine (e.g. LAN test with relay)
cargo run --example doctor -- health --duration 30 --relay <url> --bootstrap <B-node-id>

# the other machine
cargo run --example doctor -- health --duration 30 --relay <url> --bootstrap <A-node-id>

# LAN-only (no relay): both machines pass --no-relay and bootstrap each other
cargo run --example doctor -- health --duration 30 --no-relay --bootstrap <B-node-id>

# print only the copy-diagnostics block (stable labels)
cargo run --example doctor -- health --copy --duration 30 --no-relay --bootstrap <B-node-id>
```

The command:

1. Loads/generates the node secret key from the data dir.
2. Boots an iroh endpoint + gossip actor (relay configurable via
   `--relay`/`--no-relay`) and joins the internal discovery topic through
   the BORU-CP discovery service.
3. Watches peer updates; for each newly discovered peer it subscribes to
   the **deterministic direct topic** and broadcasts a small health probe
   (unique per sender: `BORU-HEALTH-PROBE-V1 ‖ sender pubkey`). The probe
   is not a chat message — the chat decoder rejects it and it never renders
   as chat. It exists purely to produce real inbound/outbound delivery
   evidence in the connectivity store.
4. Prints the human health view + the copy-diagnostics block.

## Indicators

Per peer, the six PDF 5.3 indicators are separate and clearly labelled:

| Indicator | Copy label | Values |
|---|---|---|
| Discovery | `discovery=` | `seen-<elapsed>` / `never` |
| Endpoint | `endpoint=` | `connected-<elapsed>` / `connecting` / `failed` / `not-started` / `disconnected` |
| Direct Topic | `direct_topic=` | `ready` / `not_attempted` / `failed` |
| Inbound Delivery | `inbound=` | `ok-<elapsed>` / `never` |
| Outbound Delivery | `outbound=` | `ok-<elapsed>` / `never` |
| Path | `path=` | `direct` / `relay` / `transitioning` / `unknown` |

Recent transitions (the bounded connectivity trail) are shown per peer in
the human view (`transition from→to (event)`).

## Copy-diagnostics format (stable)

```
BORU-HEALTH-V1
node=<local-node-short> uptime=<secs>s peers=<n>
peer=<short> discovery=<...> endpoint=<...> direct_topic=<...> inbound=<...> outbound=<...> path=<...> state=<...>
```

* Header identifies the machine and format version.
* One line per peer, **sorted by peer id**, so two machines' blocks line up
  peer-for-peer.
* Discovery is separate from direct-message delivery; inbound and outbound
  are separate per peer, so an asymmetric A→B vs B→A failure is obvious.

## Acceptance criteria (PDF 5.3) — evidence

### 1. Two test machines can produce directly comparable diagnostic dumps

Two in-process nodes A and B, each running the same probe harness, produce
these blocks (captured from a real two-node run on debsrv, see
`evidence/cp15/health-dump-comparison.txt`):

```
===MACHINE A COPY===
BORU-HEALTH-V1
node=A uptime=30s peers=1
peer=c2ccb841a9 discovery=seen-314ms endpoint=connected-301ms direct_topic=ready inbound=ok-301ms outbound=ok-302ms path=unknown state=direct-topic-ready

===MACHINE B COPY===
BORU-HEALTH-V1
node=B uptime=30s peers=1
peer=c657b0e3c6 discovery=seen-316ms endpoint=connected-301ms direct_topic=ready inbound=ok-301ms outbound=ok-303ms path=unknown state=direct-topic-ready
```

Both blocks use the same version header and the same label set; the only
differences are the node id and the (time-relative) elapsed values. A
machine running the same command against the same peer set produces a
block that can be diffed line-by-line.

### 2. Output makes asymmetric A→B vs B→A failures obvious

The unit/integration tests build the failure case: A has sent (outbound
`ok-…`) but B never received anything from A (inbound `never`, direct
topic `not_attempted`). Rendered side by side:

```
# machine A's dump (its view of B)
peer=<B> discovery=seen-… endpoint=connected-… direct_topic=ready inbound=never outbound=ok-… path=…

# machine B's dump (its view of A)
peer=<A> discovery=seen-… endpoint=connected-… direct_topic=not_attempted inbound=never outbound=never path=…
```

A claims it delivered to B (`outbound=ok-…`) while B reports it never
received anything (`inbound=never`) and never even joined the direct topic
(`direct_topic=not_attempted`). The mismatch is visible at a glance in the
two `inbound=`/`outbound=` columns.

### 3. Discovery success displayed separately from direct-message success

`discovery=` and `inbound=`/`outbound=` are independent labels. A peer that
is discovered but not delivering shows `discovery=seen-… inbound=never
outbound=never` — discovery success never fabricates message delivery.

## Tests

```bash
rb test --lib --features net -- control_plane::health    # 6 unit tests
rb test --test test_health_view --features net           # 3 integration tests
rb check --example doctor --features net                 # doctor health compiles
```

## Design notes / guardrails

* **Debug-only**: the health view lives in `control_plane::health` and the
  `doctor` example. Nothing in the chat UI consumes it.
* **Share-safe**: all rendered values come from the BORU-CP-13
  `PeerDiagnosticsSnapshot` (short peer ids, topic prefixes, sanitised
  errors). The copy-diagnostics block never contains a full 64-hex peer id,
  secrets, tokens, or message contents.
* **Bounded + idempotent**: one probe task per peer (dedup by peer id);
  the probe broadcast repeats on a 200 ms interval for at most 10 s, and
  the payload is tiny and identical per sender, so repeats are idempotent.
* **No control-plane/chat coupling**: the probe travels on the *data plane*
  (the deterministic direct topic) and is not a chat `SignedMessage`; the
  chat decoder rejects it and it never renders/persists as chat.
* **Path is diagnostic-only** (BORU-CP-14): `path=` reports what iroh's
  `remote_info` says when the 15 s sweep has run; before that it stays
  `unknown`, and it never proves application-level delivery.
* **Probe uniqueness**: the payload embeds the sender pubkey so the two
  sides' probes have different content hashes — gossip's plumtree message
  id is the blake3 hash of the content, so identical payloads from both
  peers would be deduplicated as one message and neither side would see the
  other's inbound delivery.
