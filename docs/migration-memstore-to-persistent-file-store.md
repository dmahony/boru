# MemStore to Persistent File and Blob Storage Migration

Status: implementation migration specification

Related designs:

- `docs/design-file-record-encrypted-sharing.md`
- `docs/design-file-recipient-sync-replication.md`
- `docs/design-file-record-deletion-cleanup.md`

## 1. Purpose and migration boundary

The current GUI uses `iroh_blobs::store::mem::MemStore` for file and image blob bytes. `MemStore` is process-local: blobs disappear on process restart, and a ticket that was valid before restart cannot be assumed to remain usable. The target design separates two durable concerns:

1. A persistent file catalog containing signed `FileRecord` metadata, recipient grants, transfer state, outbox state, tombstones, and migration markers.
2. A persistent encrypted blob store containing ciphertext objects addressed by their ciphertext hash.

This migration does not make legacy capability-only shares secure retroactively. Existing `Message::FileShare { name, ticket }` and `ImageShare { name, hash }` remain legacy-read variants, are marked unverified, and require explicit user opt-in or a new authorized share. A legacy ticket must never be converted into a recipient grant without creator authorization.

The migration must preserve the security and ownership boundaries from the three related designs:

- `FileRecord` and `RecipientGrant` signatures are authoritative for authorization.
- A `BlobTicket`, topic membership, blob hash, or room membership is not authorization.
- Recipient `Accept` is sent only after durable catalog insertion.
- `Downloaded` is sent only after ciphertext and plaintext integrity verification and durable promotion.
- Local catalog removal is not remote revoke/delete.
- Blob deletion is reference-counted, lease-aware, grace-period based garbage collection.

## 2. Target layout and interfaces

The exact backend may be selected during implementation, but application code must depend on narrow interfaces rather than `MemStore` methods. The first backend must support atomic object creation, hash-addressed reads, temporary writes followed by promotion, and a safe delete/unlink operation or an explicit "delete deferred" result.

Recommended data-directory layout:

```text
<data_dir>/
  secret_key.txt
  files.json                 # creator FileStore
  file_catalog.json          # recipient FileCatalogStore
  file_outbox.json           # durable share/revoke/delete actions
  blob-store/                # encrypted ciphertext objects, backend-owned layout
  blob-store-tmp/            # crash-cleanable temporary objects
  migration/
    state.json               # migration phase/checkpoint and counts
    legacy-export.json       # optional bounded metadata export, never DEKs/plaintext
    quarantine/              # malformed/conflicting records and orphan objects
  backups/
    pre-persistent-<timestamp>/
```

`files.json` follows the encrypted-sharing design's versioned `FileStore`:

```rust
FileStore {
    schema_version: u16,
    records: HashMap<FileId, FileRecord>,
    pending: HashMap<FileId, FileTransferState>,
    deleted: HashMap<TombstoneId, FileDeletionTombstone>,
}
```

`file_catalog.json` follows the recipient sync design's `FileCatalogStore`, including `ReceivedFileRecord`, `ReceivedFileState`, and per-creator cursors. A local blob reference contains the ciphertext hash and file/epoch identity, never a plaintext DEK.

The storage boundary should expose operations equivalent to:

```rust
trait BlobStore {
    fn contains(&self, hash: &CiphertextHash) -> Result<bool>;
    fn put_temp(&self, source: &Path, expected_hash: &CiphertextHash) -> Result<TempBlob>;
    fn promote(&self, temp: TempBlob, hash: &CiphertextHash) -> Result<BlobRef>;
    fn open(&self, hash: &CiphertextHash) -> Result<BlobReader>;
    fn release(&self, hash: &CiphertextHash) -> Result<ReleaseResult>;
    fn scan(&self) -> Result<Vec<BlobObject>>;
}
```

The trait is an application boundary, not a claim about the final `iroh-blobs` API. `BlobTicket` construction and serving remain at the blob boundary. The catalog must store the ticket as confidential metadata and must not derive a new ticket from a hash when an object is missing.

## 3. Preconditions and migration invariants

Migration is allowed only when all of the following are true:

- The application can identify its stable data directory and creator identity.
- The persistent backend passes isolated read/write, hash verification, crash-recovery, and deletion tests.
- File/catalog schemas have explicit versions and bounded decode limits.
- `atomic_write_json` is available for versioned metadata files.
- A process-lifetime export hook can enumerate all currently reachable `MemStore` blobs and the metadata that references them.
- No upload, download, or deletion job is mutating the migration set, or those jobs are paused and drained.
- A backup of all existing metadata and a manifest of the in-memory store have been created.
- Migration has enough free disk for the largest ciphertext set plus temporary copies and the backup.

Important limitation: data that was already lost when a previous process exited cannot be recovered by this migration. If `MemStore` is empty at startup, the application must not invent records or claim that legacy blobs were migrated. It should mark the corresponding legacy references as `Unavailable` and offer a creator re-upload/re-share flow.

The migration must preserve these invariants:

1. Every imported blob is verified against its expected ciphertext hash before it becomes visible.
2. Every imported catalog record is either fully valid and durable or quarantined; partial records are never active.
3. A record never points at a blob that has not been promoted successfully.
4. Existing active references, pending transfers, history references, and tombstones are included in blob reference accounting.
5. The old in-memory service remains available until the persistent cutover checkpoint is durable.
6. Re-running any phase is idempotent and cannot duplicate blobs, grants, outbox actions, or catalog entries.
7. Rollback never deletes the only copy of a successfully migrated ciphertext.

## 4. Schema initialization and versioning

### 4.1 Initialization order

On startup, before advertising any file availability:

1. Resolve `<data_dir>` and create it with existing permission policy (for example, mode `0700` on Unix).
2. Acquire an application-wide migration/storage lock. A second process must fail closed rather than share an active migration.
3. Create `migration/`, `backups/`, `blob-store/`, and `blob-store-tmp/`.
4. Load or create `migration/state.json` using an atomic write. Unknown migration versions are fatal and require operator intervention.
5. Load each persistent metadata file through a version-dispatching reader. Missing files mean an empty store only when no prior migration marker says data should exist.
6. Run schema migrations for metadata only, writing each upgraded file to a temporary path and atomically replacing the original.
7. Run blob-store recovery: complete or remove abandoned temp objects according to the backend journal, then scan for objects not represented in the reference index.
8. Validate references and quarantine inconsistencies before starting the network protocols.
9. Advertise persistent file availability only after the blob store and catalog pass verification.

### 4.2 Schema versions

Use independent version numbers so a catalog change does not require a blob rewrite:

- `FileStore.schema_version`: starts at 1.
- `FileCatalogStore.schema_version`: starts at 1.
- Outbox schema: starts at 1.
- Blob-store format: backend-specific version plus object hash/size metadata.
- `migration/state.json.schema_version`: starts at 1.

Readers must accept known older versions through explicit conversion functions and reject newer versions without modifying the file. Writers emit only the current version. Do not use permissive `serde` defaults to silently reinterpret signed fields, epochs, hashes, or deletion state.

### 4.3 Migration state

Persist a checkpoint such as:

```rust
MigrationState {
    schema_version: u16,
    migration_version: u16,
    phase: MigrationPhase,
    source_generation: String,
    started_at_ms: u64,
    updated_at_ms: u64,
    exported_blobs: u64,
    imported_blobs: u64,
    imported_records: u64,
    quarantined_records: u64,
    missing_blobs: u64,
    source_manifest_hash: String,
    target_manifest_hash: Option<String>,
    cutover_complete: bool,
}
```

Recommended phases are `Prepared`, `MetadataInitialized`, `Exported`, `BlobsImported`, `CatalogImported`, `Verified`, `CutoverReady`, `CutoverComplete`, `RolledBack`, and `NeedsRepair`. The checkpoint is written after each bounded batch, not only at the end.

## 5. Export from MemStore

### 5.1 Freeze and snapshot

The export runs while the process holding `MemStore` is alive:

1. Stop accepting new file/image uploads and new share mutations.
2. Allow active downloads to finish, or cancel them and record retryable state.
3. Stop blob GC and outbox cleanup.
4. Take a consistent application snapshot of `FileStore`, recipient catalog, history references, pending transfers, outbox entries, and local deletion tombstones.
5. Enumerate all blobs reachable from the snapshot and all blobs physically present in `MemStore`.
6. Write an export manifest containing object hash, byte length, format, source references, and a source-generation identifier. Do not write plaintext, DEKs, full tickets, or mailbox ciphertext to logs.
7. Keep the `MemStore` and source snapshot unchanged until `CutoverComplete` or rollback.

If the backend cannot provide a consistent snapshot, the application must keep the mutation gate closed and retry rather than exporting a mixed generation.

### 5.2 Export object bytes

For each reachable encrypted blob:

1. Read bytes through the supported `MemStore` reader API.
2. Bound the size before allocation or copying.
3. Compute the ciphertext hash and compare it with every referencing `FileRecord.ciphertext_hash`/blob reference.
4. Write the bytes to `migration/export/<hash>.tmp`, flush according to the backend durability policy, and record the byte count.
5. Rename into the export staging area only after hash verification.
6. If multiple records reference the same hash, export one object and retain multiple reference rows.
7. If a referenced blob is absent, record `Unavailable`/migration quarantine; do not create a replacement ticket or mark the record available.
8. If an unreferenced MemStore object exists, place it in the orphan inventory. Do not import it into an active catalog without a valid reference.

The export manifest is itself checksummed. A mismatch, source mutation, short read, or failed flush aborts the export phase and leaves the source usable.

### 5.3 Legacy messages

Legacy file/image messages are metadata-only compatibility inputs:

- Preserve their original history entry and raw ticket/hash in the legacy-read representation where needed.
- Mark the UI state `LegacyUnverified`/`Unavailable` rather than importing it as a signed `FileRecord`.
- If the legacy ticket still resolves during migration, the user may explicitly choose `Import and re-share`; this creates a new encrypted `FileRecord` through the normal authorization pipeline. It is not an automatic conversion.
- If the legacy blob cannot be resolved, retain the history reference and show a re-upload/re-share action.

## 6. Import into persistent blob storage

Import is resumable by source hash and manifest generation:

1. Initialize the target blob store and verify its format/version.
2. For each staged export object, check whether the target already contains the same hash and size.
3. If present, re-verify hash/size; treat an exact match as idempotent and a mismatch as corruption requiring quarantine.
4. Otherwise write through `put_temp`, verify the expected hash, flush/commit, and atomically promote.
5. Record the target object and reference count in the migration checkpoint.
6. Rebuild the application-level `BlobReferenceIndex` from the snapshot, including `Catalog`, `PendingTransfer`, `History`, and `DownloadCache` references.
7. Run an orphan scan. Unknown target objects are quarantined first; they are not deleted during the migration window.
8. Generate a target manifest and compare object count, hashes, and lengths against the export manifest.

The target must not serve a ticket until the object has been promoted and its reference row is durable. Ticket regeneration is required when the persistent service endpoint differs from the old ticket; retain old tickets only as legacy metadata until explicitly replaced.

## 7. Import catalog, transfer state, and outbox

Metadata import occurs only after the blob import manifest is complete:

1. Convert the snapshot to current `FileStore` and `FileCatalogStore` schemas without changing signed immutable fields.
2. Validate every `FileRecord` signature and every recipient grant signature where the required keys are available.
3. Validate `file_id`, epoch, AAD binding, ciphertext hash, size limits, and deletion/tombstone ordering.
4. For each record with a missing ciphertext object, retain the record but set transfer state to `Failed`/`Unavailable` with a non-retryable migration diagnostic until the creator re-uploads and issues a new ticket/epoch.
5. For each record with an imported object, set creator transfer state to `Uploaded` or preserve the stronger existing state. Do not synthesize `Downloaded` for a recipient.
6. Preserve recipient states exactly where valid: `Offered`, `Accepted`, `Queued`, `Available`, `Revoked`, `Removed`, `RemoteDeleted`, and retryable failures.
7. Reconcile outbox entries by `(file_id, grant_id, action kind, epoch)` and preserve unsent/retryable actions. Do not duplicate `Accept`, `Downloaded`, revoke, tombstone, or delete acknowledgements.
8. Preserve creator tombstones and local deletion tombstones. A local removal must not become a creator-signed deletion.
9. Write each store atomically, then write the reference index and migration checkpoint.

The persistent store is authoritative only after verification and cutover. Until then, the source snapshot remains the rollback source.

## 8. Backward-compatible rollout

Use a staged dual-read/dual-write rollout rather than an immediate replacement.

### Stage A: Read compatibility

Ship readers for current and target schemas. Continue writing `MemStore`, but initialize the persistent backend in shadow mode. Shadow mode records metrics and validates object copies without changing network behavior. Legacy messages remain readable as unverified.

### Stage B: Dual-write with MemStore as source of truth

For each new encrypted upload:

1. Put ciphertext into `MemStore`.
2. Verify hash and create the record/ticket.
3. Write the same ciphertext to persistent storage and verify it.
4. Persist the catalog/outbox record only after both stores succeed.
5. If persistent write fails, keep the source operation successful but mark the persistent copy pending and block durable/offline availability claims. Retry before cutover.

For downloads, read from persistent storage first only when its hash/size verification succeeds; otherwise fall back to `MemStore` and record a repair event. Never silently use a stale or mismatched object.

### Stage C: Persistent source of truth, MemStore compatibility cache

After a successful migration and soak period:

1. New uploads write persistent storage first, then optionally populate `MemStore` as a process-local serving cache.
2. New records use persistent-store tickets and the new signed file-share protocol.
3. Existing legacy tickets remain read-only compatibility data and are not advertised as durable.
4. All catalog, outbox, recipient sync, revoke, delete, and GC operations use persistent metadata.
5. A restart must recover persistent records and blobs without requiring MemStore.

### Stage D: Remove production MemStore dependency

After the rollback window and soak criteria pass, remove `MemStore` from the production GUI initialization. Keep it available in unit/integration fixtures for transfer mechanics, but label those tests non-durable. Delete legacy compatibility only in a separately versioned release after telemetry confirms no remaining users need it.

## 9. Cutover procedure

Cutover is an explicit checkpoint, not an implicit first successful write:

1. Announce a short maintenance state in the UI and stop uploads, share mutations, deletion, and GC.
2. Drain or checkpoint active transfers and outbox sends.
3. Export a final source snapshot and compare its generation with the prepared snapshot. If it changed, repeat export/import.
4. Complete any delta copy since the earlier export.
5. Verify target object and catalog manifests, signatures, references, state counts, and tombstone ordering.
6. Write `CutoverReady` and a final rollback marker atomically.
7. Switch the blob provider and persistence provider behind the application service boundary.
8. Regenerate tickets for records whose endpoint/provider changed; sign and persist the updated authorized envelope according to the record/version rules. Do not mutate immutable signed fields in place.
9. Re-open network delivery only after the new provider passes health checks.
10. Write `CutoverComplete`, retain the source snapshot and MemStore until the rollback window expires, and resume queued operations.

During cutover, an unavailable blob remains unavailable. The UI must distinguish `Migrated`, `Unavailable—re-upload required`, and `Legacy—unverified`; it must not report success merely because metadata imported.

## 10. Rollback strategy

Rollback is supported until the configured rollback deadline (recommended: 7 days and at least one clean restart/restore test):

- If preparation, export, import, or verification fails before `CutoverComplete`, discard only target staging/temp state, keep the source running, and mark the migration phase failed.
- If cutover fails before any new persistent writes are acknowledged, switch the provider back to `MemStore`, restore the pre-cutover application configuration, and replay the unchanged source outbox.
- If persistent writes occurred after cutover, do not blindly restore an old JSON backup. Freeze mutations, export a target delta, and either replay it into the source or restore the persistent provider. This prevents loss of new records and acknowledgements.
- Never delete target ciphertext during rollback unless it is outside the target reference index and the grace period; preserve it for forensic recovery.
- Restore metadata files atomically from the backup only after validating schema, identity, and manifest checksums.
- Revoke/regenerate tickets only through signed application operations; do not edit ticket strings manually.

After rollback, restart and verify that legacy and newly created records have the expected state. A rollback does not recover blobs that were already absent from MemStore at the time of export.

## 11. Verification and acceptance tests

### Pre-cutover checks

- Schema readers reject unknown future versions and accept all supported older versions.
- Atomic writes survive interruption without malformed JSON or mixed-generation files.
- Export manifest hashes and lengths match bytes read from `MemStore`.
- Target object hashes, lengths, and formats match the export manifest.
- Duplicate import is idempotent; corrupted same-hash target data is quarantined.
- Every active catalog reference resolves to a target ciphertext object, or is explicitly `Unavailable`.
- Creator and grant signatures, epochs, AAD hashes, tombstones, and recipient identities validate.
- Outbox and acknowledgement deduplication keys remain stable across restart.
- Reference counting includes pending transfers, history, local catalog, download cache, and tombstones.

### Restart and failure tests

- Kill the process after each migration batch and resume from `migration/state.json`.
- Kill during temp-object write, promotion, metadata write, and cutover; verify recovery never exposes partial objects.
- Restart after `CutoverComplete` with no usable `MemStore`; creator records, recipient catalogs, cursors, pending outbox entries, and encrypted blobs remain available.
- A missing referenced blob is reported unavailable and never receives a fabricated ticket.
- GC does not remove a blob with an active reader, pending outbox, retained history, or unexpired grace period.
- Failed unlink is retried without resurrecting a deleted catalog record.

### Protocol and compatibility tests

- Legacy file/image messages decode as unverified and cannot authorize a download automatically.
- New offers are persisted before `Accept`; verified downloads are promoted before `Downloaded`.
- Offline offer replay and pull sync remain idempotent after restart.
- Revoke and signed tombstone updates stop new downloads but do not falsely claim plaintext recall.
- Conflicting immutable records are quarantined; higher epoch authorization updates win deterministically.
- Old clients can read legacy messages and continue ordinary chat while they ignore the new file-share action; they must not be sent a payload they cannot safely interpret as a legacy message.

### Operational acceptance gates

Cutover is accepted only when:

- two consecutive startup/recovery runs complete without repair-required state;
- a two-peer offline offer, reconnect sync, download, acknowledgement, revoke, and delete scenario passes;
- source/target manifest counts and hashes reconcile;
- no unclassified missing references remain;
- metrics show zero persistent-write failures for the soak window;
- backup restore has been tested on a separate data directory;
- operators can identify and recover `NeedsRepair` without deleting user data.

## 12. Timeline and ownership

The timeline assumes one engineer familiar with the existing GUI and one reviewer. Durations are estimates and can overlap only where the stated dependency permits.

| Phase | Duration | Deliverable | Exit gate |
|---|---:|---|---|
| 0. Inventory and fixture capture | 0.5–1 day | MemStore export adapter, source manifest fixture, legacy compatibility cases | Live-process export is deterministic |
| 1. Storage boundary | 1–2 days | `BlobStore` adapter, temp/promote/open/release/scan behavior | Backend unit and crash tests pass |
| 2. Schema and reference index | 1–2 days | Versioned `FileStore`, `FileCatalogStore`, migration state, refcount index | Atomic load/save and older-schema tests pass |
| 3. Export/import engine | 2–3 days | Resumable batch copy, checksums, quarantine, checkpoints | Kill/resume and manifest reconciliation pass |
| 4. Dual-write rollout | 1–2 days | Shadow validation and persistent-copy retry path | No unclassified divergence in soak |
| 5. Catalog/outbox integration | 2–3 days | Durable transfer states, cursor/outbox recovery, ticket regeneration | Offline two-peer flow passes |
| 6. Cutover and rollback | 1–2 days | Provider switch, rollback marker, backup restore procedure | Clean restart and rollback tests pass |
| 7. Soak and deprecation | 3–7 days elapsed | Metrics, repair tooling, legacy warnings | Acceptance gates remain green |

Estimated implementation time is 8–15 engineering days plus the soak window. Security review is required before enabling encrypted sharing for general users; storage and migration code should receive a separate data-loss-focused review.

## 13. Risks and mitigations

| Risk | Impact | Likelihood | Mitigation / decision |
|---|---|---:|---|
| MemStore data disappears before export | Permanent loss of volatile blobs | High | Export only while the source process is alive; report missing objects as unavailable; require re-upload/re-share |
| Source mutates during export | Inconsistent records and bytes | Medium | Freeze mutations, use a source generation, compare final snapshot, repeat delta copy |
| Target object corruption or short write | Failed downloads or integrity failure | Low/medium | Hash and size verification before promotion; quarantine mismatch; never serve unverified objects |
| Crash between blob and metadata commits | Dangling object or missing reference | Medium | Checkpointed phases, idempotent import, orphan quarantine, reference rebuild, atomic JSON writes |
| Ticket endpoint changes at cutover | Existing recipients cannot fetch | Medium | Regenerate provider-bound tickets, sign new envelopes/updates, retain legacy ticket only for read compatibility |
| Duplicate outbox actions after retry | Repeated UI events or state corruption | Medium | Stable `(file_id, grant_id, action, epoch)` idempotency keys and durable acknowledgement state |
| Incorrect deletion during migration | Data loss or unintended remote semantics | Medium | Import tombstones separately, preserve reference kinds, defer GC through grace period, distinguish local delete/revoke/delete-and-stop-sharing |
| Plaintext/DEK leakage in migration artifacts | Confidentiality breach | Low/medium | Copy ciphertext only; redact logs; never export DEKs or plaintext; restrict migration directory permissions |
| Backend lacks safe delete | Disk growth | Medium | Release references and use backend compaction; expose deferred cleanup metrics |
| Large files exceed memory | OOM or stalled GUI | Medium | Stream bounded reads/writes; batch checkpoints; temporary files, not whole-file buffers |
| Older clients misunderstand new wire actions | Delivery failure | Medium | Keep legacy wire variants readable; negotiate/version new actions; use existing mailbox protocol version checks |
| Backup is incomplete or untested | Rollback cannot restore | Medium | Manifest/checksum backups, separate-directory restore test, retain source until soak passes |
| Concurrent kanban/GUI development changes schemas | Migration incompatibility | Medium | Land schema versions and adapters first; require design review for signed-field changes; rerun fixtures before cutover |

## 14. Final implementation checklist

- [ ] Storage interface hides `MemStore` and supports stream, promote, scan, and release semantics.
- [ ] File/catalog/outbox/migration schemas have explicit versions and bounded readers.
- [ ] Export runs only against a frozen, generation-tagged source snapshot.
- [ ] Every copied object is hash/size verified before promotion.
- [ ] Missing legacy blobs are visible as unavailable, never silently discarded or fabricated.
- [ ] Catalog, recipient state, outbox, tombstones, and references are imported atomically and idempotently.
- [ ] New writes use dual-write before cutover and persistent-first after cutover.
- [ ] New tickets and envelopes are regenerated/signed when the provider changes.
- [ ] Rollback preserves post-cutover writes and never deletes the only ciphertext copy.
- [ ] Restart, offline sync, revoke, deletion, GC, and legacy compatibility tests pass.
- [ ] Backup restore and `NeedsRepair` operator procedures are verified.
- [ ] MemStore is no longer used to make durability or offline-availability claims.
