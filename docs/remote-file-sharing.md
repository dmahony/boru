# Remote File Sharing: Milestone Architecture Note

Status: **component boundaries, written before implementation starts**. The test
files, ALPN constants, storage schema, and protocol-layer specification exist.
The handler/client modules (`catalogue_handler`, `catalogue_client`,
`catalogue_model`, `catalogue_protocol`, `download_manager`, `download`,
`file_access_handler`, `protocol_version`) have tests that reference their public
APIs but the module implementations themselves are not yet built. This note
defines each component's boundary — what it owns, what it delegates, and how
they connect — to guide implementation.

## Core principle: catalogue visibility and download permission are separate decisions

A peer who can **see** a file in a catalogue cannot necessarily **download** it.
Visibility is a read-time decision based on relationship and offer state;
download permission is a separate request-time decision that re-evaluates the
same checks plus availability, integrity, and concurrency. The two protocols
(`/boru-file-catalog/1` and `/boru-file-access/1`) are independent QUIC ALPNs
with different handlers and different response types. A client must never assume
that catalogue visibility implies download authorisation.

## Subsystem overview

```
Peer A (requester)                          Peer B (owner)
      │                                           │
      │  1. Revision notification (advisory)      │
      │◄──────────────────────────────────────────│
      │                                           │
      │  2. Catalogue retrieval                   │
      │──────────────────────────────────────────►│
      │  (/boru-file-catalog/1)                   │
      │◄──────────────────────────────────────────│
      │  signed snapshot (paginated)              │
      │                                           │
      │  3. Cache in Storage                      │
      │  (local SQLite)                           │
      │                                           │
      │  4. Download authorisation                │
      │──────────────────────────────────────────►│
      │  (/boru-file-access/1)                    │
      │◄──────────────────────────────────────────│
      │  signed short-lived descriptor            │
      │                                           │
      │  5. Blob transfer (iroh-blobs)            │
      │──────────────────────────────────────────►│
      │  content-addressed stream                 │
      │◄──────────────────────────────────────────│
      │                                           │
      │  6. Verification                          │
      │  (size + BLAKE3 hash)                     │
```

## Component boundaries

### 1. Catalogue notification

| Property | Value |
|----------|-------|
| Owner | TBD — either embedded in `AboutMe` gossip broadcasts or a dedicated notification |
| ALPN | None (piggybacks on existing gossip/presence) |
| Direction | Owner → requester (advisory one-shot) |
| Persistence | None (transient) |

**Responsibility:** Inform subscribed peers that an owner's file catalogue
revision has changed. The notification is purely advisory — it contains no file
metadata, no download grant, and no file bytes. Its only purpose is to let the
recipient know they should fetch a fresh catalogue if they care.

**Design options (not yet settled):**
- Embed the current catalogue revision in periodic `AboutMe` gossip messages.
  The recipient compares against their cached revision and skips the fetch if
  unchanged. This avoids a dedicated notification protocol but increases gossip
  payload size.
- A dedicated `/boru-file-notify/1` ALPN that the owner calls once per peer
  after a change. More targeted but adds another QUIC protocol.

**What it does NOT do:**
- No file metadata or download grant is carried in the notification
- No bytes, hashes, names, or permissions
- The recipient must still authenticate to fetch the catalogue

### 2. Catalogue retrieval

| Property | Value |
|----------|-------|
| Module | `catalogue_handler` (server), `catalogue_client` (client) |
| ALPN | `/boru-file-catalog/1` (`FILE_CATALOG_ALPN` in `net.rs`) |
| Direction | Requester-initiated QUIC bi-stream, owner responds |
| Persistence | None on the server; client caches in `Storage` |

**Responsibility:** Serve a signed, requester-filtered snapshot of the owner's
offered files. The `CatalogueHandler` opens a bi-directional QUIC stream,
authenticates the requester via `Connection::remote_id()`, looks up their
relationship in `FriendsStore`, and builds a projection of files the requester
is allowed to see.

**Requester filtering rules (in order):**
1. Blocked peers → `PermissionDenied` error
2. Confirmed friends → enabled, available offers (default)
3. Non-friend peers → only files with an explicit `read` permission in
   `shared_file_permissions`
4. Disabled offers, unavailable file objects (`file_objects` row missing),
   empty collections → silently omitted (never included)

**Pagination:** The response is a `CatalogResponse::CataloguePage` containing a
signed `SignedFileCatalogue` slice with `items`, `next_cursor`, `revision`, and
`has_more`. The client iterates with `GetCataloguePage { known_revision,
cursor, page_size }`. If the server's revision changes between pages, the
server returns `RevisionChanged { new_revision }` and the client must restart.

**NotModified:** The client can pass `known_revision` in the request. If the
catalogue revision has not changed, the server returns
`CatalogResponse::NotModified` rather than re-sending the full page.

**Key types (test-only, not yet implemented):**
- `CatalogRequest::GetCataloguePage`
- `CatalogResponse::CataloguePage`, `CatalogResponse::NotModified`,
  `CatalogResponse::RevisionChanged`
- `SignedFileCatalogue` (owner-signed, verifiable)
- `RemoteSharedFile` (file metadata visible to the requester)

**Test coverage:** `tests/test_remote_catalogue_integration.rs` — 10 scenarios
covering visibility, denial, revision changes, pagination, offline cache.

**What it does NOT do:**
- No download permission is implied by catalogue visibility
- No blob hashes available to iroh-blobs are exposed in the catalogue
- No blob tickets live in the catalogue response

### 3. Catalogue cache

| Property | Value |
|----------|-------|
| Owner | `Storage` (SQLite: `remote_catalogues`, `remote_shared_files`,
          `remote_collections` tables) |
| Protocol | None (local-only) |
| Direction | Local read/write |
| Persistence | Durable across restarts |

**Responsibility:** Store the most recent verified catalogue from each peer so
the UI can display peer profiles without continuously re-fetching. The cache is
revision-keyed: a fetch that returns `NotModified` preserves the existing cache
instead of replacing it.

**Key operations:**
- `Storage::replace_remote_catalogue(SignedFileCatalogue)` — atomically replace
  the cached catalogue for a peer, bumping `cached_at_ms`. Rejects catalogues
  whose revision is ≤ the stored revision (prevents replay of old data).
- `Storage::get_remote_catalogue_meta(peer)` — returns revision, fetched_at,
  etc. for staleness checks.
- `Storage::get_remote_shared_files(peer)` — returns the file list from cache.
- `Storage::get_remote_collections(peer)` — returns collection metadata.

**Staleness:** The frontend considers a cache stale when `fetched_at_ms` is
more than 5 minutes old (constant `STALE_THRESHOLD_MS` in app.rs). A stale
cache triggers a refresh on next view. When the network is unavailable, the
stale cache is displayed with a warning rather than showing nothing.

**What it does NOT do:**
- No write-back or sync — the cache is a local projection
- No partial updates — always a full catalogue replacement
- No merging with other peers' catalogues

### 4. Download authorisation

| Property | Value |
|----------|-------|
| Module | `file_access_handler` (not yet implemented) |
| ALPN | `/boru-file-access/1` (`FILE_ACCESS_ALPN` in `net.rs`) |
| Direction | Requester-initiated QUIC bi-stream, owner responds |
| Persistence | None (descriptor is short-lived, ~5 minutes) |

**Responsibility:** At download-request time, re-verify the requester's
relationship, permission, offer status, file availability, content hash, and
file revision — then issue a signed, short-lived download descriptor. This is
the security gate: every download must pass through this check before iroh-blobs
transfer begins.

**Request flow:**
1. Requester opens QUIC bi-stream to the owner's `/boru-file-access/1` endpoint
2. Requester sends `FileAccessRequest { content_hash, expected_revision }`
3. Owner re-evaluates all checks at request time:
   - Is requester blocked? → `PermissionDenied`
   - Is the file still offered? → `FileNotOffered`
   - Does `file_objects` contain this hash? → `FileNotAvailable`
   - Does the current revision match `expected_revision`? → `RevisionMismatch`
   - Is the owner's download slot budget saturated? → `RateLimited`
4. On success, owner issues `SignedDownloadDescriptor`:
   - Bound to: owner identity, requester identity, content hash, expected size,
     blob format indicator
   - Temporal: issue timestamp, expiry timestamp (default 5 minutes)
   - Nonce: random 32 bytes to prevent descriptor replay
   - Signed by the owner's `SecretKey`
5. Owner may optionally prepare the file into iroh-blobs at this point
   (deferred preparation).

**What it does NOT do:**
- No blob bytes are transferred over this protocol
- No catalogue entries or file metadata are exposed
- Error responses intentionally hide whether a non-existent file exists
  (generic `FileNotAvailable` for missing content_hash, un-offered files,
  and unknown content_hashes)

### 5. File-byte transfer

| Property | Value |
|----------|-------|
| Owner | `iroh-blobs` (content-addressed blob protocol) |
| ALPN | `iroh-blobs` built-in ALPN |
| Direction | Client-initiated pull |
| Persistence | iroh-blobs store + `Storage::downloads` state machine |

**Responsibility:** Transfer raw file bytes from owner to requester via
iroh-blobs' content-addressed protocol. The requester holds a
`SignedDownloadDescriptor` (from step 4) and a `blob_hash` that resolves to the
file content.

**Flow:**
1. Requester calls `iroh_blobs::get::get_blob()` with the blob hash and the
   owner as a provider candidate
2. Owner's blob store serves verified content chunks
3. Requester's `DownloadManager` ticks the download through states:
   `Queued → RequestPermission → Validating → Transferring → Verifying → Complete`
4. Bytes are written to a temporary file during transfer

**`DownloadManager` state machine:**
```
Queued → RequestPermission → Validating → Transferring → Verifying → Complete
    │                          │               │             │
    ↓                          ↓               ↓             ↓
Cancelled               VersionMismatch     Failed        Failed
```
- `Queued`: initial state after `Storage::create_download()`
- `RequestPermission`: (stub in tests — the real `/boru-file-access/1` call lives here)
- `Validating`: check content hash against cached catalogue, abort on mismatch
- `Transferring`: active iroh-blobs download with progress tracking
- `Verifying`: post-download BLAKE3 hash and size check
- `Complete`: verified output atomically renamed to destination

**Progress:** `DownloadManager` emits `TransferProgress` events via
`ChatCallbacks::on_transfer_progress()` for UI consumption.

**Safety envelope:**
- `PublicRoomSafety::max_blob_size_bytes` is checked before download starts
  (function `download_blob_with_safety()` in `chat_core.rs`)
- Per-peer concurrency: at most one active blob transfer per peer
- Global concurrency: bounded by `PublicRoomConfig::max_concurrent_blob_downloads`

**Resume semantics (iroh-blobs 0.103.0):**
- The transfer is content-addressed by the blob hash. The downloader asks the
  local blob store which BAO chunks are already present and requests only the
  missing chunks; this includes chunks retained after an interrupted transfer.
- This is chunk-level reuse, not byte-range resume of the destination file. The
  application destination is still written only after the blob is complete and
  verified, so an interrupted destination write is not continued from its
  previous file offset.
- If the partial blob is garbage-collected, the next request has no local
  chunks to reuse and the transfer starts from the beginning. A repeated
  request for an already complete hash is satisfied locally without network
  transfer.

**What it does NOT do:**
- No authorisation — relies entirely on step 4 having issued a valid descriptor
- No catalogue visibility — operates purely on content hashes
- No byte-range resume of the temporary/destination output file

### 6. Verification

| Property | Value |
|----------|-------|
| Owner | `DownloadManager` (verification step of state machine) |
| Direction | Local (post-download) |
| Persistence | `DownloadState::Complete` or `DownloadState::Failed` |

**Responsibility:** After iroh-blobs delivers the full byte stream, verify
that the output matches the expected BLAKE3 content hash and size before
installing it at the destination path. Verification happens on a temporary
file; only verified output is atomically renamed to the final path.

**Verification steps (in order):**
1. Compare downloaded byte count against `expected_size` from the catalogue →
   mismatch marks `Failed` with `content_mismatch` error
2. Compute BLAKE3 hash of the full downloaded content → compare against
   `content_hash` from the catalogue → mismatch marks `Failed`
3. On both checks passing, atomically rename temp file to destination path
4. Record `DownloadState::Complete` in `Storage::downloads`
5. Emit `TransferProgress::Completed` via `ChatCallbacks`

**What it does NOT do:**
- No re-verification after `Complete` — the state is terminal
- No partial hash verification (content is fully hashed post-download)
- No signature verification of the bytes (blob integrity is content-addressed)

## Component interaction flow (complete download lifecycle)

```
1. Owner modifies shared files → bumps manifest revision
2. Owner's `AboutMe` gossip broadcast includes new revision number
   [catalogue notification]
3. Peer A (requester) sees revision changed → fetches `/boru-file-catalog/1`
   [catalogue retrieval]
4. Peer A stores signed catalogue in `Storage`
   [catalogue cache]
5. User views Peer A's profile → frontend reads cached catalogue
   (or triggers refresh if stale)
6. User clicks "Download" on a file → `Storage::create_download()` → Queued
7. `DownloadManager` ticks → opens `/boru-file-access/1` QUIC connection
   [download authorisation]
8. Owner checks relationship, offer, availability → issues signed descriptor
9. Requester validates descriptor → starts iroh-blobs transfer
   [file-byte transfer]
10. `DownloadManager` ticks → bytes stream to temp file
11. Post-download: verify BLAKE3 hash + size
    [verification]
12. If OK: atomic rename → `DownloadState::Complete`
    If FAIL: `DownloadState::Failed`, temp file removed
```

## Existing artifacts (pre-implementation)

These files already exist and define the contracts that the component
implementations must satisfy:

| Artifact | Content |
|----------|---------|
| `docs/protocol-layers.md` | ALPN definitions, protocol responsibilities, security properties |
| `tests/test_remote_catalogue_integration.rs` | 10 tests for catalogue visibility, denial, pagination, revision changes, offline |
| `tests/test_download_integration.rs` | 16 tests for download state machine (hash mismatch, auth denial, retry, pause/resume) |
| `tests/test_ui_file_sharing_integration.rs` | 11 tests for profile data flow, staleness, collection browsing |
| `tests/test_blob_size_enforcement.rs` | 3 tests for public-room blob size cap |
| `src/net.rs` (unstaged diff) | `FILE_CATALOG_ALPN`, `FILE_ACCESS_ALPN` constants with dedup tests |
| `src/storage.rs` | `file_objects`, `shared_files`, `shared_file_permissions`, `profile_manifest_state`, `downloads` tables |
| `src/chat_core.rs` | `download_blob_with_progress()`, `download_blob_with_safety()` |
| `src/chat_callbacks.rs` | `TransferId`, `TransferKind`, `TransferProgress` |
| `src/user_profile.rs` | `UserProfile`, `SharedFile`, `SharedFileMeta` |
| `src/file_indexer.rs` | Local shared-folder scanner and filesystem watcher |
| `docs/testing.md` | "Remote file-sharing tests" section (lines 175–205) |

## Implementation order (phases)

1. **Catalogue protocol types + handler + client** — `catalogue_model`,
   `catalogue_protocol`, `catalogue_handler`, `catalogue_client`, `protocol_version`
2. **Catalogue storage + cache** — `Storage` SQLite operations for
   `replace_remote_catalogue`, `get_remote_catalogue_meta`,
   `get_remote_shared_files`, `get_remote_collections`
3. **Catalogue integration tests** — `test_remote_catalogue_integration.rs`
4. **File access handler** — `file_access_handler` implementing
   `/boru-file-access/1` with permission re-check and signed descriptor issuance
5. **Download manager** — `download` and `download_manager` modules for the
   state machine, blob transfer orchestration, and verification
6. **Transfer integration tests** — `test_download_integration.rs`,
   `test_ui_file_sharing_integration.rs`
7. **Diagnostics + observability** — diagnostic events for each state transition
8. **Remove obsolete code** — prune old file-sharing paths superseded by
   the new architecture
9. **Documentation + release gate** — this note, protocol doc updates,
   testing doc updates
