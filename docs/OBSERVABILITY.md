# Observability: Structured Tracing & Redaction Rules

This document describes how tracing is structured in the public-room DHT
subsystem, what is safe to log, and what is redacted.

## Tracing backend

All tracing uses the [`tracing`] crate with structured key-value fields.
Log consumers (tracing-subscriber, OpenTelemetry, etc.) can filter on
field values — message interpolation is avoided except in the chat
example where `tracing::info!` with format args is used for
human-readable status.

## Structured fields

Every public-room tracing event carries at least:

| Field          | Type           | Example              | Description                               |
|----------------|----------------|----------------------|-------------------------------------------|
| `room`         | `&str`         | `room-b646`          | Short room identifier (first 4 hex of topic). Safe for logs. |
| `local`        | PublicKey short | `d1a2...b3c4`       | Local EndpointId via `fmt_short()`. Safe.  |
| `duration_us`  | `u64`          | `12345`              | Wall-clock duration in microseconds.       |
| `error`        | Display        | `connection refused`  | Error message (never payload contents).    |

Publish/discovery events additionally carry operation-specific counters
(see below).

## Redaction rules

### NEVER logged

| Data                          | Rationale                                              |
|-------------------------------|--------------------------------------------------------|
| Full discovery key (32 bytes) | Could identify the room namespace externally.          |
| Decrypted tracker payloads    | Contains EndpointId which is public, but the payload   |
|                               | may carry future secret-bearing fields.                 |
| Full invitations / join tickets| Tickets contain addressing and secret-key material.    |
| Unnecessary endpoint addresses | Full SocketAddr / RelayUrl could leak network topology. |
| Record raw bytes              | Opaque encrypted data — never decrypted or dumped.     |

### SAFE to log (used in structured fields)

| Data                          | Form used                                              |
|-------------------------------|--------------------------------------------------------|
| Short room identifier         | `room-` prefix + first 4 hex chars of the topic hash   |
| Local EndpointId              | `fmt_short()` — truncated 4-char hex prefix            |
| Peer EndpointId (discovered)  | `fmt_short()` — never the full 32-byte key             |
| Counters                      | Integer values only (accepted, rejected, stale, etc.)  |
| Error messages                | Display impl — never payload contents                  |
| Duration                      | Microsecond integer                                    |
| Attempt / retry count         | Integer                                                |
| Consecutive failures          | Integer                                                |
| Network                       | Enum variant name (`Mainnet`, `Development`, `Test`)   |

## Event reference

### PublicRoomTracker

| Method            | Level  | Key fields                                        | When                           |
|-------------------|--------|---------------------------------------------------|---------------------------------|
| `start()`         | `info` | `room`                                            | Tracker constructed + ready     |
| `publish_once()`  | `info` | `room`, `local`, `duration_us`                    | Publication succeeded           |
|                   | `warn` | `room`, `local`, `error`, `duration_us`           | Publication failed              |
| `discover_once()` | `info` | `room`, `local`, `accepted`, `rejected`, `*`      | ≥1 peer found                   |
|                   | `debug`| `room`, `local`, `accepted`, `rejected`, `*`      | No peers found                  |
| `shutdown()`      | `info` | `room`                                            | Shutdown started + completed    |

\* `discover_once` `info` level also includes: `encrypted`, `records`,
`oversized`, `stale`, `future`, `decode_failure`, `identity_mismatch`,
`invalid_signature`, `self_filtered`, `duplicates`, `duration_us`.
The `debug` level includes `oversized`, `stale`, `future`,
`decode_failure` and `duration_us`.

### ContinuousTracker (background loops)

| Event                         | Level  | Key fields                                           |
|-------------------------------|--------|------------------------------------------------------|
| Publish succeeded             | `debug`| `room`, `duration_us`                                |
| Publish failed (retries)      | `warn` | `room`, `error`, `consecutive_failures`, `duration_us` |
| Publish degraded DHT state    | `warn` | Same as above, fired after ≥3 consecutive failures   |
| Discover found new peers      | `info` | `room`, `total`, `new`, `duration_us`                |
| Discover peers all known      | `trace`| `room`, `total`, `duration_us`                       |
| Discover no peers             | `debug`| `room`, `duration_us`                                |
| Discover failed (retries)     | `warn` | `room`, `error`, `consecutive_failures`, `duration_us` |
| Discover degraded DHT state   | `warn` | Same, after ≥3 consecutive failures                  |
| Channel closed (discovery)    | `info` | `room`                                                |
| Cancelled (publish/discover)  | `info` | `room`                                                |
| Shutdown                      | `info` | `room`                                                |

### Backoff (retry_with_backoff)

Each retry attempt logs at `debug` level:

| Field           | Type    | Description                           |
|-----------------|---------|---------------------------------------|
| `attempt`       | `u32`   | Retry attempt number (1-indexed).     |
| `delay_us`      | `u64`   | Current backoff delay in microseconds.|
| `jittered_us`   | `u64`   | Actual sleep duration after jitter.   |
| `error`         | Display | The error that triggered the retry.   |

### Chat example (join / leave)

| Event                              | Level  | Key fields        |
|------------------------------------|--------|-------------------|
| Initial join_peers succeeded       | `info` | `count`           |
| Initial join_peers failed          | `warn` | `count`, `error`  |
| Continuous join_peers succeeded    | `info` | `count`           |
| Continuous join_peers failed       | `warn` | `count`, `error`  |
| Leaving public room                | `info` | —                 |
| Public room left                   | `info` | —                 |

## Degraded DHT state detection

The continuous tracker tracks a `consecutive_failures` counter per loop
(publish and discovery independently). When this counter reaches 3 or
more, subsequent failure warnings use the message
`"continuous publish/discover degraded DHT state"` instead of the
standard failure message. The counter resets to 0 on any successful
operation.

## Adding new tracing

When adding tracing to public-room code:

1. Use structured fields wherever possible — avoid string interpolation
   in the message.
2. Call `identity.short_id()` for the `room` field. Never log
   `identity.discovery_key` or `identity.topic` directly.
3. Use `EndpointId::fmt_short()` for peer identifiers. Never log
   `EndpointId.as_bytes()`.
4. Use counters (integers) instead of dumping record contents or
   payloads.
5. Choose the right level: `info` for normal lifecycle events and new
   peer discoveries, `debug` for routine successes, `trace` for
   high-frequency events (already-known peers), `warn` for retries and
   degradation, `error` for unrecoverable failures.

[`tracing`]: https://docs.rs/tracing
