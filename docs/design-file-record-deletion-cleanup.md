# FileRecord Deletion and Blob Cleanup Design

Status: design specification

## Scope

This specification defines deletion for the encrypted `FileRecord` and `RecipientGrant` model in `docs/design-file-record-encrypted-sharing.md`. A file has three distinct layers:

1. The user's personal catalog entry (`FileRecord` or a received-file reference).
2. Authorization and sharing metadata (recipient grants, revocations, acknowledgements, and tombstones).
3. The encrypted blob bytes in local or future durable blob storage.

Deleting one layer must not be implicitly interpreted as deleting the others. In particular, deleting a catalog entry on one device must not delete a recipient's copy or revoke a grant unless the user explicitly chooses an owner action.

## Decisions

### Default user action: remove from my catalog

The normal `Delete` action means **Remove from my catalog**:

- For a creator, remove the record from the local active catalog and hide it from normal file/history views.
- For a recipient, remove only the local received-file reference, cached plaintext, and local transfer state.
- Do not send a remote delete or revoke message.
- Do not alter a creator's record when a recipient removes their copy.
- Do not assume that deleting a chat/history entry deletes its referenced file record.

This action is local, idempotent, and safe when the device is offline. It may be undone only while the local deletion tombstone and required metadata remain available; undo restores the catalog reference, not plaintext that has already been securely erased.

### Creator action: revoke access

`Revoke access` is separate from catalog deletion and applies to one recipient or all recipients:

- Append `revoked_at_ms` to the selected grant(s), create a creator-signed authorization update, and persist it before sending.
- Send the update privately through the whisper/mailbox path; retry until acknowledged or retained in the outbox.
- Prevent future downloads and future grants for the revoked grant/epoch.
- Do not claim that already downloaded plaintext has been recalled.
- Keep the creator's record and encrypted blob while the creator still has an active catalog reference or retention obligation.

For meaningful cryptographic revocation, use the existing key-rotation design: create a new DEK and nonce, encrypt/upload a new ciphertext blob, create a new `key_epoch`, issue grants only to remaining recipients, and mark the prior epoch revoked. A revoke-only update is still required for immediate policy enforcement, but cannot protect plaintext already obtained.

### Creator action: delete and stop sharing

`Delete and stop sharing` is the explicit destructive owner action. It combines local removal with authorization shutdown:

1. Mark the creator record deleted in a signed tombstone.
2. Revoke all active grants and prevent new grants or offers.
3. Remove the record from the creator's active catalog and from file-related history references.
4. Send the signed tombstone/revocation update to recipients through the private delivery path.
5. Schedule the encrypted blob for garbage collection only after local references and retention rules permit it.

Recipients receiving this update must stop presenting the file as available for download and mark their received reference `RemoteDeleted` or `RemoteRevoked`. They must not delete a previously downloaded plaintext solely because the creator requested deletion; local deletion remains the recipient's choice, except where a separate user/device retention policy requires secure purge.

The update is best-effort for offline recipients. A recipient that never receives it may retain a stale local record, but cannot obtain new authorization after the creator has deleted the record. The creator's signed tombstone remains authoritative if later delivered or recovered from mailbox history.

### Recipient action: remove local copy

A recipient may remove the local catalog reference without notifying the creator. The local operation should:

- Mark the received record `Removed` in a local tombstone.
- Remove UI/history references that point only to that local copy.
- Securely delete any decrypted plaintext cache and temporary download files.
- Retain the minimum signed record/grant identifiers needed for deduplication and replay rejection until the retention window expires.
- Release the local blob reference; garbage collection may remove ciphertext if no other local reference remains.

It must not send `Revoke`, `Delete`, or `Downloaded` as a consequence of local removal. If a later duplicate offer arrives, the client may suppress it using the retained `(file_id, grant_id)` tombstone or offer an explicit restore/re-download action after normal authorization checks.

## Metadata model

Add deletion state without mutating the signed immutable content fields. Suggested creator-side fields:

```rust
pub enum FileRecordState {
    Active,
    Deleted { deleted_at_ms: u64, tombstone_id: [u8; 32] },
}

pub struct FileDeletionTombstone {
    pub schema_version: u16,
    pub file_id: [u8; 32],
    pub creator: PublicKey,
    pub key_epoch: u32,
    pub deleted_at_ms: u64,
    pub reason: DeletionReason, // OwnerDeleted or RetentionExpired
    pub prior_ciphertext_hash: String,
    pub revoked_grant_ids: Vec<[u8; 32]>,
    pub creator_signature: ByteArray<64>,
}
```

The tombstone is signed over canonical fields and references the last valid record epoch/hash. It is retained in `FileStore.deleted` (or an equivalent tombstone map), not silently removed with the active record. A recipient-side store should use a separate state:

```rust
pub enum ReceivedFileState {
    Offered,
    Accepted,
    Downloaded,
    Removed,
    RemoteRevoked,
    RemoteDeleted,
}
```

A local catalog removal should not change the creator's signed `FileRecord` or fabricate a creator deletion tombstone. Store local deletion state in a device-local catalog index/tombstone. Signed creator updates are only generated by the creator identity.

All deletion records and actions require schema/protocol version checks, bounded vector sizes, canonical signing, and idempotency by `file_id`, `tombstone_id`, and grant IDs. A duplicate tombstone or revoke update is acknowledged without repeating destructive work. Older tombstones must not overwrite a newer key epoch or a later valid state; reject conflicting creator signatures rather than merging fields.

## Wire actions and ordering

Extend the existing private `FileShareAction` with explicit owner operations:

```rust
DeleteLocal { file_id: [u8; 32], catalog_entry_id: [u8; 32] }, // local-only command; never sent
Revoke { file_id: [u8; 32], grant_ids: Vec<[u8; 32]>, update: FileAuthorizationUpdate },
DeleteRecord { file_id: [u8; 32], tombstone: FileDeletionTombstone },
DeleteAck { file_id: [u8; 32], tombstone_id: [u8; 32] },
```

`DeleteLocal` is an application/UI operation and must never be serialized onto gossip or mailbox transport. `Revoke` and `DeleteRecord` are creator-signed private actions. The receiver validates the outer sender signature, requires the sender to equal `record.creator`, verifies the creator signature and epoch/hash binding, then applies the state transition atomically before sending `DeleteAck`.

Creator ordering rules:

1. Persist the updated record/tombstone and local outbox entry atomically.
2. Only then send revoke/delete actions.
3. Keep the outbox item until a signed acknowledgement is received, with retry and expiry policy.
4. Keep the tombstone even after all recipients acknowledge, subject to tombstone retention.

Deletion must not be broadcast on the room topic. Room membership, topic IDs, blob hashes, and possession of a `BlobTicket` are not authorization signals.

## Blob ownership and garbage collection

A blob is content-addressed, so cleanup must be reference-counted rather than tied to one `FileRecord`. Track a local reference for each use:

```rust
pub struct BlobReference {
    pub ciphertext_hash: String,
    pub file_id: [u8; 32],
    pub key_epoch: u32,
    pub owner: PublicKey,
    pub reference_kind: BlobReferenceKind, // Catalog, PendingTransfer, History, DownloadCache
    pub created_at_ms: u64,
    pub released_at_ms: Option<u64>,
}
```

The reference index is local metadata. A blob may be physically deleted only when all of these are true:

- No active `Catalog`, `PendingTransfer`, or retained `History` reference exists.
- No active download/reader is using it.
- The deletion grace period has elapsed (recommended configurable default: 24 hours).
- No pending outbox/retry requires serving the blob.
- The durable store's deletion operation succeeds, or the store explicitly reports that the blob is already absent.

`DownloadCache` may use a shorter independent retention policy, but must not be deleted while an active verified file-open operation is reading it. Cleanup runs as a retryable garbage-collection job, not in the UI event handler. It should use a claim/lease or equivalent to avoid racing an upload/download, and record failures for the next run.

For `MemStore`, physical deletion is currently process-local and non-durable. The design must not promise long-term deletion or availability across restart: on shutdown/restart, volatile encrypted blobs may already be gone. When a durable store is introduced, implement its delete/unlink API behind a storage trait and retain reference-counting above that trait. If the store has no safe delete primitive, mark the reference released and let the backend's own compaction policy reclaim bytes.

An orphan scan should periodically compare stored blob hashes with active references, pending transfers, and unexpired tombstones. It may reclaim only blobs outside the grace period. Unknown blobs should be quarantined first when recovery/debugging is enabled, rather than immediately deleted.

## Local plaintext and temporary-file cleanup

Encrypted ciphertext and decrypted plaintext have different handling:

- Delete plaintext output/cache files on local catalog removal, owner deletion, or failed verification.
- Remove temporary ciphertext and partial download files after transfer failure or cancellation, unless needed for a resumable transfer with an active reference.
- Use best-effort secure deletion for plaintext, then unlink; filesystem-level secure erasure is not guaranteed on copy-on-write, journaled, SSD, or cloud-backed filesystems.
- Never log plaintext paths, DEKs, tickets, or decrypted content.
- Keep only verified plaintext in a user-visible location. A deleted catalog entry must not leave an untracked plaintext copy created by an export/open operation; exported user files are outside application ownership and must not be silently deleted.

## User-facing actions and confirmation

The file context menu should expose unambiguous labels:

- `Remove from my catalog` — local only; no recipient impact.
- `Delete local downloaded copy` — recipient-side local metadata, plaintext, and cache cleanup.
- `Revoke access…` — choose recipients; stops future authorized downloads.
- `Delete and stop sharing…` — owner-only; revokes all grants, publishes a private signed tombstone, and schedules local cleanup.
- `Forget history reference` — removes only the chat/history link; leaves the file catalog and blob reference intact.

Show confirmation for revoke-all and delete-and-stop-sharing. State the scope in the confirmation text and report partial delivery separately from local success (for example, `Deleted locally; 2 recipients pending`). Do not present remote acknowledgement as proof that a recipient erased plaintext.

## State transitions

Creator:

```text
Active --Remove from my catalog--> LocallyRemoved
Active --Revoke access--> Active (selected grants revoked)
Active --Delete and stop sharing--> Deleted (signed tombstone, all grants revoked)
LocallyRemoved --Restore--> Active (if record metadata retained)
LocallyRemoved --Delete permanently--> Deleted/GC eligible (owner confirmation)
```

Recipient:

```text
Offered/Accepted/Downloaded --Remove local copy--> Removed
Offered/Accepted/Downloaded --valid creator revoke--> RemoteRevoked
Offered/Accepted/Downloaded --valid creator tombstone--> RemoteDeleted
Removed --Restore/re-download--> Offered (only after a valid new offer/grant)
```

`RemoteRevoked` and `RemoteDeleted` are metadata states, not automatic plaintext erasure commands. A recipient can still remove its local copy through the normal local action.

## Recovery, sync, and conflicts

- Local catalog tombstones are device-local and should sync only through an explicitly designed per-user catalog sync; they must not be interpreted as creator revocations.
- Creator tombstones and revoke updates are signed authoritative metadata and may be replayed through mailbox delivery after reconnect.
- If a stale `Offer` arrives after a local removal, suppress it when its `(file_id, grant_id)` is tombstoned; do not resurrect silently.
- If a new key epoch is offered after an old epoch was deleted, treat it as a new authorized offer only when its creator signature, grant, and epoch rules validate.
- A record deletion must remove or mark references in file history, chat history, pending transfers, and outbox state in one local transaction. Preserve a compact tombstone for replay/deduplication.
- If metadata says a blob is referenced but the blob is absent, mark the transfer `Unavailable` and do not recreate a ticket from the hash. The creator must re-upload and issue a new valid ticket/epoch.

## API outline

The implementation should expose explicit operations rather than one overloaded delete function:

```rust
pub fn remove_from_catalog(store: &mut FileStore, file_id: [u8; 32], now_ms: u64) -> Result<LocalDeletion>;
pub fn revoke_grants(record: &mut FileRecord, creator: &SecretKey, grant_ids: &[GrantId], now_ms: u64) -> Result<FileAuthorizationUpdate>;
pub fn delete_and_stop_sharing(store: &mut FileStore, creator: &SecretKey, file_id: [u8; 32], now_ms: u64) -> Result<FileDeletionTombstone>;
pub fn apply_deletion_update(local: &mut ReceivedFileStore, update: &FileDeletionTombstone, now_ms: u64) -> Result<DeleteAck>;
pub fn release_blob_reference(index: &mut BlobReferenceIndex, reference: BlobReferenceId, now_ms: u64) -> Result<()>;
pub async fn collect_unreferenced_blobs(index: &mut BlobReferenceIndex, blobs: &dyn BlobStore, now_ms: u64) -> Result<CleanupReport>;
```

Each mutating operation should be atomic with the corresponding store update and dirty flag. Blob deletion is deliberately a later GC operation so a failed unlink cannot roll back a successful catalog deletion.

## Verification and tests

Unit tests should cover:

- Local creator removal does not change recipients, grants, or outgoing wire actions.
- Local recipient removal does not send revoke/delete and removes plaintext/cache references.
- Delete-and-stop-sharing signs a tombstone, revokes all active grants, and prevents new offers.
- Recipient rejects an invalid sender, creator signature, file ID, epoch, or prior-hash binding.
- Duplicate revoke/tombstone delivery is idempotent; stale updates cannot overwrite newer state.
- Local deletion suppresses duplicate offers while the local tombstone is retained.
- Blob GC waits for all references, active readers, pending outbox entries, and the grace period.
- A failed blob unlink is retried and does not restore a deleted catalog record.
- Shared ciphertext referenced by two records/epochs is deleted only after both references are released.
- Plaintext and partial temporary files are cleaned on removal and failed verification, while user-exported files are preserved.
- `MemStore` restart behavior is reported as unavailable rather than treated as a successful durable delete.

Two-peer tests should cover: owner delete while recipient is offline, mailbox replay of the signed tombstone after reconnect, recipient acknowledgement idempotency, stale offer suppression, and continued local recipient choice over previously downloaded plaintext.
