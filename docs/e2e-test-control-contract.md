# Boru E2E test-control contract

Status: version 1, explicit test actions only.

This document defines the shared control-plane vocabulary used by the ROOM,
MSG, and FILE E2E lanes. It is an adapter contract, not a second RPC server
and not a production API. Implementations register actions with the existing
MCP/GUI test-action path and use the normal application pathways underneath.

## Safety boundary

* The control plane is disabled unless the process is started with the
  existing explicit GUI-test-action enable flag.
* Its listener binds to loopback by default (`127.0.0.1`; an ephemeral port is
  preferred). A non-loopback bind is not part of this contract and must never
  be an implicit fallback.
* Only named, typed actions below are accepted. There is no shell, arbitrary
  filesystem, SQL, private-key, ticket, or raw-byte operation.
* Mutating actions require the explicit enable flag. Read-only snapshots may be
  used by a harness only after the same test-control session has been
  established.
* Responses contain identifiers, counters, states, bounded aliases, and
  hashes/metadata. They never contain message bodies, raw join tickets,
  private keys, file bytes, or unrestricted local paths.

## Envelope and identifiers

Every request and response carries `schema` (`name = "boru-e2e"`) and
`version` (`1`). Unknown future fields may be ignored; an unknown schema or
unsupported version is a structured error. `run_id` scopes one harness run.
The following opaque identifiers are distinct typed strings:

* `run_id` — harness run correlation.
* `node_alias` — safe operator alias such as `node-a`, never a public key.
* `workflow_id` — named workflow instance.
* `room_marker` — stable room correlation marker; it is not a ticket.
* `message_marker` — opaque `E2E:<run_id>:<sequence>` correlation marker.
* `transfer_marker` — stable file-offer/download correlation marker.
* `fault_id` — fault-injection correlation (fault execution is owned by a
  later lane).

Markers and aliases are bounded, printable, and must not contain secrets.
Implementations should preserve them across reconnect/restart where the
underlying durable state permits it.

## Actions

The shared action vocabulary is:

| Action | Mutating | Required inputs | Safe result |
|---|---:|---|---|
| `create_room` | yes | run, node, workflow, room marker | room snapshot + safe join reference handle |
| `join_room` | yes | room marker + process-local join handle/reference | room snapshot |
| `leave_room` | yes | room marker | room snapshot |
| `rejoin_room` | yes | room marker + process-local join handle/reference | room snapshot |
| `send_test_message` | yes | room marker, message marker | message snapshot; body is consumed internally and never returned |
| `query_message` | no | room + message marker | message snapshot |
| `share_synthetic_file` | yes | room marker, transfer marker, fixture metadata | transfer snapshot |
| `accept_download` | yes | transfer marker | transfer snapshot |
| `query_transfer` | no | transfer marker | transfer snapshot |
| `query_node_status` | no | node alias | compact node snapshot |
| `query_room_status` | no | room marker | compact room snapshot |

A join handle/reference is a process-local opaque capability. It may be
stored by the harness, but must not be serialized as a raw ticket or exposed
in logs/reports. `send_test_message` takes a marker and an implementation-owned
synthetic payload; no action response or snapshot includes its body.
Synthetic-file actions expose size and a content hash only.

## Stable snapshots

`NodeSnapshot`, `RoomSnapshot`, `MessageSnapshot`, and `TransferSnapshot` are
versioned compact projections. They contain no UI text or unbounded payloads.
Delivery and transfer states are monotonic except for an explicit failure
terminal state, so a harness can assert progress without timestamp matching.

The Rust shared representations are in `boru_core::e2e_control`; downstream
lanes should implement domain adapters around these types rather than adding
domain behavior to `app.rs`.

## Reconnect and restart recovery lane

`tests/test_reconnect_recovery.rs` exercises the bounded reconnect contract
with the real two-peer endpoint lifecycle. Receiver and sender interruption
scenarios stop and relaunch the same logical node, then assert that the public
key, profile directory, correlation sequence, and monotonic delivery state are
preserved. A repeated transport observation increments duplicate telemetry but
does not cause a second logical presentation.

The current deterministic gossip harness does not provide a mailbox replay
round-trip for a marker submitted while the receiver is offline. That case is
reported as an explicit `SKIP` capability boundary, never as a fabricated
delivery PASS. These tests therefore prove reconnect/restart identity and
correlation recovery, while offline mailbox delivery remains a separate
capability until the mailbox transport is wired into the harness. The messaging lane's adapter is
`boru_core::e2e_messaging::MessagingDomainAdapter`: it allocates
`E2E:<run_id>:<sequence>` markers and maintains body-free delivery snapshots,
duplicate counts, ordered state history, and at-most-once presentation state.
The caller still submits the marker through the normal GUI/MCP composer path;
the adapter does not send arbitrary payloads or expose message bodies.

## Outcomes and errors

Every action returns an `ActionOutcome` with schema/version, run ID, action
ID, and either a typed result or an `E2eError`. Error codes are stable:

* `disabled_action` — test actions are not enabled.
* `invalid_state` — action is not valid for the current lifecycle state.
* `timeout` — bounded operation did not complete in its deadline.
* `unavailable_capability` — requested normal-path capability is unavailable.
* `unsupported_platform` — platform cannot provide the action.
* `not_found` — marker, alias, room, message, or transfer is unknown.
* `internal_failure` — unexpected implementation failure (with redacted detail).

Errors must remain bounded and must not include secrets, bodies, bytes, or
arbitrary paths.

## Compatibility and registration

The existing GUI/MCP action dispatcher remains the sole transport and gate.
The shared Rust module exports typed action descriptors and the
`TestControlConfig` loopback/enable policy so lane adapters can register named
actions without changing the coordinator. Existing `GuiTestCommand` values
remain unchanged and continue to validate and execute as before.

A conforming implementation must reject mutating actions when
`TestControlConfig::allows_mutation()` is false and must reject a configured
non-loopback bind unless an operator explicitly opts into a separately
reviewed deployment mode. The default constructor always produces a disabled,
loopback-only policy.
