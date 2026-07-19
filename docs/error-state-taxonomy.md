# Error-State Taxonomy and Recovery Guidance

Status: **specification** — source of truth for implementing and testing
user-facing error states across file sharing, catalogue retrieval, and delivery.

---

## 1. Scope

This document defines every **user-facing error state** that reaches the UI or
status display.  It covers three protocol/domain layers:

| Domain | Source enums |
|--------|-------------|
| **Download (file transfer)** | `FileAccessResponse`, `TransferProgress`, `DownloadState` |
| **Catalogue retrieval** | `RemoteCatalogueFetchError`, `CatalogErrorCode`, `FileAccessErrorCode` |
| **Delivery (outbox)** | `DeliveryFailure`, `FailureClass` |

Each state is assigned a stable identifier, a user-facing title and message, its
temporal nature, the recommended recovery action, whether automatic retry is
appropriate, and any required secondary action (e.g. requesting permission or
refreshing content).

---

## 2. Stable error identifiers

Every error state below has a **machine-readable identifier** (`code`) that
doubles as the wire-safe serialisation string and the `last_error`/`last_error_code`
database value.  These are **stable once published** — they must not be renamed
between releases.

---

## 3. Error states

### 3.1 Permission Denied

| Field | Value |
|-------|-------|
| **Code** | `permission_denied` |
| **Sources** | `FileAccessResponse::PermissionDenied`, `CatalogErrorCode::PermissionDenied`, `RemoteCatalogueFetchError::PermissionDenied`, `FileAccessErrorCode::PermissionDenied` |
| **Title** | Access denied |
| **Message** | You do not have permission to download this file. The owner may have revoked access or blocked your account. |
| **Temporality** | Permanent* — retry without action will not succeed |
| **Recovery action** | Contact the file owner and ask them to grant access |
| **Auto-retry** | No — `FailureClass::RetryableOnlyAfterUserAction` |
| **Secondary action** | Refresh the catalogue to confirm the owner's current access policy |

> *A permission grant can be re-issued later, but the system cannot infer this
> without the owner taking an explicit action.  The UI should **not** show an
> auto-retry countdown — only a manual "Refresh permissions" button.

**UI rendering** (from `DeliveryFailure` classification):

```
Failed — Access denied. Ask the owner to grant you permission, then refresh.
```

**Distinction** from `NotFound`: the file exists but the sender has explicitly
blocked or not granted access to this peer.

---

### 3.2 File Not Found (Remote)

| Field | Value |
|-------|-------|
| **Code** | `not_found` |
| **Sources** | `FileAccessResponse::NotFound`, `CatalogErrorCode::NotFound`, `FileAccessErrorCode::NotFound` |
| **Title** | File not found |
| **Message** | The file could not be found on the remote peer. It may have been deleted or the shared-file ID is incorrect. |
| **Temporality** | Permanent* |
| **Recovery action** | Ask the owner to re-share the file with a fresh link |
| **Auto-retry** | No — `FailureClass::Permanent` |
| **Secondary action** | Refresh the catalogue to see if the entry still exists |

> *A file can be re-uploaded under the same ID, but the system cannot
> distinguish "permanently deleted" from "temporarily gone, owner will restore."
> Default to permanent; the owner can issue a fresh share.

**UI rendering:**

```
Failed — File not found. The file may have been removed. Ask the owner to re-share.
```

**Distinction** from `PermissionDenied`: the server has no record of this
shared-file ID at all — no access decision was made.

---

### 3.3 File Removed (Local)

| Field | Value |
|-------|-------|
| **Code** | `file_removed` |
| **Sources** | `room_cleanup.rs` room/history file removal checks, local file-system checks (file_library.rs availability: "Missing") |
| **Title** | File removed from device |
| **Message** | The local copy of this file has been removed or is no longer available on this device. |
| **Temporality** | Permanent — the file is gone from local storage |
| **Recovery action** | Re-download from a peer who still has a copy |
| **Auto-retry** | No |
| **Secondary action** | If this was a managed file, check `downloads` history for a re-download path |

**UI rendering:**

```
File missing — The local copy was removed. Re-download from a peer that still has it.
```

**Distinction** from `not_found`: the content was known locally but was
explicitly deleted or cleaned up.  `not_found` is a remote server response;
`file_removed` is a local observation.

---

### 3.4 File Changed (Remote Content Hash Mismatch)

| Field | Value |
|-------|-------|
| **Code** | `changed` |
| **Sources** | `FileAccessResponse::Changed` |
| **Title** | File changed since catalogue |
| **Message** | The file content has changed since the catalogue was issued. The catalogue entry is stale. |
| **Temporality** | Permanent for stale catalogue — the catalogue must be refreshed |
| **Recovery action** | Refresh the catalogue, then request the download again |
| **Auto-retry** | No — the caller must first obtain a fresh catalogue |
| **Secondary action** | Trigger a catalogue refresh (`fetch_remote_catalogue` with no `known_revision` shortcut) |

**UI rendering:**

```
File changed — The catalogue is outdated. Refresh and try again.
```

**Distinction** from `version_mismatch`: `Changed` means the **content hash**
itself is different — the underlying blob is not what the catalogue advertised.
`VersionMismatch` means the **version number** in the access request doesn't
match (see 3.5 below).  A catalogue refresh resolves `Changed`; a version bump
resolves `VersionMismatch`.

---

### 3.5 Version Mismatch

| Field | Value |
|-------|-------|
| **Code** | `version_mismatch` |
| **Sources** | `DownloadState::VersionMismatch`, `FileAccessResponse::VersionMismatch { current_version }` |
| **Title** | Version mismatch |
| **Message** | The file was updated while the download was in progress. The requested version no longer matches the current version on the server. |
| **Temporality** | Terminal — the download row cannot be resumed or retried in its current state |
| **Recovery action** | Request a fresh download of the updated file |
| **Auto-retry** | No — a terminal state (`DownloadState::VersionMismatch` is in `is_terminal()`) |
| **Secondary action** | The handler returned `current_version` — the UI can show "Server has version v{current_version}" to differentiate from a stale catalogue |

**UI rendering:**

```
Failed — Version mismatch: the file was updated during transfer. Download the new version from the catalogue.
```

**Distinction** from `changed`: `VersionMismatch` occurs when the *expected
version* sent in `FileAccessRequest.expected_version` does not match
`row.updated_at_ms` on the server.  `Changed` occurs when the *expected content
hash* does not match the stored hash.  The two checks are independent — a file
can change version without changing hash (metadata-only change) or change hash
(blob replaced).

From the pause-mechanism docs (terminal diagram):

```
Queue → ResolvingPeer → RequestingPermission → Downloading → Verifying → Complete
                                                       ↓ (hash mismatch)
                                                 VersionMismatch
```

---

### 3.6 Source Unavailable (Remote)

| Field | Value |
|-------|-------|
| **Code** | `unavailable` |
| **Sources** | `FileAccessResponse::Unavailable` |
| **Title** | File temporarily unavailable |
| **Message** | The file is not currently available on the remote peer. The file object may have been removed or the peer's storage is not reachable. |
| **Temporality** | Transient — the peer may make the file available again |
| **Recovery action** | Try again later, or contact the owner |
| **Auto-retry** | Yes — short backoff (30 s → 2 min → 5 min) |
| **Secondary action** | The UI may offer a "Retry now" button alongside the auto-retry countdown |

**UI rendering:**

```
Failed — File temporarily unavailable. Auto-retrying… (try again in N s)
```

**Distinction** from `not_found`: the shared-file ID is valid but the
underlying `file_object` row is missing or the file cannot be prepared
(e.g. referenced file path no longer exists on the owner's filesystem).
`not_found` means the shared-file ID itself is unrecognised.

---

### 3.7 File Sharing Disabled

| Field | Value |
|-------|-------|
| **Code** | `disabled` |
| **Sources** | `FileAccessResponse::Disabled` |
| **Title** | Sharing disabled |
| **Message** | The owner has disabled sharing for this file. |
| **Temporality** | Permanent — until the owner re-enables the offer |
| **Recovery action** | Contact the owner to re-enable sharing |
| **Auto-retry** | No |
| **Secondary action** | Refresh the catalogue to confirm the current offer state |

**UI rendering:**

```
Failed — Sharing is disabled. Ask the owner to enable sharing, then refresh.
```

**Distinction** from `permission_denied`: the offer itself is toggled off,
not a per-peer access decision.

---

### 3.8 Peer Offline / Unreachable

| Field | Value |
|-------|-------|
| **Code** | `peer_offline` |
| **Sources** | `DeliveryFailure::PeerOffline`, `DeliveryFailure::AddressUnavailable`, `DeliveryFailure::ConnectionFailed` |
| **Title** | Peer offline |
| **Message** | The recipient peer is not currently reachable. They may be offline or behind a restrictive network. |
| **Temporality** | Transient — the peer may come online |
| **Recovery action** | Wait for the peer to come online. The message will be delivered automatically when the peer is reachable again. |
| **Auto-retry** | Yes — exponential backoff with jitter, bounded by the message TTL (`DeliveryFailure::PeerOffline` → `FailureClass::Transient`) |
| **Secondary action** | The UI may show the delivery status: "Waiting for {name} to come online…" |

**UI rendering** (from `outbox_delivery.rs`):

```
Delivery pending — {name} is offline. Message will be delivered automatically when they're available.
```

In a **failed download** context (a download that could not start because the
provider peer is unreachable):

```
Failed — Download source is offline. Auto-retrying… (next attempt in N s)
```

**Distinction** from `unavailable`: `peer_offline` means we cannot reach the
peer at all (QUIC connection fails).  `unavailable` means we reached the peer
but the file object is gone.

**Sibling codes** (same recovery pattern — all `FailureClass::Transient`):

| Code | When |
|------|------|
| `address_unavailable` | No usable addresses exist for the peer |
| `connection_failed` | Connection attempt failed before protocol exchange |
| `timeout` | Operation timed out |
| `relay_unavailable` | The configured relay could not be reached |

All four surface the same user-facing message with the same retry policy.

---

### 3.9 Verification Failure (Hash / Size Mismatch)

| Field | Value |
|-------|-------|
| **Code** | `verification_failed` |
| **Sources** | `ImportError::VerificationFailed { expected_hash, actual_hash }`, `verify_download_file()` (size mismatch or BLAKE3 hash mismatch) |
| **Title** | Verification failed |
| **Message** | The downloaded file failed integrity verification. The content received does not match the expected hash or size. The transfer may have been corrupted. |
| **Temporality** | Transient — a fresh transfer may succeed |
| **Recovery action** | Retry the download |
| **Auto-retry** | Yes (up to 3 attempts) — corruption is often transient in content-addressed transfers; a re-request fetches fresh chunks |
| **Secondary action** | After 3 failed verifications, transition to `Failed` with `last_error = "verification_failed: retries exhausted"`. The UI should then offer "Retry manually" instead of auto-retry. |

**UI rendering** (first failure):

```
Verification failed — The downloaded file was corrupted. Retrying… (attempt N of 3)
```

After retries exhausted:

```
Verification failed — The file could not be verified after 3 attempts. Try again later.
```

**Distinction** from `version_mismatch`: verification failure is about *bytes on
disk* — the temporary file does not match the expected size or BLAKE3 hash.
`VersionMismatch` is about protocol state — the server's current version no
longer matches what the requester specified.  A verification-failed download
can be retried without changing anything on the server; a `VersionMismatch`
download cannot because the agreed-upon version is no longer valid.

**Technical detail**: `verify_download_file` checks size first, then hashes.
If size does not match, the error message is:

```
verification_failed: expected {size} bytes, got {actual}
```

If size matches but hash does not:

```
verification_failed: expected hash {expected}, got {actual}
```

The UI should collapse both to the same user-facing title; the detailed message
is available for diagnostics (e.g. a "Show details" expander).

---

### 3.10 Source Busy / Rate Limited

| Field | Value |
|-------|-------|
| **Code** | `busy` / `rate_limited` |
| **Sources** | `FileAccessResponse::Busy`, `FileAccessResponse::RateLimited`, `CatalogErrorCode::Busy`, `CatalogErrorCode::RateLimited`, `FileAccessErrorCode::Busy`, `FileAccessErrorCode::RateLimited` |
| **Title** | Server busy / Rate limited |
| **Message** | The peer is busy or has rate-limited your requests. Please wait before trying again. |
| **Temporality** | Transient — the peer's concurrency/rate limit will clear |
| **Recovery action** | Wait and retry |
| **Auto-retry** | Yes — exponential backoff with jitter (longer base than `peer_offline`: 60 s → 5 min → 15 min) |
| **Secondary action** | None needed — the peer will accept requests again once the throttling window passes |

**UI rendering:**

```
Server busy — The remote peer is handling many requests. Auto-retrying… (next attempt in N s)
```

---

### 3.11 Service Unavailable (Protocol / Relay)

| Field | Value |
|-------|-------|
| **Code** | `relay_unavailable` / `protocol_rejected` / `invalid_recipient_state` |
| **Sources** | `DeliveryFailure::RelayUnavailable`, `DeliveryFailure::ProtocolRejected`, `DeliveryFailure::InvalidRecipientState` |
| **Title** | Service unavailable |
| **Message** | The delivery could not be completed due to a network or protocol issue. |
| **Temporality** | Depends on the specific code |
| **Recovery action** | Wait for service to recover or update to a compatible protocol version |
| **Auto-retry** | `relay_unavailable` → Yes (Transient); `protocol_rejected` → No (Permanent); `invalid_recipient_state` → No (RetryableOnlyAfterUserAction) |
| **Secondary action** | `protocol_rejected`: suggest the user update their app. `invalid_recipient_state`: suggest the recipient check their settings. |

**UI rendering:**

```
relay_unavailable: Service temporarily down — Auto-retrying…
protocol_rejected: Protocol version incompatible — Update your app to continue.
invalid_recipient_state: Recipient cannot accept messages right now.
```

---

### 3.12 Catalogue Revision Changed

| Field | Value |
|-------|-------|
| **Code** | `revision_changed` |
| **Sources** | `RemoteCatalogueFetchError::RevisionChanged { new_revision }` |
| **Title** | Catalogue updated |
| **Message** | The file catalogue was updated during browsing. The new revision (v{new_revision}) is available. |
| **Temporality** | Transient — the catalogue must be re-fetched |
| **Recovery action** | Re-fetch the catalogue from the beginning |
| **Auto-retry** | Yes — immediately re-fetch without user interaction (the UI should show a brief "Refreshing catalogue…" indicator) |
| **Secondary action** | The paginated scan is automatically restarted |

**UI rendering:**

```
Catalogue updated — Refreshing to show the latest files…
```

---

### 3.13 Catalogue Not Modified

| Field | Value |
|-------|-------|
| **Code** | `not_modified` |
| **Sources** | `RemoteCatalogueFetchError::NotModified` |
| **Title** | Catalogue unchanged |
| **Message** | The file catalogue has not changed since your last refresh. |
| **Temporality** | Informational — not an error |
| **Recovery action** | No action needed |
| **Auto-retry** | N/A — this is a success signal, not an error |

---

### 3.14 Local Storage Failure

| Field | Value |
|-------|-------|
| **Code** | `local_storage_failure` |
| **Sources** | `DeliveryFailure::LocalStorageFailure`, `ImportError::DatabaseError`, `ImportError::CreateDirFailed`, `ImportError::RenameFailed` |
| **Title** | Storage error |
| **Message** | A local database or filesystem error occurred. The operation could not be completed. |
| **Temporality** | Transient (disk pressure) / Permanent (corruption) |
| **Recovery action** | Check available disk space and file permissions. Restart the app if the issue persists. |
| **Auto-retry** | Depends — disk full will not recover without user action; transient I/O contention may clear |
| **Secondary action** | Expose the OS-level error message in a "Show details" expander |

**UI rendering:**

```
Storage error — {operation} failed: {os_error}. Check disk space and permissions.
```

---

### 3.15 Internal / Unclassified Error

| Field | Value |
|-------|-------|
| **Code** | `internal_error` |
| **Sources** | `FileAccessErrorCode::InternalError`, `CatalogErrorCode::InternalError`, `DeliveryFailure::InternalError`, all unknown/fallback deserialisations |
| **Title** | Unexpected error |
| **Message** | An unexpected internal error occurred. No details are disclosed for security. |
| **Temporality** | Transient (best guess — treat as transient when in doubt) |
| **Recovery action** | Try again. If the issue persists, restart the app and contact support. |
| **Auto-retry** | Yes (up to 2 attempts) — if it fails again, escalate to `FailureClass::Permanent` |
| **Secondary action** | The app logs contain full details; include a correlation ID if available |

**UI rendering:**

```
Unexpected error — Something went wrong. Try again, or restart if it persists.
```

---

## 4. Classification matrix

| State | Temporality | Auto-retry | Retry policy | User action needed? |
|-------|-------------|------------|--------------|--------------------|
| `permission_denied` | Permanent | No | `RetryableOnlyAfterUserAction` | Yes — contact owner |
| `not_found` | Permanent | No | `Permanent` | Yes — ask for re-share |
| `file_removed` | Permanent | No | `Permanent` | Yes — re-download |
| `changed` | Permanent (stale catalogue) | No | — | Yes — refresh catalogue |
| `version_mismatch` | Terminal | No | — | Yes — re-download fresh |
| `unavailable` | Transient | Yes | Incremental backoff 30 s → 2 m → 5 m | Optional — retry button |
| `disabled` | Permanent | No | `RetryableOnlyAfterUserAction` | Yes — contact owner |
| `peer_offline` | Transient | Yes | Exponential backoff, bounded by TTL | Optional — wait |
| `verification_failed` | Transient | Yes (up to 3) | Fixed backoff 10 s | After exhaustion |
| `busy` / `rate_limited` | Transient | Yes | Exponential backoff 60 s → 5 m → 15 m | No |
| `relay_unavailable` | Transient | Yes | Standard exponential backoff | No |
| `protocol_rejected` | Permanent | No | `Permanent` | Yes — update app |
| `revision_changed` | Transient | Yes (immediate) | Immediate re-fetch | No |
| `local_storage_failure` | Depends | Conditional | — | Yes — check disk |
| `internal_error` | Unknown | Yes (2 attempts) | Fallback to `Permanent` after exhaustion | Yes — restart |

---

## 5. Distinction boundaries

These are the key distinctions the UI must maintain:

| Confusable pair | How to distinguish |
|-----------------|-------------------|
| `not_found` vs `permission_denied` | `not_found` = server has no record of the ID; `permission_denied` = server knows the ID but access is blocked |
| `not_found` vs `file_removed` | `not_found` = remote server response; `file_removed` = local observation (downloaded copy was deleted from this device) |
| `changed` vs `version_mismatch` | `changed` = content hash differs (entirely different blob); `version_mismatch` = version number differs (blob may or may not have changed) |
| `version_mismatch` vs `verification_failed` | `version_mismatch` = protocol/state check (cannot be retried); `verification_failed` = integrity check (can be retried) |
| `unavailable` vs `peer_offline` | `unavailable` = file specifically is gone from the peer we reached; `peer_offline` = we could not reach the peer at all |
| `peer_offline` vs `relay_unavailable` | `peer_offline` = the specific peer is unreachable; `relay_unavailable` = the infrastructure relay is down (affects all peers using it) |

---

## 6. UI rendering guidance

### 6.1 Color / severity mapping

| Severity | Tone color | Icon metaphor |
|----------|-----------|---------------|
| Transient (will auto-resolve) | Yellow/amber `#D4A72C` | Clock / hourglass |
| Perma-fail (user action required) | Red `#CC3333` | Lock / shield |
| Terminal (data lost, cannot retry) | Red `#CC3333` plus bold | X-mark / broken link |
| Informational (not an error) | Gray `#999999` | Info circle |

### 6.2 Message structure

Every error display should follow the pattern:

```
Failed — {title}. {action}.
```

Where `{title}` is the single-line title from the taxonomy, and `{action}` is
the recommended recovery action.  If auto-retry is active, append:

```
Auto-retrying… (next attempt in N s)
```

### 6.3 Auto-retry countdown

When the state has `Auto-retry: Yes`, the UI should display a live countdown
to the next retry.  The countdown text replaces the stale `N s` placeholder at
least every second.  When the countdown reaches zero and the retry fails, the
countdown restarts with the new backoff interval.

---

## 7. Implementation checklist for tests

Each error state must have at least one test verifying:

1. The stable code serialises and deserialises correctly (no breaking renames).
2. The UI renders the correct title and message for that code.
3. The auto-retry behaviour matches the policy (retry/no-retry, backoff interval).
4. For confusable pairs (section 5), a test verifies the correct state transitions
   are produced for each distinct scenario.

---

## 8. Derivation from codebase

This taxonomy was derived from the following source enums and structs (current
as of commit `794cf1f`):

- `src/download.rs:14` — `DownloadState` (Queued → Verifying → VersionMismatch)
- `src/file_access.rs:22` — `FileAccessErrorCode` (wire-safe error codes)
- `src/file_access_protocol.rs:370` — `FileAccessResponse` (Granted, PermissionDenied, NotFound, Disabled, Changed, Unavailable, Busy, RateLimited, VersionMismatch)
- `src/catalogue_protocol.rs:30` — `CatalogErrorCode` (PermissionDenied, NotFound, InvalidRequest, UnsupportedVersion, RateLimited, Busy, ResponseTooLarge, InternalError)
- `src/catalogue_client.rs:43` — `RemoteCatalogueFetchError` (PermissionDenied, NotModified, IncompleteCatalogue, RevisionChanged, ConnectionFailed, Timeout, SignatureInvalid, ProtocolError)
- `src/outbox_delivery.rs:150` — `FailureClass` (Transient, Permanent, RetryableOnlyAfterUserAction)
- `src/outbox_delivery.rs:161` — `DeliveryFailure` (PeerOffline, AddressUnavailable, ConnectionFailed, Timeout, RelayUnavailable, ProtocolRejected, Unauthorised, InvalidRecipientState, MessageExpired, ContactRevoked, PayloadTooLarge, LocalStorageFailure, InternalError)
- `src/file_access_handler.rs:740` — `check_permission` response mapping
- `examples/iced_chat/file_library_ops.rs:275` — `ImportError` (VerificationFailed, HashFailed, CopyFailed, Cancelled, etc.)
- `examples/iced_chat/app.rs:513` — UI `DownloadState` (Ready, Active, Completed, Failed, Cancelled)
- `src/chat_callbacks.rs:75` — `TransferProgress` (Started, Progress, Completed, Failed, Cancelled)
