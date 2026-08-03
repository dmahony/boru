# FS-01 — Existing file-sharing lifecycle and security model

## CARD / STATUS

- Card: FS-01 — Map the existing file-sharing lifecycle and security model
- Status: Discovery complete; implementation code was not changed.
- Repository: `/home/dan/iroh-gossip-chat`
- Upstream baseline: `docs/fs-00-baseline.md`, commit `a7f3aee4`
- Audit scope: current Boru core, Iced GUI, SQLite storage, catalogue/file-access protocols, transfer workers, and tests.

## SUMMARY

Boru has two materially different file paths:

1. The profile catalogue path is the intended authorised sharing design. A native OS file picker creates a local `file_objects` plus `shared_files` record. A requester receives a filtered, signed catalogue, then obtains a fresh recipient-bound, expiring descriptor from `/boru-file-access/1`; the blob transfer streams to a temporary file and completes only after size and BLAKE3 verification.
2. The current Iced peer-catalogue download button does not use that durable authorised path. `AppMessage::RequestFileDownload` calls the legacy `chat_core::download_blob_to_file` directly with a content hash and peer providers. It has progress callbacks and a local output file, but does not create a `downloads` row, request a signed descriptor, or perform request-time permission checks. This is the most important integration gap for the dashboard and must not be mistaken for the backend lifecycle.

The backend persists local offers, permissions, remote catalogue snapshots, and download rows. It does not persist a first-class "shared with me" relation separate from remote catalogue rows, nor a durable outbound-share/recipient transfer record. Active durable downloads do identify their source peer through `downloads.remote_peer`; active outbound serving requests identify the requester only in the live protocol connection/log context.

## 1. End-to-end sequence maps

### 1.1 Local item → profile catalogue offer

```text
Iced AddSharedFile
  -> rfd::AsyncFileDialog::pick_file (native OS picker)
  -> AppMessage::SharedFilePicked(path)
  -> spawn_blocking: metadata + full file read
  -> BLAKE3 content hash and metadata_id(filename, size, mtime)
  -> Storage::put_file_object(hash, size, mime, filename, data)
  -> Storage::set_file_object_source_path(hash, path)
  -> Storage::upsert_shared_file(hash, local_public, metadata_id, filename, ..., offered=true)
  -> AppMessage::SharedFileAdded; refresh Storage::list_shared_files(local, true)
```

Verified source: `examples/iced_chat/app.rs:16602-16721`.

The local `SharedFile` model in `src/user_profile.rs:360-459` contains `id`, `filename`, absolute/local `path`, size, MIME type, and validation flags. The active SQLite representation is `Storage::SharedFileRow` (`src/storage.rs:121-143`) joined to immutable/content-addressed `file_objects`. The GUI currently reads `SharedFileRow`; the legacy JSON `UserProfile.shared_files` is retained for compatibility, not the authoritative catalogue persistence.

The source path is stored locally in `file_objects.source_path`; it is not a wire field. `RemoteSharedFile` intentionally contains only `shared_file_id`, display name, description, MIME type, size, content hash, version/update metadata, and collection IDs (`src/catalogue_model.rs:134-164`).

### 1.2 Local offer → remote catalogue

```text
Remote peer opens QUIC /boru-file-catalog/1
  -> CatalogueHandler::accept authenticates Connection::remote_id()
  -> requester-specific FriendsStore blocked check
  -> Storage::catalogue_entries_for_peer(owner, requester, friends)
       * only offered=true rows with existing file_object
       * deny grant wins
       * explicit read grant permits selected peer
       * with no active grants, only Friends relationship permits
  -> CatalogueHandler builds CatalogueView
  -> SignedFileCatalogue signed by owner's SecretKey
  -> versioned CatalogWireResponse (possibly paginated / NotModified)
  -> catalogue client validates signature, owner, metadata, limits, duplicates, page revision
  -> process_and_store_remote_catalogue persists a complete snapshot
```

Server implementation: `src/catalogue_handler.rs:163-360`, especially `build_catalogue_for_requester`; storage projection: `src/storage.rs:3308-3373`; wire/client validation: `src/catalogue_client.rs:122-228, 460-509` and `src/catalogue_protocol.rs`.

`CatalogueHandler` signs a fresh envelope per requester and never reuses a catalogue signed for another requester. The manifest revision is used for cache/not-modified detection. A blocked requester receives `PermissionDenied`; a non-friend or non-granted peer receives an empty filtered view rather than hidden local fields.

### 1.3 Remote catalogue → durable download initiation (backend path)

```text
Verified catalogue is stored for owner public key
  -> initiate_download(storage, content_hash, remote_peer, known_size)
  -> require remote catalogue meta
  -> require matching RemoteSharedFileRow and non-empty MIME/name/non-zero size
  -> reject conflicting download state for same hash + peer
  -> Storage::create_download(..., state='queued')
  -> TRANSFER_TELEMETRY.download_queued(download_id, size, ...)
```

Implementation: `src/download_initiation.rs:118-263`. Durable row type: `Storage::Download` (`src/storage.rs:216-241`). The source peer is `remote_peer`, stored as the peer's public-key string.

The remote catalogue cache is represented in the same SQLite domain tables as local offers: `profile_manifest_state` stores remote catalogue metadata and `shared_files` rows are replaced as a complete snapshot by `Storage::replace_remote_catalogue` (`src/storage.rs:4510-4592`). `get_remote_shared_files` returns only content hash, display name, MIME type, and size (`src/storage.rs:4618-4646`). This is a local projection of "available from peer", not a separate `shared_with_me` table.

### 1.4 Queued download → permission → transfer → completion (backend path)

```text
DownloadManager::tick
  -> oldest queued row atomically becomes resolving_peer
  -> worker resolves source peer and enters requesting_permission
  -> FileAccessRequest(shared_file_id, expected_content_hash, expected_version, filename, size)
  -> file_access_client::request_download_permission
  -> owner FileAccessHandler::check_permission at request time
       * request validation
       * shared_files lookup by metadata_id
       * offered flag
       * blocked relationship
       * deny/read grant or Friends fallback
       * expected content hash and version
       * file object/blob availability and bounded preparation
  -> sign_download_descriptor(owner, requester, shared_file_id, blob hash, size, nonce, issued/expiry)
  -> client verifies owner, requester, signature, expiry, content hash, size
  -> row becomes downloading
  -> transfer_blob_to_temp streams blob to temp_path
       * bounded 128 KiB copy buffer
       * cancellation and transfer/chunk timeouts
       * periodic SQLite byte progress
  -> row becomes verifying
  -> verify_download_file checks regular file, exact size, streaming BLAKE3
  -> verify_install_and_complete atomically renames temp -> destination
  -> Storage::complete_download and terminal telemetry completion
```

Manager: `src/download_manager.rs:285-362, 399-458`; access protocol: `src/file_access_client.rs:75-230, 377-552`; combined worker: `src/blob_transfer.rs:571-674`; transfer and final verification: `src/blob_transfer.rs:1-22, 142-220` and `src/download.rs:65-179`.

State vocabulary exists in `DownloadState` (`src/download.rs:14-62`): `Queued`, `ResolvingPeer`, `RequestingPermission`, `Downloading`, `Verifying`, `Complete`, `Paused`, `Failed`, `Cancelled`, and `VersionMismatch`. SQLite rows use the corresponding snake-case strings.

### 1.5 Current Iced catalogue download path (integration warning)

```text
AppMessage::RequestFileDownload { peer, file }
  -> marks in-memory catalogue_downloads[content_hash] = Pending
  -> creates boru_downloads_dir
  -> computes hash and provider candidates
  -> chat_core::download_blob_to_file(..., save_path, progress callback)
  -> callback pushes TransferProgress into download_progress_queue
  -> DownloadDonePeerFile / DownloadFailed updates GUI projection
```

Verified source: `examples/iced_chat/app.rs:14859-14925`. This path bypasses `initiate_download`, `FileAccessRequest`, `SignedDownloadDescriptor`, `Storage::create_download`, and `verify_install_and_complete`. It is therefore not equivalent to the backend-authorised lifecycle. The dashboard must either label this as legacy/compatibility behavior or route future UI actions through the backend path; do not silently claim that the current button provides descriptor-based authorization.

## 2. Existing domain types and data ownership

| Requirement | Existing source of truth | Current status |
|---|---|---|
| Local file offer | `file_objects`, `shared_files`; `SharedFileRow` | Persisted. `offered` supports disable without deleting bytes. |
| Local path | `file_objects.source_path` / legacy `SharedFile.path` | Local-only; never put in remote metadata or dashboard telemetry. |
| Remote offer/catalogue item | `RemoteSharedFile`, then `shared_files` snapshot rows keyed by remote `profile_user_id` | Available as a projection. No separate `shared_with_me` domain table. |
| Recipient identity | `Connection::remote_id()`, `FileAccessRequest.requester`, `SignedDownloadDescriptor.requester`; friends `FriendId` | Available and cryptographically bound for authorised backend path. |
| Permission | `shared_file_permissions` (`read`, `deny`, optional expiry) plus Friends fallback | Persisted and checked at request time. |
| Signed descriptor | `SignedDownloadDescriptor` | Backend type exists; includes owner, requester, shared-file ID, blob/content hash, size, nonce, issue/expiry, signature. |
| Content identity | `file_objects.content_hash`, `RemoteSharedFile.content_hash`, descriptor hash | BLAKE3/hex and size are checked before completion. |
| Durable transfer session | `downloads` / `Storage::Download` | Incoming backend downloads persisted; current Iced catalogue button does not use it. |
| Progress | `downloads.bytes_downloaded`, `update_download_progress`/batch writer; `BlobTransferProgress`; `TransferProgress`; `TransferTelemetry` | Available through adapter/projection. GUI has an in-memory queue and periodic drain, not a durable dashboard query. |
| Completion/failure | `complete_download`, `fail_download`, `cancel_download`, `reject_resumed_permission`; telemetry diagnostics | Backend available; GUI projection handles `Completed`, `Failed`, `Cancelled`. |
| Expiry | permission `expires_at_ms` is stored but access logic must be checked against active grants; descriptor `expires_at_ms` is verified | Descriptor expiry is explicit. The permission query's expiry enforcement should be treated as a semantic question to verify before relying on it for dashboard copy. |
| Revocation | `offered=false`, delete, permission `deny`, friend/block changes | Request-time denial works. No live revoke signal for already-issued descriptors/transfers. |
| "Shared by me" | `Storage::list_shared_files(local_user, offered_only)` | Already available for current offers; version field projection is inconsistent (see limitations). |
| "Shared with me" | Remote catalogue snapshot: `get_remote_catalogue_meta`, `get_remote_shared_files`, `get_remote_collections` | Available only as an owner-scoped catalogue projection. No durable recipient/share relation or per-item accepted/declined state. |
| Native file selection | `rfd::AsyncFileDialog::pick_file` | Already available; preserve native OS behavior. |

### Local persistence schema

The v2 migration (`src/storage.rs:857-947`) creates:

- `file_objects`: immutable content-addressed object metadata/data/source path.
- `message_attachments`: chat-message relationship, separate from profile offers.
- `shared_files`: `(content_hash, profile_user_id)` offer row, display metadata, `offered`, timestamps.
- `file_collections` and `file_collection_items`: collection organisation.
- `shared_file_permissions`: per-file/per-grantee grants and optional expiry.
- `downloads`: durable remote transfer state, source peer, byte counters, errors/retry fields.
- `profile_manifest_state`: revision/hash metadata used for catalogue freshness and remote snapshot bookkeeping.

A notable implementation mismatch is present in the current checkout: `SharedFileRow` and several tests refer to a `version` field, and `FileAccessHandler` compares `row.version`, but the base `shared_files` migration shown at `src/storage.rs:860-870` has no `version` column and `list_shared_files`/`get_shared_file*` construct `version: 0`. The targeted `download_initiation` tests fail during fixture setup with `table shared_files has no column named version`. This is a backend/schema defect, not a dashboard projection choice.

## 3. Event, callback, and GUI observation sources

### Typed frontend transfer events

`src/chat_callbacks.rs:22-130` defines `TransferId`, `TransferKind`, and `TransferProgress` (`Started`, `Progress`, `Completed`, `Failed`, `Cancelled`). `ChatCallbacks::on_transfer_progress` is the frontend callback (`src/chat_callbacks.rs:365-367`). The Iced implementation stores events in `download_progress_queue` (`examples/iced_chat/app.rs:2997-3003`), converts them into `AppMessage::DownloadProgress`, and `handle_download_progress` updates chat attachment state or `catalogue_downloads` (`app.rs:5926-6112`).

This is suitable for an adapter into a dashboard, but it is process-local and keyed by a generated `TransferId`; it is not a queryable durable history. The durable backend identifier is the SQLite download row ID.

### Structured diagnostics/telemetry

`src/transfer_telemetry.rs` emits `TransferLifecycleEvent` records through the shared diagnostics store. Events include queue, access requested/granted, transfer started, progress/failure, completion, pause, resume, and cancellation. Payloads intentionally omit filenames, paths, hashes, and peer identifiers (`src/transfer_telemetry.rs:39-49`). This is a privacy-safe operational stream, but it is not enough alone to render a file dashboard because it intentionally excludes display identity and source peer.

`src/catalogue_client.rs:460-509, 806-821` emits catalogue lifecycle diagnostics (notice/fetch/store/revision/signature failure). The GUI's catalogue browser currently holds `peer_catalogue_view: Option<(PublicKey, Vec<RemoteSharedFile>)>` in memory (`examples/iced_chat/app.rs:13793-13822`), not in the durable snapshot store.

### GUI/MCP observation

FS-00 verified the loopback-only MCP diagnostics surface and `boru_get_gui_snapshot`; it can observe Iced screen/state/journal but does not expose the durable file-transfer tables as a dashboard API. `boru_browse_peer_catalogue` and `boru_download_file` exist in `examples/iced_chat/mcp_server.rs`; the latter persists catalogue data and calls `initiate_download` before the subsequent worker integration. This MCP route is closer to the backend lifecycle than the current Iced button and is useful runtime evidence, but it does not make the GUI button equivalent.

## 4. Authorization and security invariants

1. **Peer identity comes from authenticated Iroh connections.** Server handlers use `Connection::remote_id()`, not a user-supplied display label. Friend permission lookup maps that public key to `FriendId`.
2. **Catalogues are requester-scoped and signed.** `CatalogueHandler` creates a fresh `SignedFileCatalogue` for each requester. The client verifies the signature and that `owner_id` equals the connected server key.
3. **Remote metadata is path-safe.** `RemoteSharedFile` has no path, database row ID, blob ticket, source path, or upload secret. Validation rejects unsafe identifiers, separators, control/format characters, invalid MIME, excessive lengths, and unreasonable future timestamps.
4. **Access is re-authorized at request time.** A stale catalogue cannot by itself grant access. `FileAccessHandler::check_permission` checks current offer status, blocked state, current grants/friend relationship, hash, version, and availability.
5. **Descriptors are recipient-bound and short-lived.** The signed payload binds owner, requester, shared-file ID, blob hash/content hash, size, nonce, issue time, and expiry. The client verifies owner/requester/signature/time/hash/size.
6. **Replay is prevented.** `NonceStore::check_and_mark` consumes descriptor nonces and lazily evicts expired entries. The descriptor itself is single-use at the file-access transfer layer.
7. **Integrity is checked twice at the transfer boundary.** Descriptor metadata is checked before transfer; the temporary file is then checked for regular-file status, exact byte length, and streaming BLAKE3 before atomic installation.
8. **Local destinations are constrained.** `safe_destination_path`/`prepare_download_destination` reject traversal and unsafe names; temporary and final paths are persisted only for local crash recovery.
9. **Resource use is bounded.** Catalogue, preparation, upload, download, per-peer, queue, hash-verification, response-size, timeout, and copy-buffer limits prevent unbounded work.
10. **Logs/telemetry minimize sensitive data.** Access diagnostics use short peer IDs and a prefix of the shared-file ID; transfer telemetry excludes filenames, paths, hashes, and peer IDs. Do not add those fields to the privacy-safe stream without an explicit design decision.

## 5. Revocation semantics (current behavior; no changes made)

### Before a descriptor is issued

- `set_shared_file_offered(..., false)` makes the row absent from the catalogue and `FileAccessHandler` returns `Disabled` if a stale requester asks by metadata ID.
- `delete_shared_file` makes the metadata lookup return `NotFound`.
- Removing a permission or adding `deny` causes the live request to return `PermissionDenied`.
- Blocking the requester causes catalogue/access denial.
- A changed content hash or row version causes `Changed`/`VersionMismatch`.

### Queued or pre-permission download

A queued durable row is not automatically deleted or cancelled when the owner revokes. When the worker reaches `requesting_permission`, the fresh request is denied/disabled/not-found/version-mismatched. `handle_permission_response` records permission-like/retryable denials through `reject_resumed_permission`, which transitions `resolving_peer`/`requesting_permission` to `paused`, increments `retry_count`, and stores `last_error` (`src/download_manager.rs:534-565`). `Changed` and protocol-integrity failures are terminal `failed` paths. The row therefore remains a local historical record and can be displayed as paused/failed according to the response.

### Descriptor already issued or transfer active

There is no revoke message, server push, or per-transfer cancellation hook from the owner in the file-access protocol. A valid descriptor is checked for signature/time/recipient/hash, and the transfer proceeds using the descriptor/blob path. Revoking the offer or permission after issuance does not retroactively invalidate an already-issued descriptor in the current implementation. Local user cancellation (`DownloadManager::cancel_download`) and local pause signal active workers, but remote revocation does not.

This is a deliberate discovery finding, not a recommendation to rewrite the protocol on FS-01. FS-04/FS-05 must decide whether dashboard copy says "revoked" only for not-yet-authorized/failed requests, or whether a future protocol/backend addition is required for active-transfer revocation.

## 6. Dashboard gap classification

### UI-only (existing source can drive the view)

- Render current local "Shared by me" rows from `list_shared_files(local, false/true)` with offered/disabled state, display name, size, MIME, and updated time.
- Render current in-memory remote catalogue from `RemoteSharedFile` after `BrowsePeerCatalogue`.
- Render active backend download progress by adapting `Storage::Download` plus `TransferProgress`/`BlobTransferProgress`.
- Render terminal status and retry/error text from `Download.state`, `last_error`, and telemetry.
- Preserve the native `rfd` picker for adding files.
- Add dashboard navigation/empty states/formatting without changing protocol semantics.

### Adapter/projection/state work (existing backend data, missing GUI projection)

- Query durable local downloads by all relevant states, not only the current in-memory `catalogue_downloads` map; include source `remote_peer`, counters, timestamps, retry count, and errors.
- Project remote catalogue snapshots into a stable "shared with me" list keyed by `(owner peer, content_hash/shared_file_id)`; currently the GUI discards the result after `PeerCatalogueReceived` and the storage cache omits owner/version/description in `RemoteSharedFileRow`.
- Correlate `TransferId`/telemetry events with durable SQLite download IDs. The callback stream is process-local and can arrive late; terminal-state guards already exist in `handle_download_progress`.
- Add owner/recipient display-name resolution only at the presentation layer. Never persist or transmit local paths.
- Surface pause/resume/cancel controls through `DownloadManager` and persist the resulting state.

### Protocol/backend additions or fixes

- Wire `AppMessage::RequestFileDownload` to `initiate_download` + `request_and_transfer_blob` + `verify_install_and_complete`; do not keep the direct `download_blob_to_file` path for an authorised catalogue dashboard.
- Fix the `shared_files.version` schema/model inconsistency, then make per-file version increments durable and ensure manifest revisions are bumped on offer/permission/content changes. Current `increment_shared_file_revision` is explicitly a no-op (`src/storage.rs:4825-4835`).
- Decide whether active descriptor/transfer revocation is required. Current protocol has no revoke signal; adding one is a protocol/backend change, not UI work.
- Add durable remote catalogue fields needed by the dashboard (`shared_file_id`, description, version, owner/peer relation, fetched/updated times) or define a projection that retains them without duplicating authority.
- Make permission expiry enforcement explicit and test it against current time; the schema stores `expires_at_ms`, while the inspected access path's grant matching primarily distinguishes `read`/`deny`.
- Provide a durable event/query adapter for startup and restart so the dashboard does not lose active/completed records when the process-local callback queue is empty.

## 7. Verification and evidence

Commands run from `/home/dan/iroh-gossip-chat`:

- `cargo test --lib catalogue_model -- --nocapture` → PASS, 28 tests. Covered remote metadata validation, local-path rejection, signed catalogue round-trip/tamper rejection, and unsafe metadata.
- `cargo test --lib file_access_handler -- --nocapture` → PASS, 41 tests. Covered happy-path grant, blocked peer, permission revocation after catalogue fetch, disabled offer, changed source/hash/version, nonce replay, preparation limits, and missing/unavailable objects.
- `cargo test --test test_catalogue_lifecycle_events --features net -- --nocapture` → PASS, 12 tests. Covered catalogue notice/fetch/store lifecycle events, failures, signature rejection, revision handling, and privacy-safe payloads.
- `cargo test --test test_remote_catalogue_integration --features net -- --nocapture` → PASS, 13 tests. Covered real two-peer catalogue fetch/projection behavior, contacts-only visibility, explicit grant revocation after fetch, dynamic blocking, offer removal/cache cleanup, pagination/revision changes, and wrong-owner/signature rejection.
- `cargo test --lib blob_transfer::tests::transfer_small_blob_success -- --nocapture` → PASS, 1 test. Exercised a real local blob-store transfer to a temporary file with descriptor metadata and progress machinery.
- `cargo test --lib download_initiation -- --nocapture` → FAIL, 12 failures during fixture setup because the tests insert a nonexistent `shared_files.version` column; 3 tests passed. This confirms the schema/model gap above.
- `cargo test --test test_file_access --features net -- --nocapture` → FAIL before execution: no such test target exists in the current checkout. The available test target list contains related catalogue/download/security tests but not `test_file_access`.
- FS-00 upstream runtime evidence: GUI event-loop smoke test stayed alive under Xvfb and loopback MCP `boru_ping`, `boru_get_node_status`, and `boru_get_gui_snapshot` returned valid JSON-RPC responses. No two-process GUI file-download scenario was completed; the real protocol evidence above comes from the catalogue integration suite and file-access unit tests.

## 8. Known limitations and unresolved questions

- The current GUI download button bypasses the authorised durable backend flow; this must be resolved before presenting it as a security-dashboard source of truth.
- `shared_files.version` is inconsistent between schema, models, access logic, and tests; version/revocation conclusions involving exact row versions need a schema repair and retest.
- Remote catalogue persistence is implemented by reusing local-looking `shared_files` rows keyed by remote owner; it is a projection, not a clean first-class "shared with me" record.
- Active outbound serving has authenticated requester identity only while the access connection is live; there is no durable outbound transfer table keyed by recipient.
- The current inspection found no separate durable record for a remote peer accepting, declining, or downloading a local offer.
- Permission expiry must be explicitly verified in a focused test before UI claims that an expired grant is automatically revoked.
- A full two-peer GUI picker → catalogue → authorised descriptor → blob transfer run requires runtime fixtures and was not fabricated. The report distinguishes that absence from the passing protocol/unit evidence.

## FOLLOW-UPS FOR FS-04/FS-05

1. Treat `Storage::Download` plus the authenticated source peer as the backend source of truth for incoming transfer rows.
2. Treat `shared_files` local rows and `RemoteSharedFile`/remote snapshot rows as separate projections; do not infer recipient transfer state from catalogue presence.
3. Route new dashboard download actions through the durable descriptor path, preserving the native picker and existing Iroh protocols.
4. Resolve the version-column migration and permission-expiry semantics before implementing revocation/status copy.
5. Decide explicitly whether active-transfer remote revocation is in scope; current behavior allows an already-issued valid descriptor to finish.

## CHANGED FILES

- `docs/fs-01-file-sharing-lifecycle.md` (new discovery report; implementation code untouched)

## COMMIT

To be recorded after verification: commit message must reference FS-01.
