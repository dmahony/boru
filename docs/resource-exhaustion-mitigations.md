# Resource Exhaustion Attack Mitigations

This document catalogues every resource-exhaustion attack scenario that Boru
protects against, the protection mechanism, and the test that validates it.

A single malicious client **cannot** cause a denial of service against a properly
configured Boru node.

---

## Catalogue Protocol

The catalogue protocol is the richest attack surface because it accepts incoming
QUIC connections, allocates per-connection state, deserialises untrusted payloads,
and streams potentially large responses.

### 1. Concurrent connection flood

**Attack:** Open 17+ catalogue connections simultaneously. The server must allocate
a task slot for each connection.

**Mitigation:** [`CatalogueConcurrencyLimiter`] caps concurrent serving slots at
[`MAX_CONCURRENT_CATALOGUE_CONNECTIONS`] (16). When the semaphore is exhausted,
`try_acquire()` returns `None` and the handler immediately writes a `Busy`
response and closes the stream — no task slot is consumed.

**Test:** `concurrent_connections_exhaustion_sends_busy`
— fires 17 connections in parallel; at most 16 succeed, at least 1 gets `Busy`.
Fresh connection after the storm works.

### 2. Per-peer request frequency flood

**Attack:** A single peer sends catalogue requests faster than 32 per 10 seconds.

**Mitigation:** [`PeerCatalogueAbuseLimiter::admit()`] implements a sliding-window
counter per authenticated peer identity. After [`MAX_CATALOGUE_REQUESTS_PER_PEER`]
(32) requests within [`CATALOGUE_RATE_LIMIT_WINDOW`] (10 s), `admit()` returns
`RateLimited` and the handler sends a `RateLimited` error. State is keyed by
the iroh PublicKey — an attacker cannot spoof their identity because the
connection is authenticated by the QUIC TLS handshake.

**Tests:**
- `per_peer_rate_limiting_blocks_after_budget_exhausted`
- `per_peer_rate_limiting_clears_after_window_expiry`

### 3. Oversized request payload

**Attack:** Send a catalogue request larger than 256 KiB.

**Mitigation:** The handler reads the request frame into a bounded buffer.
After deserialisation the `CatalogueHandler::handle_catalogue_request()` checks
`payload.len() > MAX_CATALOGUE_REQUEST_BYTES` (256 KiB) and immediately returns
`InvalidRequest` without allocating a response. The oversized payload also
counts toward the peer's malformed-attempt budget.

**Tests:**
- `oversized_request_payload_is_rejected`
- `oversized_requests_count_as_malformed_attempts`

### 4. Garbage / malformed bytes on catalogue stream

**Attack:** Send random bytes or truncated postcard frames as a catalogue request.

**Mitigation:** Failed postcard decode increments a per-peer invalid-attempt
counter in [`PeerCatalogueAbuseLimiter`]. After [`MAX_INVALID_CATALOGUE_ATTEMPTS_PER_PEER`]
(3) the peer transitions to `Blocked` state and all subsequent requests receive
`RateLimited` (mapped from `Blocked`) until the rate-limit window expires.

**Tests:**
- `malformed_requests_block_peer_after_threshold`
- `blocked_peer_can_recover_after_window_expiry`

### 5. Blocked peer trying to fetch catalogue

**Attack:** A friend-blocked peer issues legitimate catalogue requests.

**Mitigation:** The handler performs an early `FriendsStore` relationship check.
If the requester's record has `relationship == Blocked`, the handler returns
`PermissionDenied` before reaching the abuse limiter or storage layer.

**Test:** `legitimate_peer_can_fetch_after_abuser_blocked_integration`

### 6. Storage file count limit

**Attack:** Add more than 10,000 files to the local catalogue.

**Mitigation:** `Storage::upsert_shared_file()` runs inside an immediate
transaction that atomically checks the current file count against
`max_files_per_catalogue` (10,000) before inserting. Existing entries can
still be updated (metadata changes, renames, description edits).

**Test:** `catalogue_file_limit_atomic_in_storage`

### 7. Storage collection count limit

**Attack:** Create more than 1,000 collections.

**Mitigation:** `Storage::ensure_collection()` atomically checks the collection
count against `max_collections` (1,000). Duplicate name lookups do not count
as new collections.

**Test:** `catalogue_collection_limit_in_storage`

### 8. Entries per collection limit

**Attack:** Add more than 10,000 files into a single collection.

**Mitigation:** `Storage::add_to_collection()` atomically checks the entry count
against `max_entries_per_collection` (10,000). Duplicate add calls are idempotent.

**Test:** `collection_entry_limit_in_storage`

### 9. Response volume from one peer

**Attack:** A single peer consumes excessive response bandwidth (>16 MiB/10 s).

**Mitigation:** [`PeerCatalogueAbuseLimiter::admit()`] also tracks cumulative
response bytes via `record_response_bytes()`. When `max_response_bytes_per_peer`
(16 MiB) is exceeded within the sliding window, `admit()` returns
`ResponseBudgetExceeded`. The budget resets when the window expires.

**Test:** `response_budget_exhaustion_blocks_until_window_expires`

### 10. Combined concurrent + rate-limit

**Attack:** Open many connections AND send high-frequency requests.

**Mitigation:** Both limiters are independent and composed: the concurrency
semaphore gates task slots, and the abuse limiter gates per-peer request /
byte / invalid budgets. An attacker that can't get a task slot (busy) also
can't consume per-peer budget, and vice versa.

**Test:** `combined_concurrent_and_rate_limit_stress`

### 11. High-frequency progress writes

**Attack:** A download generates thousands of progress events per second.

**Mitigation:** [`ProgressUpdateGate`] coalesces writes: `should_persist()`
returns `true` at most once per configurable interval (default 250 ms).
Between intervals the gate returns `false`, so progress events are silently
dropped instead of hammering the database.

**Test:** `progress_update_gate_coalesces_high_frequency_writes`

### 12. Download queue overflow

**Attack:** Initiate more than 32 simultaneous downloads.

**Mitigation:** [`DownloadLimiter`] enforces three independent budgets:

- **Global queue depth:** `max_queued_downloads` (32). `try_enqueue()` returns
  `QueueFull` when exceeded.
- **Per-peer depth:** `max_downloads_per_peer` (2). `try_enqueue()` returns
  `PeerQueueFull` when a peer has reached its limit.
- **Hash verification:** `max_active_hash_verifications` (2). Independent
  semaphore from download slots so hash verification doesn't block transfers.

**Tests:**
- `download_limiter_queue_full_rejects_excess`
- `download_limiter_per_peer_limit_enforced`
- `hash_verification_budget_independent_of_downloads`

### 13. Catalogue updates when full

**Attack:** Attempt to add new files to a catalogue that is already at capacity.

**Mitigation:** The atomic count check described in scenario 6 rejects
new-file upserts but allows metadata updates to existing entries. This
prevents resource exhaustion while still permitting legitimate edits.

**Test:** `catalogue_full_but_existing_updates_allowed`

### 14. Multiple attacking peers in parallel

**Attack:** Several peers attack the same server simultaneously.

**Mitigation:** Per-peer accounting in `PeerCatalogueAbuseLimiter` uses the
authenticated PublicKey as the key. Each peer has an independent sliding
window; one peer's rate-limit does not affect other peers.

**Test:** `multiple_peers_independent_rate_budgets`

### 15. Response payload size abuse

**Attack:** Request a catalogue response larger than 4 MiB, or file-details
response larger than 256 KiB.

**Mitigation:** Hard byte caps enforced on both the sending (handler) and
receiving (client) sides: [`MAX_CATALOGUE_RESPONSE_BYTES`] (4 MiB),
[`MAX_CATALOGUE_PAGE_BYTES`] (1 MiB), [`MAX_FILE_DETAILS_PAYLOAD_BYTES`]
(256 KiB). Each has a `check_*_payload_size()` function that returns `Err`
when exceeded.

**Test:** `response_payload_size_limits_enforced`

### 16. File-access upload queue overflow

**Attack:** Initiate more file-access requests than the server can handle.

**Mitigation:** [`UploadLimiter`] caps three independent budgets:
- **Global active uploads:** `max_active_uploads` — exceeds get `QueueFull`.
- **Per-peer depth:** `max_uploads_per_peer` — exceeds get `PeerLimitReached`.
- **Verification concurrency:** independent semaphore returns `VerificationBusy`.

**Tests:**
- `upload_limiter_full_rejects_excess_global`
- `upload_limiter_per_peer_full_rejects_excess`
- `upload_limiter_verification_full_rejects_excess`

### 17. Combined abuse budgets exhausted

**Attack:** Exhaust request-frequency AND response-byte AND invalid-attempt
budgets simultaneously.

**Mitigation:** [`PeerCatalogueAbuseLimiter`] enforces all three budgets
independently in a single `admit()` call. Exceeding any one budget produces
the appropriate `CatalogueAdmission` variant. The most restrictive budget
always governs.

**Test:** `abuse_limiter_combined_budgets_all_independent`

### 18. Abuser-blocked peer recovery

**Attack:** N/A (recovery path). A blocked peer should be re-admitted after
the abuse-limiter window expires.

**Mitigation:** The sliding-window design naturally unblocks peers when their
window has elapsed. [`PeerCatalogueAbuseLimiter::admit()`] and
`record_invalid()` purge expired entries on every call.

**Test:** `abuser_blocked_integration_recovers_after_ban_window`
— blocks a peer with oversized requests, verifies the server stays responsive
for others, and verifies the blocked peer receives `RateLimited` errors.

### 19. Zero-length request payload

**Attack:** Send a frame with version header but zero payload bytes.

**Mitigation:** The handler deserialises the empty payload through postcard,
which fails gracefully. The connection is closed without hanging, crashing,
or leaking resources.

**Test:** `zero_byte_payload_handled_gracefully`

### 20. Connection storm (rapid connect/disconnect)

**Attack:** Open and close many catalogue connections in rapid succession.

**Mitigation:** Each connection creates a fresh endpoint that is dropped at
the end of the cycle. The server's QUIC stack handles connection teardown.
Rate limiting (scenario 2) bounds how many requests a single peer can make
in a window, preventing true flooding.

**Test:** `connection_storm_rapid_cycles` — 5 rapid connect/disconnect
cycles succeed, and a fresh peer can still fetch after the storm.

### 21. Concurrent mixed attack (valid + malicious peers)

**Attack:** Simultaneously flood with both legitimate and oversized/invalid
requests from different peers.

**Mitigation:** Per-peer independent budgets (scenario 14) ensure that
attackers exhausting their own budget don't affect legitimate peers.
The concurrency limiter (scenario 1) bounds total serving slots.

**Test:** `concurrent_mixed_valid_and_invalid_stress` — 5 legitimate friends
and 5 attackers fire requests concurrently. Legitimate peers succeed;
attackers get blocked/rate-limited independently.

### 22. Rate-limit boundary at exact window expiry

**Attack:** N/A (boundary condition). Validate that requests arriving at the
exact window boundary are correctly admitted after the window has expired.

**Mitigation:** The sliding-window purge logic in `PeerCatalogueAbuseLimiter`
removes entries older than `now - window_duration`. Entries exactly at the
boundary are admitted because they are no longer inside the window.

**Test:** `rate_limiter_boundary_exact_window_expiry`

---

## Chat Protocol (message-level)

The hostile-input test suite (`test_hostile_input.rs`) validates that the
chat message processing pipeline rejects all forms of resource-exhaustion
input without crashing, leaking memory, or corrupting state.

| Attack | Mitigation | Tests |
|--------|-----------|-------|
| Invalid signature / tampered envelope | `SignedMessage::verify_and_decode` rejects before any state mutation | `invalid_signature_*` (3 tests) |
| Spoofed sender | Signature verification authenticates `from` field — key replacement detected | `spoofed_sender_rejected_by_signature` |
| Expired timestamp (>1h TTL) | `handle_net_event` checks `sent_at` against current time + TTL | `expired_timestamp_dropped_by_handle_net_event` |
| Future timestamp (>300s skew) | Clock-skew check rejects messages too far in the future | `future_timestamp_beyond_skew_rejected` |
| Oversized plaintext (1 MB) | Message body allocated on heap; no stack copies | `oversized_plaintext_does_not_panic` |
| Message replay flood | Dedup set keyed by `(sender_pk, content_hash, sent_at)` with 2-hour TTL | `replay_flood_identical_messages_suppressed` |
| Blocked sender | Early `is_blocked()` check drops before any processing | `blocked_sender_*` (3 tests) |
| Invalid ack | Unknown message hash silently ignored | `invalid_ack_for_unknown_hash_does_not_panic` |
| Large sync batch (500 msgs) | Each message individually validated; no batch pre-allocation | `rejected_input_does_not_cause_unbounded_dedup_set` |

---

## Architecture Summary

All resource-exhaustion protections share a common architectural pattern:

1. **Authenticated identity** — all limits are keyed by the iroh `PublicKey`
   from the QUIC TLS handshake. An attacker cannot spoof or forge this identity.
2. **Independent per-peer budgets** — one peer exhausting its budget cannot
   affect other peers.
3. **Sliding time windows** — all budgets reset naturally when the window
   expires, so legitimate peers can never be permanently blocked by past abuse.
4. **Composable limiters** — concurrency, rate, response-byte, and invalid-attempt
   budgets are independent and compose freely. The most restrictive budget
   always governs.
5. **Atomic storage operations** — limit checks and mutations run in the same
   transaction, preventing TOCTOU races.
6. **No unbounded allocations** — every incoming payload is bounded by hard
   byte caps, and every queue is bounded by configurable depth limits.

[`CatalogueConcurrencyLimiter`]: ../src/catalogue_rate_limits.rs
[`PeerCatalogueAbuseLimiter`]: ../src/catalogue_rate_limits.rs
[`PeerCatalogueRateLimiter`]: ../src/catalogue_rate_limits.rs
[`ProgressUpdateGate`]: ../src/download_limits.rs
[`DownloadLimiter`]: ../src/download_limits.rs
[`UploadLimiter`]: ../src/file_access_handler.rs
[`MAX_CONCURRENT_CATALOGUE_CONNECTIONS`]: ../src/catalogue_rate_limits.rs
[`MAX_CATALOGUE_REQUESTS_PER_PEER`]: ../src/catalogue_rate_limits.rs
[`MAX_CATALOGUE_RESPONSE_BYTES_PER_PEER`]: ../src/catalogue_rate_limits.rs
[`MAX_INVALID_CATALOGUE_ATTEMPTS_PER_PEER`]: ../src/catalogue_rate_limits.rs
[`CATALOGUE_RATE_LIMIT_WINDOW`]: ../src/catalogue_rate_limits.rs
[`MAX_CATALOGUE_RESPONSE_BYTES`]: ../src/catalogue_limits.rs
[`MAX_CATALOGUE_PAGE_BYTES`]: ../src/catalogue_limits.rs
[`MAX_FILE_DETAILS_PAYLOAD_BYTES`]: ../src/catalogue_limits.rs
