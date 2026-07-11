# Recipient File Catalog Synchronization and Offline Replication

Status: design specification

Related design: `docs/design-file-record-encrypted-sharing.md`

## Goals and boundaries

This design defines how a recipient discovers, accepts, and keeps locally available the encrypted files represented by `FileRecord`. It reuses the existing mailbox/inbox primitives for offline delivery and acknowledgement, while keeping file authorization separate from transport reachability.

The design provides:

- reliable delivery of new shares while the recipient is offline;
- durable recipient-side catalog and transfer state;
- idempotent receipt, download, and acknowledgement handling;
- reconnect synchronization for missed offers and state changes;
- bounded replication for offline availability;
- deterministic conflict handling for immutable records and authorization epochs.

It does not make a gossip topic, `TopicId`, blob hash, or `BlobTicket` an authorization signal. The signed `FileRecord` and recipient grant remain authoritative as specified by the encrypted-sharing design.

## Terminology and state ownership

The creator owns the canonical authorization record. A recipient owns a local replica and transfer state. A mailbox stores opaque `MailboxEnvelope` values and is not a file catalog.

Recommended persistent stores under the existing data directory:

```rust
pub struct FileCatalogStore {
    pub schema_version: u16,
    pub records: HashMap<FileId, ReceivedFileRecord>,
    pub sync: CatalogSyncState,
}

pub struct ReceivedFileRecord {
    pub record: FileRecord,
    pub grant: RecipientGrant,
    pub state: ReceivedFileState,
    pub local_blob: Option<LocalBlobRef>,
    pub last_error: Option<String>,
    pub updated_at_ms: u64,
}

pub enum ReceivedFileState {
    Offered,
    Accepted,
    Queued,
    Downloading,
    Available,
    Failed { retry_after_ms: u64 },
    Revoked,
}

pub struct CatalogSyncState {
    pub creator_cursors: HashMap<PublicKey, CatalogCursor>,
}

pub struct CatalogCursor {
    pub last_seen_created_at_ms: u64,
    pub last_seen_record_id: Option<FileId>,
    pub last_sync_at_ms: u64,
}
```

`FileId` is `FileRecord.id`. `LocalBlobRef` must identify a durable local ciphertext object and include its ciphertext hash; it must not contain a plaintext DEK. If resumable downloads require retaining a DEK, store it only in an OS-protected secret store or an independently encrypted local file.

The creator additionally maintains the existing `FileStore` and an outgoing-share state table. The outgoing state is separate from the recipient catalog:

```rust
pub enum ShareDeliveryState {
    Queued,
    Sent,
    Received,
    Accepted,
    Downloaded,
    Revoked,
    Failed { retry_after_ms: u64 },
}
```

All stores use the existing versioned serde format and `atomic_write_json`. Mutations set the frontend dirty flag and use the existing debounced/atomic save path; no second persistence mechanism should be introduced.

## Share delivery protocol

A new share is delivered as a private `FileShareAction::Offer(FileShareEnvelope)` inside a `MailboxEnvelope`. The offer is never broadcast on a room topic or placed in public room metadata.

### Creator flow

1. Encrypt the plaintext, upload ciphertext, and construct/sign the `FileRecord` as described in the sharing design.
2. Resolve the recipient's current `MailboxPublicKey` and create the signed `RecipientGrant`.
3. Persist the creator record and `ShareDeliveryState::Queued` before attempting network delivery.
4. Seal the versioned `Offer` payload with `seal_for` and enqueue the resulting mailbox envelope in the durable outgoing mailbox/outbox.
5. Attempt direct whisper/inbox delivery when the peer is reachable. If it fails, retain the envelope for later replay.
6. Mark the share `Sent` only after the transport accepts the envelope. Transport success is not receipt.
7. Keep the envelope until a valid recipient acknowledgement is received. Acknowledgements are idempotent and may be replayed safely.

The creator must retain the encrypted blob for at least the offer retention period plus the expected retry window. With the current `MemStore`, this requirement is not durable across process restart; production offline replication therefore requires a durable blob store or a startup re-upload/re-ticket operation before offers are advertised as persistently available.

### Recipient flow

The recipient's personal inbox service is started at application startup and kept alive independently of the visible chat room. Existing authorization, timestamp/TTL, sender allow-list, signature, encryption, and message-id deduplication checks run before application processing.

For `EnvelopeReceived`:

1. Decode a bounded `FileShareAction` payload and reject unsupported protocol/schema versions.
2. Validate the outer mailbox envelope and sender authorization.
3. Verify the `FileShareEnvelope` sender signature and require the sender to equal `record.creator`.
4. Verify the record signature, grant signature, recipient identity, epoch, AAD hash, and wrapped-key binding.
5. Reject revoked, expired, malformed, oversized, or duplicate records. A duplicate offer is an idempotent replay, not a second catalog entry.
6. Atomically insert the record/grant with state `Offered` before acknowledging receipt.
7. Send a signed `Accept { file_id, grant_id }` action after durable insertion. This means “the signed offer was received and authorized,” not “the file is downloaded.”
8. Queue the transfer according to local policy and available disk space.

The recipient must not send `Accept` before persistence. If the process crashes before the acknowledgement, the creator retries the same envelope and the recipient deduplicates it by mailbox message ID and file/grant identity.

### Transfer completion

A background transfer worker processes `Accepted`/`Queued` records:

1. Revalidate the record and grant immediately before fetching; revocation or expiry must stop the transfer.
2. Parse the confidential `BlobTicket` and enforce endpoint/address policy.
3. Download ciphertext to a temporary file or temporary blob object with a bounded size.
4. Verify `ciphertext_hash` and expected blob format.
5. Decrypt using the grant, verify plaintext size and `content_hash`, then atomically promote the ciphertext/plaintext result to the local availability store.
6. Persist `Available` and the local blob reference.
7. Send signed `Downloaded { file_id, grant_id, ciphertext_hash }` to the creator only after integrity verification and durable local promotion.

`Downloaded` means the recipient has a verified locally available copy. It does not mean the user has opened or viewed the file. UI read/view state, if later added, must be a separate event.

## Reconnect and catalog synchronization

Use a hybrid strategy:

- push-based delivery is the fast path for new offers and revocations;
- pull-based synchronization is the correctness path after reconnect, restart, address changes, or suspected packet loss.

The current inbox already supports signed `SyncRequest { since_ms }` and `SyncResponse { envelopes }`, plus a pending-envelope provider backed by `MailboxStore::pending_for_recipient`. File offers can use this path without adding a second offline queue:

1. On startup and after a successful friend/endpoint reconnect, load each creator cursor.
2. Send `send_sync_request(endpoint, secret_key, creator, cursor.last_seen_created_at_ms)`.
3. The creator returns pending mailbox envelopes addressed to the requester. The response is bounded and paginated if it exceeds the inbox payload limit.
4. Process each offer through the same validation and persistence pipeline as live delivery.
5. Send `Accept` or an idempotent acknowledgement for every newly persisted offer.
6. Advance the cursor only after the response has been fully validated and persisted. Use `(created_at_ms, record_id)` ordering so equal timestamps do not cause gaps.
7. Repeat until the response is smaller than the page limit, then persist the cursor atomically.

The sync response is a delivery backfill, not an authorization bypass. The creator must filter envelopes by recipient identity, and the recipient still validates every record and grant. A future optimized catalog endpoint may return signed summaries first and fetch full offers by record ID, but it must preserve the same signed-offer validation and cursor semantics.

For revocation and key rotation, the creator sends a push `Revoke(FileAuthorizationUpdate)` and retains the update in the pending mailbox until acknowledged. During pull sync, authorization updates are returned in the same ordered stream. The recipient applies them before starting queued downloads.

### Cursor and retry rules

- Cursors are per creator, not global; one offline creator must not block synchronization with others.
- Keep a small overlap when requesting `since_ms` (for example, one retention interval or the last acknowledged timestamp) and deduplicate by envelope ID/file ID/grant ID. This handles clock skew and equal timestamps.
- Never advance a cursor past an invalid entry silently. Record the failure and continue only when the protocol defines the entry as safely ignorable; otherwise retry the page.
- Use exponential backoff with a bounded retry interval for unavailable peers.
- If the creator's mailbox retention has expired, the recipient cannot infer that no share existed. A catalog summary/digest endpoint can later distinguish “no changes” from “history unavailable,” but this is optional for version 1.

## Replication policy for offline availability

Replication is recipient-initiated and pull-based after authorization. The creator push-delivers metadata and the encrypted capability; the recipient decides whether and when to fetch bytes.

Default policy:

- automatically download small files below a configured size limit;
- queue larger files until the user requests them or the device is charging/Wi-Fi-only policy permits;
- preserve encrypted ciphertext locally for restart/resume, but expose plaintext only after hash and size verification;
- limit concurrent downloads and total local replicated bytes;
- evict least-recently-used local ciphertext/plaintext while retaining the signed record and grant as `Accepted`/`Queued`;
- retry transient network failures, disk-full errors, and malformed data with distinct bounded error classes;
- never retry authorization, signature, revocation, or hash failures indefinitely.

If the application promises availability while the creator is offline, the recipient must finish replication before the creator disappears. A `BlobTicket` generally points at the creator's blob service; it is not itself a replica. For true creator-independent availability, upload the encrypted blob to one or more explicitly configured replica peers or a durable blob service, and include signed replica capability metadata in a later record extension. Replica peers store ciphertext only and never receive the DEK unless they are also authorized recipients.

The first implementation should support local recipient replication and durable creator storage. Multi-peer encrypted replica placement is a separate extension with its own capacity, trust, and deletion protocol.

## Conflict resolution and idempotency

`FileRecord` is immutable by `(creator, file_id, key_epoch)` except for creator-signed authorization updates. Apply the following rules:

1. A record is accepted only if its creator and signatures validate.
2. The same `file_id` with byte-identical canonical record/grant is an idempotent duplicate.
3. The same `file_id` with different immutable fields is a security conflict. Keep the first valid record, quarantine the conflicting payload, and do not download it.
4. A valid authorization update is ordered by `(key_epoch, update_sequence)` (or the update's signed monotonic version). Never accept a lower epoch over a higher epoch.
5. For equal epoch/version, canonical-byte-identical updates are duplicates; differing signed updates are conflicting creator state and must be quarantined rather than resolved by local wall-clock time.
6. Revocation wins over queued work at the same or older epoch. A completed plaintext cannot be recalled; mark it revoked and prevent future downloads/use according to product policy.
7. A new epoch uses a new DEK, nonce, ciphertext, and ciphertext hash. It is a new encrypted object associated with the same logical file lineage, not an in-place mutation of the old ciphertext.
8. Mailbox message IDs, grant IDs, and `(file_id, grant_id)` provide deduplication keys for offers and acknowledgements. Replayed `Accept` and `Downloaded` actions are harmless.

Do not use arrival order, local timestamps, topic membership, or content hash as conflict resolution. They are not authoritative.

## API and integration outline

Add a library module such as `src/file_catalog.rs` with narrow operations:

```rust
impl FileCatalogStore {
    pub fn load_or_default(data_dir: &Path) -> Result<Self>;
    pub fn save(&self) -> Result<PathBuf>;
    pub fn apply_offer(&mut self, envelope: FileShareEnvelope, local: &MailboxIdentity) -> Result<ApplyOffer>;
    pub fn apply_authorization_update(&mut self, update: FileAuthorizationUpdate) -> Result<()>;
    pub fn mark_queued(&mut self, file_id: FileId) -> Result<()>;
    pub fn mark_available(&mut self, file_id: FileId, blob: LocalBlobRef) -> Result<()>;
    pub fn next_download(&mut self, now_ms: u64) -> Option<ReceivedFileRecord>;
    pub fn cursor(&self, creator: PublicKey) -> CatalogCursor;
    pub fn advance_cursor(&mut self, creator: PublicKey, cursor: CatalogCursor) -> Result<()>;
}
```

In `examples/iced_chat/main.rs`, load the catalog alongside `MailboxStore`, register the existing inbox handler, and trigger sync after startup and friend reconnect. In `app.rs`, route `InboxEvent::EnvelopeReceived` and `AckReceived` through one file-share handler; do not implement a second ad-hoc path in the visible room UI. Persist catalog dirty state with the existing application save cycle.

Use the existing `send_ack` for mailbox-level receipt and add the signed file-share actions (`Accept`, `Downloaded`, and `Revoke`) to the existing private whisper/inbox message handling. The action acknowledgement must identify both `file_id` and `grant_id`; a mailbox message ID alone is insufficient once an envelope is re-sealed for retry.

## Failure and recovery matrix

| Failure | Durable state | Recovery |
|---|---|---|
| Crash before offer persistence | No recipient record | Creator retries; offer is applied once |
| Crash after persistence before `Accept` | `Offered` | Replay offer, send idempotent `Accept` |
| Crash during download | `Accepted`/`Downloading` | Remove incomplete temp object; retry from ciphertext |
| Hash/decryption failure | `Failed` with permanent error | Quarantine; do not acknowledge as downloaded |
| Crash after local promotion before `Downloaded` | `Available` | Startup scans available objects and resends idempotent `Downloaded` |
| Lost acknowledgement | Sender retains `Queued`/`Sent` | Re-send offer; recipient deduplicates |
| Creator offline | Recipient retains `Accepted`/`Queued` | Retry when creator/address/blob service returns |
| Mailbox TTL exceeded | Cursor unchanged or marked history-limited | Request a fresh offer/catalog reconciliation |
| Revocation during queueing | `Revoked` | Cancel download and reject old epoch |
| Conflicting signed record | Original + quarantine entry | Surface diagnostic; never merge fields |

## Security and operational requirements

- Bound every decoded envelope, catalog page, number of recipients, filename, and blob size before allocation.
- Verify signatures and grant bindings before persisting a recipient record or initiating a download.
- Keep `BlobTicket`, wrapped grants, and local ciphertext metadata out of public logs and room broadcasts.
- Use atomic writes and crash-safe temporary-file promotion for catalog and replicated blobs.
- Treat a successful QUIC/inbox write as transport delivery only; only durable recipient state justifies `Accept`.
- Keep creator outbox entries until the required acknowledgement is verified; clean them only after TTL or terminal state.
- Metrics/logs should expose record IDs, states, and error classes, but not plaintext, DEKs, full tickets, or mailbox ciphertext.

## Test plan

Unit tests:

- catalog load/save round trip and migration from an empty/older schema;
- offer validation rejects wrong recipient, creator, grant, epoch, AAD, revoked, expired, and tampered records;
- duplicate offer and duplicate acknowledgement are idempotent;
- conflicting immutable records are quarantined;
- higher-epoch revocation/update ordering is deterministic;
- cursor ordering handles equal timestamps and overlapping pages;
- download completion requires ciphertext and plaintext hash/size verification;
- crash recovery preserves `Offered`, `Accepted`, and `Available` semantics.

Two-peer integration tests:

- online offer is persisted and acknowledged;
- offer queued while recipient is offline is replayed through `send_sync_request` after reconnect;
- recipient downloads after reconnect and sends `Downloaded` only after verification;
- repeated sync requests do not duplicate catalog entries or UI items;
- revocation prevents a queued download and a new epoch can be accepted;
- creator restart retains the pending offer and durable encrypted blob;
- recipient restart retains its catalog cursor and locally replicated file.

The tests must use a durable temporary blob fixture for offline-availability claims. Tests using only `MemStore` may verify transfer mechanics but must not claim restart durability.
