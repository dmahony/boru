# Pause Mechanism: Design Specification

Status: **implemented behaviour** — documents the durable download pause/resume
state machine and its storage guards. The initial implementation covers file
downloads; the other work types below remain future extensions.

---

## 1. Scope and terminology

The pause mechanism allows a user or the system itself to **suspend active
background work** (file transfers, protocol exchanges, DHT publications) and
later **resume** it without data loss, corruption, or redundant full restarts.
It is the counterpart of the existing retry-after-failure path — a deliberate,
user- or system-initiated suspension rather than an error recovery path.

| Term | Meaning |
|------|---------|
| **Pause** | A deliberate transition from an active (non-terminal) state to the `paused` state. All persisted metadata about the work is retained. |
| **Resume** | A deliberate restart of paused work. The system re-establishes the preconditions that were valid when the work was active (peer resolution, permission grants) before re-entering the transfer phase. |
| **Active work** | Any background operation that consumes network or compute resources: blob transfers, peer resolution, permission requests, verification. |
| **Terminal state** | A state from which the system cannot transition: `complete`, `failed`, `cancelled`, `version_mismatch`. |
| **Stale progress race** | A concurrent worker thread that continues writing to a download row after it has been paused. |

### 1.1 Supported work types (initial scope)

The initial implementation covers **file downloads** via the iroh-blobs
transfer pipeline.  The same pattern is extensible to:

- Outbox delivery retry scheduling (pause outbox worker)
- DHT publication loops (pause public_room_continuous republish)
- File indexer filesystem watcher (pause notify-based scanner)
- Catalogue retrieval (there is no continuous remote catalogue polling; refresh
  is event-driven and may be skipped while offline)
- Backfill request serving (pause backfill protocol handler)

Each extension follows the same state-machine and persistent-state pattern
described here.

---

## 2. Data structures

### 2.1 `DownloadState` (in `src/download.rs`)

```rust
pub enum DownloadState {
    /// Waiting for a worker slot.
    Queued,
    /// Resolving the remote peer.
    ResolvingPeer,
    /// Requesting a fresh access descriptor.
    RequestingPermission,
    /// Receiving bytes into a temporary file.
    Downloading,
    /// Checking size and content hash.
    Verifying,
    /// Installed and durably recorded as verified.
    Complete,
    /// Paused by the user or during restart recovery.
    Paused,
    /// Failed and eligible for retry.
    Failed,
    /// Cancelled by the user.
    Cancelled,
    /// The catalogue no longer matches the requested content.
    VersionMismatch,
}
```

The `Paused` variant is **not terminal** — a paused download can be resumed.
It is **not** a transient state; it survives application restarts via the
SQLite `downloads` table.

### 2.2 `Download` row (in `src/storage.rs` SQLite `downloads` table)

```sql
CREATE TABLE downloads (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    content_hash    TEXT NOT NULL,       -- blake3 hex of expected content
    remote_peer     TEXT NOT NULL,       -- peer public key hex
    state           TEXT NOT NULL,       -- one of the DownloadState string values
    bytes_downloaded INTEGER NOT NULL DEFAULT 0,
    total_bytes     INTEGER NOT NULL,
    created_at_ms   INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL,
    last_error      TEXT,
    retry_count     INTEGER NOT NULL DEFAULT 0,
    next_retry_at_ms INTEGER
);
```

#### Preserved metadata on pause

When a download enters the `paused` state, the following fields are
**retained without modification**:

| Field | Retained? | Purpose on resume |
|-------|-----------|-------------------|
| `content_hash` | Yes | Verify file has not changed; reject `accept_resumed_descriptor` on mismatch → `version_mismatch` |
| `remote_peer` | Yes | Re-resolve peer address |
| `bytes_downloaded` | Yes | Track partial progress; usable for iroh-blobs chunk reuse |
| `total_bytes` | Yes | Size guard for verification |
| `retry_count` | Yes | Unchanged; resume is not a retry |
| `last_error` | Yes | Unchanged; cleared only by `accept_resumed_descriptor` |
| `next_retry_at_ms` | Yes | Unchanged; cleared only on successful descriptor acceptance |

The only fields modified by `pause_download` are `state` (→ `'paused'`) and
`updated_at_ms` (→ current time).

---

## 3. Interface design

### 3.1 `Storage::pause_download(id: i64) -> Result<()>`

SQL-level operation that transitions a single download row to `'paused'`.

**Preconditions:**
- Row exists (`SELECT` before update, result must be `Some`).
- Current state is **not** terminal (`complete`, `completed`, `cancelled`,
  `failed`, `version_mismatch`).

**Guarantees:**
- Only `state` and `updated_at_ms` are written. All other fields are
  **immutable** on this path.
- An already-paused row is a no-op (idempotent). This is critical because
  both user action and a system-level pause trigger may race and call
  `pause_download` for the same row.
- Terminal downloads return an error: `"cannot pause terminal download in
  state {current}"`.

**SQL:**
```sql
UPDATE downloads SET state = 'paused', updated_at_ms = ?1
WHERE id = ?2 AND state = ?3
```

The `state = ?3` guard prevents pausing a row that changed state between the
precondition check and the update. Execute returns `changed == 0` on conflict;
the caller uses the pre-check result for the error message rather than
retrying with the new state.

### 3.2 `Storage::resume_download(id: i64) -> Result<()>`

Transition a paused download to `'resolving_peer'` — the first active state.
Resume never jumps directly to byte transfer.

**Preconditions:**
- Row exists and is in `'paused'` state.

**Guarantees:**
- State goes to `'resolving_peer'`, not to `'downloading'`. The worker must
  re-resolve the peer and obtain a fresh permission descriptor because
  descriptors are short-lived (60 seconds) and permissions may have changed.
- `bytes_downloaded` and `content_hash` are preserved so the worker can
  reuse iroh-blobs chunks after re-acquiring permission.
- Idempotent: if the row is already `'resolving_peer'` (or any active
  non-paused state), it is a no-op. This prevents a double-resume from
  resetting an in-progress resolution.

**SQL:**
```sql
UPDATE downloads SET state = 'resolving_peer',
    updated_at_ms = ?1
WHERE id = ?2 AND (state = 'paused' OR state = 'resolving_peer')
```

### 3.3 `Storage::accept_resumed_descriptor(id, content_hash, total_bytes) -> Result<()>`

After the worker has re-resolved the peer and obtained a new
`SignedDownloadDescriptor`, this call validates the descriptor against the
paused download's metadata.

**Preconditions:**
- Row exists and is in an active state (`'resolving_peer'` or
  `'requesting_permission'`).
- Provided `content_hash` must match the row's `content_hash`. A mismatch
  means the catalogue changed while paused → transition to
  `'version_mismatch'` with a clear error.

**Guarantees:**
- On hash match: state → `'downloading'`, `total_bytes` updated (may have
  changed if file was updated), `last_error` and `next_retry_at_ms` cleared.
- On hash mismatch: state → `'version_mismatch'`, `last_error` set to
  `"resume descriptor content hash mismatch"`.
- If the descriptor has already expired: state stays `'paused'`, `last_error`
  set to `"resume descriptor expired"`.

**SQL (hash match):**
```sql
UPDATE downloads SET state = 'downloading', total_bytes = ?1,
    last_error = NULL, next_retry_at_ms = NULL, updated_at_ms = ?2
WHERE id = ?3
```

### 3.4 Guard functions — active-work barrier on paused rows

Three functions reject writes against paused downloads:

| Function | Guard in SQL |
|----------|-------------|
| `update_download_progress` | `WHERE id = ?4 AND state != 'paused'` |
| `fail_download` | `WHERE id = ?4 AND state != 'paused'` |
| `complete_download` | `WHERE id = ?3 AND state NOT IN ('complete', 'cancelled', 'paused')` |

**On zero rows affected**, each function queries the current state and
returns a diagnostic error:

- `"download is paused; active work must be cancelled"` — when state is
  `'paused'`.  The caller (download worker) SHOULD treat this as a signal to
  cancel in-flight network operations and discard temporary files.
- `"download not found"` — when the row does not exist.
- Generic affected-rows error — when state is active but the SQL constraint
  rejected the write (should not happen in normal operation).

**Integrity invariant:** A paused download cannot be moved out of `paused`
except by an explicit `resume_download()` call.  A stale worker thread cannot
incrementally write progress or complete a download that the user has
paused.

---

## 4. Flow: pause lifecycle

### 4.1 Pause trigger

```
User action (UI "Pause" button)
  │
  ▼
Invoke Service::pause_download(id)          [layer above Storage]
  │
  ▼
Storage::pause_download(id)
  ├─ Read current state
  ├─ Reject if terminal                                   → error
  ├─ No-op if already 'paused' (idempotent)
  └─ UPDATE state = 'paused', updated_at_ms = now
      ├─ SQL-level guard: WHERE state = current_state
      └─ On success:
          │
          ▼
        Signal download worker to cancel active operations
        (via shared cancel token / mpsc channel)
          │
          ▼
        Worker cancels:
          1. iroh-blobs blob transfer (drop stream)
          2. Close QUIC stream if open
          3. Remove temporary file if one exists
```

**Active-work cancellation mechanism:**

The download worker holds a per-download `CancellationToken` (tokio-util).
When `pause_download` succeeds at the persistence layer:

1. The caller (service layer) drops or cancels the token associated with
   this download id.
2. The worker's event loop observes the cancellation on its next tick.
3. The worker unwinds:
   - Drops the iroh-blobs `get_blob` future (chunk downloads drop).
   - Closes any open QUIC connection if no other download uses it.
   - Deletes the temporary `.part` file (partial transfer is not valid).
4. The worker does NOT write progress after cancellation — the guard
   functions (section 3.4) would reject it anyway, but explicit cleanup
   avoids wasted work and confusing log messages.

### 4.2 State machine with pause and resume

```
   ┌──────────┐
   │  Queued   │
   └─────┬─────┘
         │ worker picks up
         ▼
  ┌──────────────┐
  │ ResolvingPeer │◄──────────────────────────┐
  └──────┬───────┘                            │
         │ peer resolved                      │
         ▼                                    │
  ┌──────────────────┐                        │
  │RequestingPermission│                      │
  └────────┬─────────┘                        │
           │ descriptor received              │
           ▼                                  │
     ┌───────────┐                            │
     │Downloading │                            │
     └─────┬─────┘                            │
           │ bytes received                   │
           ▼                                  │
     ┌──────────┐                   ┌──────┐  │
     │Verifying │                   │Paused│──┘
     └────┬─────┘                   └──────┘
          │ pass/fail                     ▲
          ▼                               │
  ┌────────┬────────┐              resume_download()
  │        │        │              (→ ResolvingPeer)
  ▼        ▼        ▼
Complete  Failed  Cancelled  VersionMismatch
(terminal)(terminal)(terminal)  (terminal)
```

Key invariants:
- **Pause is always available** from any non-terminal, non-paused state.
- **Resume always returns to `ResolvingPeer`**, never to `Downloading`.
  Permissions are re-checked at resume time.
- **Multiple pause/resume cycles** are supported. The worker must be
  prepared to find `paused` state on its next tick after any active
  operation (i.e., the pause can hit between any two I/O operations).

### 4.3 Resume lifecycle

```
User action (UI "Resume" button)
  │
  ▼
Invoke Service::resume_download(id)
  │
  ▼
Storage::resume_download(id)
  ├─ Read current state
  ├─ Reject if terminal or not 'paused'         → error
  ├─ No-op if already active (idempotent guard)
  └─ UPDATE state = 'resolving_peer'
      │
      ▼
Worker picks up the resumed download:
  1. Resolve peer address (peer may be offline)
  2. Request new SignedDownloadDescriptor
     a. Open /boru-file-access/1 QUIC connection
     b. Send FileAccessRequest { content_hash, expected_revision }
     c. Receive signed descriptor (or error)
  3. Storage::accept_resumed_descriptor(id, content_hash, total_bytes)
     ├─ Hash match   → state = 'downloading', proceed to blob transfer
     ├─ Hash mismatch → state = 'version_mismatch', abort with notification
     └─ Descriptor expired → state remains 'paused', user must retry
  4. Start iroh-blobs transfer from resumed bytes_downloaded offset
     (iroh-blobs 0.103.0 chunk-level reuse)
  5. Verify and install (same path as a fresh download)
```

---

## 5. Error handling

### 5.1 Terminal-state rejection

| Scenario | Behaviour |
|----------|-----------|
| Pause a `complete` download | Error: `"cannot pause terminal download in state complete"` |
| Pause a `cancelled` download | Error: `"cannot pause terminal download in state cancelled"` |
| Pause a `failed` download | Error: `"cannot pause terminal download in state failed"` |
| Pause a `version_mismatch` download | Error: `"cannot pause terminal download in state version_mismatch"` |
| Pause a non-existent download | Error: `"download not found"` |

### 5.2 Stale worker race detection

| Scenario | Guard | Behaviour |
|----------|-------|-----------|
| Worker writes progress after pause | SQL `WHERE state != 'paused'` | Zero rows → caller gets `"download is paused; active work must be cancelled"` |
| Worker calls `fail_download` after pause | SQL `WHERE state != 'paused'` | Same |
| Worker calls `complete_download` after pause | SQL `WHERE state NOT IN ('complete', 'cancelled', 'paused')` | Same |

The worker SHOULD treat this error as a signal to cancel in-flight
operations.  It MUST NOT retry the write.

### 5.3 Resume descriptor mismatch

| Scenario | Behaviour |
|----------|-----------|
| Content hash changed while paused | State → `version_mismatch`, `last_error` = `"resume descriptor content hash mismatch"` |
| Descriptor was already expired at resume time | State stays `'paused'`, `last_error` = `"resume descriptor expired"` |
| Peer unreachable at resume time | Worker marks `Failed` with `PeerOffline`; retry policy applies |

### 5.4 Idempotency guarantees

| Operation | Idempotent? | Rationale |
|-----------|-------------|-----------|
| `pause_download(id)` | Yes (paused → paused is a no-op) | User may double-click pause, or UI may debounce imperfectly |
| `resume_download(id)` | Yes (non-paused active → no-op) | Dispatcher may tick the same download twice |
| `accept_resumed_descriptor(id, ...)` | No (rejected if already `downloading`) | Once accepted, a duplicate descriptor is a programming error |

---

## 6. Integrity requirements

### 6.1 Persistence guarantees

1. **Atomic state transitions.** Each SQL `UPDATE` is an atomic
   single-statement write that either succeeds entirely or fails without
   partial effect.  No pause/resume operation touches more than one row.

2. **WAL mode durability.** The SQLite database uses WAL journaling. A
   successful `pause_download` call implies the paused state is durable
   across process crashes.  No additional fsync coordination is needed.

3. **No cascading writes.** Pausing one download has zero side effects on
   other downloads, outbox entries, or any other table.  Each download's
   state is independent.

4. **Temporary file cleanup is caller responsibility.** The database does
   not track intermediate `.part` file paths.  The download worker must
   track and clean up temporary files on cancellation.  This is by design:
   the SQLite row is the system of record for state; the filesystem is a
   transient working artifact.

5. **Process restart recovery.** On startup, the storage layer reads all
   downloads.  Downloads in `'paused'` state remain paused — they are not
   auto-resumed.  The UI layer is responsible for presenting paused
   downloads and offering a resume action.

### 6.2 Data integrity verification

- **Content hash is always preserved** during pause/resume cycles.
  `accept_resumed_descriptor` enforces hash match before allowing transfer
  to resume.  If the remote peer's file changed while paused (new content
  hash), the download transitions to `version_mismatch` rather than
  downloading stale content.
- **Downloading incorrect content.** The existing `verify_download_file`
  path (BLAKE3 hash + size check before atomic install) is unaffected by
  pause — all post-transfer verification runs identically whether the
  download was paused or not.

---

## 7. Extensibility to other subsystems

The same pattern (active state → `paused` → retained metadata → resume
re-establishes preconditions) applies to other background work in the
system.  Each extension adds a `paused` variant to that subsystem's state
machine and implements the guard-plus-resume protocol.

### 7.1 Outbox delivery worker pause

| Item | Design |
|------|--------|
| State field | `OutboxRow::state` — add `'paused'` alongside `'pending'`, `'in_flight'`, `'acknowledged'`, `'failed'` |
| Preserved on pause | Recipient peer, envelope bytes, retry count, next_retry_at_ms, delivery lease |
| Guard | `claim_next_pending` skips `'paused'` rows; `advance_delivery_state` rejects writes on paused rows |
| Resume | Re-enter `'pending'` with preserved retry schedule; next `claim_next_pending` tick picks it up |
| Active-work cancellation | Send abort signal to `OutboxDeliveryWorker` for that peer's in-flight delivery |

### 7.2 DHT publication loop pause

| Item | Design |
|------|--------|
| State | `PublishState::Paused` in `public_room_continuous` state machine |
| Preserved on pause | Last published revision, backoff timer |
| Guard | Periodic republish timer skips paused topic |
| Resume | Restart the republish timer with preserved backoff |

### 7.3 File indexer pause

| Item | Design |
|------|--------|
| State | `IndexerState::Paused` in `file_indexer` |
| Preserved on pause | In-memory file index (unchanged) |
| Guard | Filesystem watcher (`notify`) events are buffered but not processed |
| Resume | Drain buffered events and rebuild index diff |

### 7.4 Backfill server pause

| Item | Design |
|------|--------|
| State | Semaphore-based concurrency limit in `backfill.rs` becomes a `BackfillState::Paused` flag |
| Preserved on pause | None (backfill is stateless request/response) |
| Guard | Accept new QUIC connections but reject immediately with `BackfillError::Paused` |
| Active-work cancellation | Drop `Semaphore::acquire` future, cancel in-flight response streaming |

---

## 8. Test coverage requirements (for child task `t_b4a9ecc4`)

The following scenarios MUST have automated coverage:

| # | Scenario | Verifies |
|---|----------|----------|
| 1 | Pause during `resolving_peer` | Pause works from non-transfer states |
| 2 | Pause during `requesting_permission` | Pause works mid-permission flow |
| 3 | Pause during `downloading` | Pause works during active byte transfer |
| 4 | Repeated pause (idempotent) | No-op on already-paused row |
| 5 | Pause rejects terminal states | `complete`, `cancelled`, `failed` cannot be paused |
| 6 | Stale progress race | `update_download_progress` after pause returns error |
| 7 | Resume transitions to `resolving_peer` | Resume does not skip re-resolution |
| 8 | Resume preserved `bytes_downloaded` | Partial progress survives pause/resume cycle |
| 9 | Resume with hash mismatch → `version_mismatch` | Content change during pause is detected |
| 10 | Resume with expired descriptor | Expired descriptor leaves state as `paused` |
| 11 | Resume on terminal download | Error on `complete`/`cancelled`/`failed` resume |
| 12 | Resume on non-existent download | Error on `i64::MAX` |
| 13 | Multiple pause/resume cycles | Full cycle works repeatedly |
