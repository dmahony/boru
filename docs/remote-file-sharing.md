# Remote file sharing

Status: implemented. Remote file sharing has two authenticated QUIC protocols and a separate iroh-blobs transfer. A catalogue is an advertisement, not a capability: seeing metadata never grants download access.

## End-to-end workflow

1. The owner stores a content-addressed `file_object`, marks it as an offered `shared_file`, and maintains a profile manifest revision.
2. A requester connects to `/boru-file-catalog/1`. The owner identifies the requester from the authenticated QUIC connection, applies relationship and per-file permission rules, and signs a requester-specific `SignedFileCatalogue`.
3. The requester validates field limits, collection references, duplicate IDs/hashes, the Ed25519 signature, and that `owner_id` is the connected peer before using or caching the catalogue.
4. A catalogue change is represented by the owner’s monotonically increasing `revision`. A requester can send `known_revision`; an unchanged requester-specific view returns `NotModified`. During pagination, a revision change returns `RevisionChanged` and the requester must restart.
5. When the user downloads an entry, the requester sends a fresh `FileAccessRequest` to `/boru-file-access/1`. The owner re-authorises against live storage; cached catalogue state is not a grant.
6. On success, the owner signs a short-lived, requester-bound `SignedDownloadDescriptor` (default lifetime: 60 seconds). The descriptor includes the owner, requester, shared-file ID, content/blob hash, size, timestamps, nonce, and signature.
7. Iroh-blobs transfers the bytes. The downloader writes a bounded stream to a temporary file, then verifies the expected size and BLAKE3 content hash. Only verified output is atomically installed and the durable download is marked complete.

In short: profiles advertise signed metadata; access is re-authorised at download time; Iroh transfers the bytes; the receiver verifies the content hash before completion.

## Catalogue protocol

ALPN: `/boru-file-catalog/1`; wire version: 1. Requests are `GetCatalogue`, `GetCataloguePage`, or `GetFileDetails`.

Each full catalogue contains:

- `owner_id`, monotonic `revision`, and `generated_at_ms`;
- requester-visible collections;
- `RemoteSharedFile` entries containing stable ID, display name, optional description, MIME type, size, content hash, version number, update time, and collection IDs.

The signed payload covers every catalogue field except the signature. The wire representation does not contain local filesystem paths, database row IDs, permission rows, upload secrets, blob tickets, or unrestricted addresses.

### Per-peer filtering

Filtering happens before signing and is performed for the authenticated requester:

- blocked peers receive `PermissionDenied`;
- with no selected-peer grants, confirmed friends see enabled, available offers and other peers see an empty catalogue;
- when any `read` grant exists for a file, only explicitly granted peers see it;
- explicit `deny`, disabled offers, missing `file_objects`, and empty/unavailable entries are omitted.

Selected-peer grants carry optional `expires_at_ms` metadata in storage. Download authorization still evaluates the live permission rows rather than trusting a cached catalogue, and the issued descriptor has its own enforced 60-second expiry. Catalogue visibility and download permission are deliberately separate.

### Revisions, pagination, and refresh

The catalogue revision is read from `profile_manifest_state`. `known_revision` is a request optimization, not an authorization decision. `NotModified` means the server’s cached requester-view hash still matches; permission changes are included in that view check even when the global revision did not change. A paginated response is limited to 500 items and 1 MiB; all pages must have the same revision and a final page with no cursor.

There is no continuous catalogue-polling loop and no separate implemented catalogue-notification ALPN. Applications may trigger a refresh after observing a profile/manifest revision change, on manual refresh, when a cache is stale, or when a requested item is missing. The test name “revision notice and refresh” refers to this revision/`NotModified` request behavior, not a guaranteed push notification.

## Local catalogue cache

Verified catalogues are stored by the local `Storage::replace_remote_catalogue` path. The current schema reuses `profile_manifest_state` for peer, revision, generated time, and fetch time, and stores remote file/collection projections in the existing `file_objects`, `shared_files`, and `file_collections` tables. The cache is a local display/reconciliation projection; it is not an access-control source and does not contain the owner’s local path.

A later fetch replaces/upserts rows for entries returned by that snapshot. Callers should use the fetched revision and refresh rather than treating cached rows as current authorization. Offline UI may display the last locally stored projection, but a download still needs a live access request.

## Download authorization

`/boru-file-access/1` accepts a versioned `FileAccessRequest` containing the shared-file ID, expected content hash/version, filename, and expected size. The handler checks the live relationship, grants/denials, offer state, file availability, expected content, and size/version before issuing a descriptor. It also applies request deadlines, upload concurrency, preparation limits, and rate limits.

Descriptors are signed by the owner, bound to the requester’s public key, expire after 60 seconds, and use a random nonce. The nonce store rejects reuse while the descriptor is valid. A requester verifies lifetime, owner, requester, signature, content hash, and size before starting transfer. Errors intentionally avoid disclosing inaccessible-file details where the protocol supports a generic refusal.

## Transfer and verification

The file-access protocol transfers no file bytes. Iroh-blobs performs the content-addressed transfer from the owner/provider. The transfer implementation uses bounded buffers, cancellation, per-chunk and overall timeouts, periodic progress persistence, and temporary output cleanup on failure.

The receiver verifies size and BLAKE3 hash after bytes have arrived. A mismatch fails the download and removes the temporary output. A successful verification is followed by an atomic rename and durable completion update. The hash is the integrity check; the descriptor signature authenticates authorization and metadata, not the file bytes themselves.

## Pause, resume, and retry limitations

Pause/resume is implemented for durable file-download rows, not as byte-range continuation of the destination file:

- pause preserves download metadata and prevents stale workers from writing progress/completion;
- resume returns to peer resolution and requests a fresh descriptor, because descriptors expire and permissions can change;
- retained iroh-blobs chunks may be reused by a later content-addressed request, but the application does not append to a prior destination offset;
- temporary output is removed on cancellation/failure; if partial blob chunks were garbage-collected, transfer starts over;
- a changed content hash becomes `version_mismatch`; terminal states cannot be resumed;
- catalogue retrieval has no continuous pauseable polling worker.

## Resource limits

Defaults enforced by the implementation include:

| Area | Default |
|---|---:|
| Catalogue request payload | 256 KiB |
| Catalogue response payload | 4 MiB |
| Paginated response | 1 MiB / 500 files |
| Files / collections per catalogue | 10,000 / 1,000 |
| Individual catalogue file size | 10 TiB |
| File-access preparations | 4 concurrent, 1 GiB/file, 60 s timeout |
| Active upload requests | 8 global, 2 per peer, 32 queued |
| Permission verifications | 4 concurrent |
| Download transfers | 4 global, 2 per peer, 32 queued |
| Hash verifications | 2 concurrent |
| Blob transfer timeout / no-progress timeout | 5 min / 30 s |

The exact catalogue limits are maintained in [`catalogue-limits.md`](catalogue-limits.md). Limits are admission and safety controls, not a promise that a peer will accept every request.

## Manual verification checklist

1. Publish an enabled file and verify that a friend receives a signed catalogue containing metadata but no local path or blob ticket.
2. Fetch as a non-friend, explicitly granted peer, and blocked peer; compare the requester-specific projections and refusal behavior.
3. Fetch twice with `known_revision`; confirm `NotModified`, then change the offer/revision and confirm a fresh snapshot. During pagination, change the revision and restart when `RevisionChanged` is returned.
4. Remove/disable an offer and verify it is absent from a subsequent catalogue; do not use an old cache entry as download authorization.
5. Request access with current and stale hash/version data; confirm only a live, matching request produces a signed descriptor, and verify expiry, requester binding, signature, and nonce replay rejection.
6. Transfer bytes through iroh-blobs, corrupt or truncate the temporary output, and confirm size/hash verification fails and unverified output is not installed.
7. Pause and resume a download; confirm peer resolution and authorization run again, and that resume does not claim byte-range destination-file support.
8. Exercise queue, per-peer, size, timeout, and hash-verification limits with structured errors/state transitions.

Relevant automated coverage is in `tests/test_remote_catalogue_integration.rs`, `tests/test_download_integration.rs`, `tests/test_corrupted_content.rs`, `tests/test_interruption_restart.rs`, and the unit tests for catalogue/access/limit modules.
