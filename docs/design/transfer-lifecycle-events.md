# Transfer lifecycle event contract

Status: **specification** — version 1

This document defines the privacy-safe telemetry contract for desktop file
transfers. It is an observability contract, not a wire protocol and not a
source of authorization. Events may be emitted locally, persisted in a bounded
 diagnostic buffer, or exported to an approved telemetry sink.

## 1. Contract version and envelope

Every event is an object with these required fields:

| Field | Type | Meaning |
|---|---|---|
| `schema_version` | integer | Contract version. `1` for this document. Readers must reject unsupported versions or handle them explicitly. |
| `event_id` | opaque string | Locally generated event identifier, unique for the retention period. It is not a transfer identifier. |
| `event_name` | string | One of the stable names in section 3. Names are lowercase `snake_case` and must never be renamed. |
| `transfer_id` | string | Short stable identifier for the logical transfer. See section 2. |
| `sequence` | unsigned integer | Monotonic sequence within this transfer, starting at `0` for the first emitted event. |
| `occurred_at_ms` | unsigned integer | Local Unix epoch milliseconds when the event was observed. |
| `attempt` | unsigned integer | Attempt number, starting at `1`. A retry increments this value; pause/resume does not. |

The optional envelope field `payload` contains only the event-specific fields
listed below. It must be an object when present and must be absent when the
event has no optional fields. Producers must not add arbitrary fields to a v1
event; new fields require a contract version or an explicitly documented
backward-compatible extension.

Example (illustrative values only):

```json
{
  "schema_version": 1,
  "event_id": "evt-opaque-local-id",
  "event_name": "progress_checkpoint",
  "transfer_id": "42…",
  "sequence": 7,
  "occurred_at_ms": 1780000000123,
  "attempt": 1,
  "payload": {
    "bytes_transferred": 524288,
    "total_bytes": 1048576,
    "percent_millis": 500000,
    "checkpoint_interval_ms": 250
  }
}
```

## 2. Transfer identifier

`transfer_id` identifies one logical download record, not one network
connection or retry. It is derived from the local durable download row ID by
the existing `short_transfer_id` policy: at most eight ASCII characters from
the stable local ID, followed by `…` when shortened. It must:

- remain unchanged across retries, pause/resume, process restarts, and recovery;
- be opaque outside the local diagnostic context;
- contain no filename, URL, path, peer identity, content hash, credential, or
  access token; and
- never be used as an authorization capability.

If a transfer has no durable row yet, telemetry must wait until one exists (or
use a separately generated opaque local ID); it must not substitute a filename
or content identifier.

## 3. Event names and event-specific fields

All events carry the envelope fields. Required payload fields are marked
**required**; fields marked optional may be omitted, but must not be sent as a
sensitive substitute.

| Event name | Required payload | Optional payload | Semantics |
|---|---|---|---|
| `download_queued` | `total_bytes` | `queue_depth` | Durable download work was accepted before networking was scheduled. |
| `access_requested` | — | `request_kind` | A fresh access/permission request was sent. `request_kind` is a bounded value such as `initial` or `resume`. |
| `access_granted` | — | `grant_ttl_ms` | The access response authorized the transfer. The grant itself is never recorded. |
| `transfer_started` | `total_bytes` | `resumed_bytes` | Byte transfer began for the current attempt. `resumed_bytes` is the verified/reusable starting offset, default `0`. |
| `progress_checkpoint` | `bytes_transferred`, `total_bytes`, `percent_millis` | `bytes_delta`, `checkpoint_interval_ms`, `rate_bytes_per_sec` | A sampled cumulative progress point. It is not emitted for every byte or chunk. |
| `pause` | `reason` | `bytes_transferred` | Work was deliberately suspended. `reason` is `user`, `system`, or `restart_recovery`. |
| `resume` | `reason` | `bytes_transferred` | A paused logical transfer was resumed. `reason` is `user` or `system`. This is not a retry. |
| `verification` | `result` | `bytes_transferred`, `total_bytes` | Local size/integrity verification finished. `result` is `passed` or `failed`; no hash or content is included. |
| `completion` | `bytes_transferred` | `duration_ms` | Verified content was installed and the durable download reached its successful terminal state. |
| `failure` | `error_category`, `retryable` | `bytes_transferred`, `will_retry`, `retry_delay_ms` | The current attempt failed. `error_category` is from section 5. |
| `cancellation` | `reason` | `bytes_transferred` | The transfer was cancelled and reached its terminal cancelled state. `reason` is `user`, `shutdown`, or `superseded`. |

`total_bytes` is the expected size in bytes. It must be non-negative and must
remain constant for a logical transfer attempt; unknown totals use `null` only
where the implementation cannot know the size. `bytes_transferred` is a
non-negative cumulative count and must not exceed `total_bytes` when the total
is known. `percent_millis` is an integer in `[0, 1_000_000]`, calculated as
`floor(bytes_transferred * 1_000_000 / total_bytes)`; it is `null` when the total
is unknown. Producers should emit checkpoints no more often than the configured
progress interval (currently 250 ms) and should emit a final checkpoint before
verification when practical.

## 4. Ordering, duplicates, retries, and interruption

- `sequence` orders events from one producer for one transfer. Consumers sort
  by `(transfer_id, sequence)` and use `occurred_at_ms` only as display context;
  wall-clock time is not an ordering authority.
- A producer must allocate sequence numbers atomically. If a process crashes,
  a later event may have a gap; gaps are not evidence of a missing state.
- `event_id` is the deduplication key. Replayed events with the same
  `event_id` are ignored. Consumers must also tolerate duplicate semantic
  events with different IDs (for example, two `resume` events caused by a
  race); applying the durable state-machine guard makes them harmless.
- A retry keeps the same `transfer_id`, increments `attempt`, and emits a new
  `access_requested`/`access_granted` pair when permission must be reacquired.
  It does not reset the logical transfer's identity. `failure` reports whether
  another attempt is expected.
- A pause followed by resume keeps the same `attempt`; `resume` is not a retry.
  Persisted `bytes_transferred` may be lower than the last telemetry checkpoint
  after a crash; the durable download row is authoritative.
- An interrupted process may end without `failure`, `cancellation`, or
  `completion`. On startup recovery, the implementation may emit
  `download_queued` (with `attempt` incremented if a retry is started) and
  `resume` with reason `restart_recovery`; it must not infer a successful
  terminal event.
- Terminal events are `completion`, `failure` when no retry will occur, and
  `cancellation`. No later event may be emitted for a terminal transfer.
  Unknown or late events are ignored after terminal state.
- Consumers must ignore unknown future `event_name` values while preserving the
  envelope for forward compatibility. They must not reinterpret an unknown
  event as success, failure, or cancellation.

## 5. Stable bounded error taxonomy

`failure.error_category` is one of these stable lowercase identifiers. Error
messages and raw library errors are not part of the telemetry contract.

| Category | Meaning | Default retry policy |
|---|---|---|
| `permission_denied` | Access was refused or the grant expired without authorization. | no; user action or fresh permission |
| `not_found` | The requested shared object is not available on the remote peer. | no |
| `peer_unavailable` | The remote peer is offline, unreachable, or disconnected. | yes, bounded backoff |
| `timeout` | A configured transfer or operation deadline elapsed. | yes, bounded backoff |
| `rate_limited` | Admission or remote policy rejected the operation temporarily. | yes, bounded backoff |
| `cancelled` | Cancellation interrupted the attempt. | no unless explicitly re-queued |
| `paused` | The operation stopped because it was paused. | no automatic retry |
| `size_mismatch` | Received size differs from the expected size. | no; treat as integrity failure |
| `integrity_mismatch` | Verification failed for the received bytes. | no; do not install |
| `version_mismatch` | The remote version changed while the transfer was pending. | no; refresh and request again |
| `storage_error` | Local temporary-file, database, or installation operation failed. | implementation-defined, bounded |
| `protocol_error` | The peer response or transfer protocol was invalid. | no |
| `resource_exhausted` | Local queue, concurrency, memory, or disk limits prevented progress. | bounded retry when capacity may recover |
| `unknown` | An error cannot safely be classified into the published categories. | no by default |

The taxonomy is closed for v1. Producers must map new implementation errors to
`unknown` until a future contract revision adds a category. Consumers must treat
unrecognized categories as `unknown` and must not branch on human-readable
strings.

## 6. Privacy and redaction requirements

Telemetry payloads must never contain: file content or content previews,
filenames, URLs, filesystem paths, peer IDs or addresses, credentials, access
tokens or grants, message text, raw tickets, content hashes, or unbounded raw
error strings. The same prohibition applies to nested objects and debug
metadata. Byte counts, bounded durations, state names, retry counts, and the
closed error category are permitted.

A privacy test for every serialized event should recursively inspect object keys
and values and assert that:

1. no forbidden key (case-insensitive match for `content`, `filename`, `name`,
   `url`, `path`, `peer`, `credential`, `token`, `ticket`, `hash`, `secret`,
   `message`, or `error_message`) is present;
2. no string value resembles a URL, absolute/parent filesystem path, raw ticket,
   access token, or unbounded exception text; and
3. `failure.error_category` is in the taxonomy above and `transfer_id` matches
   the opaque short-ID format.

These checks are contract tests, not optional logging guidance. A diagnostic
sink must redact or reject an event that fails them rather than forwarding it.

## 7. Compatibility and retention

The v1 contract is additive only. Existing event names, field meanings, enum
values, and identifier rules are stable. A breaking change increments
`schema_version`; readers retain the raw event only if doing so is permitted by
local privacy and retention policy. Producers should use bounded diagnostic
storage and may drop old progress checkpoints, but must not fabricate terminal
outcomes when events were dropped.

The durable `downloads` row and its state machine remain authoritative for
recovery and user-visible state. Telemetry is best-effort evidence of observed
transitions and must never grant access, decide authorization, or be required
to resume a transfer.
