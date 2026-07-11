# FileRecord Metadata and Encrypted Sharing Design

Status: design specification

## Scope and constraints

The current GUI stores blobs in an `iroh_blobs::store::mem::MemStore` and advertises downloads with `iroh_blobs::ticket::BlobTicket`. A ticket contains enough endpoint/address and blob information for a peer to request the blob; it is therefore a capability, not an authorization mechanism. A bare blob hash is insufficient for a remote download and must not be treated as a secure share token.

This design makes the blob itself encrypted before upload. A leaked `BlobTicket` can then expose ciphertext availability, but not file contents. Access to the plaintext is controlled by per-recipient encrypted key grants, authenticated by the creator. The design reuses the existing `MailboxPublicKey`, `MailboxIdentity`, `seal_for`, `MailboxEnvelope`, `SecretKey` signatures, `blake3`, and atomic JSON store patterns.

The design applies to files and images. `ImageShare` should be migrated from its current hash-only payload to the same file-share envelope.

## Data model

### FileRecord

`FileRecord` is the creator's canonical metadata record. It is versioned and immutable except for authorization/revocation state.

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileRecord {
    pub schema_version: u16,       // currently 1
    pub id: [u8; 32],              // random record identifier
    pub name: String,               // UTF-8 display name, validated and length-limited
    pub media_type: Option<String>, // advisory MIME type; never trusted for decoding
    pub size: u64,                  // plaintext byte length
    pub content_hash: String,       // blake3 hex of plaintext bytes
    pub ciphertext_hash: String,    // blake3 hex of encrypted blob bytes
    pub blob_ticket: String,        // BlobTicket for the encrypted blob; secret capability
    pub blob_format: BlobFormatWire, // immutable iroh blob format
    pub creator: PublicKey,
    pub created_at_ms: u64,
    pub expires_at_ms: Option<u64>,
    pub key_epoch: u32,             // increments when a new DEK is issued
    pub recipients: Vec<RecipientGrant>,
    pub revoked_at_ms: Option<u64>,
    pub creator_signature: ByteArray<64>,
}
```

`BlobFormatWire` is an application enum rather than a serialized dependency-specific type, for example `Raw` or `HashAndFormat { format: String }`. Conversion to `iroh_blobs::BlobFormat` occurs at the blob boundary.

The record ID is random and independent of the content hash. The plaintext hash is useful for integrity and duplicate detection, but does not identify or authorize access. The ciphertext hash is the hash downloaded from the blob store and is checked before decryption. Do not put the plaintext `blob_ticket` in public room metadata, AboutMe broadcasts, logs, or a filename preview.

### RecipientGrant

A grant identifies one intended recipient and carries only a copy of the encrypted content key for that recipient.

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecipientGrant {
    pub recipient: PublicKey,                 // identity key
    pub recipient_encryption: [u8; 32],       // MailboxPublicKey::encryption at grant time
    pub grant_id: [u8; 32],                   // random, unique within FileRecord
    pub wrapped_key: MailboxEnvelope,         // encrypted DEK + grant context
    pub granted_at_ms: u64,
    pub revoked_at_ms: Option<u64>,
    pub grant_signature: ByteArray<64>,       // creator signature over all grant fields
}
```

`wrapped_key` is produced by `seal_for(&creator_secret, recipient_mailbox_key, grant_payload)`. The payload is a versioned `FileKeyGrant`:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileKeyGrant {
    pub schema_version: u16,
    pub file_id: [u8; 32],
    pub key_epoch: u32,
    pub dek: [u8; 32], // never persisted or sent outside this recipient envelope
    pub aad_hash: [u8; 32],
}
```

`aad_hash` binds the DEK grant to the immutable record context. It is the blake3 hash of canonical serialized `(file_id, name, size, content_hash, ciphertext_hash, key_epoch)`. The recipient rejects a grant whose file ID, epoch, or AAD hash does not match the received record.

### Recipient lists and authorization state

The creator persists a private `FileStore` containing records and the DEKs only indirectly through grants. The recommended file is `files.json`, written with the existing atomic JSON helper:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileStore {
    pub schema_version: u16,
    pub records: HashMap<[u8; 32], FileRecord>,
    pub pending: HashMap<[u8; 32], FileTransferState>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FileTransferState {
    Pending, Uploaded, Shared, Failed { reason: String },
}
```

A record's recipient list is authoritative only after creator signature verification. A recipient stores accepted records in a local `ReceivedFileStore` with the record, its own grant, and transfer state. It does not store a creator's plaintext DEK outside the grant-processing path; if caching the DEK for resumable downloads is necessary, store it in an OS-protected secret store or a separately encrypted local file.

Revocation is represented by `revoked_at_ms` and a creator-signed authorization update. Revocation prevents future key grants and causes compliant clients to reject new downloads, but cannot retract plaintext already downloaded or a previously leaked ticket. For strong revocation, rotate to a new key epoch, upload a re-encrypted blob, issue grants only to remaining recipients, and mark the old epoch revoked.

## Encryption and upload flow

1. Read the plaintext file, enforce configured size limits, and compute `content_hash = blake3(plaintext)`.
2. Generate a random 32-byte data-encryption key (DEK) and random 12-byte nonce. Encrypt the file with AES-256-GCM. Use canonical record context as associated data (AAD), including file ID and key epoch.
3. Upload ciphertext to the local `MemStore` (or a future durable blob store), compute `ciphertext_hash`, and construct a `BlobTicket` pointing to the ciphertext blob.
4. Build the unsigned `FileRecord` with the creator identity and empty or initial recipient list. The ticket remains confidential.
5. For each intended recipient, resolve a current `MailboxPublicKey`, create a `FileKeyGrant`, encrypt it with `seal_for`, and sign the `RecipientGrant` with the creator's `SecretKey`.
6. Sign the canonical `FileRecord` excluding `creator_signature`. Persist the record atomically before sending any share message.
7. Send the recipient a private `FileShareEnvelope` over the existing whisper path or encrypted inbox. Do not broadcast it on the room gossip topic.
8. Keep the creator's outbox entry until the recipient acknowledges receipt of the signed envelope. Acknowledgement confirms receipt, not that the file was downloaded.

The encrypted blob can be fetched before authorization without revealing plaintext, but clients should not fetch until record and grant validation succeeds. The creator should avoid exposing a reusable ticket to untrusted relays or logs.

## Share wire protocol

The application message should be versioned and separated from the existing text/file message variants:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileShareEnvelope {
    pub protocol_version: u16,
    pub record: FileRecord,              // includes only encrypted-blob capability
    pub recipient: PublicKey,
    pub grant: RecipientGrant,
    pub sender_signature: ByteArray<64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FileShareAction {
    Offer(FileShareEnvelope),
    Revoke(FileAuthorizationUpdate),
    Accept { file_id: [u8; 32], grant_id: [u8; 32] },
    Downloaded { file_id: [u8; 32], grant_id: [u8; 32], ciphertext_hash: String },
}
```

`sender_signature` covers protocol version, recipient, record, and grant. The grant also has its own creator signature so it can be verified independently when persisted or replayed. `Accept` and `Downloaded` are signed by the recipient's identity key and sent privately to the creator; they must not be interpreted as proof that the recipient viewed the file.

For mailbox/offline delivery, serialize `FileShareAction::Offer` as the plaintext payload inside the existing `MailboxEnvelope`. The mailbox's existing checks remain mandatory: sender allow-list, timestamp/TTL, recipient identity, X25519-derived encryption, AES-GCM authentication, signature verification, deduplication, and signed acknowledgement.

## Verification algorithm

On receipt, a client must perform these checks in order:

1. Decode only supported protocol/schema versions and enforce maximum encoded sizes before allocation.
2. Verify the outer sender signature and require `sender == record.creator`.
3. Verify `record.creator_signature` over canonical record bytes.
4. Require `envelope.recipient == local_public_key` and `grant.recipient == local_public_key`.
5. Verify `grant.grant_signature` against `record.creator`.
6. Require `grant.revoked_at_ms` and `record.revoked_at_ms` to be absent or in the future according to the local clock policy; reject stale/future timestamps outside the allowed skew.
7. Require `grant.file_id == record.id`, matching `key_epoch`, matching recipient encryption key, and matching `aad_hash`.
8. Decrypt `grant.wrapped_key` with the local `MailboxIdentity`; reject any envelope addressed to another mailbox identity. Parse and validate `FileKeyGrant`.
9. Parse `record.blob_ticket`, verify its embedded blob hash/format corresponds to `record.ciphertext_hash` and `record.blob_format`, and reject malformed or unexpected addresses according to the endpoint policy.
10. Download ciphertext, verify its length and `ciphertext_hash`, then decrypt with the DEK and the same AAD.
11. Verify the resulting plaintext length equals `record.size` and `blake3(plaintext) == record.content_hash` before exposing or rendering it.
12. Persist the accepted record and transfer state atomically, then send `Accept` only after successful authorization and `Downloaded` only after successful integrity verification.

Authorization is therefore not inferred from topic membership, a `TopicId`, a room roster, a blob hash, or possession of a ticket. All are separate from the signed per-recipient grant.

## API outline

The implementation should introduce a library module, preferably `src/file_share.rs`, and expose narrowly scoped operations:

```rust
pub fn create_file_record(
    creator: &SecretKey,
    name: String,
    media_type: Option<String>,
    plaintext: &[u8],
    blob_ticket: BlobTicket,
    ciphertext: &[u8],
    key_epoch: u32,
) -> Result<(FileRecord, [u8; 32])>; // record and DEK

pub fn add_recipient(
    record: &mut FileRecord,
    creator: &SecretKey,
    recipient: MailboxPublicKey,
    dek: [u8; 32],
) -> Result<RecipientGrant>;

pub fn authorize_share(
    envelope: &FileShareEnvelope,
    local: &MailboxIdentity,
    now_ms: u64,
) -> Result<FileKeyGrant>;

pub fn verify_and_decrypt_blob(
    record: &FileRecord,
    grant: &FileKeyGrant,
    ciphertext: &[u8],
) -> Result<Vec<u8>>;

pub fn revoke_recipient(
    record: &mut FileRecord,
    creator: &SecretKey,
    recipient: PublicKey,
    now_ms: u64,
) -> Result<FileAuthorizationUpdate>;
```

Persistence APIs should mirror existing stores:

```rust
impl FileStore {
    pub fn load_or_default(data_dir: &Path) -> Result<Self>;
    pub fn save(&self) -> Result<PathBuf>;
    pub fn insert(&mut self, record: FileRecord) -> Result<()>;
    pub fn get(&self, id: &[u8; 32]) -> Option<&FileRecord>;
}
```

The GUI integration should route both `/file` and `/image` through one share pipeline, retain the transfer state separately from `ChatHistoryStore`, and render only verified plaintext. `HistoryEntry` may retain a file record ID and display metadata, but should not duplicate the DEK or plaintext ticket unnecessarily.

## Security and operational requirements

- Use cryptographically random IDs, DEKs, and nonces. Never derive a DEK from a filename, content hash, room ID, or ticket.
- Never reuse an AES-GCM nonce with the same DEK. A new key epoch must use a new DEK and new nonce.
- Canonicalize serialized data before signing. Do not sign JSON text whose map ordering can vary; postcard tuples/structs with a fixed schema are suitable.
- Enforce maximum filename, metadata, grant, envelope, plaintext, and ciphertext sizes.
- Sanitize display names and never use `FileRecord.name` directly as a filesystem path.
- Keep authorization and transfer errors non-leaky: do not reveal whether another recipient exists or whether a ticket is valid to unauthorized peers.
- Treat ticket strings and mailbox public encryption keys as sensitive metadata. Public keys identify recipients but do not grant access by themselves.
- Version all wire and persistence schemas. Readers should accept older records where safe and reject unknown versions explicitly.
- `MemStore` is currently non-durable. On restart, regenerate/re-upload the encrypted blob and issue a new ticket, or add a durable blob store before claiming long-term file availability.

## Test and migration matrix

Unit tests should cover: record signing round-trip; grant signing and recipient mismatch; unauthorized recipient; tampered record/grant/ticket; wrong epoch/AAD; expired/revoked grant; ciphertext hash mismatch; AES-GCM tamper failure; plaintext hash/size mismatch; duplicate offer; and schema-version rejection.

Two-peer integration tests should cover: authorized recipient downloads and verifies a file; non-recipient receives an offer but cannot decrypt; offline offer replays through `MailboxEnvelope` after reconnect; recipient acknowledgement is idempotent; and revocation blocks a new epoch while an already downloaded plaintext remains unavailable for recall.

Migration should preserve existing `Message::FileShare { name, ticket }` only as a legacy-read variant. Legacy shares are capability-only and cannot provide recipient authorization; mark them as legacy/unverified and require an explicit user opt-in or re-share through the new protocol. Existing `ImageShare { name, hash }` messages should be read for compatibility but never treated as sufficient to authorize a download.
