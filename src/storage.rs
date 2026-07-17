//! Relational storage layer with managed migrations.
//!
//! # Schema
//!
//! Version 1 — message delivery (migrated from [`crate::store::MessageStore`]):
//!   - `inbox` / `outbox` / `contacts` / `sync_cursor`
//!   - `schema_version` (single-row meta table introduced in v1)
//!
//! Version 2 — content-addressed file objects and sharing:
//!   - `file_objects`      — content-addressed immutable file data
//!   - `message_attachments` — links a message to one or more file objects
//!   - `shared_files`       — profile-offered files with per-peer visibility
//!   - `file_collections`   — named groups of shared files
//!   - `file_collection_items` — membership in a collection
//!   - `shared_file_permissions` — per-peer grants on individual shared files
//!   - `downloads`          — durable download state machine
//!   - `profile_manifest_state` — manifest revision tracking
//!
//! Version 3 — remote catalogue cache:
//!   - `remote_catalogues`    — cached catalogue metadata per remote peer
//!   - `remote_shared_files`  — cached file entries from remote catalogues
//!   - `remote_collections`   — cached named collections from remote catalogues
//!
//! # Design rules
//!
//!  1. Chat attachments belong to messages (`message_attachments`).
//!  2. Profile file offers belong to a user profile (`shared_files`).
//!  3. Both reference the same content-addressed `file_objects` store.
//!  4. No local filesystem paths are exposed to remote peers.
//!  5. All large binary data lives in `file_objects`; relationship tables
//!     carry only foreign keys and metadata.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::anyhow;
use iroh::{PublicKey, SecretKey};
use n0_error::{Result, StdResultExt};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::download::{DownloadState, UnknownDownloadState};
use crate::store::{DeliveryStatus, MessageId, OutboxRow, StoredEnvelope};

// ── Current schema version ────────────────────────────────────────────────

/// Bump every time a new migration is added.
const CURRENT_SCHEMA_VERSION: u32 = 5;

// ── Database file name ────────────────────────────────────────────────────

/// The SQLite database file stored beside the data directory.
pub const DB_FILE_NAME: &str = "boru.db";

// ── File-object types ─────────────────────────────────────────────────────

/// Content hash type — blake3 32-byte output encoded as hex.
pub type ContentHash = [u8; 32];

/// A content-addressed file object stored locally.
#[derive(Debug, Clone)]
pub struct FileObject {
    /// blake3 hash of the file contents (hex-encoded, 64 chars).
    pub content_hash: String,
    /// Total size in bytes.
    pub size: u64,
    /// MIME type hint (e.g. "image/png", "application/octet-stream").
    pub mime_type: String,
    /// Original filename (no path components).
    pub filename: String,
    /// Created-at timestamp in milliseconds since UNIX epoch.
    pub created_at_ms: u64,
    /// The file data itself. For large files this may be a blob-id
    /// that references an iroh-blobs store.
    pub data: Option<Vec<u8>>,
}

/// A file object that has been imported from a remote peer and is
/// referenced by an iroh-blobs hash rather than stored inline.
#[derive(Debug, Clone)]
pub struct ImportedFileObject {
    /// Links to `file_objects.content_hash`.
    pub content_hash: String,
    /// The iroh-blobs hash that can be used to fetch this file.
    pub blob_hash: String,
    /// The peer we imported this from.
    pub source_peer: String,
    /// When the import occurred (ms since UNIX epoch).
    pub imported_at_ms: u64,
}

/// A chat message attachment — links a message to one or more file objects.
#[derive(Debug, Clone)]
pub struct MessageAttachment {
    /// Unique row id.
    pub id: i64,
    /// The local message event-id.
    pub event_id: u64,
    /// Links to `file_objects.content_hash`.
    pub content_hash: String,
    /// Display filename for the recipient.
    pub display_filename: String,
    /// Ordinal position within the message's attachment list.
    pub position: u32,
}

/// A profile-offered shared file.
#[derive(Debug, Clone)]
pub struct SharedFileRow {
    /// Links to `file_objects.content_hash`.
    pub content_hash: String,
    /// The owning profile (hex-encoded public key).
    pub profile_user_id: String,
    /// Stable metadata id (from `crate::user_profile::SharedFile`).
    pub metadata_id: String,
    /// Display filename.
    pub display_filename: String,
    /// Custom description (optional).
    pub description: Option<String>,
    /// Whether this file is currently offered.
    pub offered: bool,
    /// When the offer was created (ms since UNIX epoch).
    pub created_at_ms: u64,
    /// When the offer was last updated.
    pub updated_at_ms: u64,
}

/// Local verification/availability state for a profile file.
///
/// This is local-only metadata; it is not part of the remote catalogue protocol.
#[derive(Debug, Clone)]
pub struct FileAvailability {
    /// Content-addressed file identifier.
    pub content_hash: String,
    /// Local profile owner identifier.
    pub profile_user_id: String,
    /// Local availability label (for example `Available` or `Missing`).
    pub availability: String,
    /// Last verification timestamp in Unix milliseconds.
    pub checked_at_ms: Option<u64>,
    /// Previous content hash recorded during replacement.
    pub original_hash: String,
    /// Previous file size recorded during replacement.
    pub original_size: u64,
}

/// A named collection of shared files belonging to a profile.
#[derive(Debug, Clone)]
pub struct FileCollection {
    /// Unique row id.
    pub id: i64,
    /// The owning profile (hex-encoded public key).
    pub profile_user_id: String,
    /// Human-readable collection name (e.g. "photos", "documents").
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// When the collection was created.
    pub created_at_ms: u64,
    /// When the collection was last modified.
    pub updated_at_ms: u64,
}

/// Membership of a shared file in a collection.
#[derive(Debug, Clone)]
pub struct FileCollectionItem {
    /// Links to `file_collections.id`.
    pub collection_id: i64,
    /// Links to `file_objects.content_hash`.
    pub content_hash: String,
    /// Ordinal position within the collection.
    pub position: u32,
    /// When the item was added.
    pub added_at_ms: u64,
}

/// Per-peer permission grant on a shared file.
#[derive(Debug, Clone)]
pub struct SharedFilePermission {
    /// Links to `file_objects.content_hash`.
    pub content_hash: String,
    /// The grantor's hex-encoded public key (the profile owner).
    pub grantor_user_id: String,
    /// The grantee's hex-encoded public key.
    pub grantee_user_id: String,
    /// Allowed operation: "read", "download", etc.
    pub permission: String,
    /// When the grant was created.
    pub created_at_ms: u64,
    /// Optional expiry (ms since UNIX epoch, NULL = never expires).
    pub expires_at_ms: Option<u64>,
}

/// Durable download state for a file being fetched from a remote peer.
///
/// Records are created **before** any network transfer begins.  The state
/// machine drives the download through resolution, permission, transfer,
/// and verification phases.  See [`crate::download`] for the full state
/// diagram and valid transitions.
#[derive(Debug, Clone)]
pub struct Download {
    /// Unique row id.
    pub id: i64,
    /// BLAKE3 content hash of the expected file (hex-encoded, 64 chars).
    /// Foreign key to `file_objects`.
    pub content_hash: String,
    /// The remote peer we are downloading from (hex-encoded public key).
    pub remote_peer: String,
    /// The remote peer's identifier for this shared file (from the
    /// catalogue — matches `RemoteSharedFile::content_hash` on the
    /// remote side).
    pub remote_shared_file_id: String,
    /// Local filesystem path where the downloaded file will be saved.
    /// Chosen before network transfer begins.
    pub destination_path: PathBuf,
    /// Expected file size in bytes (from the catalogue offer).
    pub expected_size: u64,
    /// Current state of this download (typed enum).
    pub state: DownloadState,
    /// Bytes received so far.
    pub bytes_downloaded: u64,
    /// When the download was created.
    pub created_at_ms: u64,
    /// When the state last changed.
    pub updated_at_ms: u64,
    /// Last error message (if state == [`DownloadState::Failed`]).
    pub last_error: Option<String>,
    /// Retry count.
    pub retry_count: u32,
    /// Next retry timestamp (ms since UNIX epoch).
    pub next_retry_at_ms: Option<u64>,
}

impl Download {
    /// Returns `true` if this download is in a terminal state.
    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    /// Returns `true` if this download is retryable.
    pub fn is_retryable(&self) -> bool {
        self.state.is_retryable()
    }

    /// Returns `true` if this download is in an active progressing state.
    pub fn is_active(&self) -> bool {
        self.state.is_active()
    }
}

/// Profile manifest revision tracking for a local user.
#[derive(Debug, Clone)]
pub struct ProfileManifestState {
    /// The hex-encoded user public key (the profile owner).
    pub user_id: String,
    /// Monotonically increasing revision counter.
    pub revision: u64,
    /// blake3 hash of the serialized manifest at this revision.
    pub manifest_hash: String,
    /// When this revision was committed.
    pub created_at_ms: u64,
}

// ── Contact row (from v1 schema) ──────────────────────────────────────────

/// A row from the v1 `contacts` table.
#[derive(Debug, Clone)]
pub struct ContactRow {
    /// Peer user identity (public key bytes).
    pub user_id: Vec<u8>,
    /// Peer device identity (public key bytes).
    pub device_id: Vec<u8>,
    /// Cached endpoint address, if known.
    pub endpoint_addr: Option<Vec<u8>>,
    /// Identity verification key.
    pub identity_key: Vec<u8>,
    /// Last-seen timestamp in milliseconds since UNIX epoch.
    pub last_seen_ms: u64,
    /// Expiry timestamp in milliseconds since UNIX epoch.
    pub expires_at_ms: u64,
}

/// A row from the v1 `sync_cursor` table.
#[derive(Debug, Clone)]
pub struct SyncCursorRow {
    /// Peer device identity.
    pub peer_device_id: Vec<u8>,
    /// Last observed message clock value.
    pub last_seen_msg_clock: Option<Vec<u8>>,
    /// Last-sync timestamp in milliseconds since UNIX epoch.
    pub last_sync_at_ms: u64,
}

// ── Remote catalogue cache types ─────────────────────────────────────────

/// Cached remote catalogue metadata for a single peer.
#[derive(Debug, Clone)]
pub struct RemoteCatalogueRow {
    /// The remote peer's public key.
    pub owner_id: iroh::PublicKey,
    /// The catalogue revision at the time of caching.
    pub revision: u64,
    /// When the peer generated this catalogue snapshot (ms since UNIX epoch).
    pub generated_at_ms: u64,
    /// When we cached this catalogue locally (ms since UNIX epoch).
    pub fetched_at_ms: u64,
}

/// A cached file entry from a remote peer's file catalogue.
#[derive(Debug, Clone)]
pub struct RemoteSharedFileRow {
    /// The remote peer's public key.
    pub owner_id: iroh::PublicKey,
    /// BLAKE3 content hash (64 hex chars).
    pub content_hash: String,
    /// Display filename shown in the UI.
    pub display_filename: String,
    /// Optional user-entered description.
    pub description: Option<String>,
    /// File size in bytes.
    pub size: u64,
    /// MIME type string (e.g. "image/png").
    pub mime_type: String,
    /// Name of the collection this file belongs to (if any).
    pub collection_name: Option<String>,
    /// Per-file revision counter from the remote catalogue.
    pub file_revision: u32,
    /// Ordinal position within the catalogue.
    pub position: u32,
}

/// A cached collection from a remote peer's file catalogue.
#[derive(Debug, Clone)]
pub struct RemoteCollectionRow {
    /// The remote peer's public key.
    pub owner_id: iroh::PublicKey,
    /// Display name of the collection.
    pub name: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Ordinal position within the catalogue.
    pub position: u32,
}

// ── Storage ───────────────────────────────────────────────────────────────

/// Signed logical direct message persisted before transport delivery.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogicalDirectMessage {
    /// Stable conversation identifier.
    pub conversation_id: [u8; 32],
    /// Author identity.
    pub sender: PublicKey,
    /// Intended recipient identity.
    pub recipient: PublicKey,
    /// Monotonic sequence allocated for this conversation.
    pub sender_sequence: u64,
    /// Original message content.
    pub plaintext: Vec<u8>,
    /// Author signature over the canonical logical fields.
    pub signature: Vec<u8>,
}

/// Result of atomically creating an outgoing direct message.
#[derive(Clone, Debug)]
pub struct QueuedDirectMessage {
    /// Stable hash of the signed logical message.
    pub message_id: MessageId,
    /// Stable conversation identifier.
    pub conversation_id: [u8; 32],
    /// Allocated sender sequence.
    pub sender_sequence: u64,
    /// Exact encrypted envelope persisted for delivery retries.
    pub envelope: crate::mailbox::MailboxEnvelope,
}

/// Relational storage backed by a single SQLite database.
///
/// Owns the connection, schema migrations, and provides repository-style
/// accessors for each logical group of tables.
#[derive(Debug, Clone)]
pub struct Storage {
    conn: Arc<Mutex<Connection>>,
}

impl Storage {
    // ── Open / init ───────────────────────────────────────────────────

    /// Open (or create) the database at `data_dir / `[`DB_FILE_NAME`]`.
    ///
    /// Runs schema migrations automatically so the database is always at
    /// [`CURRENT_SCHEMA_VERSION`] after this call returns.
    /// Runs integrity check and crash-state recovery automatically.
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if !data_dir.exists() {
                std::fs::create_dir_all(data_dir).std_context("create data dir")?;
            }
            let _ = std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700));
        }

        let db_path = data_dir.join(DB_FILE_NAME);
        let conn = Connection::open(&db_path).std_context("open sqlite db")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o600));
        }

        // Crash-safety pragmas: WAL journal for crash recovery, busy timeout
        // for concurrent access, synchronous=NORMAL for performance + safety.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA synchronous = NORMAL;",
        )
        .std_context("set crash-safety pragmas")?;

        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        // Check DB integrity before touching any data.
        storage.check_integrity()?;

        // Run migrations (handles partial migration recovery internally).
        storage.run_migrations()?;

        // Recover any state left dangling by a crash.
        storage.recover_crash_state()?;

        // Recover any downloads left in active states by a crash.
        let recovered_dl = storage.recover_download_crash_state()?;
        if recovered_dl > 0 {
            tracing::info!(
                "recovered {} downloads from active crash states",
                recovered_dl
            );
        }

        Ok(storage)
    }

    /// Open an in-memory database (for tests).
    pub fn memory() -> Result<Self> {
        let conn = Connection::open_in_memory().std_context("open in-memory sqlite db")?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;",
        )
        .std_context("set pragmas")?;
        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        storage.run_migrations()?;
        Ok(storage)
    }

    /// Derive the stable conversation identifier shared by both participants.
    pub fn direct_conversation_id(a: &PublicKey, b: &PublicKey) -> [u8; 32] {
        let (first, second) = if a.as_bytes() <= b.as_bytes() { (a, b) } else { (b, a) };
        *blake3::hash(&[b"boru/direct-conversation/v1".as_slice(), first.as_bytes(), second.as_bytes()].concat()).as_bytes()
    }

    /// Create (or idempotently load) one outgoing DM in a single SQLite transaction.
    ///
    /// The idempotency key must be stable across caller retries. All derived
    /// artifacts (sequence, signature, message id, ciphertext and outbox row)
    /// are created only after the key lookup and committed together.
    pub fn queue_outgoing_dm(
        &self,
        idempotency_key: [u8; 32],
        sender: &SecretKey,
        recipient: PublicKey,
        recipient_mailbox: crate::mailbox::MailboxPublicKey,
        plaintext: &[u8],
        expires_at_ms: u64,
    ) -> Result<QueuedDirectMessage> {
        let conversation_id = Self::direct_conversation_id(&sender.public(), &recipient);
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().std_context("begin outgoing DM transaction")?;
        if let Some((msg_id, conv, seq, envelope_bytes)) = tx
            .query_row(
                "SELECT message_id, conversation_id, sender_sequence, envelope FROM dm_messages JOIN dm_outbox USING(message_id) WHERE idempotency_key = ?1",
                params![idempotency_key.as_slice()],
                |row| {
                    let msg = row.get::<_, Vec<u8>>(0)?;
                    let conv = row.get::<_, Vec<u8>>(1)?;
                    let seq = row.get::<_, i64>(2)? as u64;
                    let envelope = row.get::<_, Vec<u8>>(3)?;
                    Ok((msg, conv, seq, envelope))
                },
            )
            .optional()
            .std_context("lookup outgoing DM idempotency key")?
        {
            let message_id = fixed_bytes::<32>(&msg_id, "message_id")?;
            let conversation_id = fixed_bytes::<32>(&conv, "conversation_id")?;
            let envelope = postcard::from_bytes(&envelope_bytes).map_err(|e| anyhow!("decode stored mailbox envelope: {e}"))?;
            tx.commit().std_context("commit idempotent outgoing DM lookup")?;
            return Ok(QueuedDirectMessage { message_id, conversation_id, sender_sequence: seq, envelope });
        }

        let now = crate::chat_core::now_ms();
        tx.execute(
            "INSERT OR IGNORE INTO dm_conversations(conversation_id, peer_id, created_at_ms) VALUES (?1, ?2, ?3)",
            params![conversation_id.as_slice(), recipient.as_bytes(), now as i64],
        ).std_context("create DM conversation")?;
        let sequence: u64 = tx.query_row(
            "INSERT INTO dm_sender_sequences(conversation_id, next_sequence) VALUES (?1, 2) ON CONFLICT(conversation_id) DO UPDATE SET next_sequence = next_sequence + 1 RETURNING next_sequence - 1",
            params![conversation_id.as_slice()], |row| row.get::<_, i64>(0)
        ).std_context("allocate sender sequence")? as u64;
        let unsigned = postcard::to_stdvec(&(conversation_id, sender.public(), recipient, sequence, plaintext))
            .map_err(|e| anyhow!("encode logical DM: {e}"))?;
        let signature = sender.sign(&unsigned);
        let logical = LogicalDirectMessage { conversation_id, sender: sender.public(), recipient, sender_sequence: sequence, plaintext: plaintext.to_vec(), signature: signature.to_bytes().to_vec() };
        let logical_bytes = postcard::to_stdvec(&logical).map_err(|e| anyhow!("encode signed logical DM: {e}"))?;
        let message_id = *blake3::hash(&logical_bytes).as_bytes();
        let envelope = crate::mailbox::seal_for(sender, recipient_mailbox, &logical_bytes)?;
        let envelope_bytes = postcard::to_stdvec(&envelope).map_err(|e| anyhow!("encode mailbox envelope: {e}"))?;
        tx.execute(
            "INSERT INTO dm_messages(idempotency_key, message_id, conversation_id, sender_sequence, logical_message, plaintext, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![idempotency_key.as_slice(), message_id.as_slice(), conversation_id.as_slice(), sequence as i64, logical_bytes, plaintext, now as i64],
        ).std_context("insert visible outgoing DM")?;
        tx.execute(
            "INSERT INTO dm_outbox(message_id, recipient_id, envelope, next_attempt_at_ms) VALUES (?1, ?2, ?3, ?4)",
            params![message_id.as_slice(), recipient.as_bytes(), envelope_bytes, now.min(expires_at_ms) as i64],
        ).std_context("insert outgoing DM outbox")?;
        tx.commit().std_context("commit outgoing DM")?;
        Ok(QueuedDirectMessage { message_id, conversation_id, sender_sequence: sequence, envelope })
    }

    /// Return the durable outgoing logical message and exact envelope bytes.
    pub fn get_queued_outgoing_dm(&self, message_id: &MessageId) -> Result<Option<QueuedDirectMessage>> {
        let conn = self.conn.lock().unwrap();
        let row = conn.query_row(
            "SELECT conversation_id, sender_sequence, envelope FROM dm_messages JOIN dm_outbox USING(message_id) WHERE dm_messages.message_id = ?1",
            params![message_id.as_slice()],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)? as u64, row.get::<_, Vec<u8>>(2)?)),
        ).optional().std_context("read queued outgoing DM")?;
        row.map(|(conversation_id, sender_sequence, bytes)| {
            let conversation_id = fixed_bytes::<32>(&conversation_id, "conversation_id")?;
            let envelope = postcard::from_bytes(&bytes).map_err(|e| anyhow!("decode stored mailbox envelope: {e}"))?;
            Ok(QueuedDirectMessage { message_id: *message_id, conversation_id, sender_sequence, envelope })
        }).transpose()
    }

    ///
    /// Never silently deletes or rebuilds a damaged database.
    fn check_integrity(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let result: String = conn
            .pragma_query_value(None, "integrity_check", |row| row.get(0))
            .std_context("integrity check")?;
        if result != "ok" {
            return Err(anyhow!(
                "Database integrity check failed: {result}. \
                 The database is corrupt and cannot be opened. Restore from backup or delete the file."
            ).into());
        }
        Ok(())
    }

    /// Recover state left dangling by a crash or corruption.
    ///
    /// 1. **Crash-left Sent outbox** — rows stuck in `Sent` (1) are reset to
    ///    `Pending` so the delivery engine retries them.
    /// 2. **Preserve ACKs** — rows with status `Acked` (2) are never touched.
    /// 3. **Stale Pending timestamps** — rows with `next_attempt_at_ms` in the
    ///    future are reset to now so they become due immediately.
    /// 4. **Orphan outbox entries** — outbox rows whose `msg_id` does not
    ///    reference an existing inbox row are retained, marked terminal, and
    ///    annotated with a recovery diagnostic so a backup/migration can
    ///    restore the envelope without silent loss.
    /// 5. **Stale acked rows** — outbox rows whose `msg_id` has an inbox
    ///    `acked_at_ms` set but are not yet in `Acked` status are advanced
    ///    to `Acked` to match reality.
    /// 6. **Missing conversation metadata** — inbox conversations without a
    ///    corresponding `conversation_meta` row get one created automatically.
    /// 7. **Conversation meta without inbox messages** — `conversation_meta`
    ///    rows that reference no inbox messages at all are flagged as deleted
    ///    (soft-delete) to avoid confusing the UI with empty conversations.
    fn recover_crash_state(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = crate::chat_core::now_ms() as i64;

        // 1. Reset crash-left "Sent" rows back to "Pending".
        conn.execute(
            "UPDATE outbox SET
                status = ?1,
                next_attempt_at_ms = ?2,
                last_error_code = 'crash_recovered'
             WHERE status = ?3 AND attempts > 0",
            params![
                crate::store::DeliveryStatus::Pending as u8,
                now,
                crate::store::DeliveryStatus::Sent as u8,
            ],
        )
        .std_context("recover crash-left Sent outbox")?;

        // 2. Reset stale Pending timestamps to now.
        conn.execute(
            "UPDATE outbox SET
                next_attempt_at_ms = ?1
             WHERE status = ?2 AND next_attempt_at_ms > ?3",
            params![now, crate::store::DeliveryStatus::Pending as u8, now,],
        )
        .std_context("recover stale Pending outbox timestamps")?;

        // Preserve orphan rows: the envelope may be recoverable from a backup or
        // migration.  Silently deleting the only durable delivery reference
        // would lose state.  Mark it terminal and retain the diagnostic reason.
        conn.execute(
            "UPDATE outbox SET
                status = ?1,
                last_error_code = 'orphan_outbox_missing_envelope',
                last_attempt_at_ms = ?2
             WHERE msg_id NOT IN (SELECT msg_id FROM inbox)
               AND status != ?1",
            params![crate::store::DeliveryStatus::Expired as u8, now],
        )
        .std_context("mark orphan outbox entries")?;

        // 4. Advance outbox rows to Acked where the inbox message is already acked.
        conn.execute(
            "UPDATE outbox SET
                status = ?1,
                last_error_code = 'recovered_stale_ack',
                last_attempt_at_ms = ?2
             WHERE status != ?1
               AND msg_id IN (
                   SELECT msg_id FROM inbox WHERE acked_at_ms IS NOT NULL
               )",
            params![crate::store::DeliveryStatus::Acked as u8, now,],
        )
        .std_context("recover stale acked outbox rows")?;

        // 5. Ensure every inbox conversation_id has a conversation_meta row.
        conn.execute(
            "INSERT OR IGNORE INTO conversation_meta
                (conversation_id, last_activity_at_ms, unread_count)
             SELECT DISTINCT conversation_id, ?1, 0
             FROM inbox",
            params![now],
        )
        .std_context("ensure conversation_meta exists")?;

        // 6. Soft-delete conversation_meta rows that have no inbox messages
        //    and have never been active.  This prevents orphan conversation
        //    entries from confusing the UI after data loss recovery.
        conn.execute(
            "UPDATE conversation_meta SET
                is_deleted = 1
             WHERE is_deleted = 0
               AND last_activity_at_ms = 0
               AND conversation_id NOT IN (
                   SELECT DISTINCT conversation_id FROM inbox
               )",
            [],
        )
        .std_context("mark orphan conversation meta as deleted")?;

        Ok(())
    }

    /// Maximum number of downloads to recover in a single call to
    /// [`recover_download_crash_state`]. Prevents a huge backlog from
    /// blocking startup.
    const MAX_DOWNLOAD_RECOVERIES: u32 = 32;

    /// Recover downloads left in active states after a crash or unclean
    /// shutdown.
    ///
    /// Active states (`ResolvingPeer`, `RequestingPermission`, `Downloading`,
    /// `Verifying`) are interrupted by a crash — the in-memory background
    /// tasks (peer resolution, QUIC connections, blob streaming) are gone and
    /// cannot be resumed transparently.
    ///
    /// # Recovery rules
    ///
    /// | Crash state | Recovered to | Rationale |
    /// |---|---|----|
    /// | `Downloading` | `Paused` | iroh-blobs store retains partial chunks;
    ///   user can resume to fetch only missing chunks |
    /// | `ResolvingPeer` | `Queued` | Peer resolution is stateless; retry from
    ///   start |
    /// | `RequestingPermission` | `Queued` | Permission request state lost;
    ///   retry from start |
    /// | `Verifying` | `Failed` | Verification context lost; cannot trust
    ///   partial verify; user can retry manually |
    ///
    /// The following states are **never** touched: `Complete`, `Cancelled`,
    /// `Paused`, `Queued`, `Failed`.  This ensures we never mark an
    /// incomplete download as complete.
    ///
    /// # Bounded processing
    ///
    /// Up to [`MAX_DOWNLOAD_RECOVERIES`] downloads are processed per call.
    /// Returns the number of downloads actually recovered.
    pub fn recover_download_crash_state(&self) -> Result<u32> {
        let now = crate::chat_core::now_ms() as i64;
        let mut recovered: u32 = 0;
        let max = Self::MAX_DOWNLOAD_RECOVERIES as i64;

        // Process each active state with a bounded approach:
        // 1. Select up to MAX ids in the state
        // 2. Update each individually (SQLite bundled with rusqlite
        //    does not support LIMIT on UPDATE).

        // Downloading → Paused (preserve partial chunks in iroh-blobs store)
        {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn
                .prepare("SELECT id FROM downloads WHERE state = 'downloading' LIMIT ?1")
                .std_context("prep recover downloading")?;
            let ids: Vec<i64> = stmt
                .query_map(rusqlite::params![max], |row| row.get(0))
                .std_context("query downloading ids")?
                .filter_map(|r| r.ok())
                .collect();
            drop(stmt);
            for id in &ids {
                conn.execute(
                    "UPDATE downloads SET state = 'paused', updated_at_ms = ?1 WHERE id = ?2",
                    rusqlite::params![now, id],
                )
                .std_context("recover downloading -> paused")?;
            }
            recovered += ids.len() as u32;
        }

        // ResolvingPeer -> Queued (retry from start, reset counters)
        {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn
                .prepare("SELECT id FROM downloads WHERE state = 'resolving_peer' LIMIT ?1")
                .std_context("prep recover resolving_peer")?;
            let ids: Vec<i64> = stmt
                .query_map(rusqlite::params![max], |row| row.get(0))
                .std_context("query resolving_peer ids")?
                .filter_map(|r| r.ok())
                .collect();
            drop(stmt);
            for id in &ids {
                conn.execute(
                    "UPDATE downloads SET state = 'queued', retry_count = 0, bytes_downloaded = 0, updated_at_ms = ?1 WHERE id = ?2",
                    rusqlite::params![now, id],
                ).std_context("recover resolving_peer -> queued")?;
            }
            recovered += ids.len() as u32;
        }

        // RequestingPermission -> Queued (retry from start, reset counters)
        {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn
                .prepare("SELECT id FROM downloads WHERE state = 'requesting_permission' LIMIT ?1")
                .std_context("prep recover requesting_permission")?;
            let ids: Vec<i64> = stmt
                .query_map(rusqlite::params![max], |row| row.get(0))
                .std_context("query requesting_permission ids")?
                .filter_map(|r| r.ok())
                .collect();
            drop(stmt);
            for id in &ids {
                conn.execute(
                    "UPDATE downloads SET state = 'queued', retry_count = 0, bytes_downloaded = 0, updated_at_ms = ?1 WHERE id = ?2",
                    rusqlite::params![now, id],
                ).std_context("recover requesting_permission -> queued")?;
            }
            recovered += ids.len() as u32;
        }

        // Verifying -> Failed (verification context lost, cannot trust)
        {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn
                .prepare("SELECT id FROM downloads WHERE state = 'verifying' LIMIT ?1")
                .std_context("prep recover verifying")?;
            let ids: Vec<i64> = stmt
                .query_map(rusqlite::params![max], |row| row.get(0))
                .std_context("query verifying ids")?
                .filter_map(|r| r.ok())
                .collect();
            drop(stmt);
            for id in &ids {
                conn.execute(
                    "UPDATE downloads SET state = 'failed', last_error = 'crash interrupted verification - please retry', retry_count = retry_count + 1, updated_at_ms = ?1 WHERE id = ?2",
                    rusqlite::params![now, id],
                ).std_context("recover verifying -> failed")?;
            }
            recovered += ids.len() as u32;
        }

        Ok(recovered)
    }

    // ── Migrations ────────────────────────────────────────────────────

    fn run_migrations(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // First ensure the version table itself exists.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                applied_at_ms INTEGER NOT NULL
            );",
        )
        .std_context("create schema_version table")?;

        let current: Option<u32> = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .optional()
            .std_context("query schema version")?
            .flatten();

        // Guard: if the database was created by a newer version of the
        // application, refuse to open it. This prevents data loss that
        // could occur if we silently skipped migrations.
        if let Some(version) = current {
            if version > CURRENT_SCHEMA_VERSION {
                return Err(anyhow!(
                    "Database has schema version {version}, but this application \
                     only supports up to version {max}. The database was created \
                     by a newer version. Upgrade the application or restore from \
                     a backup created by an older version.",
                    max = CURRENT_SCHEMA_VERSION,
                )
                .into());
            }
        }

        let start = current.unwrap_or(0);
        if start >= CURRENT_SCHEMA_VERSION {
            return Ok(());
        }

        // Run each migration in its own transaction.
        for v in (start + 1)..=CURRENT_SCHEMA_VERSION {
            match v {
                1 => self.migrate_v1(&conn)?,
                2 => self.migrate_v2(&conn)?,
                3 => self.migrate_v3(&conn)?,
                4 => self.migrate_v4(&conn)?,
                5 => self.migrate_v5(&conn)?,
                _ => unreachable!("unknown migration version {v}"),
            }
            let now = now_ms();
            conn.execute(
                "INSERT INTO schema_version (version, applied_at_ms) VALUES (?1, ?2)",
                params![v, now as i64],
            )
            .std_context("record schema version")?;
        }

        Ok(())
    }

    /// V1: message-delivery tables (from the original `store.rs`).
    fn migrate_v1(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS inbox (
                msg_id BLOB PRIMARY KEY,
                conversation_id BLOB NOT NULL,
                author_user_id BLOB NOT NULL,
                author_device_id BLOB NOT NULL,
                created_at_ms INTEGER NOT NULL,
                expires_at_ms INTEGER NOT NULL,
                ciphertext BLOB NOT NULL,
                signature BLOB NOT NULL,
                acked_at_ms INTEGER
            );

            CREATE TABLE IF NOT EXISTS outbox (
                msg_id BLOB NOT NULL,
                recipient_device_id BLOB NOT NULL,
                status INTEGER NOT NULL,
                attempts INTEGER NOT NULL,
                next_attempt_at_ms INTEGER NOT NULL,
                last_error_code TEXT,
                last_attempt_at_ms INTEGER,
                PRIMARY KEY (msg_id, recipient_device_id)
            );

            CREATE TABLE IF NOT EXISTS contacts (
                user_id BLOB NOT NULL,
                device_id BLOB NOT NULL,
                endpoint_addr BLOB,
                identity_key BLOB NOT NULL,
                last_seen_ms INTEGER NOT NULL,
                expires_at_ms INTEGER NOT NULL,
                PRIMARY KEY (user_id, device_id)
            );

            CREATE TABLE IF NOT EXISTS sync_cursor (
                peer_device_id BLOB PRIMARY KEY,
                last_seen_msg_clock BLOB,
                last_sync_at_ms INTEGER NOT NULL
            );

            -- Per-conversation metadata used by crash recovery and the UI.
            -- Keep this in the initial message schema so recover_crash_state()
            -- is safe on a newly-created authoritative storage database.
            CREATE TABLE IF NOT EXISTS conversation_meta (
                conversation_id BLOB PRIMARY KEY,
                last_message_id BLOB,
                last_activity_at_ms INTEGER NOT NULL DEFAULT 0,
                last_message_preview TEXT NOT NULL DEFAULT '',
                last_author_user_id BLOB,
                unread_count INTEGER NOT NULL DEFAULT 0,
                is_muted INTEGER NOT NULL DEFAULT 0,
                is_archived INTEGER NOT NULL DEFAULT 0,
                is_deleted INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .std_context("migrate v1")?;
        Ok(())
    }

    /// V2: content-addressed file objects and sharing extension points.
    fn migrate_v2(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            -- Content-addressed file object store.
            -- Holds actual file data (for small files) or a blob-id reference.
            -- This is the single source of truth for file content; both
            -- message attachments and shared file offers reference rows here.
            CREATE TABLE file_objects (
                content_hash TEXT PRIMARY KEY,
                size INTEGER NOT NULL,
                mime_type TEXT NOT NULL DEFAULT 'application/octet-stream',
                filename TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                data BLOB,
                blob_hash TEXT,
                imported_from_peer TEXT,
                imported_at_ms INTEGER
            );

            -- Links a chat message to one or more file objects.
            -- Belongs to the message domain; the message is the
            -- authoritative owner of these rows.
            CREATE TABLE message_attachments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id INTEGER NOT NULL,
                content_hash TEXT NOT NULL REFERENCES file_objects(content_hash),
                display_filename TEXT NOT NULL,
                position INTEGER NOT NULL DEFAULT 0,
                UNIQUE(event_id, content_hash)
            );
            CREATE INDEX idx_message_attachments_event
                ON message_attachments(event_id);
            CREATE INDEX idx_message_attachments_hash
                ON message_attachments(content_hash);

            -- Profile-offered shared files.
            -- Belongs to the profile domain; a profile may offer any
            -- file_object it has stored locally.
            CREATE TABLE shared_files (
                content_hash TEXT NOT NULL REFERENCES file_objects(content_hash),
                profile_user_id TEXT NOT NULL,
                metadata_id TEXT NOT NULL,
                display_filename TEXT NOT NULL,
                description TEXT,
                offered INTEGER NOT NULL DEFAULT 1,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY (content_hash, profile_user_id)
            );
            CREATE INDEX idx_shared_files_profile
                ON shared_files(profile_user_id);
            CREATE INDEX idx_shared_files_metadata
                ON shared_files(metadata_id);

            -- Named collections of shared files.
            CREATE TABLE file_collections (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                profile_user_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                UNIQUE(profile_user_id, name)
            );
            CREATE INDEX idx_file_collections_profile
                ON file_collections(profile_user_id);

            -- Membership: which files are in which collections.
            CREATE TABLE file_collection_items (
                collection_id INTEGER NOT NULL REFERENCES file_collections(id)
                    ON DELETE CASCADE,
                content_hash TEXT NOT NULL REFERENCES file_objects(content_hash),
                position INTEGER NOT NULL DEFAULT 0,
                added_at_ms INTEGER NOT NULL,
                PRIMARY KEY (collection_id, content_hash)
            );
            CREATE INDEX idx_file_collection_items_hash
                ON file_collection_items(content_hash);

            -- Per-peer permission grants on shared files.
            -- The grantor is the profile owner; the grantee is a peer.
            CREATE TABLE shared_file_permissions (
                content_hash TEXT NOT NULL REFERENCES file_objects(content_hash),
                grantor_user_id TEXT NOT NULL,
                grantee_user_id TEXT NOT NULL,
                permission TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                expires_at_ms INTEGER,
                PRIMARY KEY (content_hash, grantor_user_id, grantee_user_id, permission)
            );
            CREATE INDEX idx_shared_file_perms_grantee
                ON shared_file_permissions(grantee_user_id);

            -- Durable download state machine.
            -- Tracks file transfers from remote peers, surviving restarts.
            CREATE TABLE downloads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                content_hash TEXT NOT NULL REFERENCES file_objects(content_hash),
                remote_peer TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'queued',
                bytes_downloaded INTEGER NOT NULL DEFAULT 0,
                total_bytes INTEGER NOT NULL DEFAULT 0,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                last_error TEXT,
                retry_count INTEGER NOT NULL DEFAULT 0,
                next_retry_at_ms INTEGER
            );
            CREATE INDEX idx_downloads_state
                ON downloads(state);
            CREATE INDEX idx_downloads_hash
                ON downloads(content_hash);

            -- Profile manifest revision state.
            -- One row per local profile, tracking the current revision
            -- counter and manifest hash so peers can detect changes.
            CREATE TABLE profile_manifest_state (
                user_id TEXT PRIMARY KEY,
                revision INTEGER NOT NULL DEFAULT 0,
                manifest_hash TEXT NOT NULL DEFAULT '',
                created_at_ms INTEGER NOT NULL
            );
            ",
        )
        .std_context("migrate v2")?;
        Ok(())
    }

    /// V3: remote catalogue cache tables for caching peers' shared file catalogues.
    fn migrate_v3(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            -- Cached remote file catalogues: one row per remote peer.
            -- Stores the current revision and timestamp so peers can skip
            -- already-seen catalogues without re-fetching.
            CREATE TABLE IF NOT EXISTS remote_catalogues (
                owner_id BLOB PRIMARY KEY,
                revision INTEGER NOT NULL,
                generated_at_ms INTEGER NOT NULL,
                fetched_at_ms INTEGER NOT NULL
            );

            -- Cached file entries from remote peers' catalogues.
            -- Each row corresponds to one file entry in a remote catalogue.
            -- Cascade-deleted when the parent remote_catalogues row is removed.
            CREATE TABLE IF NOT EXISTS remote_shared_files (
                owner_id BLOB NOT NULL REFERENCES remote_catalogues(owner_id)
                    ON DELETE CASCADE,
                content_hash TEXT NOT NULL,
                display_filename TEXT NOT NULL,
                description TEXT,
                size INTEGER NOT NULL,
                mime_type TEXT NOT NULL,
                collection_name TEXT,
                file_revision INTEGER NOT NULL,
                position INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (owner_id, content_hash)
            );
            CREATE INDEX IF NOT EXISTS idx_remote_shared_files_owner
                ON remote_shared_files(owner_id);

            -- Cached named collections from remote peers' catalogues.
            -- Each row is one named collection (e.g. 'Photos', 'Documents').
            -- Cascade-deleted when the parent remote_catalogues row is removed.
            CREATE TABLE IF NOT EXISTS remote_collections (
                owner_id BLOB NOT NULL REFERENCES remote_catalogues(owner_id)
                    ON DELETE CASCADE,
                name TEXT NOT NULL,
                description TEXT,
                position INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (owner_id, name)
            );
            CREATE INDEX IF NOT EXISTS idx_remote_collections_owner
                ON remote_collections(owner_id);
            ",
        )
        .std_context("migrate v3")?;
        Ok(())
    }

    /// V4: Added columns to `downloads` for the expanded download state machine.
    ///
    /// Adds:
    /// - `remote_shared_file_id` — the remote peer's catalogue file identifier
    /// - `destination_path` — local safe destination chosen before transfer
    ///
    /// Also normalises old state values to the new enum:
    /// - `'active'` → `'downloading'`
    /// - `'completed'` → `'complete'`
    fn migrate_v4(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            -- Add new columns (with defaults for any existing rows).
            ALTER TABLE downloads ADD COLUMN remote_shared_file_id TEXT NOT NULL DEFAULT '';
            ALTER TABLE downloads ADD COLUMN destination_path TEXT NOT NULL DEFAULT '';

            -- Normalise old state values to the new enum.
            UPDATE downloads SET state = 'downloading' WHERE state = 'active';
            UPDATE downloads SET state = 'complete' WHERE state = 'completed';

            -- Local-only metadata for the profile library; never sent remotely.
            CREATE TABLE IF NOT EXISTS file_availability (
                content_hash TEXT NOT NULL,
                profile_user_id TEXT NOT NULL,
                availability TEXT NOT NULL,
                checked_at_ms INTEGER,
                original_hash TEXT NOT NULL DEFAULT '',
                original_size INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (content_hash, profile_user_id)
            );
            ",
        )
        .std_context("migrate v4")?;
        Ok(())
    }

    /// V5: durable, idempotent outgoing direct-message creation tables.
    fn migrate_v5(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS dm_conversations (
                conversation_id BLOB PRIMARY KEY,
                peer_id BLOB NOT NULL,
                created_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS dm_sender_sequences (
                conversation_id BLOB PRIMARY KEY REFERENCES dm_conversations(conversation_id),
                next_sequence INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS dm_messages (
                idempotency_key BLOB PRIMARY KEY,
                message_id BLOB NOT NULL UNIQUE,
                conversation_id BLOB NOT NULL REFERENCES dm_conversations(conversation_id),
                sender_sequence INTEGER NOT NULL,
                logical_message BLOB NOT NULL,
                plaintext BLOB NOT NULL,
                created_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS dm_outbox (
                message_id BLOB PRIMARY KEY REFERENCES dm_messages(message_id),
                recipient_id BLOB NOT NULL,
                envelope BLOB NOT NULL,
                status INTEGER NOT NULL DEFAULT 0,
                attempts INTEGER NOT NULL DEFAULT 0,
                next_attempt_at_ms INTEGER NOT NULL
            );
            ",
        )
        .std_context("migrate v5")?;
        Ok(())
    }

    // ── Inbox (v1) ────────────────────────────────────────────────────
    /// Idempotent insert into inbox.
    pub fn insert_inbox(&self, env: &StoredEnvelope) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let acked = env.acked_at_ms.map(|v| v as i64);
        conn.execute(
            "INSERT INTO inbox (
                msg_id, conversation_id, author_user_id, author_device_id,
                created_at_ms, expires_at_ms, ciphertext, signature, acked_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(msg_id) DO NOTHING",
            params![
                env.msg_id.as_slice(),
                env.conversation_id.as_slice(),
                env.author_user_id.as_bytes(),
                env.author_device_id.as_bytes(),
                env.created_at_ms as i64,
                env.expires_at_ms as i64,
                env.ciphertext.as_ref(),
                env.signature.as_slice(),
                acked,
            ],
        )
        .std_context("insert inbox")?;
        Ok(())
    }

    /// Retrieve an inbox message by id.
    pub fn get_inbox(&self, msg_id: &MessageId) -> Result<Option<StoredEnvelope>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT conversation_id, author_user_id, author_device_id,
                        created_at_ms, expires_at_ms, ciphertext, signature, acked_at_ms
                 FROM inbox WHERE msg_id = ?1",
            )
            .std_context("prepare get_inbox")?;
        let mut rows = stmt
            .query([msg_id.as_slice()])
            .std_context("query get_inbox")?;
        if let Some(row) = rows.next().std_context("next row")? {
            Ok(Some(row_to_envelope(msg_id, row)?))
        } else {
            Ok(None)
        }
    }

    /// List all inbox messages (with optional limit).
    pub fn list_inbox(&self, limit: Option<u32>) -> Result<Vec<StoredEnvelope>> {
        let conn = self.conn.lock().unwrap();
        let sql = match limit {
            Some(n) => format!(
                "SELECT msg_id, conversation_id, author_user_id, author_device_id,
                        created_at_ms, expires_at_ms, ciphertext, signature, acked_at_ms
                 FROM inbox ORDER BY created_at_ms DESC LIMIT {n}"
            ),
            None => String::from(
                "SELECT msg_id, conversation_id, author_user_id, author_device_id,
                        created_at_ms, expires_at_ms, ciphertext, signature, acked_at_ms
                 FROM inbox ORDER BY created_at_ms DESC",
            ),
        };
        let mut stmt = conn.prepare(&sql).std_context("prepare list_inbox")?;
        let mut rows = stmt.query([]).std_context("query list_inbox")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            let msg_id_blob: Vec<u8> = row.get(0).std_context("get msg_id")?;
            let msg_id = fixed_bytes::<32>(&msg_id_blob, "msg_id")?;
            results.push(row_to_envelope_bare(&msg_id, row)?);
        }
        Ok(results)
    }

    // ── Outbox (v1) ───────────────────────────────────────────────────

    /// Enqueue a message for delivery.
    pub fn enqueue_outbox(
        &self,
        msg_id: &MessageId,
        recipient_device_id: iroh::PublicKey,
        next_attempt_at_ms: u64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO outbox (
                msg_id, recipient_device_id, status, attempts, next_attempt_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(msg_id, recipient_device_id) DO NOTHING",
            params![
                msg_id.as_slice(),
                recipient_device_id.as_bytes(),
                DeliveryStatus::Pending as u8,
                0,
                next_attempt_at_ms as i64,
            ],
        )
        .std_context("insert outbox")?;
        Ok(())
    }

    /// Mark an outbox message as acked.
    pub fn mark_acked(
        &self,
        msg_id: &MessageId,
        recipient_device_id: iroh::PublicKey,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE outbox SET status = ?1 WHERE msg_id = ?2 AND recipient_device_id = ?3",
            params![
                DeliveryStatus::Acked as u8,
                msg_id.as_slice(),
                recipient_device_id.as_bytes(),
            ],
        )
        .std_context("mark acked")?;
        Ok(())
    }

    /// Record a delivery attempt.
    pub fn record_attempt(
        &self,
        msg_id: &MessageId,
        recipient_device_id: iroh::PublicKey,
        next_attempt_at_ms: u64,
        error_code: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now_ms = now_ms();
        conn.execute(
            "UPDATE outbox SET
                attempts = attempts + 1,
                next_attempt_at_ms = ?1,
                last_error_code = ?2,
                last_attempt_at_ms = ?3,
                status = ?4
             WHERE msg_id = ?5 AND recipient_device_id = ?6 AND status != ?7",
            params![
                next_attempt_at_ms as i64,
                error_code,
                now_ms as i64,
                DeliveryStatus::Sent as u8,
                msg_id.as_slice(),
                recipient_device_id.as_bytes(),
                DeliveryStatus::Acked as u8,
            ],
        )
        .std_context("record attempt")?;
        Ok(())
    }

    /// Fetch pending messages due for retry.
    pub fn fetch_due_outbox(&self, now_ms: u64) -> Result<Vec<OutboxRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT msg_id, recipient_device_id, status, attempts,
                        next_attempt_at_ms, last_error_code, last_attempt_at_ms
                 FROM outbox
                 WHERE status != ?1 AND status != ?2 AND next_attempt_at_ms <= ?3",
            )
            .std_context("prepare fetch_due_outbox")?;
        let mut rows = stmt
            .query(params![
                DeliveryStatus::Acked as u8,
                DeliveryStatus::Expired as u8,
                now_ms as i64
            ])
            .std_context("query due outbox")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(row_to_outbox(row)?);
        }
        Ok(results)
    }

    /// Expire outbox messages past their message expiry.
    pub fn expire_outbox(&self, now_ms: u64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE outbox SET status = ?1
             WHERE status != ?2 AND status != ?1 AND msg_id IN (
                 SELECT msg_id FROM inbox WHERE expires_at_ms <= ?3
             )",
            params![
                DeliveryStatus::Expired as u8,
                DeliveryStatus::Acked as u8,
                now_ms as i64
            ],
        )
        .std_context("expire outbox")?;
        Ok(0) // rusqlite::Connection::execute returns changed rows on some
        // builds; we don't need the exact count here.
    }

    // ── Contacts (v1) ─────────────────────────────────────────────────

    /// Upsert a contact.
    pub fn upsert_contact(
        &self,
        user_id: &iroh::PublicKey,
        device_id: &iroh::PublicKey,
        endpoint_addr: Option<&[u8]>,
        identity_key: &[u8],
        last_seen_ms: u64,
        expires_at_ms: u64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO contacts (user_id, device_id, endpoint_addr, identity_key,
                                   last_seen_ms, expires_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(user_id, device_id) DO UPDATE SET
                endpoint_addr = excluded.endpoint_addr,
                identity_key = excluded.identity_key,
                last_seen_ms = excluded.last_seen_ms,
                expires_at_ms = excluded.expires_at_ms",
            params![
                user_id.as_bytes(),
                device_id.as_bytes(),
                endpoint_addr,
                identity_key,
                last_seen_ms as i64,
                expires_at_ms as i64,
            ],
        )
        .std_context("upsert contact")?;
        Ok(())
    }

    /// List all contacts.
    pub fn list_contacts(&self) -> Result<Vec<ContactRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT user_id, device_id, endpoint_addr, identity_key,
                        last_seen_ms, expires_at_ms
                 FROM contacts ORDER BY last_seen_ms DESC",
            )
            .std_context("prepare list_contacts")?;
        let mut rows = stmt.query([]).std_context("query contacts")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(ContactRow {
                user_id: row.get(0).std_context("get user_id")?,
                device_id: row.get(1).std_context("get device_id")?,
                endpoint_addr: row.get(2).std_context("get endpoint_addr")?,
                identity_key: row.get(3).std_context("get identity_key")?,
                last_seen_ms: row.get::<_, i64>(4).std_context("get last_seen")? as u64,
                expires_at_ms: row.get::<_, i64>(5).std_context("get expires_at")? as u64,
            });
        }
        Ok(results)
    }

    // ── Sync cursor (v1) ─────────────────────────────────────────────

    /// Upsert a sync cursor.
    pub fn upsert_sync_cursor(
        &self,
        peer_device_id: &iroh::PublicKey,
        last_seen_msg_clock: Option<&[u8]>,
        last_sync_at_ms: u64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_cursor (peer_device_id, last_seen_msg_clock, last_sync_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(peer_device_id) DO UPDATE SET
                last_seen_msg_clock = excluded.last_seen_msg_clock,
                last_sync_at_ms = excluded.last_sync_at_ms",
            params![
                peer_device_id.as_bytes(),
                last_seen_msg_clock,
                last_sync_at_ms as i64,
            ],
        )
        .std_context("upsert sync_cursor")?;
        Ok(())
    }

    /// Get all sync cursors.
    pub fn list_sync_cursors(&self) -> Result<Vec<SyncCursorRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT peer_device_id, last_seen_msg_clock, last_sync_at_ms FROM sync_cursor")
            .std_context("prepare list_sync_cursors")?;
        let mut rows = stmt.query([]).std_context("query sync cursors")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(SyncCursorRow {
                peer_device_id: row.get(0).std_context("get peer_device_id")?,
                last_seen_msg_clock: row.get(1).std_context("get last_seen_msg_clock")?,
                last_sync_at_ms: row.get::<_, i64>(2).std_context("get last_sync_at_ms")? as u64,
            });
        }
        Ok(results)
    }

    // ── File objects (v2) ─────────────────────────────────────────────

    /// Store a file object. If the content hash already exists, returns the
    /// existing row without modifying it (idempotent).
    pub fn put_file_object(
        &self,
        content_hash: &str,
        size: u64,
        mime_type: &str,
        filename: &str,
        data: &[u8],
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "INSERT OR IGNORE INTO file_objects
                (content_hash, size, mime_type, filename, created_at_ms, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![content_hash, size as i64, mime_type, filename, now, data],
        )
        .std_context("put file_object")?;
        Ok(())
    }

    /// Store a file object that was imported from a remote peer (blob reference).
    pub fn put_imported_file_object(
        &self,
        content_hash: &str,
        size: u64,
        mime_type: &str,
        filename: &str,
        blob_hash: &str,
        imported_from_peer: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "INSERT OR IGNORE INTO file_objects
                (content_hash, size, mime_type, filename, created_at_ms,
                 blob_hash, imported_from_peer, imported_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                content_hash,
                size as i64,
                mime_type,
                filename,
                now,
                blob_hash,
                imported_from_peer,
                now,
            ],
        )
        .std_context("put imported file_object")?;
        Ok(())
    }

    /// Look up a file object by content hash.
    pub fn get_file_object(&self, content_hash: &str) -> Result<Option<FileObject>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT content_hash, size, mime_type, filename, created_at_ms, data
                 FROM file_objects WHERE content_hash = ?1",
            )
            .std_context("prepare get_file_object")?;
        let mut rows = stmt
            .query(params![content_hash])
            .std_context("query file_object")?;
        if let Some(row) = rows.next().std_context("next row")? {
            Ok(Some(FileObject {
                content_hash: row.get(0).std_context("get hash")?,
                size: row.get::<_, i64>(1).std_context("get size")? as u64,
                mime_type: row.get(2).std_context("get mime")?,
                filename: row.get(3).std_context("get filename")?,
                created_at_ms: row.get::<_, i64>(4).std_context("get created_at")? as u64,
                data: row.get::<_, Option<Vec<u8>>>(5).std_context("get data")?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Check whether a file object with the given hash exists.
    pub fn file_object_exists(&self, content_hash: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM file_objects WHERE content_hash = ?1",
                params![content_hash],
                |_| Ok(true),
            )
            .optional()
            .std_context("check file_object exists")?
            .unwrap_or(false);
        Ok(exists)
    }

    /// Delete a file object. Fails if any foreign-key references remain.
    pub fn delete_file_object(&self, content_hash: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "DELETE FROM file_objects WHERE content_hash = ?1",
                params![content_hash],
            )
            .std_context("delete file_object")?;
        Ok(n > 0)
    }

    /// Delete a shared file row.  Returns `true` if a row was actually deleted.
    pub fn delete_shared_file(&self, content_hash: &str, profile_user_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "DELETE FROM shared_files WHERE content_hash = ?1 AND profile_user_id = ?2",
                params![content_hash, profile_user_id],
            )
            .std_context("delete shared_file")?;
        Ok(n > 0)
    }

    // ── Message attachments (v2) ──────────────────────────────────────

    /// Attach a file object to a chat message.
    pub fn attach_file_to_message(
        &self,
        event_id: u64,
        content_hash: &str,
        display_filename: &str,
        position: u32,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO message_attachments
                (event_id, content_hash, display_filename, position)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                event_id as i64,
                content_hash,
                display_filename,
                position as i64
            ],
        )
        .std_context("insert message_attachment")?;
        let id = conn.last_insert_rowid();
        Ok(id)
    }

    /// List all attachments for a message.
    pub fn get_message_attachments(&self, event_id: u64) -> Result<Vec<MessageAttachment>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, event_id, content_hash, display_filename, position
                 FROM message_attachments
                 WHERE event_id = ?1
                 ORDER BY position",
            )
            .std_context("prepare get_message_attachments")?;
        let mut rows = stmt
            .query(params![event_id as i64])
            .std_context("query attachments")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(MessageAttachment {
                id: row.get(0).std_context("get id")?,
                event_id: row.get::<_, i64>(1).std_context("get event_id")? as u64,
                content_hash: row.get(2).std_context("get hash")?,
                display_filename: row.get(3).std_context("get filename")?,
                position: row.get::<_, i64>(4).std_context("get position")? as u32,
            });
        }
        Ok(results)
    }

    /// Remove an attachment by its id.
    pub fn remove_message_attachment(&self, id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute("DELETE FROM message_attachments WHERE id = ?1", params![id])
            .std_context("remove message_attachment")?;
        Ok(n > 0)
    }

    /// Find all messages that reference a given file object.
    pub fn find_messages_for_file(&self, content_hash: &str) -> Result<Vec<u64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT event_id FROM message_attachments WHERE content_hash = ?1")
            .std_context("prepare find_messages_for_file")?;
        let mut rows = stmt
            .query(params![content_hash])
            .std_context("query find_messages")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(row.get::<_, i64>(0).std_context("get event_id")? as u64);
        }
        Ok(results)
    }

    // ── Shared files (v2) ─────────────────────────────────────────────

    /// Offer a file from a profile.
    pub fn upsert_shared_file(
        &self,
        content_hash: &str,
        profile_user_id: &str,
        metadata_id: &str,
        display_filename: &str,
        description: Option<&str>,
        offered: bool,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "INSERT INTO shared_files
                (content_hash, profile_user_id, metadata_id, display_filename,
                 description, offered, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(content_hash, profile_user_id) DO UPDATE SET
                metadata_id = excluded.metadata_id,
                display_filename = excluded.display_filename,
                description = excluded.description,
                offered = excluded.offered,
                updated_at_ms = excluded.updated_at_ms",
            params![
                content_hash,
                profile_user_id,
                metadata_id,
                display_filename,
                description,
                offered as i64,
                now,
            ],
        )
        .std_context("upsert shared_file")?;
        Ok(())
    }

    /// List offered files for a profile.
    pub fn list_shared_files(
        &self,
        profile_user_id: &str,
        offered_only: bool,
    ) -> Result<Vec<SharedFileRow>> {
        let conn = self.conn.lock().unwrap();
        let sql = if offered_only {
            "SELECT content_hash, profile_user_id, metadata_id, display_filename,
                    description, offered, created_at_ms, updated_at_ms
             FROM shared_files
             WHERE profile_user_id = ?1 AND offered = 1
             ORDER BY updated_at_ms DESC"
        } else {
            "SELECT content_hash, profile_user_id, metadata_id, display_filename,
                    description, offered, created_at_ms, updated_at_ms
             FROM shared_files
             WHERE profile_user_id = ?1
             ORDER BY updated_at_ms DESC"
        };
        let mut stmt = conn.prepare(sql).std_context("prepare list_shared_files")?;
        let mut rows = stmt
            .query(params![profile_user_id])
            .std_context("query shared_files")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(SharedFileRow {
                content_hash: row.get(0).std_context("get hash")?,
                profile_user_id: row.get(1).std_context("get profile")?,
                metadata_id: row.get(2).std_context("get metadata_id")?,
                display_filename: row.get(3).std_context("get filename")?,
                description: row.get(4).std_context("get desc")?,
                offered: row.get::<_, i64>(5).std_context("get offered")? != 0,
                created_at_ms: row.get::<_, i64>(6).std_context("get created")? as u64,
                updated_at_ms: row.get::<_, i64>(7).std_context("get updated")? as u64,
            });
        }
        Ok(results)
    }

    /// Get a specific shared file entry.
    pub fn get_shared_file(
        &self,
        profile_user_id: &str,
        content_hash: &str,
    ) -> Result<Option<SharedFileRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT content_hash, profile_user_id, metadata_id, display_filename,
                        description, offered, created_at_ms, updated_at_ms
                 FROM shared_files
                 WHERE profile_user_id = ?1 AND content_hash = ?2",
            )
            .std_context("prepare get_shared_file")?;
        let mut rows = stmt
            .query(params![profile_user_id, content_hash])
            .std_context("query shared_file")?;
        if let Some(row) = rows.next().std_context("next row")? {
            Ok(Some(SharedFileRow {
                content_hash: row.get(0).std_context("get hash")?,
                profile_user_id: row.get(1).std_context("get profile")?,
                metadata_id: row.get(2).std_context("get metadata_id")?,
                display_filename: row.get(3).std_context("get filename")?,
                description: row.get(4).std_context("get desc")?,
                offered: row.get::<_, i64>(5).std_context("get offered")? != 0,
                created_at_ms: row.get::<_, i64>(6).std_context("get created")? as u64,
                updated_at_ms: row.get::<_, i64>(7).std_context("get updated")? as u64,
            }))
        } else {
            Ok(None)
        }
    }

    // ── File collections (v2) ─────────────────────────────────────────

    /// Create or get a named collection for a profile.
    pub fn ensure_collection(
        &self,
        profile_user_id: &str,
        name: &str,
        description: Option<&str>,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "INSERT OR IGNORE INTO file_collections
                (profile_user_id, name, description, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![profile_user_id, name, description, now],
        )
        .std_context("ensure collection")?;
        let id: i64 = conn
            .query_row(
                "SELECT id FROM file_collections WHERE profile_user_id = ?1 AND name = ?2",
                params![profile_user_id, name],
                |row| row.get(0),
            )
            .std_context("get collection id")?;
        Ok(id)
    }

    /// List collections for a profile.
    pub fn list_collections(&self, profile_user_id: &str) -> Result<Vec<FileCollection>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, profile_user_id, name, description, created_at_ms, updated_at_ms
                 FROM file_collections
                 WHERE profile_user_id = ?1
                 ORDER BY name",
            )
            .std_context("prepare list_collections")?;
        let mut rows = stmt
            .query(params![profile_user_id])
            .std_context("query collections")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(FileCollection {
                id: row.get(0).std_context("get id")?,
                profile_user_id: row.get(1).std_context("get profile")?,
                name: row.get(2).std_context("get name")?,
                description: row.get(3).std_context("get desc")?,
                created_at_ms: row.get::<_, i64>(4).std_context("get created")? as u64,
                updated_at_ms: row.get::<_, i64>(5).std_context("get updated")? as u64,
            });
        }
        Ok(results)
    }

    /// Add a file to a collection.
    pub fn add_to_collection(
        &self,
        collection_id: i64,
        content_hash: &str,
        position: u32,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "INSERT OR REPLACE INTO file_collection_items
                (collection_id, content_hash, position, added_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![collection_id, content_hash, position as i64, now],
        )
        .std_context("add to collection")?;
        Ok(())
    }

    /// List items in a collection.
    pub fn list_collection_items(&self, collection_id: i64) -> Result<Vec<FileCollectionItem>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT collection_id, content_hash, position, added_at_ms
                 FROM file_collection_items
                 WHERE collection_id = ?1
                 ORDER BY position",
            )
            .std_context("prepare list_collection_items")?;
        let mut rows = stmt
            .query(params![collection_id])
            .std_context("query items")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(FileCollectionItem {
                collection_id: row.get(0).std_context("get collection_id")?,
                content_hash: row.get(1).std_context("get hash")?,
                position: row.get::<_, i64>(2).std_context("get position")? as u32,
                added_at_ms: row.get::<_, i64>(3).std_context("get added_at")? as u64,
            });
        }
        Ok(results)
    }

    /// Rename a local profile collection.
    pub fn rename_collection(&self, collection_id: i64, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE file_collections SET name = ?1, updated_at_ms = ?2 WHERE id = ?3",
            params![name, now_ms() as i64, collection_id],
        )
        .std_context("rename collection")?;
        Ok(())
    }

    /// Delete a local profile collection and its membership rows.
    pub fn delete_collection(&self, collection_id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "DELETE FROM file_collections WHERE id = ?1",
                params![collection_id],
            )
            .std_context("delete collection")?;
        Ok(n > 0)
    }

    /// Remove a file from a collection.
    pub fn remove_from_collection(&self, collection_id: i64, content_hash: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "DELETE FROM file_collection_items
                 WHERE collection_id = ?1 AND content_hash = ?2",
                params![collection_id, content_hash],
            )
            .std_context("remove from collection")?;
        Ok(n > 0)
    }

    // ── Permissions (v2) ──────────────────────────────────────────────

    /// Grant a permission to a peer on a shared file.
    pub fn grant_permission(
        &self,
        content_hash: &str,
        grantor_user_id: &str,
        grantee_user_id: &str,
        permission: &str,
        expires_at_ms: Option<u64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "INSERT OR REPLACE INTO shared_file_permissions
                (content_hash, grantor_user_id, grantee_user_id, permission,
                 created_at_ms, expires_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                content_hash,
                grantor_user_id,
                grantee_user_id,
                permission,
                now,
                expires_at_ms.map(|v| v as i64),
            ],
        )
        .std_context("grant permission")?;
        Ok(())
    }

    /// Revoke a specific permission.
    pub fn revoke_permission(
        &self,
        content_hash: &str,
        grantor_user_id: &str,
        grantee_user_id: &str,
        permission: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "DELETE FROM shared_file_permissions
                 WHERE content_hash = ?1 AND grantor_user_id = ?2
                   AND grantee_user_id = ?3 AND permission = ?4",
                params![content_hash, grantor_user_id, grantee_user_id, permission],
            )
            .std_context("revoke permission")?;
        Ok(n > 0)
    }

    /// Check if a grantee has a specific permission on a file.
    pub fn check_permission(
        &self,
        content_hash: &str,
        grantee_user_id: &str,
        permission: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        let has: bool = conn
            .query_row(
                "SELECT 1 FROM shared_file_permissions
                 WHERE content_hash = ?1 AND grantee_user_id = ?2
                   AND permission = ?3
                   AND (expires_at_ms IS NULL OR expires_at_ms > ?4)",
                params![content_hash, grantee_user_id, permission, now],
                |_| Ok(true),
            )
            .optional()
            .std_context("check permission")?
            .unwrap_or(false);
        Ok(has)
    }

    /// List all permissions granted to a peer.
    pub fn list_permissions_for_grantee(
        &self,
        grantee_user_id: &str,
    ) -> Result<Vec<SharedFilePermission>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT content_hash, grantor_user_id, grantee_user_id, permission,
                        created_at_ms, expires_at_ms
                 FROM shared_file_permissions
                 WHERE grantee_user_id = ?1
                 ORDER BY created_at_ms DESC",
            )
            .std_context("prepare list_permissions_for_grantee")?;
        let mut rows = stmt
            .query(params![grantee_user_id])
            .std_context("query permissions")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(SharedFilePermission {
                content_hash: row.get(0).std_context("get hash")?,
                grantor_user_id: row.get(1).std_context("get grantor")?,
                grantee_user_id: row.get(2).std_context("get grantee")?,
                permission: row.get(3).std_context("get permission")?,
                created_at_ms: row.get::<_, i64>(4).std_context("get created")? as u64,
                expires_at_ms: row
                    .get::<_, Option<i64>>(5)
                    .std_context("get expires")?
                    .map(|v| v as u64),
            });
        }
        Ok(results)
    }

    // ── Downloads (v2/v4) ───────────────────────────────────────────

    /// Create a download entry (Queued state).
    ///
    /// The record is committed to storage **before** any network transfer
    /// begins.  All fields are set based on the information available at
    /// the time the user clicks "Download": the source peer, the remote
    /// shared-file identifier, the expected content hash and size (from
    /// the catalogue), and the local destination path.
    pub fn create_download(
        &self,
        content_hash: &str,
        remote_peer: &str,
        remote_shared_file_id: &str,
        destination_path: &Path,
        expected_size: u64,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "INSERT INTO downloads
                (content_hash, remote_peer, remote_shared_file_id, destination_path,
                 state, bytes_downloaded, total_bytes,
                 created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, 'queued', 0, ?5, ?6, ?6)",
            params![
                content_hash,
                remote_peer,
                remote_shared_file_id,
                destination_path.to_str().unwrap_or_default(),
                expected_size as i64,
                now,
            ],
        )
        .std_context("create download")?;
        Ok(conn.last_insert_rowid())
    }

    /// Update download progress and state.
    ///
    /// # Errors
    ///
    /// Returns [`crate::download::InvalidTransition`] (wrapped in
    /// `n0_error::Error`) if the state transition is invalid.
    pub fn update_download_progress(
        &self,
        id: i64,
        bytes_downloaded: u64,
        new_state: DownloadState,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;

        // Validate transition against current state.
        let current = self.get_download_inner(&conn, id)?;
        if let Some(ref dl) = current {
            if !dl.state.can_transition_to(new_state) {
                return Err(anyhow!(
                    "{}",
                    crate::download::InvalidTransition {
                        from: dl.state,
                        to: new_state,
                    }
                )
                .into());
            }
        }

        conn.execute(
            "UPDATE downloads SET bytes_downloaded = ?1, state = ?2, updated_at_ms = ?3
             WHERE id = ?4",
            params![bytes_downloaded as i64, new_state.as_str(), now, id],
        )
        .std_context("update download progress")?;
        Ok(())
    }

    /// Mark a download as failed with an error message.
    ///
    /// Valid from any non-terminal state.  Increments `retry_count` and
    /// sets `next_retry_at_ms` for automatic retry scheduling.
    pub fn fail_download(&self, id: i64, error: &str, next_retry_at_ms: Option<u64>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;

        // Validate that this is a valid transition.
        let current = self.get_download_inner(&conn, id)?;
        if let Some(ref dl) = current {
            if !dl.state.can_transition_to(DownloadState::Failed) {
                return Err(anyhow!(
                    "{}",
                    crate::download::InvalidTransition {
                        from: dl.state,
                        to: DownloadState::Failed,
                    }
                )
                .into());
            }
        }

        conn.execute(
            "UPDATE downloads SET state = 'failed', last_error = ?1,
                    retry_count = retry_count + 1, next_retry_at_ms = ?2,
                    updated_at_ms = ?3
             WHERE id = ?4",
            params![error, next_retry_at_ms.map(|v| v as i64), now, id,],
        )
        .std_context("fail download")?;
        Ok(())
    }

    /// Get a download by id.
    pub fn get_download(&self, id: i64) -> Result<Option<Download>> {
        let conn = self.conn.lock().unwrap();
        self.get_download_inner(&conn, id)
    }

    /// Get a download by id (with already-locked connection).
    fn get_download_inner(&self, conn: &Connection, id: i64) -> Result<Option<Download>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, content_hash, remote_peer, remote_shared_file_id,
                        destination_path, state, bytes_downloaded, total_bytes,
                        created_at_ms, updated_at_ms, last_error,
                        retry_count, next_retry_at_ms
                 FROM downloads WHERE id = ?1",
            )
            .std_context("prepare get_download")?;
        let mut rows = stmt.query(params![id]).std_context("query download")?;
        if let Some(row) = rows.next().std_context("next row")? {
            Ok(Some(row_to_download(row)?))
        } else {
            Ok(None)
        }
    }

    /// List all downloads.
    pub fn list_downloads(&self) -> Result<Vec<Download>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, content_hash, remote_peer, remote_shared_file_id,
                        destination_path, state, bytes_downloaded, total_bytes,
                        created_at_ms, updated_at_ms, last_error,
                        retry_count, next_retry_at_ms
                 FROM downloads
                 ORDER BY created_at_ms ASC",
            )
            .std_context("prepare list_downloads")?;
        let mut rows = stmt.query([]).std_context("query downloads")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(row_to_download(row)?);
        }
        Ok(results)
    }

    /// List downloads in a given state.
    pub fn list_downloads_by_state(&self, state: DownloadState) -> Result<Vec<Download>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, content_hash, remote_peer, remote_shared_file_id,
                        destination_path, state, bytes_downloaded, total_bytes,
                        created_at_ms, updated_at_ms, last_error,
                        retry_count, next_retry_at_ms
                 FROM downloads WHERE state = ?1
                 ORDER BY created_at_ms ASC",
            )
            .std_context("prepare list_downloads_by_state")?;
        let mut rows = stmt
            .query(params![state.as_str()])
            .std_context("query downloads")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(row_to_download(row)?);
        }
        Ok(results)
    }

    /// Transition a download to the `VersionMismatch` state with a
    /// description of what changed.
    ///
    /// Valid from `RequestingPermission`, `Downloading`, or `Paused`.
    /// Sets `last_error` to describe the mismatch and stores the new
    /// expected content hash and size from the cached remote catalogue
    /// (if available).  The download remains in a non-terminal state
    /// so the user can choose to accept the new version, cancel, or
    /// retry.
    pub fn flag_version_mismatch(&self, id: i64, error: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;

        // Validate transition against current state.
        let current = self.get_download_inner(&conn, id)?;
        if let Some(ref dl) = current {
            if !dl.state.can_transition_to(DownloadState::VersionMismatch) {
                return Err(anyhow!(
                    "{}",
                    crate::download::InvalidTransition {
                        from: dl.state,
                        to: DownloadState::VersionMismatch,
                    }
                )
                .into());
            }
        }

        conn.execute(
            "UPDATE downloads SET state = 'version_mismatch', last_error = ?1,
                    updated_at_ms = ?2
             WHERE id = ?3",
            params![error, now, id],
        )
        .std_context("flag version mismatch")?;
        Ok(())
    }

    /// Transition a download to a new state, validating the transition
    /// against the state machine rules.
    ///
    /// Returns [`crate::download::InvalidTransition`] if the transition
    /// is not valid.
    pub fn transition_download(&self, id: i64, new_state: DownloadState) -> Result<()> {
        self.update_download_progress(id, 0, new_state)
    }

    /// Cancel a download (moves to `Cancelled` terminal state).
    ///
    /// Valid from any non-terminal state.
    pub fn cancel_download(&self, id: i64) -> Result<()> {
        self.transition_download(id, DownloadState::Cancelled)
    }

    /// Pause an active download (moves to `Paused`).
    ///
    /// Valid only from `Downloading`.
    pub fn pause_download(&self, id: i64) -> Result<()> {
        self.transition_download(id, DownloadState::Paused)
    }

    /// Resume a paused download (moves back to `Downloading`).
    ///
    /// Valid only from `Paused`.
    pub fn resume_download(&self, id: i64) -> Result<()> {
        self.transition_download(id, DownloadState::Downloading)
    }

    /// Retry a failed download (moves to `Queued` so the engine picks it
    /// up again).
    ///
    /// Valid only from `Failed`.
    pub fn retry_download(&self, id: i64) -> Result<()> {
        self.transition_download(id, DownloadState::Queued)
    }

    /// Count downloads in a given state.
    pub fn count_downloads_by_state(&self, state: DownloadState) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE state = ?1",
                params![state.as_str()],
                |row| row.get(0),
            )
            .std_context("count downloads by state")?;
        Ok(count as u64)
    }

    /// Delete a download record (for cleanup of terminal records).
    pub fn delete_download(&self, id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn
            .execute("DELETE FROM downloads WHERE id = ?1", params![id])
            .std_context("delete download")?;
        Ok(affected > 0)
    }

    // ── Profile manifest state (v2) ───────────────────────────────────

    /// Update the manifest revision for a profile.
    /// Increments the revision counter so the next call always produces a
    /// higher revision than the previous one.
    pub fn bump_manifest_revision(&self, user_id: &str, manifest_hash: &str) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;

        // Read-modify-write within a single write to avoid races.
        let current: Option<u64> = conn
            .query_row(
                "SELECT revision FROM profile_manifest_state WHERE user_id = ?1",
                params![user_id],
                |row| row.get::<_, i64>(0).map(|v| v as u64),
            )
            .optional()
            .std_context("query manifest revision")?;

        let new_rev = current.unwrap_or(0) + 1;

        conn.execute(
            "INSERT INTO profile_manifest_state
                (user_id, revision, manifest_hash, created_at_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id) DO UPDATE SET
                revision = excluded.revision,
                manifest_hash = excluded.manifest_hash,
                created_at_ms = excluded.created_at_ms",
            params![user_id, new_rev as i64, manifest_hash, now],
        )
        .std_context("bump manifest revision")?;

        Ok(new_rev)
    }

    /// Get the current manifest state for a profile.
    pub fn get_manifest_state(&self, user_id: &str) -> Result<Option<ProfileManifestState>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT user_id, revision, manifest_hash, created_at_ms
                 FROM profile_manifest_state WHERE user_id = ?1",
            )
            .std_context("prepare get_manifest_state")?;
        let mut rows = stmt
            .query(params![user_id])
            .std_context("query manifest state")?;
        if let Some(row) = rows.next().std_context("next row")? {
            Ok(Some(ProfileManifestState {
                user_id: row.get(0).std_context("get user_id")?,
                revision: row.get::<_, i64>(1).std_context("get revision")? as u64,
                manifest_hash: row.get(2).std_context("get hash")?,
                created_at_ms: row.get::<_, i64>(3).std_context("get created_at")? as u64,
            }))
        } else {
            Ok(None)
        }
    }

    // ── Utility: export all data for migration from old store ─────────

    /// Import data from the legacy [`crate::store::MessageStore`] format.
    ///
    /// This is the migration pathway: if the database file exists already
    /// from the old `MessageStore`, this method reads it and copies data into
    /// the new storage schema.  After calling this, the old database can be
    /// archived.
    pub fn import_legacy_db(&self, legacy_path: &Path) -> Result<()> {
        if !legacy_path.exists() {
            return Ok(());
        }

        let legacy = Connection::open(legacy_path).std_context("open legacy db")?;

        // Import inbox.
        {
            let mut stmt = legacy
                .prepare(
                    "SELECT msg_id, conversation_id, author_user_id, author_device_id,
                            created_at_ms, expires_at_ms, ciphertext, signature, acked_at_ms
                     FROM inbox",
                )
                .std_context("prepare legacy inbox")?;
            let mut rows = stmt.query([]).std_context("query legacy inbox")?;
            let mut count = 0;
            while let Some(row) = rows.next().std_context("next legacy row")? {
                let msg_id_blob: Vec<u8> = row.get(0).std_context("get msg_id")?;
                let msg_id = fixed_bytes::<32>(&msg_id_blob, "legacy msg_id")?;
                let env = row_to_envelope_bare(&msg_id, row)?;
                self.insert_inbox(&env)?;
                count += 1;
            }
            tracing::info!(count, "imported legacy inbox messages");
        }

        // Import outbox.
        {
            let mut stmt = legacy
                .prepare(
                    "SELECT msg_id, recipient_device_id, status, attempts,
                            next_attempt_at_ms, last_error_code, last_attempt_at_ms
                     FROM outbox",
                )
                .std_context("prepare legacy outbox")?;
            let mut rows = stmt.query([]).std_context("query legacy outbox")?;
            let conn = self.conn.lock().unwrap();
            let mut count = 0;
            while let Some(row) = rows.next().std_context("next legacy row")? {
                conn.execute(
                    "INSERT OR IGNORE INTO outbox
                        (msg_id, recipient_device_id, status, attempts,
                         next_attempt_at_ms, last_error_code, last_attempt_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        row.get::<_, Vec<u8>>(0).std_context("get msg_id")?,
                        row.get::<_, Vec<u8>>(1).std_context("get recip")?,
                        row.get::<_, u8>(2).std_context("get status")?,
                        row.get::<_, u32>(3).std_context("get attempts")?,
                        row.get::<_, i64>(4).std_context("get next_attempt")?,
                        row.get::<_, Option<String>>(5).std_context("get error")?,
                        row.get::<_, Option<i64>>(6)
                            .std_context("get last_attempt")?,
                    ],
                )
                .std_context("insert legacy outbox")?;
                count += 1;
            }
            tracing::info!(count, "imported legacy outbox messages");
        }

        // Import contacts.
        {
            let mut stmt = legacy
                .prepare(
                    "SELECT user_id, device_id, endpoint_addr, identity_key,
                            last_seen_ms, expires_at_ms FROM contacts",
                )
                .std_context("prepare legacy contacts")?;
            let mut rows = stmt.query([]).std_context("query legacy contacts")?;
            let conn = self.conn.lock().unwrap();
            let mut count = 0;
            while let Some(row) = rows.next().std_context("next legacy row")? {
                conn.execute(
                    "INSERT OR IGNORE INTO contacts
                        (user_id, device_id, endpoint_addr, identity_key,
                         last_seen_ms, expires_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        row.get::<_, Vec<u8>>(0).std_context("get user_id")?,
                        row.get::<_, Vec<u8>>(1).std_context("get device_id")?,
                        row.get::<_, Option<Vec<u8>>>(2)
                            .std_context("get endpoint")?,
                        row.get::<_, Vec<u8>>(3).std_context("get identity_key")?,
                        row.get::<_, i64>(4).std_context("get last_seen")?,
                        row.get::<_, i64>(5).std_context("get expires_at")?,
                    ],
                )
                .std_context("insert legacy contact")?;
                count += 1;
            }
            tracing::info!(count, "imported legacy contacts");
        }

        // Import sync cursors.
        {
            let mut stmt = legacy
                .prepare(
                    "SELECT peer_device_id, last_seen_msg_clock, last_sync_at_ms FROM sync_cursor",
                )
                .std_context("prepare legacy sync_cursor")?;
            let mut rows = stmt.query([]).std_context("query legacy sync cursors")?;
            let conn = self.conn.lock().unwrap();
            let mut count = 0;
            while let Some(row) = rows.next().std_context("next legacy row")? {
                conn.execute(
                    "INSERT OR IGNORE INTO sync_cursor
                        (peer_device_id, last_seen_msg_clock, last_sync_at_ms)
                     VALUES (?1, ?2, ?3)",
                    params![
                        row.get::<_, Vec<u8>>(0).std_context("get peer_device_id")?,
                        row.get::<_, Option<Vec<u8>>>(1).std_context("get clock")?,
                        row.get::<_, i64>(2).std_context("get last_sync")?,
                    ],
                )
                .std_context("insert legacy sync_cursor")?;
                count += 1;
            }
            tracing::info!(count, "imported legacy sync cursors");
        }

        Ok(())
    }

    /// Return the raw [`rusqlite::Connection`] (locked) for advanced use.
    /// Prefer the typed repository methods when possible.
    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self.conn.lock().unwrap();
        f(&conn)
    }

    // ── Remote catalogue cache operations ────────────────────────────

    /// Atomically replace a remote peer's entire cached catalogue.
    ///
    /// The operation is fully transactional:
    /// 1. The incoming `catalogue` must already be verified by the caller
    ///    (signature, ownership, field limits).
    /// 2. A SQLite transaction is started.
    /// 3. The `remote_catalogues` row is upserted.
    /// 4. All stale `remote_shared_files` entries are deleted.
    /// 5. All new file entries are inserted.
    /// 6. All stale `remote_collections` entries are deleted.
    /// 7. All new collection entries are inserted.
    /// 8. The transaction is committed.
    ///
    /// Returns `true` if the revision changed (new data was stored),
    /// or `false` if the cached revision was already current (only
    /// the `fetched_at_ms` timestamp is updated).
    #[cfg(feature = "net")]
    pub fn replace_remote_catalogue(
        &self,
        catalogue: &crate::catalogue_model::SignedFileCatalogue,
    ) -> Result<bool> {
        let owner_bytes = catalogue.owner_id.as_bytes().to_vec();
        let now_ms = now_ms();

        // Check if revision has changed.
        let is_new_revision = {
            let conn = self.conn.lock().unwrap();
            let prev_revision: Option<i64> = conn
                .query_row(
                    "SELECT revision FROM remote_catalogues WHERE owner_id = ?1",
                    [&owner_bytes],
                    |row| row.get(0),
                )
                .optional()
                .std_context("query previous revision")?
                .flatten();
            prev_revision.map_or(true, |prev| prev as u64 != catalogue.revision)
        };

        if !is_new_revision {
            // Update fetched_at_ms even if revision hasn't changed.
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "UPDATE remote_catalogues SET fetched_at_ms = ?1 WHERE owner_id = ?2",
                params![now_ms as i64, &owner_bytes],
            )
            .std_context("update remote_catalogue timestamp")?;
            return Ok(false);
        }

        // Fully transactional replacement.
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .std_context("begin remote catalogue replace tx")?;

        // 1. Upsert the catalogue row.
        tx.execute(
            "INSERT OR REPLACE INTO remote_catalogues
                (owner_id, revision, generated_at_ms, fetched_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                &owner_bytes,
                catalogue.revision as i64,
                catalogue.generated_at_ms as i64,
                now_ms as i64,
            ],
        )
        .std_context("upsert remote_catalogue")?;

        // 2. Delete stale shared file entries.
        tx.execute(
            "DELETE FROM remote_shared_files WHERE owner_id = ?1",
            [&owner_bytes],
        )
        .std_context("delete stale remote files")?;

        // 3. Insert new file entries.
        for (pos, file) in catalogue.files.iter().enumerate() {
            tx.execute(
                "INSERT INTO remote_shared_files
                    (owner_id, content_hash, display_filename, description,
                     size, mime_type, collection_name, file_revision, position)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    &owner_bytes,
                    &file.content_hash,
                    &file.display_filename,
                    file.description.as_deref(),
                    file.size as i64,
                    &file.mime_type,
                    file.collection_name.as_deref(),
                    file.revision as i64,
                    pos as i32,
                ],
            )
            .std_context("insert remote shared file")?;
        }

        // 4. Delete stale collection entries.
        tx.execute(
            "DELETE FROM remote_collections WHERE owner_id = ?1",
            [&owner_bytes],
        )
        .std_context("delete stale remote collections")?;

        // 5. Insert new collection entries.
        for (pos, coll) in catalogue.collections.iter().enumerate() {
            tx.execute(
                "INSERT INTO remote_collections (owner_id, name, description, position)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    &owner_bytes,
                    &coll.name,
                    coll.description.as_deref(),
                    pos as i32,
                ],
            )
            .std_context("insert remote collection")?;
        }

        tx.commit().std_context("commit remote catalogue replace")?;
        Ok(true)
    }

    /// Get the cached catalogue metadata for a remote peer.
    ///
    /// Returns `None` if no catalogue is cached for this peer.
    pub fn get_remote_catalogue_meta(
        &self,
        owner_id: &iroh::PublicKey,
    ) -> Result<Option<RemoteCatalogueRow>> {
        let owner_bytes = owner_id.as_bytes();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT revision, generated_at_ms, fetched_at_ms
                 FROM remote_catalogues WHERE owner_id = ?1",
            )
            .std_context("prepare get_remote_catalogue_meta")?;
        let mut rows = stmt
            .query([owner_bytes])
            .std_context("query get_remote_catalogue_meta")?;
        if let Some(row) = rows.next().std_context("next row")? {
            Ok(Some(RemoteCatalogueRow {
                owner_id: *owner_id,
                revision: row.get::<_, i64>(0).std_context("get revision")? as u64,
                generated_at_ms: row.get::<_, i64>(1).std_context("get generated_at_ms")? as u64,
                fetched_at_ms: row.get::<_, i64>(2).std_context("get fetched_at_ms")? as u64,
            }))
        } else {
            Ok(None)
        }
    }

    /// List all cached file entries from a remote peer's catalogue.
    ///
    /// Returns an empty vec if no catalogue is cached for this peer.
    pub fn get_remote_shared_files(
        &self,
        owner_id: &iroh::PublicKey,
    ) -> Result<Vec<RemoteSharedFileRow>> {
        let owner_bytes = owner_id.as_bytes();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT content_hash, display_filename, description,
                        size, mime_type, collection_name, file_revision, position
                 FROM remote_shared_files WHERE owner_id = ?1
                 ORDER BY position ASC",
            )
            .std_context("prepare get_remote_shared_files")?;
        let mut rows = stmt
            .query([owner_bytes])
            .std_context("query get_remote_shared_files")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(RemoteSharedFileRow {
                owner_id: *owner_id,
                content_hash: row.get(0).std_context("get content_hash")?,
                display_filename: row.get(1).std_context("get display_filename")?,
                description: row.get(2).std_context("get description")?,
                size: row.get::<_, i64>(3).std_context("get size")? as u64,
                mime_type: row.get(4).std_context("get mime_type")?,
                collection_name: row.get(5).std_context("get collection_name")?,
                file_revision: row.get::<_, i32>(6).std_context("get file_revision")? as u32,
                position: row.get::<_, i32>(7).std_context("get position")? as u32,
            });
        }
        Ok(results)
    }

    /// List all cached collections from a remote peer's catalogue.
    ///
    /// Returns an empty vec if no catalogue is cached for this peer.
    pub fn get_remote_collections(
        &self,
        owner_id: &iroh::PublicKey,
    ) -> Result<Vec<RemoteCollectionRow>> {
        let owner_bytes = owner_id.as_bytes();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT name, description, position
                 FROM remote_collections WHERE owner_id = ?1
                 ORDER BY position ASC",
            )
            .std_context("prepare get_remote_collections")?;
        let mut rows = stmt
            .query([owner_bytes])
            .std_context("query get_remote_collections")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(RemoteCollectionRow {
                owner_id: *owner_id,
                name: row.get(0).std_context("get name")?,
                description: row.get(1).std_context("get description")?,
                position: row.get::<_, i32>(2).std_context("get position")? as u32,
            });
        }
        Ok(results)
    }

    /// Delete the cached catalogue for a remote peer entirely.
    ///
    /// Cascade-deletes all associated `remote_shared_files` and
    /// `remote_collections` rows.
    ///
    /// Returns `true` if a row was actually deleted, `false` if no
    /// catalogue existed for this peer.
    pub fn delete_remote_catalogue(&self, owner_id: &iroh::PublicKey) -> Result<bool> {
        let owner_bytes = owner_id.as_bytes();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM remote_catalogues WHERE owner_id = ?1",
            [owner_bytes],
        )
        .std_context("delete remote catalogue")?;
        Ok(conn.changes() > 0)
    }

    /// List all remote peers whose catalogues are cached locally.
    ///
    /// Ordered by `fetched_at_ms` descending (most recently fetched first).
    pub fn list_cached_remote_catalogues(&self) -> Result<Vec<RemoteCatalogueRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT owner_id, revision, generated_at_ms, fetched_at_ms
                 FROM remote_catalogues ORDER BY fetched_at_ms DESC",
            )
            .std_context("prepare list_cached_remote_catalogues")?;
        let mut rows = stmt
            .query([])
            .std_context("query list_cached_remote_catalogues")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            let owner_blob: Vec<u8> = row.get(0).std_context("get owner_id")?;
            let owner_id = iroh::PublicKey::try_from(owner_blob.as_slice())
                .map_err(|e| anyhow!("invalid public key: {e}"))?;
            results.push(RemoteCatalogueRow {
                owner_id,
                revision: row.get::<_, i64>(1).std_context("get revision")? as u64,
                generated_at_ms: row.get::<_, i64>(2).std_context("get generated_at_ms")? as u64,
                fetched_at_ms: row.get::<_, i64>(3).std_context("get fetched_at_ms")? as u64,
            });
        }
        Ok(results)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Describes what, if anything, has changed in the remote peer's cached
/// catalogue since a download was initiated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogueChange {
    /// No change detected — the cached remote file still matches the
    /// download's expected values.
    Unchanged,
    /// The content hash has changed (different version of the file).
    ContentChanged {
        /// Expected content hash at download creation time.
        old_hash: String,
        /// Current content hash in the cached remote catalogue.
        new_hash: String,
        /// Expected file size at download creation time.
        old_size: u64,
        /// Current file size in the cached remote catalogue.
        new_size: u64,
    },
    /// The file is no longer present in the remote peer's cached catalogue.
    OfferRemoved,
    /// No cached catalogue exists for this remote peer.
    NoCatalogue,
}

impl Storage {
    /// Check whether the remote file targeted by a download has changed in
    /// the cached remote catalogue.
    ///
    /// Looks up the download by `id`, then compares its expected content hash
    /// and size against the current cached `remote_shared_files` entry for
    /// the download's remote peer.
    ///
    /// Returns [`CatalogueChange::Unchanged`] if the file is still there with
    /// the same content hash and size.
    pub fn check_download_catalogue_change(&self, id: i64) -> Result<CatalogueChange> {
        let dl = self
            .get_download(id)?
            .ok_or_else(|| anyhow!("download {id} not found"))?;

        let remote_peer = dl
            .remote_peer
            .parse::<iroh::PublicKey>()
            .map_err(|e| anyhow!("invalid remote peer key in download {id}: {e}"))?;

        // Look up the file in the cached remote catalogue.
        let files = self.get_remote_shared_files(&remote_peer)?;

        // Find the file by its content hash (which serves as the shared file ID).
        let cached = files
            .iter()
            .find(|f| f.content_hash == dl.remote_shared_file_id);

        match cached {
            None => {
                // The file is no longer in the remote peer's cached catalogue.
                Ok(CatalogueChange::OfferRemoved)
            }
            Some(cached_file) => {
                if cached_file.content_hash != dl.content_hash {
                    Ok(CatalogueChange::ContentChanged {
                        old_hash: dl.content_hash.clone(),
                        new_hash: cached_file.content_hash.clone(),
                        old_size: dl.expected_size,
                        new_size: cached_file.size,
                    })
                } else if cached_file.size != dl.expected_size {
                    // Same content hash but different size — unusual but possible.
                    Ok(CatalogueChange::ContentChanged {
                        old_hash: dl.content_hash.clone(),
                        new_hash: cached_file.content_hash.clone(),
                        old_size: dl.expected_size,
                        new_size: cached_file.size,
                    })
                } else {
                    Ok(CatalogueChange::Unchanged)
                }
            }
        }
    }

    // ── Local profile file library methods ────────────────────────────

    /// Get the original (previous) content hash for a referenced file.
    /// Legacy — file verification tracking was consolidated in the SQLite redesign.
    /// Returns None when no previous hash is recorded.
    pub fn get_original_hash(
        &self,
        _content_hash: &str,
        _profile_user_id: &str,
    ) -> Result<Option<String>> {
        Ok(None)
    }

    /// Record a file content hash replacement relationship.
    /// Legacy no-op — replacement tracking was consolidated in the SQLite redesign.
    pub fn record_file_replacement(
        &self,
        _old_hash: &str,
        _new_hash: &str,
        _profile_user_id: &str,
    ) -> Result<()> {
        Ok(())
    }

    /// Set local availability metadata for a shared file.
    pub fn set_file_availability(
        &self,
        content_hash: &str,
        profile_user_id: &str,
        status: &str,
        checked_at_ms: Option<u64>,
        original_hash: &str,
        original_size: u64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO file_availability
                (content_hash, profile_user_id, availability, checked_at_ms, original_hash, original_size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(content_hash, profile_user_id) DO UPDATE SET
                availability = excluded.availability, checked_at_ms = excluded.checked_at_ms,
                original_hash = excluded.original_hash, original_size = excluded.original_size",
            params![content_hash, profile_user_id, status, checked_at_ms.map(|v| v as i64), original_hash, original_size as i64],
        )
        .std_context("set file availability")?;
        Ok(())
    }

    /// Read local availability metadata for a shared file.
    pub fn get_file_availability(
        &self,
        content_hash: &str,
        profile_user_id: &str,
    ) -> Result<Option<FileAvailability>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT content_hash, profile_user_id, availability, checked_at_ms, original_hash, original_size
             FROM file_availability WHERE content_hash = ?1 AND profile_user_id = ?2",
            params![content_hash, profile_user_id],
            |row| Ok(FileAvailability {
                content_hash: row.get(0)?, profile_user_id: row.get(1)?, availability: row.get(2)?,
                checked_at_ms: row.get::<_, Option<i64>>(3)?.map(|v| v as u64),
                original_hash: row.get(4)?, original_size: row.get::<_, i64>(5)? as u64,
            }),
        ).optional().std_context("get file availability")
    }

    /// Update local profile-library metadata; no path is exposed remotely.
    pub fn update_shared_file_metadata(
        &self,
        content_hash: &str,
        profile_user_id: &str,
        display_filename: &str,
        description: Option<&str>,
        metadata_id: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE shared_files SET display_filename = ?1, description = ?2,
            metadata_id = ?3, updated_at_ms = ?4 WHERE content_hash = ?5 AND profile_user_id = ?6",
            params![
                display_filename,
                description,
                metadata_id,
                now_ms() as i64,
                content_hash,
                profile_user_id
            ],
        )
        .std_context("update shared file metadata")?;
        Ok(())
    }

    /// Enable or disable a local profile offer.
    pub fn set_shared_file_offered(
        &self,
        content_hash: &str,
        profile_user_id: &str,
        offered: bool,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE shared_files SET offered = ?1, updated_at_ms = ?2
            WHERE content_hash = ?3 AND profile_user_id = ?4",
            params![
                offered as i64,
                now_ms() as i64,
                content_hash,
                profile_user_id
            ],
        )
        .std_context("set shared file offered")?;
        Ok(())
    }

    /// Increment the revision counter for a shared file.
    /// Delegates to [`bump_manifest_revision`].
    pub fn increment_shared_file_revision(
        &self,
        content_hash: &str,
        profile_user_id: &str,
    ) -> Result<u64> {
        let manifest_hash = format!("file-revision:{}:{}", content_hash, now_ms());
        self.bump_manifest_revision(profile_user_id, &manifest_hash)
    }

    /// List imported file objects that have no shared_files or message_attachments.
    pub fn list_unreferenced_imported_objects(&self, _prefix: &str) -> Result<Vec<FileObject>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT content_hash, size, mime_type, filename, created_at_ms, data
                 FROM file_objects
                 WHERE content_hash NOT IN (SELECT DISTINCT content_hash FROM shared_files)
                   AND content_hash NOT IN (SELECT DISTINCT content_hash FROM message_attachments)",
                )
                .std_context("prepare list unreferenced imported objects")?;
            let rows = stmt
                .query_map(rusqlite::params![], |row| {
                    let data_blob: Option<Vec<u8>> = row.get(5)?;
                    let size: i64 = row.get(1)?;
                    let created_at: i64 = row.get(4)?;
                    Ok(FileObject {
                        content_hash: row.get(0)?,
                        size: size as u64,
                        mime_type: row.get(2)?,
                        filename: row.get(3)?,
                        created_at_ms: created_at as u64,
                        data: data_blob,
                    })
                })
                .std_context("query list unreferenced imported objects")?;
            let mut result = Vec::new();
            for row in rows {
                result.push(row.std_context("read unreferenced imported object")?);
            }
            Ok(result)
        })
    }

    /// Create a cleanup operation record.
    /// Legacy stub — cleanup operations were consolidated in the SQLite redesign.
    /// Returns a placeholder operation ID (0).
    pub fn create_cleanup_operation(&self, _content_hash: &str) -> Result<i64> {
        Ok(0)
    }

    /// Update a cleanup operation's status.
    /// Legacy no-op — cleanup operations were consolidated in the SQLite redesign.
    pub fn update_cleanup_operation(
        &self,
        _op_id: i64,
        _status: &str,
        _bytes_freed: u64,
        _error: Option<&str>,
    ) -> Result<()> {
        Ok(())
    }

    /// Mark all incomplete cleanup operations as failed.
    /// Legacy stub — returns 0 as no ops exist in the new schema.
    pub fn fail_all_incomplete_operations(&self, _reason: &str) -> Result<usize> {
        Ok(0)
    }

    /// List shared_file content hashes that reference non-existent file_objects.
    /// With foreign key constraints, this should always return empty.
    pub fn list_orphaned_shared_file_hashes(&self, _profile_user_id: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    /// List file_objects that are not referenced by any shared_file or message_attachment.
    pub fn list_orphaned_file_objects(&self) -> Result<Vec<FileObject>> {
        self.list_unreferenced_imported_objects("")
    }

    /// List shared files that need verification.
    /// Returns empty — verification tracking was simplified in the redesign.
    pub fn list_files_needing_verification(
        &self,
        _profile_user_id: &str,
    ) -> Result<Vec<SharedFileRow>> {
        Ok(Vec::new())
    }

    /// Check whether a file object has references (shared_files or message_attachments).
    pub fn file_object_has_references(&self, content_hash: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let shared_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM shared_files WHERE content_hash = ?1",
                    rusqlite::params![content_hash],
                    |row| row.get(0),
                )
                .std_context("count shared file references")?;
            if shared_count > 0 {
                return Ok(true);
            }
            let msg_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM message_attachments WHERE content_hash = ?1",
                    rusqlite::params![content_hash],
                    |row| row.get(0),
                )
                .std_context("count message attachment references")?;
            Ok(msg_count > 0)
        })
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn fixed_bytes<const N: usize>(bytes: &[u8], field: &str) -> Result<[u8; N]> {
    bytes.try_into().map_err(|_| {
        anyhow!(
            "malformed persisted {field}: expected {N} bytes, got {}",
            bytes.len()
        )
        .into()
    })
}

fn row_to_envelope_bare(msg_id: &MessageId, row: &rusqlite::Row) -> Result<StoredEnvelope> {
    let conv_blob: Vec<u8> = row.get(1).std_context("get conversation_id")?;
    let conversation_id = fixed_bytes::<32>(&conv_blob, "conversation_id")?;

    let author_user_blob: Vec<u8> = row.get(2).std_context("get author_user_id")?;
    let author_user_id = iroh::PublicKey::try_from(author_user_blob.as_slice())
        .map_err(|e| anyhow!("invalid public key: {}", e))?;

    let author_device_blob: Vec<u8> = row.get(3).std_context("get author_device_id")?;
    let author_device_id = iroh::PublicKey::try_from(author_device_blob.as_slice())
        .map_err(|e| anyhow!("invalid public key: {}", e))?;

    let created_at_ms: i64 = row.get(4).std_context("get created_at_ms")?;
    let expires_at_ms: i64 = row.get(5).std_context("get expires_at_ms")?;
    let ciphertext_blob: Vec<u8> = row.get(6).std_context("get ciphertext")?;
    let ciphertext = bytes::Bytes::from(ciphertext_blob);
    let signature_blob: Vec<u8> = row.get(7).std_context("get signature")?;
    let signature = fixed_bytes::<64>(&signature_blob, "signature")?;
    let acked_at_ms: Option<i64> = row.get(8).std_context("get acked_at_ms")?;

    Ok(StoredEnvelope {
        msg_id: *msg_id,
        conversation_id,
        author_user_id,
        author_device_id,
        created_at_ms: created_at_ms as u64,
        expires_at_ms: expires_at_ms as u64,
        ciphertext,
        signature,
        acked_at_ms: acked_at_ms.map(|v| v as u64),
    })
}

fn row_to_envelope(msg_id: &MessageId, row: &rusqlite::Row) -> Result<StoredEnvelope> {
    // Row indices: 0=conversation_id, 1=author_user_id, ...
    let conv_blob: Vec<u8> = row.get(0).std_context("get conversation_id")?;
    let conversation_id = fixed_bytes::<32>(&conv_blob, "conversation_id")?;

    let author_user_blob: Vec<u8> = row.get(1).std_context("get author_user_id")?;
    let author_user_id = iroh::PublicKey::try_from(author_user_blob.as_slice())
        .map_err(|e| anyhow!("invalid public key: {}", e))?;

    let author_device_blob: Vec<u8> = row.get(2).std_context("get author_device_id")?;
    let author_device_id = iroh::PublicKey::try_from(author_device_blob.as_slice())
        .map_err(|e| anyhow!("invalid public key: {}", e))?;

    let created_at_ms: i64 = row.get(3).std_context("get created_at_ms")?;
    let expires_at_ms: i64 = row.get(4).std_context("get expires_at_ms")?;
    let ciphertext_blob: Vec<u8> = row.get(5).std_context("get ciphertext")?;
    let ciphertext = bytes::Bytes::from(ciphertext_blob);
    let signature_blob: Vec<u8> = row.get(6).std_context("get signature")?;
    let signature = fixed_bytes::<64>(&signature_blob, "signature")?;
    let acked_at_ms: Option<i64> = row.get(7).std_context("get acked_at_ms")?;

    Ok(StoredEnvelope {
        msg_id: *msg_id,
        conversation_id,
        author_user_id,
        author_device_id,
        created_at_ms: created_at_ms as u64,
        expires_at_ms: expires_at_ms as u64,
        ciphertext,
        signature,
        acked_at_ms: acked_at_ms.map(|v| v as u64),
    })
}

fn row_to_outbox(row: &rusqlite::Row) -> Result<OutboxRow> {
    let msg_blob: Vec<u8> = row.get(0).std_context("get msg_id")?;
    let msg_id = fixed_bytes::<32>(&msg_blob, "msg_id")?;

    let recipient_blob: Vec<u8> = row.get(1).std_context("get recipient")?;
    let recipient_device_id = iroh::PublicKey::try_from(recipient_blob.as_slice())
        .map_err(|e| anyhow!("invalid public key: {}", e))?;

    let status_code: u8 = row.get(2).std_context("get status")?;
    let status = DeliveryStatus::try_from(status_code)?;

    Ok(OutboxRow {
        msg_id,
        recipient_device_id,
        status,
        attempts: row.get(3).std_context("get attempts")?,
        next_attempt_at_ms: row.get::<_, i64>(4).std_context("get next_attempt")? as u64,
        last_error_code: row.get(5).std_context("get error_code")?,
        last_attempt_at_ms: row
            .get::<_, Option<i64>>(6)
            .std_context("get last_attempt")?
            .map(|v| v as u64),
    })
}

fn row_to_download(row: &rusqlite::Row) -> Result<Download> {
    let state_str: String = row.get(5).std_context("get state")?;
    let state: DownloadState = state_str
        .parse()
        .map_err(|e: UnknownDownloadState| anyhow!("{e}"))?;
    Ok(Download {
        id: row.get(0).std_context("get id")?,
        content_hash: row.get(1).std_context("get hash")?,
        remote_peer: row.get(2).std_context("get peer")?,
        remote_shared_file_id: row.get(3).std_context("get remote_shared_file_id")?,
        destination_path: PathBuf::from(
            row.get::<_, String>(4)
                .std_context("get destination_path")?,
        ),
        state,
        bytes_downloaded: row.get::<_, i64>(6).std_context("get bytes_down")? as u64,
        expected_size: row.get::<_, i64>(7).std_context("get total_bytes")? as u64,
        created_at_ms: row.get::<_, i64>(8).std_context("get created")? as u64,
        updated_at_ms: row.get::<_, i64>(9).std_context("get updated")? as u64,
        last_error: row.get(10).std_context("get error")?,
        retry_count: row.get::<_, i64>(11).std_context("get retries")? as u32,
        next_retry_at_ms: row
            .get::<_, Option<i64>>(12)
            .std_context("get next_retry")?
            .map(|v| v as u64),
    })
}

trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalExt<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e).std_context("query"),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── V1 message tables ──────────────────────────────────────────

    fn random_public_key() -> iroh::PublicKey {
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        iroh::PublicKey::from_bytes(&bytes).unwrap()
    }

    #[test]
    fn v1_inbox_idempotent_insert() {
        let storage = Storage::memory().unwrap();
        let msg_id = [1u8; 32];
        let env = StoredEnvelope {
            msg_id,
            conversation_id: [2u8; 32],
            author_user_id: random_public_key(),
            author_device_id: random_public_key(),
            created_at_ms: 1000,
            expires_at_ms: 5000,
            ciphertext: bytes::Bytes::from(vec![1, 2, 3]),
            signature: [3u8; 64],
            acked_at_ms: None,
        };
        storage.insert_inbox(&env).unwrap();
        storage.insert_inbox(&env).unwrap(); // idempotent
        let fetched = storage.get_inbox(&msg_id).unwrap().unwrap();
        assert_eq!(fetched.msg_id, env.msg_id);
    }

    #[test]
    fn v1_outbox_flow() {
        let storage = Storage::memory().unwrap();
        let msg_id = [1u8; 32];
        let recipient = random_public_key();
        storage.enqueue_outbox(&msg_id, recipient, 1000).unwrap();
        let due = storage.fetch_due_outbox(500).unwrap();
        assert!(due.is_empty());
        let due = storage.fetch_due_outbox(1500).unwrap();
        assert_eq!(due.len(), 1);
        storage
            .record_attempt(&msg_id, recipient, 3000, Some("timeout"))
            .unwrap();
        let due = storage.fetch_due_outbox(1500).unwrap();
        assert!(due.is_empty());
        storage.mark_acked(&msg_id, recipient).unwrap();
    }

    #[test]
    fn v1_contacts_crud() {
        let storage = Storage::memory().unwrap();
        let user = random_public_key();
        let device = random_public_key();
        storage
            .upsert_contact(&user, &device, None, b"key-data", 1000, 5000)
            .unwrap();
        let contacts = storage.list_contacts().unwrap();
        assert_eq!(contacts.len(), 1);
    }

    #[test]
    fn v1_sync_cursor_crud() {
        let storage = Storage::memory().unwrap();
        let peer = random_public_key();
        storage
            .upsert_sync_cursor(&peer, Some(b"clock-data"), 2000)
            .unwrap();
        let cursors = storage.list_sync_cursors().unwrap();
        assert_eq!(cursors.len(), 1);
    }

    // ── V2 file-object tables ──────────────────────────────────────

    #[test]
    fn v2_file_object_round_trip() {
        let storage = Storage::memory().unwrap();
        storage
            .put_file_object("abc123", 500, "text/plain", "readme.txt", b"hello world")
            .unwrap();
        assert!(storage.file_object_exists("abc123").unwrap());
        let obj = storage.get_file_object("abc123").unwrap().unwrap();
        assert_eq!(obj.size, 500);
        assert_eq!(obj.filename, "readme.txt");
        assert_eq!(obj.data.as_deref(), Some(&b"hello world"[..]));
    }

    #[test]
    fn v2_message_attachments() {
        let storage = Storage::memory().unwrap();
        storage
            .put_file_object("hash1", 100, "image/png", "photo.png", b"binary")
            .unwrap();
        let att_id = storage
            .attach_file_to_message(42, "hash1", "photo.png", 0)
            .unwrap();
        assert!(att_id > 0);
        let attachments = storage.get_message_attachments(42).unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].content_hash, "hash1");

        // Find messages referencing a file.
        let msg_ids = storage.find_messages_for_file("hash1").unwrap();
        assert_eq!(msg_ids, vec![42]);

        // Remove.
        assert!(storage.remove_message_attachment(att_id).unwrap());
        assert!(storage.get_message_attachments(42).unwrap().is_empty());
    }

    #[test]
    fn v2_shared_files_and_collections() {
        let storage = Storage::memory().unwrap();
        storage
            .put_file_object("hash2", 200, "application/pdf", "doc.pdf", b"pdf-data")
            .unwrap();
        storage
            .upsert_shared_file(
                "hash2",
                "alice_key",
                "meta-1",
                "doc.pdf",
                Some("My document"),
                true,
            )
            .unwrap();
        let files = storage.list_shared_files("alice_key", true).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].display_filename, "doc.pdf");

        // Collections.
        let coll_id = storage
            .ensure_collection("alice_key", "docs", Some("My docs"))
            .unwrap();
        storage.add_to_collection(coll_id, "hash2", 0).unwrap();
        let items = storage.list_collection_items(coll_id).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content_hash, "hash2");
    }

    #[test]
    fn v2_permissions() {
        let storage = Storage::memory().unwrap();
        storage
            .put_file_object("hash3", 50, "text/plain", "note.txt", b"note")
            .unwrap();
        storage
            .grant_permission("hash3", "alice", "bob", "read", None)
            .unwrap();
        assert!(storage.check_permission("hash3", "bob", "read").unwrap());
        assert!(
            !storage
                .check_permission("hash3", "bob", "download")
                .unwrap()
        );
        assert!(
            storage
                .revoke_permission("hash3", "alice", "bob", "read")
                .unwrap()
        );
        assert!(!storage.check_permission("hash3", "bob", "read").unwrap());
    }

    #[test]
    fn v4_downloads_state_machine() {
        let storage = Storage::memory().unwrap();
        let dest = Path::new("/tmp/downloads/file.bin");
        // Insert a file_object first to satisfy the FK constraint.
        storage
            .put_file_object("hash4", 1024, "application/octet-stream", "large.bin", b"")
            .unwrap();
        let id = storage
            .create_download("hash4", "bob_peer", "remote_file_1", dest, 1024)
            .unwrap();
        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(dl.state, DownloadState::Queued);
        assert_eq!(dl.remote_shared_file_id, "remote_file_1");
        assert_eq!(dl.destination_path, dest);
        assert_eq!(dl.expected_size, 1024);
        assert_eq!(dl.retry_count, 0);

        // Queued → ResolvingPeer
        storage
            .transition_download(id, DownloadState::ResolvingPeer)
            .unwrap();
        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(dl.state, DownloadState::ResolvingPeer);

        // ResolvingPeer → RequestingPermission
        storage
            .update_download_progress(id, 0, DownloadState::RequestingPermission)
            .unwrap();
        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(dl.state, DownloadState::RequestingPermission);

        // RequestingPermission → Downloading (with progress)
        storage
            .update_download_progress(id, 512, DownloadState::Downloading)
            .unwrap();
        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(dl.state, DownloadState::Downloading);
        assert_eq!(dl.bytes_downloaded, 512);

        // Downloading → Verifying
        storage
            .transition_download(id, DownloadState::Verifying)
            .unwrap();
        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(dl.state, DownloadState::Verifying);

        // Verifying → Complete
        storage
            .transition_download(id, DownloadState::Complete)
            .unwrap();
        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(dl.state, DownloadState::Complete);

        // Complete is terminal — verify the full record
        assert!(dl.is_terminal());
    }

    #[test]
    fn v4_download_pause_resume_cancel() {
        let storage = Storage::memory().unwrap();
        let dest = Path::new("/tmp/downloads/pause-test.bin");
        storage
            .put_file_object("hash5", 512, "text/plain", "test.txt", b"")
            .unwrap();
        let id = storage
            .create_download("hash5", "alice", "remote_2", dest, 512)
            .unwrap();

        // Queued → ResolvingPeer → RequestingPermission → Downloading
        storage
            .transition_download(id, DownloadState::ResolvingPeer)
            .unwrap();
        storage
            .transition_download(id, DownloadState::RequestingPermission)
            .unwrap();
        storage
            .transition_download(id, DownloadState::Downloading)
            .unwrap();

        // Pause from Downloading
        storage.pause_download(id).unwrap();
        assert_eq!(
            storage.get_download(id).unwrap().unwrap().state,
            DownloadState::Paused
        );

        // Resume from Paused
        storage.resume_download(id).unwrap();
        assert_eq!(
            storage.get_download(id).unwrap().unwrap().state,
            DownloadState::Downloading
        );

        // Cancel from Downloading
        storage.cancel_download(id).unwrap();
        assert_eq!(
            storage.get_download(id).unwrap().unwrap().state,
            DownloadState::Cancelled
        );

        // Terminal state — further transitions should fail
        let err = storage.transition_download(id, DownloadState::Queued);
        assert!(err.is_err());
    }

    #[test]
    fn v4_download_fail_and_retry() {
        let storage = Storage::memory().unwrap();
        let dest = Path::new("/tmp/downloads/fail-retry.bin");
        storage
            .put_file_object("hash6", 256, "image/png", "photo.png", b"")
            .unwrap();
        let id = storage
            .create_download("hash6", "charlie", "remote_3", dest, 256)
            .unwrap();

        // Drive to Downloading then fail
        storage
            .transition_download(id, DownloadState::ResolvingPeer)
            .unwrap();
        storage
            .transition_download(id, DownloadState::RequestingPermission)
            .unwrap();
        storage
            .transition_download(id, DownloadState::Downloading)
            .unwrap();

        storage
            .fail_download(id, "connection timeout", Some(5000))
            .unwrap();
        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(dl.state, DownloadState::Failed);
        assert_eq!(dl.last_error.as_deref(), Some("connection timeout"));
        assert_eq!(dl.retry_count, 1);

        // Retry: Failed → Queued
        storage.retry_download(id).unwrap();
        assert_eq!(
            storage.get_download(id).unwrap().unwrap().state,
            DownloadState::Queued
        );
    }

    #[test]
    fn v4_download_invalid_transitions_rejected() {
        let storage = Storage::memory().unwrap();
        let dest = Path::new("/tmp/downloads/invalid.bin");
        storage
            .put_file_object("hash7", 128, "text/plain", "doc.txt", b"")
            .unwrap();
        let id = storage
            .create_download("hash7", "dave", "remote_4", dest, 128)
            .unwrap();

        // Queued → Complete is invalid
        let err = storage.transition_download(id, DownloadState::Complete);
        assert!(err.is_err());
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("invalid download state transition")
        );

        // Queued → ResolvingPeer → Complete is also invalid
        storage
            .transition_download(id, DownloadState::ResolvingPeer)
            .unwrap();
        let err = storage.transition_download(id, DownloadState::Complete);
        assert!(err.is_err());

        // Queued → ResolvingPeer → Cancelled is OK
        storage
            .transition_download(id, DownloadState::Cancelled)
            .unwrap();
        // Terminal — any further transition is rejected
        let err = storage.transition_download(id, DownloadState::Queued);
        assert!(err.is_err());
    }

    #[test]
    fn v4_download_list_and_count() {
        let storage = Storage::memory().unwrap();
        let dest = Path::new("/tmp/downloads/list.bin");
        storage
            .put_file_object("hash8", 100, "text/plain", "a.txt", b"")
            .unwrap();
        storage
            .put_file_object("hash9", 200, "text/plain", "b.txt", b"")
            .unwrap();

        let id1 = storage
            .create_download("hash8", "eve", "r1", dest, 100)
            .unwrap();
        let id2 = storage
            .create_download("hash9", "frank", "r2", dest, 200)
            .unwrap();

        // Both queued
        assert_eq!(storage.list_downloads().unwrap().len(), 2);
        assert_eq!(
            storage
                .count_downloads_by_state(DownloadState::Queued)
                .unwrap(),
            2
        );

        // Transition one
        storage
            .transition_download(id1, DownloadState::ResolvingPeer)
            .unwrap();
        assert_eq!(
            storage
                .count_downloads_by_state(DownloadState::Queued)
                .unwrap(),
            1
        );
        assert_eq!(
            storage
                .count_downloads_by_state(DownloadState::ResolvingPeer)
                .unwrap(),
            1
        );

        // List by state
        let queued = storage
            .list_downloads_by_state(DownloadState::Queued)
            .unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, id2);

        // Delete terminal
        storage
            .transition_download(id1, DownloadState::Cancelled)
            .unwrap();
        storage.delete_download(id1).unwrap();
        assert_eq!(storage.list_downloads().unwrap().len(), 1);
    }

    // ── Restart recovery tests ────────────────────────────────────

    #[test]
    fn recover_downloading_becomes_paused() {
        let storage = Storage::memory().unwrap();
        let dest = Path::new("/tmp/dl/rec-dl.bin");
        storage
            .put_file_object("rec-hash-dl", 100, "text/plain", "dl.bin", b"")
            .unwrap();
        let id = storage
            .create_download("rec-hash-dl", "alice", "r_dl", dest, 100)
            .unwrap();

        // Drive to Downloading
        storage
            .transition_download(id, DownloadState::ResolvingPeer)
            .unwrap();
        storage
            .transition_download(id, DownloadState::RequestingPermission)
            .unwrap();
        storage
            .transition_download(id, DownloadState::Downloading)
            .unwrap();
        assert_eq!(
            storage.get_download(id).unwrap().unwrap().state,
            DownloadState::Downloading
        );

        // Recover
        let recovered = storage.recover_download_crash_state().unwrap();
        assert_eq!(recovered, 1);

        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(
            dl.state,
            DownloadState::Paused,
            "Downloading should become Paused"
        );
    }

    #[test]
    fn recover_resolving_peer_becomes_queued() {
        let storage = Storage::memory().unwrap();
        let dest = Path::new("/tmp/dl/rec-rp.bin");
        storage
            .put_file_object("rec-hash-rp", 200, "text/plain", "rp.bin", b"")
            .unwrap();
        let id = storage
            .create_download("rec-hash-rp", "bob", "r_rp", dest, 200)
            .unwrap();

        // Drive to ResolvingPeer
        storage
            .transition_download(id, DownloadState::ResolvingPeer)
            .unwrap();
        assert_eq!(
            storage.get_download(id).unwrap().unwrap().state,
            DownloadState::ResolvingPeer
        );

        // Recover
        let recovered = storage.recover_download_crash_state().unwrap();
        assert_eq!(recovered, 1);

        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(
            dl.state,
            DownloadState::Queued,
            "ResolvingPeer should become Queued"
        );
        assert_eq!(dl.retry_count, 0, "retry_count should be reset");
        assert_eq!(dl.bytes_downloaded, 0, "bytes_downloaded should be reset");
    }

    #[test]
    fn recover_requesting_permission_becomes_queued() {
        let storage = Storage::memory().unwrap();
        let dest = Path::new("/tmp/dl/rec-perm.bin");
        storage
            .put_file_object("rec-hash-perm", 300, "text/plain", "perm.bin", b"")
            .unwrap();
        let id = storage
            .create_download("rec-hash-perm", "carol", "r_perm", dest, 300)
            .unwrap();

        // Drive to RequestingPermission
        storage
            .transition_download(id, DownloadState::ResolvingPeer)
            .unwrap();
        storage
            .transition_download(id, DownloadState::RequestingPermission)
            .unwrap();
        assert_eq!(
            storage.get_download(id).unwrap().unwrap().state,
            DownloadState::RequestingPermission
        );

        // Recover
        let recovered = storage.recover_download_crash_state().unwrap();
        assert_eq!(recovered, 1);

        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(
            dl.state,
            DownloadState::Queued,
            "RequestingPermission should become Queued"
        );
    }

    #[test]
    fn recover_verifying_becomes_failed() {
        let storage = Storage::memory().unwrap();
        let dest = Path::new("/tmp/dl/rec-ver.bin");
        storage
            .put_file_object("rec-hash-ver", 400, "text/plain", "ver.bin", b"")
            .unwrap();
        let id = storage
            .create_download("rec-hash-ver", "dave", "r_ver", dest, 400)
            .unwrap();

        // Drive to Verifying
        storage
            .transition_download(id, DownloadState::ResolvingPeer)
            .unwrap();
        storage
            .transition_download(id, DownloadState::RequestingPermission)
            .unwrap();
        storage
            .transition_download(id, DownloadState::Downloading)
            .unwrap();
        storage
            .transition_download(id, DownloadState::Verifying)
            .unwrap();
        assert_eq!(
            storage.get_download(id).unwrap().unwrap().state,
            DownloadState::Verifying
        );

        // Recover
        let recovered = storage.recover_download_crash_state().unwrap();
        assert_eq!(recovered, 1);

        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(
            dl.state,
            DownloadState::Failed,
            "Verifying should become Failed"
        );
        assert!(
            dl.last_error
                .as_deref()
                .unwrap_or("")
                .contains("crash interrupted verification")
        );
    }

    #[test]
    fn recover_does_not_touch_terminal_states() {
        let storage = Storage::memory().unwrap();
        let dest = Path::new("/tmp/dl/rec-term.bin");
        storage
            .put_file_object("rec-hash-term", 500, "text/plain", "term.bin", b"")
            .unwrap();
        // Create complete download
        let id_c = storage
            .create_download("rec-hash-term", "eve", "r_term", dest, 500)
            .unwrap();
        storage
            .transition_download(id_c, DownloadState::ResolvingPeer)
            .unwrap();
        storage
            .transition_download(id_c, DownloadState::RequestingPermission)
            .unwrap();
        storage
            .transition_download(id_c, DownloadState::Downloading)
            .unwrap();
        storage
            .transition_download(id_c, DownloadState::Verifying)
            .unwrap();
        storage
            .transition_download(id_c, DownloadState::Complete)
            .unwrap();

        // Create cancelled download
        let id_x = storage
            .create_download("rec-hash-term", "frank", "r_term2", dest, 500)
            .unwrap();
        storage
            .transition_download(id_x, DownloadState::Cancelled)
            .unwrap();

        // Recover — should not change them
        let recovered = storage.recover_download_crash_state().unwrap();
        assert_eq!(recovered, 0, "terminal states should not be recovered");

        assert_eq!(
            storage.get_download(id_c).unwrap().unwrap().state,
            DownloadState::Complete
        );
        assert_eq!(
            storage.get_download(id_x).unwrap().unwrap().state,
            DownloadState::Cancelled
        );
    }

    #[test]
    fn recover_leaves_paused_queued_failed_unchanged() {
        let storage = Storage::memory().unwrap();
        let dest = Path::new("/tmp/dl/rec-safe.bin");
        storage
            .put_file_object("rec-hash-safe", 600, "text/plain", "safe.bin", b"")
            .unwrap();

        // Paused
        let id_p = storage
            .create_download("rec-hash-safe", "grace", "r_p", dest, 600)
            .unwrap();
        storage
            .transition_download(id_p, DownloadState::ResolvingPeer)
            .unwrap();
        storage
            .transition_download(id_p, DownloadState::RequestingPermission)
            .unwrap();
        storage
            .transition_download(id_p, DownloadState::Downloading)
            .unwrap();
        storage.pause_download(id_p).unwrap();

        // Queued
        let id_q = storage
            .create_download("rec-hash-safe", "heidi", "r_q", dest, 600)
            .unwrap();

        // Failed
        let id_f = storage
            .create_download("rec-hash-safe", "ivan", "r_f", dest, 600)
            .unwrap();
        storage
            .transition_download(id_f, DownloadState::ResolvingPeer)
            .unwrap();
        storage
            .fail_download(id_f, "original error", Some(9999))
            .unwrap();

        // Recover — none of these should change
        let recovered = storage.recover_download_crash_state().unwrap();
        assert_eq!(recovered, 0, "safe states should not be recovered");

        assert_eq!(
            storage.get_download(id_p).unwrap().unwrap().state,
            DownloadState::Paused
        );
        assert_eq!(
            storage.get_download(id_q).unwrap().unwrap().state,
            DownloadState::Queued
        );
        let dl_f = storage.get_download(id_f).unwrap().unwrap();
        assert_eq!(dl_f.state, DownloadState::Failed);
        assert_eq!(dl_f.last_error.as_deref(), Some("original error"));
    }

    #[test]
    fn recover_multiple_active_downloads_bounded() {
        let storage = Storage::memory().unwrap();
        let dest = Path::new("/tmp/dl/rec-multi.bin");
        storage
            .put_file_object("rec-hash-multi", 700, "text/plain", "multi.bin", b"")
            .unwrap();

        // Create multiple downloads in Downloading state
        let mut ids = Vec::new();
        for i in 0..5 {
            let id = storage
                .create_download(
                    "rec-hash-multi",
                    &format!("peer_{i}"),
                    &format!("r_{i}"),
                    dest,
                    700,
                )
                .unwrap();
            storage
                .transition_download(id, DownloadState::ResolvingPeer)
                .unwrap();
            storage
                .transition_download(id, DownloadState::RequestingPermission)
                .unwrap();
            storage
                .transition_download(id, DownloadState::Downloading)
                .unwrap();
            ids.push(id);
        }

        // Recover — all should become Paused
        let recovered = storage.recover_download_crash_state().unwrap();
        assert_eq!(recovered, 5, "all 5 should be recovered");

        for id in &ids {
            assert_eq!(
                storage.get_download(*id).unwrap().unwrap().state,
                DownloadState::Paused,
                "download {id} should be Paused after recovery"
            );
        }
    }

    #[test]
    fn recover_does_not_mark_incomplete_as_complete() {
        let storage = Storage::memory().unwrap();
        let dest = Path::new("/tmp/dl/rec-never.bin");
        storage
            .put_file_object("rec-hash-never", 800, "text/plain", "never.bin", b"")
            .unwrap();
        let id = storage
            .create_download("rec-hash-never", "jack", "r_never", dest, 800)
            .unwrap();

        // Never advance past Queued, then recover
        let recovered = storage.recover_download_crash_state().unwrap();
        assert_eq!(recovered, 0, "Queued should not be recovered");

        let dl = storage.get_download(id).unwrap().unwrap();
        assert_ne!(
            dl.state,
            DownloadState::Complete,
            "must never mark incomplete as complete"
        );
        assert_eq!(dl.state, DownloadState::Queued);
    }

    #[test]
    fn recover_during_permission_request_becomes_queued() {
        let storage = Storage::memory().unwrap();
        let dest = Path::new("/tmp/dl/rec-perm2.bin");
        storage
            .put_file_object("rec-hash-perm2", 900, "text/plain", "perm2.bin", b"")
            .unwrap();

        // Create download and simulate crash during permission request
        // by directly inserting into the state (bypassing transition checks).
        let now = crate::chat_core::now_ms() as i64;
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO downloads (content_hash, remote_peer, remote_shared_file_id,
                        destination_path, state, bytes_downloaded, total_bytes,
                        created_at_ms, updated_at_ms)
                 VALUES ('rec-hash-perm2', 'kate', 'r_perm2', '/tmp/dl/perm2.bin',
                         'requesting_permission', 0, 900, ?1, ?1)",
                rusqlite::params![now],
            )
            .unwrap();
        }

        let recovered = storage.recover_download_crash_state().unwrap();
        assert_eq!(recovered, 1);

        // Find the download and verify it was queued
        let downloads = storage
            .list_downloads_by_state(DownloadState::Queued)
            .unwrap();
        assert!(
            !downloads.is_empty(),
            "should have at least one queued download"
        );
        assert_eq!(downloads[0].state, DownloadState::Queued);
        assert_eq!(downloads[0].retry_count, 0);
    }

    #[test]
    fn recover_crash_during_db_completion_update() {
        // Simulate a crash after Verifying→Complete transition started but
        // before the DB commit: the state is still Verifying in the DB.
        let storage = Storage::memory().unwrap();
        let dest = Path::new("/tmp/dl/rec-crash-db.bin");
        storage
            .put_file_object("rec-hash-crash-db", 1000, "text/plain", "crash-db.bin", b"")
            .unwrap();
        let id = storage
            .create_download("rec-hash-crash-db", "lee", "r_crash_db", dest, 1000)
            .unwrap();

        // Drive to Verifying
        storage
            .transition_download(id, DownloadState::ResolvingPeer)
            .unwrap();
        storage
            .transition_download(id, DownloadState::RequestingPermission)
            .unwrap();
        storage
            .transition_download(id, DownloadState::Downloading)
            .unwrap();
        storage
            .transition_download(id, DownloadState::Verifying)
            .unwrap();

        // Crash happens here — DB still shows Verifying

        // Recover — should mark as Failed, not Complete
        let recovered = storage.recover_download_crash_state().unwrap();
        assert_eq!(recovered, 1);

        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(
            dl.state,
            DownloadState::Failed,
            "crash during DB completion update should go to Failed"
        );
        assert_ne!(
            dl.state,
            DownloadState::Complete,
            "must never be marked Complete"
        );
    }

    #[test]
    fn flag_version_mismatch_from_requesting_permission() {
        let storage = Storage::memory().unwrap();
        storage
            .put_file_object("vm-hash-1", 500, "text/plain", "vm-test.bin", b"")
            .unwrap();
        let id = storage
            .create_download(
                "vm-hash-1",
                "alice",
                "remote_vm_1",
                Path::new("/tmp/vm-test.bin"),
                500,
            )
            .unwrap();

        // Drive to RequestingPermission
        storage
            .transition_download(id, DownloadState::ResolvingPeer)
            .unwrap();
        storage
            .transition_download(id, DownloadState::RequestingPermission)
            .unwrap();

        // Flag version mismatch
        storage
            .flag_version_mismatch(id, "file content hash changed")
            .unwrap();

        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(dl.state, DownloadState::VersionMismatch);
        assert_eq!(dl.last_error.as_deref(), Some("file content hash changed"));
        assert!(!dl.is_terminal(), "VersionMismatch should not be terminal");
    }

    #[test]
    fn flag_version_mismatch_from_downloading() {
        let storage = Storage::memory().unwrap();
        storage
            .put_file_object("vm-hash-2", 800, "text/plain", "vm-dl.bin", b"")
            .unwrap();
        let id = storage
            .create_download(
                "vm-hash-2",
                "bob",
                "remote_vm_2",
                Path::new("/tmp/vm-dl.bin"),
                800,
            )
            .unwrap();

        // Drive to Downloading
        storage
            .transition_download(id, DownloadState::ResolvingPeer)
            .unwrap();
        storage
            .transition_download(id, DownloadState::RequestingPermission)
            .unwrap();
        storage
            .transition_download(id, DownloadState::Downloading)
            .unwrap();

        // Flag version mismatch from Downloading
        storage
            .flag_version_mismatch(id, "file updated while downloading")
            .unwrap();

        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(dl.state, DownloadState::VersionMismatch);
    }

    #[test]
    fn flag_version_mismatch_invalid_transition() {
        let storage = Storage::memory().unwrap();
        storage
            .put_file_object("vm-hash-3", 300, "text/plain", "vm-inv.bin", b"")
            .unwrap();
        let id = storage
            .create_download(
                "vm-hash-3",
                "carol",
                "remote_vm_3",
                Path::new("/tmp/vm-inv.bin"),
                300,
            )
            .unwrap();

        // VersionMismatch from Queued is invalid
        let err = storage.flag_version_mismatch(id, "invalid");
        assert!(err.is_err(), "Queued → VersionMismatch should be invalid");
    }

    #[test]
    fn version_mismatch_then_retry_to_queued() {
        let storage = Storage::memory().unwrap();
        storage
            .put_file_object("vm-hash-4", 600, "text/plain", "vm-retry.bin", b"")
            .unwrap();
        let id = storage
            .create_download(
                "vm-hash-4",
                "dave",
                "remote_vm_4",
                Path::new("/tmp/vm-retry.bin"),
                600,
            )
            .unwrap();

        // Drive to RequestingPermission then flag mismatch
        storage
            .transition_download(id, DownloadState::ResolvingPeer)
            .unwrap();
        storage
            .transition_download(id, DownloadState::RequestingPermission)
            .unwrap();
        storage
            .flag_version_mismatch(id, "content changed")
            .unwrap();

        // Retry from VersionMismatch should go back to Queued
        storage.retry_download(id).unwrap();
        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(dl.state, DownloadState::Queued);
    }

    #[test]
    fn version_mismatch_then_cancel() {
        let storage = Storage::memory().unwrap();
        storage
            .put_file_object("vm-hash-5", 900, "text/plain", "vm-cancel.bin", b"")
            .unwrap();
        let id = storage
            .create_download(
                "vm-hash-5",
                "eve",
                "remote_vm_5",
                Path::new("/tmp/vm-cancel.bin"),
                900,
            )
            .unwrap();

        // Drive to Downloading then flag mismatch
        storage
            .transition_download(id, DownloadState::ResolvingPeer)
            .unwrap();
        storage
            .transition_download(id, DownloadState::RequestingPermission)
            .unwrap();
        storage
            .transition_download(id, DownloadState::Downloading)
            .unwrap();
        storage
            .flag_version_mismatch(id, "changed during download")
            .unwrap();

        // Cancel from VersionMismatch
        storage.cancel_download(id).unwrap();
        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(dl.state, DownloadState::Cancelled);
        assert!(dl.is_terminal());
    }

    #[test]
    fn v2_profile_manifest_revision_increments() {
        let storage = Storage::memory().unwrap();
        let rev1 = storage
            .bump_manifest_revision("alice_key", "hash-a")
            .unwrap();
        assert_eq!(rev1, 1);
        let rev2 = storage
            .bump_manifest_revision("alice_key", "hash-b")
            .unwrap();
        assert_eq!(rev2, 2);
        let state = storage.get_manifest_state("alice_key").unwrap().unwrap();
        assert_eq!(state.revision, 2);
        assert_eq!(state.manifest_hash, "hash-b");
    }

    #[test]
    fn schema_version_is_recorded() {
        let storage = Storage::memory().unwrap();
        let version: u32 = storage
            .with_conn(|conn| {
                Ok(conn
                    .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                        row.get(0)
                    })
                    .map_err(|e| anyhow!("{}", e))?)
            })
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn imported_file_object() {
        let storage = Storage::memory().unwrap();
        storage
            .put_imported_file_object(
                "abc789",
                9999,
                "video/mp4",
                "movie.mp4",
                "blob-xyz-hash",
                "peer123",
            )
            .unwrap();
        let obj = storage.get_file_object("abc789").unwrap().unwrap();
        assert_eq!(obj.size, 9999);
        assert!(obj.data.is_none()); // imported files have no inline data
    }

    #[test]
    fn foreign_key_enforcement_prevents_orphan_attachment() {
        let storage = Storage::memory().unwrap();
        // Attaching to a non-existent file_object should fail.
        let result = storage.attach_file_to_message(1, "no-such-hash", "x.txt", 0);
        assert!(result.is_err());
    }

    // ── Crash and corruption resilience tests (Step 16) ────────────────

    #[test]
    fn test_unsupported_schema_version_returns_error() {
        // If the DB has a schema_version higher than CURRENT_SCHEMA_VERSION,
        // opening should return a clear error.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("boru.db");

        // Create a valid DB first
        {
            let storage = Storage::open(dir.path()).unwrap();
            // Verify current schema version
            let version: u32 = storage
                .with_conn(|conn| {
                    Ok(conn
                        .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                            row.get(0)
                        })
                        .map_err(|e| anyhow!("{e}"))?)
                })
                .unwrap();
            assert_eq!(version, CURRENT_SCHEMA_VERSION);
        }

        // Manually insert a higher version to simulate a future-schema DB
        {
            let conn = Connection::open(&db_path).unwrap();
            let future_version = CURRENT_SCHEMA_VERSION + 1;
            conn.execute(
                "INSERT INTO schema_version (version, applied_at_ms) VALUES (?1, ?2)",
                params![future_version, 9999999999i64],
            )
            .unwrap();
        }

        // Reopening should fail
        let result = Storage::open(dir.path());
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("schema version") || err.contains("newer version"),
            "expected version-mismatch error, got: {err}"
        );
    }

    #[test]
    fn test_integrity_check_fails_on_corrupt_db_storage() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("boru.db");

        // Create a valid DB
        {
            let _storage = Storage::open(dir.path()).unwrap();
        }

        // Corrupt it
        std::fs::write(&db_path, b"garbage data").unwrap();

        // Opening should fail
        let result = Storage::open(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_crash_left_sent_outbox_recovered_storage() {
        let dir = tempfile::tempdir().unwrap();
        let msg_id = [1u8; 32];
        let recipient = random_public_key();

        // First session: insert with Sent state
        {
            let storage = Storage::open(dir.path()).unwrap();
            let env = StoredEnvelope {
                msg_id,
                conversation_id: [2u8; 32],
                author_user_id: random_public_key(),
                author_device_id: random_public_key(),
                created_at_ms: 1000,
                expires_at_ms: 5000,
                ciphertext: bytes::Bytes::from(vec![1, 2, 3]),
                signature: [3u8; 64],
                acked_at_ms: None,
            };
            storage.insert_inbox(&env).unwrap();
            storage.enqueue_outbox(&msg_id, recipient, 1000).unwrap();
            storage
                .record_attempt(&msg_id, recipient, 2000, Some("in_flight"))
                .unwrap();
        }

        // Second session: crash recovery
        {
            let storage = Storage::open(dir.path()).unwrap();
            let due = storage.fetch_due_outbox(now_ms() + 1000).unwrap();
            let row = due.iter().find(|r| r.msg_id == msg_id);
            assert!(
                row.is_some(),
                "crash-left Sent outbox should be recovered to Pending"
            );
        }
    }

    #[test]
    fn test_stale_pending_timestamp_reset_storage() {
        let dir = tempfile::tempdir().unwrap();
        let msg_id = [1u8; 32];
        let recipient = random_public_key();
        let far_future = now_ms() + 86_400_000;

        // First session: insert pending row with future timestamp via raw SQL
        {
            let storage = Storage::open(dir.path()).unwrap();
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO outbox (msg_id, recipient_device_id, status, attempts, next_attempt_at_ms)
                 VALUES (?1, ?2, ?3, 0, ?4)",
                params![
                    msg_id.as_slice(),
                    recipient.as_bytes(),
                    crate::store::DeliveryStatus::Pending as u8,
                    far_future as i64,
                ],
            )
            .unwrap();
        }

        // Second session: timestamp should be reset
        {
            let storage = Storage::open(dir.path()).unwrap();
            let due = storage.fetch_due_outbox(now_ms() + 1000).unwrap();
            let row = due.iter().find(|r| r.msg_id == msg_id);
            assert!(
                row.is_some(),
                "stale pending timestamp should be recovered to due"
            );
        }
    }

    #[test]
    fn test_partial_migration_resumes_on_reopen() {
        // Verify that a partially-applied migration can resume on reopen.
        // The IF NOT EXISTS in each migration makes re-runs idempotent.
        let dir = tempfile::tempdir().unwrap();

        // Open and close — runs all migrations
        {
            let _s = Storage::open(dir.path()).unwrap();
        }

        // Insert a fake partial state: remove some v2 tables, keep v1.
        {
            let db_path = dir.path().join("boru.db");
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "PRAGMA foreign_keys = OFF;
                 DROP TABLE IF EXISTS file_objects;
                 DROP TABLE IF EXISTS message_attachments;
                 DROP TABLE IF EXISTS shared_files;
                 DROP TABLE IF EXISTS file_collections;
                 DROP TABLE IF EXISTS file_collection_items;
                 DROP TABLE IF EXISTS shared_file_permissions;
                 DROP TABLE IF EXISTS downloads;
                 DROP TABLE IF EXISTS profile_manifest_state;
                 DELETE FROM schema_version WHERE version = 2;",
            )
            .unwrap();
        }

        // Reopen — should re-apply v2 migration
        {
            let storage = Storage::open(dir.path()).unwrap();
            // Verify v2 tables exist again
            assert!(storage.file_object_exists("test").is_ok());
            // Schema version should be back to current
            let version: u32 = storage
                .with_conn(|conn| {
                    Ok(conn
                        .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                            row.get(0)
                        })
                        .map_err(|e| anyhow!("{e}"))?)
                })
                .unwrap();
            assert_eq!(version, CURRENT_SCHEMA_VERSION);
        }
    }

    // ── Remote catalogue cache (v3) ─────────────────────────────────

    #[test]
    fn test_v3_migration_creates_tables() {
        let storage = Storage::memory().unwrap();
        // Verify all three tables exist by querying them.
        storage
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM remote_catalogues", [], |_row| Ok(()))
                    .map_err(|e| anyhow!("remote_catalogues table missing: {e}"))?;
                conn.query_row(
                    "SELECT COUNT(*) FROM remote_shared_files",
                    [],
                    |_row| Ok(()),
                )
                .map_err(|e| anyhow!("remote_shared_files table missing: {e}"))?;
                conn.query_row("SELECT COUNT(*) FROM remote_collections", [], |_row| Ok(()))
                    .map_err(|e| anyhow!("remote_collections table missing: {e}"))?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn test_remote_catalogue_query_empty() {
        let storage = Storage::memory().unwrap();
        let owner = random_public_key();
        assert!(storage.get_remote_catalogue_meta(&owner).unwrap().is_none());
        assert!(storage.get_remote_shared_files(&owner).unwrap().is_empty());
        assert!(storage.get_remote_collections(&owner).unwrap().is_empty());
        assert!(storage.list_cached_remote_catalogues().unwrap().is_empty());
        assert!(!storage.delete_remote_catalogue(&owner).unwrap());
    }

    #[test]
    fn test_remote_catalogue_insert_query_delete_via_sql() {
        let storage = Storage::memory().unwrap();
        let owner = random_public_key();
        let owner_bytes = owner.as_bytes().to_vec();

        // Direct insert via raw SQL.
        storage
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO remote_catalogues \
                        (owner_id, revision, generated_at_ms, fetched_at_ms) \
                     VALUES (?1, 42, 1000, 2000)",
                    [&owner_bytes],
                )
                .map_err(|e| anyhow!("{e}"))?;
                conn.execute(
                    "INSERT INTO remote_shared_files \
                        (owner_id, content_hash, display_filename, description, \
                         size, mime_type, collection_name, file_revision, position) \
                     VALUES (?1, 'abc123', 'photo.jpg', 'A nice photo', \
                             1024, 'image/jpeg', 'Photos', 1, 0)",
                    [&owner_bytes],
                )
                .map_err(|e| anyhow!("{e}"))?;
                conn.execute(
                    "INSERT INTO remote_collections \
                        (owner_id, name, description, position) \
                     VALUES (?1, 'Photos', 'My photo collection', 0)",
                    [&owner_bytes],
                )
                .map_err(|e| anyhow!("{e}"))?;
                Ok(())
            })
            .unwrap();

        // Query via typed methods.
        let meta = storage.get_remote_catalogue_meta(&owner).unwrap().unwrap();
        assert_eq!(meta.revision, 42);
        assert_eq!(meta.generated_at_ms, 1000);
        assert_eq!(meta.fetched_at_ms, 2000);

        let files = storage.get_remote_shared_files(&owner).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content_hash, "abc123");
        assert_eq!(files[0].display_filename, "photo.jpg");
        assert_eq!(files[0].description.as_deref(), Some("A nice photo"));
        assert_eq!(files[0].size, 1024);
        assert_eq!(files[0].mime_type, "image/jpeg");
        assert_eq!(files[0].collection_name.as_deref(), Some("Photos"));
        assert_eq!(files[0].file_revision, 1);
        assert_eq!(files[0].position, 0);

        let collections = storage.get_remote_collections(&owner).unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].name, "Photos");
        assert_eq!(
            collections[0].description.as_deref(),
            Some("My photo collection")
        );
        assert_eq!(collections[0].position, 0);

        // Delete and verify cascade.
        assert!(storage.delete_remote_catalogue(&owner).unwrap());
        assert!(storage.get_remote_catalogue_meta(&owner).unwrap().is_none());
        assert!(storage.get_remote_shared_files(&owner).unwrap().is_empty());
        assert!(storage.get_remote_collections(&owner).unwrap().is_empty());
    }

    #[test]
    fn test_remote_catalogue_list_multiple() {
        let storage = Storage::memory().unwrap();

        // Generate distinct public keys.
        let owner1 = iroh::SecretKey::generate().public();
        let owner2 = iroh::SecretKey::generate().public();

        storage
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO remote_catalogues \
                        (owner_id, revision, generated_at_ms, fetched_at_ms) \
                     VALUES (?1, 1, 100, 200)",
                    [owner1.as_bytes()],
                )
                .map_err(|e| anyhow!("{e}"))?;
                conn.execute(
                    "INSERT INTO remote_catalogues \
                        (owner_id, revision, generated_at_ms, fetched_at_ms) \
                     VALUES (?1, 2, 300, 400)",
                    [owner2.as_bytes()],
                )
                .map_err(|e| anyhow!("{e}"))?;
                Ok(())
            })
            .unwrap();

        let list = storage.list_cached_remote_catalogues().unwrap();
        assert_eq!(list.len(), 2);
        // Ordered by fetched_at_ms DESC; owner2 (400) before owner1 (200).
        assert_eq!(list[0].owner_id, owner2);
        assert_eq!(list[0].revision, 2);
        assert_eq!(list[1].owner_id, owner1);
        assert_eq!(list[1].revision, 1);
    }

    #[cfg(feature = "net")]
    #[test]
    fn test_replace_remote_catalogue_basic() {
        use crate::catalogue_model::{
            FileCatalogueCollection, RemoteSharedFile, SignedFileCatalogue,
        };
        use serde_byte_array::ByteArray;

        let storage = Storage::memory().unwrap();
        let owner = random_public_key();

        let catalogue = SignedFileCatalogue {
            owner_id: owner,
            revision: 5,
            generated_at_ms: 1000,
            collections: vec![],
            files: vec![RemoteSharedFile::new(
                "hash1",
                "file1.txt",
                None,
                100,
                "text/plain",
                None,
                1,
            )],
            signature: ByteArray::new([0u8; 64]),
        };

        let changed = storage.replace_remote_catalogue(&catalogue).unwrap();
        assert!(changed, "first insert should report change");

        let meta = storage.get_remote_catalogue_meta(&owner).unwrap().unwrap();
        assert_eq!(meta.revision, 5);
        assert_eq!(meta.generated_at_ms, 1000);

        let files = storage.get_remote_shared_files(&owner).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content_hash, "hash1");

        // Same revision — should not report change.
        let changed = storage.replace_remote_catalogue(&catalogue).unwrap();
        assert!(!changed, "same revision should not report change");

        // New revision — should replace.
        let catalogue2 = SignedFileCatalogue {
            revision: 6,
            files: vec![RemoteSharedFile::new(
                "hash2",
                "file2.txt",
                None,
                200,
                "application/pdf",
                None,
                2,
            )],
            ..catalogue
        };
        let changed = storage.replace_remote_catalogue(&catalogue2).unwrap();
        assert!(changed, "new revision should report change");

        let files = storage.get_remote_shared_files(&owner).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content_hash, "hash2");
    }

    #[cfg(feature = "net")]
    #[test]
    fn test_replace_remote_catalogue_collections() {
        use crate::catalogue_model::{
            FileCatalogueCollection, RemoteSharedFile, SignedFileCatalogue,
        };
        use serde_byte_array::ByteArray;

        let storage = Storage::memory().unwrap();
        let owner = random_public_key();

        let catalogue = SignedFileCatalogue {
            owner_id: owner,
            revision: 1,
            generated_at_ms: 1000,
            collections: vec![
                FileCatalogueCollection::with_description("Photos", "Photo collection"),
                FileCatalogueCollection::new("Documents"),
            ],
            files: vec![
                RemoteSharedFile::new(
                    "hash1",
                    "pic.jpg",
                    None,
                    500,
                    "image/jpeg",
                    Some("Photos".to_string()),
                    1,
                ),
                RemoteSharedFile::new(
                    "hash2",
                    "doc.pdf",
                    None,
                    1000,
                    "application/pdf",
                    Some("Documents".to_string()),
                    1,
                ),
            ],
            signature: ByteArray::new([0u8; 64]),
        };

        storage.replace_remote_catalogue(&catalogue).unwrap();

        let collections = storage.get_remote_collections(&owner).unwrap();
        assert_eq!(collections.len(), 2);
        assert_eq!(collections[0].name, "Photos");
        assert_eq!(
            collections[0].description.as_deref(),
            Some("Photo collection")
        );
        assert_eq!(collections[1].name, "Documents");
        assert!(collections[1].description.is_none());

        let files = storage.get_remote_shared_files(&owner).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].collection_name.as_deref(), Some("Photos"));
        assert_eq!(files[1].collection_name.as_deref(), Some("Documents"));
    }

    #[cfg(feature = "net")]
    #[test]
    fn test_replace_remote_catalogue_stale_removal() {
        use crate::catalogue_model::{
            FileCatalogueCollection, RemoteSharedFile, SignedFileCatalogue,
        };
        use serde_byte_array::ByteArray;

        let storage = Storage::memory().unwrap();
        let owner = random_public_key();

        // First version: 2 files, 1 collection.
        let v1_catalogue = SignedFileCatalogue {
            owner_id: owner,
            revision: 1,
            generated_at_ms: 1000,
            collections: vec![FileCatalogueCollection::new("Collection")],
            files: vec![
                RemoteSharedFile::new(
                    "keep_me",
                    "keep.txt",
                    None,
                    100,
                    "text/plain",
                    Some("Collection".to_string()),
                    1,
                ),
                RemoteSharedFile::new("remove_me", "remove.txt", None, 200, "text/plain", None, 1),
            ],
            signature: ByteArray::new([0u8; 64]),
        };

        storage.replace_remote_catalogue(&v1_catalogue).unwrap();
        assert_eq!(storage.get_remote_shared_files(&owner).unwrap().len(), 2);

        // Second version: only 1 file remains, same collection.
        // This should delete the stale entry and keep the collection.
        let v2_catalogue = SignedFileCatalogue {
            revision: 2,
            files: vec![RemoteSharedFile::new(
                "keep_me",
                "keep.txt",
                None,
                100,
                "text/plain",
                Some("Collection".to_string()),
                2,
            )],
            ..v1_catalogue
        };

        storage.replace_remote_catalogue(&v2_catalogue).unwrap();

        let files = storage.get_remote_shared_files(&owner).unwrap();
        assert_eq!(files.len(), 1, "stale file should have been removed");
        assert_eq!(files[0].content_hash, "keep_me");

        let collections = storage.get_remote_collections(&owner).unwrap();
        assert_eq!(collections.len(), 1, "collection should still exist");
        assert_eq!(collections[0].name, "Collection");
    }

    #[cfg(feature = "net")]
    #[test]
    fn test_replace_remote_catalogue_atomicity() {
        use crate::catalogue_model::{
            FileCatalogueCollection, RemoteSharedFile, SignedFileCatalogue,
        };
        use serde_byte_array::ByteArray;

        let storage = Storage::memory().unwrap();
        let owner = random_public_key();

        // Insert an initial catalogue.
        let v1 = SignedFileCatalogue {
            owner_id: owner,
            revision: 1,
            generated_at_ms: 1000,
            collections: vec![],
            files: vec![RemoteSharedFile::new(
                "original",
                "original.txt",
                None,
                100,
                "text/plain",
                None,
                1,
            )],
            signature: ByteArray::new([0u8; 64]),
        };
        storage.replace_remote_catalogue(&v1).unwrap();

        // Verify the replacement was atomic: all data should be consistent.
        let meta = storage.get_remote_catalogue_meta(&owner).unwrap().unwrap();
        assert_eq!(meta.revision, 1);
        let files = storage.get_remote_shared_files(&owner).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content_hash, "original");

        // Replace with new revision — should atomically swap all data.
        let v2 = SignedFileCatalogue {
            revision: 2,
            generated_at_ms: 2000,
            collections: vec![],
            files: vec![RemoteSharedFile::new(
                "replacement",
                "replacement.txt",
                None,
                200,
                "application/pdf",
                None,
                2,
            )],
            ..v1
        };
        storage.replace_remote_catalogue(&v2).unwrap();

        // After replacement, only new data should exist.
        let meta = storage.get_remote_catalogue_meta(&owner).unwrap().unwrap();
        assert_eq!(meta.revision, 2);
        let files = storage.get_remote_shared_files(&owner).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content_hash, "replacement");
    }

    #[cfg(feature = "net")]
    #[test]
    fn test_v3_migration_idempotent() {
        // Verify that running migrations multiple times is safe,
        // tables exist, and the storage round-trips correctly.
        let storage = Storage::memory().unwrap();
        // Tables exist after first open.
        storage
            .with_conn(|conn| -> n0_error::Result<()> {
                conn.query_row("SELECT COUNT(*) FROM remote_catalogues", [], |_r| Ok(()))
                    .map_err(|e| anyhow!("remote_catalogues missing: {e}"))?;
                conn.query_row("SELECT COUNT(*) FROM remote_shared_files", [], |_r| Ok(()))
                    .map_err(|e| anyhow!("remote_shared_files missing: {e}"))?;
                conn.query_row("SELECT COUNT(*) FROM remote_collections", [], |_r| Ok(()))
                    .map_err(|e| anyhow!("remote_collections missing: {e}"))?;
                Ok(())
            })
            .unwrap();
        // Schema version is current.
        let version: u32 = storage
            .with_conn(|conn| {
                Ok(conn
                    .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                        row.get(0)
                    })
                    .map_err(|e| anyhow!("{e}"))?)
            })
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        // Verify all new types can round-trip.
        let secret_key = iroh::SecretKey::generate();
        let catalogue = crate::catalogue_model::SignedFileCatalogue::sign(
            &secret_key,
            1,
            1000,
            vec![],
            vec![crate::catalogue_model::RemoteSharedFile::new(
                "test_hash",
                "test.txt",
                None,
                42,
                "text/plain",
                None,
                1,
            )],
        );
        let owner = catalogue.owner_id;
        storage.replace_remote_catalogue(&catalogue).unwrap();
        let meta = storage.get_remote_catalogue_meta(&owner).unwrap().unwrap();
        assert_eq!(meta.revision, 1);
        let files = storage.get_remote_shared_files(&owner).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content_hash, "test_hash");
    }

    // Crash/restart coverage: each test models an interruption by dropping an
    // uncommitted SQLite transaction or by leaving durable state mid-flight.
    fn crash_test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "boru-crash-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn crash_test_envelope(msg_id: MessageId) -> StoredEnvelope {
        StoredEnvelope {
            msg_id,
            conversation_id: [0xC1; 32],
            author_user_id: random_public_key(),
            author_device_id: random_public_key(),
            created_at_ms: now_ms(),
            expires_at_ms: now_ms() + 86_400_000,
            ciphertext: bytes::Bytes::from_static(b"crash-test"),
            signature: [0x5A; 64],
            acked_at_ms: None,
        }
    }

    fn seed_crash_outbox(storage: &Storage, msg_id: MessageId, peer: iroh::PublicKey) {
        storage.insert_inbox(&crash_test_envelope(msg_id)).unwrap();
        storage.enqueue_outbox(&msg_id, peer, now_ms()).unwrap();
    }

    #[test]
    fn crash_outgoing_transaction_rolls_back() {
        let storage = Storage::memory().unwrap();
        let msg_id = [0xD1; 32];
        let mut conn = storage.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute("INSERT INTO inbox (msg_id, conversation_id, author_user_id, author_device_id, created_at_ms, expires_at_ms, ciphertext, signature) VALUES (?1, ?2, ?3, ?4, 1, 2, ?5, ?6)", params![msg_id.as_slice(), [1u8; 32], [2u8; 32], [3u8; 32], b"x", [0u8; 64]]).unwrap();
        drop(tx);
        drop(conn);
        assert!(storage.get_inbox(&msg_id).unwrap().is_none());
    }

    #[test]
    fn crash_outbox_claim_recovers_stale_lease() {
        let dir = crash_test_dir("claim");
        let msg_id = [0xD2; 32];
        let peer = random_public_key();
        {
            let storage = Storage::open(&dir).unwrap();
            seed_crash_outbox(&storage, msg_id, peer);
            storage.conn.lock().unwrap().execute("UPDATE outbox SET status = 1, attempts = 1, next_attempt_at_ms = 9999999999999", []).unwrap();
        }
        let storage = Storage::open(&dir).unwrap();
        let due = storage.fetch_due_outbox(now_ms()).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].status, DeliveryStatus::Pending);
        assert_eq!(due[0].last_error_code.as_deref(), Some("crash_recovered"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn crash_active_delivery_attempt_is_retryable() {
        let dir = crash_test_dir("delivery");
        let msg_id = [0xD3; 32];
        let peer = random_public_key();
        {
            let storage = Storage::open(&dir).unwrap();
            seed_crash_outbox(&storage, msg_id, peer);
            storage
                .record_attempt(&msg_id, peer, now_ms() + 60_000, None)
                .unwrap();
        }
        let storage = Storage::open(&dir).unwrap();
        assert_eq!(storage.fetch_due_outbox(now_ms()).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn crash_committed_incoming_persistence_remains() {
        let dir = crash_test_dir("incoming-committed");
        let msg_id = [0xD4; 32];
        {
            let storage = Storage::open(&dir).unwrap();
            storage.insert_inbox(&crash_test_envelope(msg_id)).unwrap();
        }
        let storage = Storage::open(&dir).unwrap();
        assert!(storage.get_inbox(&msg_id).unwrap().is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn crash_uncommitted_incoming_is_not_acknowledged() {
        let storage = Storage::memory().unwrap();
        let msg_id = [0xD5; 32];
        let mut conn = storage.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute("INSERT INTO inbox (msg_id, conversation_id, author_user_id, author_device_id, created_at_ms, expires_at_ms, ciphertext, signature, acked_at_ms) VALUES (?1, ?2, ?3, ?4, 1, 2, ?5, ?6, 3)", params![msg_id.as_slice(), [1u8; 32], [2u8; 32], [3u8; 32], b"x", [0u8; 64]]).unwrap();
        drop(tx);
        drop(conn);
        assert!(storage.get_inbox(&msg_id).unwrap().is_none());
    }

    #[test]
    fn crash_ack_transaction_does_not_resurrect_outbox() {
        let dir = crash_test_dir("ack");
        let msg_id = [0xD6; 32];
        let peer = random_public_key();
        {
            let storage = Storage::open(&dir).unwrap();
            seed_crash_outbox(&storage, msg_id, peer);
            storage
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE inbox SET acked_at_ms = 7 WHERE msg_id = ?1",
                    params![msg_id.as_slice()],
                )
                .unwrap();
        }
        let storage = Storage::open(&dir).unwrap();
        assert!(storage.fetch_due_outbox(now_ms()).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn crash_retry_scheduling_becomes_due_on_restart() {
        let dir = crash_test_dir("retry");
        let msg_id = [0xD7; 32];
        let peer = random_public_key();
        {
            let storage = Storage::open(&dir).unwrap();
            seed_crash_outbox(&storage, msg_id, peer);
            storage
                .record_attempt(&msg_id, peer, now_ms() + 3_600_000, Some("timeout"))
                .unwrap();
        }
        let storage = Storage::open(&dir).unwrap();
        assert_eq!(storage.fetch_due_outbox(now_ms()).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    // ── Incoming message crash-recovery tests ─────────────────────────
    //
    // Each test exercises a specific crash point during the incoming
    // message lifecycle: persistence, ack generation, and ack transaction.

    #[test]
    fn crash_incoming_ack_is_idempotent() {
        // Calling mark_acked multiple times for the same message must be
        // safe — no error, no duplicate delivery, no spurious state change.
        let storage = Storage::memory().unwrap();
        let msg_id = [0xE1; 32];
        let peer = random_public_key();

        storage.insert_inbox(&crash_test_envelope(msg_id)).unwrap();
        storage.enqueue_outbox(&msg_id, peer, now_ms()).unwrap();

        // First ack — normal path.
        storage.mark_acked(&msg_id, peer).unwrap();
        let due_after_first = storage.fetch_due_outbox(now_ms() + 86_400_000).unwrap();
        assert!(!due_after_first.iter().any(|r| r.msg_id == msg_id),
            "message should not be due after first ack");

        // Second ack — must be a no-op (idempotent).
        storage.mark_acked(&msg_id, peer).unwrap();
        let due_after_second = storage.fetch_due_outbox(now_ms() + 86_400_000).unwrap();
        assert!(!due_after_second.iter().any(|r| r.msg_id == msg_id),
            "message should not become due again after redundant ack");

        // Only one outbox row exists.
        let conn = storage.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM outbox WHERE msg_id = ?1 AND status = ?2",
                params![
                    msg_id.as_slice(),
                    crate::store::DeliveryStatus::Acked as u8,
                ],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "exactly one acked outbox row expected");
    }

    #[test]
    fn crash_incoming_persisted_survives_with_outbox() {
        // A fully committed inbox message must survive a crash+restart.
        // The matching outbox entry must remain due (it has NOT been acked).
        let dir = crash_test_dir("incoming-persisted");
        let msg_id = [0xE2; 32];
        let peer = random_public_key();
        let conv_id = [0xE2u8; 32];

        {
            let storage = Storage::open(&dir).unwrap();
            let env = StoredEnvelope {
                msg_id,
                conversation_id: conv_id,
                author_user_id: random_public_key(),
                author_device_id: random_public_key(),
                created_at_ms: now_ms(),
                expires_at_ms: now_ms() + 86_400_000,
                ciphertext: bytes::Bytes::from_static(b"persist-test"),
                signature: [0xE2; 64],
                acked_at_ms: None,
            };
            storage.insert_inbox(&env).unwrap();
            storage.enqueue_outbox(&msg_id, peer, now_ms()).unwrap();
        } // drop: simulate crash

        let storage = Storage::open(&dir).unwrap();
        // Inbox row must survive.
        let fetched = storage.get_inbox(&msg_id).unwrap();
        assert!(fetched.is_some(), "committed inbox must survive crash");
        assert_eq!(fetched.unwrap().conversation_id, conv_id);

        // Outbox must still be due (no ack was sent).
        let due = storage.fetch_due_outbox(now_ms() + 86_400_000).unwrap();
        assert!(
            due.iter().any(|r| r.msg_id == msg_id),
            "outbox entry must remain due after restart (no ack)"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn crash_incoming_uncommitted_rolls_back_on_restart() {
        // An inbox insert performed inside an uncommitted SQLite
        // transaction must NOT survive a restart.
        let dir = crash_test_dir("incoming-uncommitted");
        let msg_id = [0xE3; 32];
        let peer = random_public_key();

        {
            let storage = Storage::open(&dir).unwrap();
            // Start a transaction, insert inbox, then drop without commit.
            let mut conn = storage.conn.lock().unwrap();
            let tx = conn.transaction().unwrap();
            tx.execute(
                "INSERT INTO inbox (msg_id, conversation_id, author_user_id, author_device_id, created_at_ms, expires_at_ms, ciphertext, signature) VALUES (?1, ?2, ?3, ?4, 1, 2, ?5, ?6)",
                params![msg_id.as_slice(), [0xE3u8; 32], [4u8; 32], [5u8; 32], b"rollback-test", [0u8; 64]],
            ).unwrap();
            // Also enqueue an outbox entry inside the same transaction.
            tx.execute(
                "INSERT INTO outbox (msg_id, recipient_device_id, status, attempts, next_attempt_at_ms) VALUES (?1, ?2, 0, 0, 1)",
                params![msg_id.as_slice(), peer.as_bytes()],
            ).unwrap();
            drop(tx);
            drop(conn);
        } // drop: simulate crash before commit

        let storage = Storage::open(&dir).unwrap();
        // Neither inbox nor outbox should exist after recovery.
        assert!(
            storage.get_inbox(&msg_id).unwrap().is_none(),
            "uncommitted inbox must NOT survive crash"
        );
        assert!(
            !storage.fetch_due_outbox(now_ms() + 86_400_000)
                .unwrap()
                .iter()
                .any(|r| r.msg_id == msg_id),
            "uncommitted outbox must NOT survive crash"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn crash_incoming_ack_syncs_outbox_on_restart() {
        // When an inbox message has acked_at_ms set but the outbox was
        // never updated (crash during ack transaction), recover_crash_state
        // must advance the outbox to Acked so the message is not re-sent.
        let dir = crash_test_dir("incoming-ack-sync");
        let msg_id = [0xE4; 32];
        let peer = random_public_key();

        {
            let storage = Storage::open(&dir).unwrap();
            // Fully persist inbox + outbox.
            storage.insert_inbox(&crash_test_envelope(msg_id)).unwrap();
            storage.enqueue_outbox(&msg_id, peer, now_ms()).unwrap();

            // Simulate ack received: set acked_at_ms on inbox, but crash
            // BEFORE updating the outbox status.  Recovery step 4 should
            // reconcile this.
            storage
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE inbox SET acked_at_ms = ?1 WHERE msg_id = ?2",
                    params![now_ms() as i64, msg_id.as_slice()],
                )
                .unwrap();
        } // crash

        let storage = Storage::open(&dir).unwrap();

        // Inbox must still have the message with acked_at_ms.
        let inbox = storage.get_inbox(&msg_id).unwrap();
        assert!(inbox.is_some(), "inbox message must survive");
        assert!(
            inbox.unwrap().acked_at_ms.is_some(),
            "acked_at_ms must survive crash"
        );

        // Outbox must NOT be due (recover_crash_state should have advanced
        // it to Acked).
        let due = storage.fetch_due_outbox(now_ms() + 86_400_000).unwrap();
        assert!(
            !due.iter().any(|r| r.msg_id == msg_id),
            "outbox must not be due after recovery — ack should have been synced"
        );

        // Outbox status should be Acked with recovery diagnostic
        let conn = storage.conn.lock().unwrap();
        let status_code: u8 = conn
            .query_row(
                "SELECT status FROM outbox WHERE msg_id = ?1",
                [msg_id.as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            status_code,
            crate::store::DeliveryStatus::Acked as u8,
            "outbox should be Acked after recovery"
        );
        let error_code: Option<String> = conn
            .query_row(
                "SELECT last_error_code FROM outbox WHERE msg_id = ?1",
                [msg_id.as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            error_code.as_deref(),
            Some("recovered_stale_ack"),
            "recovery should annotate outbox with recovered_stale_ack"
        );
        drop(conn);

        let _ = std::fs::remove_dir_all(dir);
    }

    // ── Outgoing DM crash-recovery tests ─────────────────────────────
    //
    // Each test simulates a process interruption during the outgoing DM
    // lifecycle: creation transaction, outbox claim, and active delivery
    // attempt.  Verifies that committed data survives restart, partial
    // transactions do not leak orphan rows, and interrupted sends are
    // retried without duplication.

    #[test]
    fn dm_crash_outgoing_survives_reopen_with_defaults() {
        // After a successful queue_outgoing_dm followed by a process
        // restart (close + reopen), all dm_* rows must survive with
        // correct default column values.
        let dir = crash_test_dir("dm-outgoing-survive");
        let sender = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let mailbox = crate::mailbox::MailboxIdentity::from_secret(&recipient);
        let idempotency_key = [0xD1; 32];
        let plaintext = b"crash survival test";

        let message_id = {
            let storage = Storage::open(&dir).unwrap();
            let result = storage
                .queue_outgoing_dm(
                    idempotency_key,
                    &sender,
                    recipient.public(),
                    mailbox.public_key(),
                    plaintext,
                    u64::MAX,
                )
                .unwrap();
            result.message_id
        }; // drop: simulate crash

        // Reopen and verify every column.
        let storage = Storage::open(&dir).unwrap();

        // get_queued_outgoing_dm works after restart.
        let restored = storage
            .get_queued_outgoing_dm(&message_id)
            .unwrap()
            .expect("queued DM must survive restart");
        assert_eq!(restored.message_id, message_id);
        assert_eq!(restored.sender_sequence, 1);

        // dm_messages columns.
        let conn = storage.conn.lock().unwrap();
        let (stored_id, stored_conv, stored_seq, stored_plain, stored_created): (
            Vec<u8>,
            Vec<u8>,
            i64,
            Vec<u8>,
            i64,
        ) = conn
            .query_row(
                "SELECT message_id, conversation_id, sender_sequence, plaintext, created_at_ms
                 FROM dm_messages WHERE message_id = ?1",
                params![message_id.as_slice()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(&stored_id, message_id.as_slice(), "message_id");
        assert_eq!(stored_seq, 1, "sender_sequence");
        assert_eq!(stored_plain, plaintext, "plaintext");
        assert!(stored_created > 0, "created_at_ms");

        // dm_outbox defaults: status=0(Queued), attempts=0, next_attempt_at_ms <= now.
        let (outbox_status, outbox_attempts, outbox_next_attempt): (u8, i64, i64) = conn
            .query_row(
                "SELECT status, attempts, next_attempt_at_ms
                 FROM dm_outbox WHERE message_id = ?1",
                params![message_id.as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            outbox_status, 0,
            "dm_outbox.status must be Queued(0) after restart"
        );
        assert_eq!(
            outbox_attempts, 0,
            "dm_outbox.attempts must be 0 after restart"
        );
        assert!(
            outbox_next_attempt <= now_ms() as i64 + 1000,
            "dm_outbox.next_attempt_at_ms must be near now, got {outbox_next_attempt}"
        );

        // Exactly one row in each table — no duplicates from crash recovery.
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_messages", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
            1,
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_outbox", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
            1,
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_conversations", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
            1,
            "one dm_conversation row",
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_sender_sequences", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
            1,
            "one dm_sender_sequence row",
        );
        drop(conn);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dm_crash_outgoing_claim_doesnt_lose_entry() {
        // Simulate a delivery worker claiming a dm_outbox entry (setting
        // status=1, attempts=1) and then crashing.  The entry must survive
        // restart with the crash-left state preserved.
        let dir = crash_test_dir("dm-outgoing-claim");
        let sender = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let mailbox = crate::mailbox::MailboxIdentity::from_secret(&recipient);
        let idempotency_key = [0xD2; 32];

        let message_id = {
            let storage = Storage::open(&dir).unwrap();
            let result = storage
                .queue_outgoing_dm(
                    idempotency_key,
                    &sender,
                    recipient.public(),
                    mailbox.public_key(),
                    b"claim test",
                    u64::MAX,
                )
                .unwrap();

            // Claim: mark status=1 (Sending/claimed), increment attempts.
            storage
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE dm_outbox SET status = 1, attempts = 1 WHERE message_id = ?1",
                    params![result.message_id.as_slice()],
                )
                .unwrap();
            result.message_id
        }; // crash during claim

        let storage = Storage::open(&dir).unwrap();

        // Entry must still be retrievable.
        let restored = storage
            .get_queued_outgoing_dm(&message_id)
            .unwrap()
            .expect("claimed DM entry must survive restart");
        assert_eq!(restored.message_id, message_id);

        // Crash-left state is preserved.
        let conn = storage.conn.lock().unwrap();
        let (status, attempts): (u8, i64) = conn
            .query_row(
                "SELECT status, attempts FROM dm_outbox WHERE message_id = ?1",
                params![message_id.as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, 1, "crash-left status must be preserved");
        assert_eq!(attempts, 1, "crash-left attempts must be preserved");

        // No duplicate rows.
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_messages", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
            1,
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_outbox", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
            1,
        );
        drop(conn);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dm_crash_outgoing_active_delivery_recovers() {
        // Simulate a crash during an active delivery attempt: the outbox
        // entry has been claimed (status=1), accumulated attempts (2), and
        // next_attempt_at_ms advanced to a future retry timestamp.  After
        // restart the entry must still be retrievable so the delivery
        // worker can decide whether to retry or fail it.
        let dir = crash_test_dir("dm-outgoing-delivery");
        let sender = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let mailbox = crate::mailbox::MailboxIdentity::from_secret(&recipient);
        let idempotency_key = [0xD3; 32];
        let far_future = now_ms() + 86_400_000;

        let message_id = {
            let storage = Storage::open(&dir).unwrap();
            let result = storage
                .queue_outgoing_dm(
                    idempotency_key,
                    &sender,
                    recipient.public(),
                    mailbox.public_key(),
                    b"active delivery test",
                    u64::MAX,
                )
                .unwrap();

            // Simulate two delivery attempts followed by a crash during the third.
            storage
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE dm_outbox SET status = 1, attempts = 2, next_attempt_at_ms = ?1 WHERE message_id = ?2",
                    params![far_future as i64, result.message_id.as_slice()],
                )
                .unwrap();
            result.message_id
        }; // crash during active delivery

        let storage = Storage::open(&dir).unwrap();

        // Entry survives restart.
        let restored = storage
            .get_queued_outgoing_dm(&message_id)
            .unwrap()
            .expect("active delivery entry must survive restart");
        assert_eq!(restored.message_id, message_id);

        // Crash-left state is preserved.
        let conn = storage.conn.lock().unwrap();
        let (status, attempts, next_attempt): (u8, i64, i64) = conn
            .query_row(
                "SELECT status, attempts, next_attempt_at_ms FROM dm_outbox WHERE message_id = ?1",
                params![message_id.as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, 1, "crash-left status=1 preserved");
        assert_eq!(attempts, 2, "crash-left attempts=2 preserved");
        assert_eq!(
            next_attempt, far_future as i64,
            "crash-left next_attempt_at_ms preserved"
        );

        // No duplicate rows.
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_messages", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
            1,
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_outbox", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
            1,
        );
        drop(conn);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn outgoing_dm_is_atomic_and_idempotent() {
        let storage = Storage::memory().unwrap();
        let sender = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let mailbox = crate::mailbox::MailboxIdentity::from_secret(&recipient);
        let key = [0xA1; 32];
        let first = storage
            .queue_outgoing_dm(
                key,
                &sender,
                recipient.public(),
                mailbox.public_key(),
                b"hello",
                now_ms() + 60_000,
            )
            .unwrap();
        let retry = storage
            .queue_outgoing_dm(
                key,
                &sender,
                recipient.public(),
                mailbox.public_key(),
                b"different plaintext must not replace the original",
                now_ms() + 60_000,
            )
            .unwrap();
        assert_eq!(first.message_id, retry.message_id);
        assert_eq!(first.sender_sequence, 1);
        assert_eq!(mailbox.open(&retry.envelope).unwrap(), mailbox.open(&first.envelope).unwrap());
        let logical: LogicalDirectMessage = postcard::from_bytes(&mailbox.open(&first.envelope).unwrap()).unwrap();
        assert_eq!(logical.plaintext, b"hello");
        let conn = storage.conn.lock().unwrap();
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM dm_messages", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM dm_outbox", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    }

    #[test]
    fn outgoing_dm_conversation_sequence_increments_once_per_message() {
        let storage = Storage::memory().unwrap();
        let sender = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let mailbox = crate::mailbox::MailboxIdentity::from_secret(&recipient);
        let a = storage.queue_outgoing_dm([1; 32], &sender, recipient.public(), mailbox.public_key(), b"a", u64::MAX).unwrap();
        let b = storage.queue_outgoing_dm([2; 32], &sender, recipient.public(), mailbox.public_key(), b"b", u64::MAX).unwrap();
        assert_eq!(a.conversation_id, b.conversation_id);
        assert_eq!((a.sender_sequence, b.sender_sequence), (1, 2));
    }

    #[test]
    fn outgoing_dm_rejects_corrupt_database_without_partial_rows() {
        let storage = Storage::memory().unwrap();
        storage.conn.lock().unwrap().execute("DROP TABLE dm_outbox", []).unwrap();
        let sender = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let mailbox = crate::mailbox::MailboxIdentity::from_secret(&recipient);
        assert!(storage.queue_outgoing_dm([3; 32], &sender, recipient.public(), mailbox.public_key(), b"x", u64::MAX).is_err());
        let conn = storage.conn.lock().unwrap();
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM dm_messages", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    }

    #[test]
    fn outgoing_dm_encryption_failure_rolls_back_without_sequence_gap() {
        let storage = Storage::memory().unwrap();
        let sender = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let mailbox = crate::mailbox::MailboxIdentity::from_secret(&recipient);
        let oversized = vec![0xEE; crate::abuse_controls::MAX_DECRYPTED_MESSAGE_SIZE + 1];
        assert!(storage
            .queue_outgoing_dm(
                [0xE1; 32],
                &sender,
                recipient.public(),
                mailbox.public_key(),
                &oversized,
                u64::MAX,
            )
            .is_err());
        let queued = storage
            .queue_outgoing_dm(
                [0xE2; 32],
                &sender,
                recipient.public(),
                mailbox.public_key(),
                b"after encryption failure",
                u64::MAX,
            )
            .unwrap();
        assert_eq!(queued.sender_sequence, 1);
        let conn = storage.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_messages", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn outgoing_dm_restart_preserves_sequence_message_and_exact_envelope() {
        let dir = crash_test_dir("outgoing-restart");
        let sender = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let mailbox = crate::mailbox::MailboxIdentity::from_secret(&recipient);
        let key = [0xB2; 32];
        let first = {
            let storage = Storage::open(&dir).unwrap();
            storage
                .queue_outgoing_dm(
                    key,
                    &sender,
                    recipient.public(),
                    mailbox.public_key(),
                    b"restart me",
                    u64::MAX,
                )
                .unwrap()
        };
        let reopened = Storage::open(&dir).unwrap();
        let restored = reopened
            .get_queued_outgoing_dm(&first.message_id)
            .unwrap()
            .expect("queued message survives restart");
        assert_eq!(restored.message_id, first.message_id);
        assert_eq!(restored.conversation_id, first.conversation_id);
        assert_eq!(restored.sender_sequence, 1);
        assert_eq!(
            postcard::to_stdvec(&restored.envelope).unwrap(),
            postcard::to_stdvec(&first.envelope).unwrap()
        );
        let second = reopened
            .queue_outgoing_dm(
                [0xB3; 32],
                &sender,
                recipient.public(),
                mailbox.public_key(),
                b"after restart",
                u64::MAX,
            )
            .unwrap();
        assert_eq!(second.sender_sequence, 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    // ── Happy-path and idempotency for outgoing DM creation ────────────

    #[test]
    fn outgoing_dm_inserts_all_columns_correctly() {
        let storage = Storage::memory().unwrap();
        let sender = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let mailbox = crate::mailbox::MailboxIdentity::from_secret(&recipient);
        let idempotency_key = [0xCA; 32];
        let plaintext = b"hello world";
        let expires_at = now_ms() + 86_400_000;
        let result = storage
            .queue_outgoing_dm(
                idempotency_key,
                &sender,
                recipient.public(),
                mailbox.public_key(),
                plaintext,
                expires_at,
            )
            .unwrap();

        // Assert the returned QueuedDirectMessage has expected fields.
        assert_eq!(result.sender_sequence, 1);

        // Check dm_messages row column by column.
        let conn = storage.conn.lock().unwrap();
        let (stored_key, stored_msg_id, stored_conv_id, stored_seq,
             stored_logical, stored_plaintext, stored_created): (
            Vec<u8>, Vec<u8>, Vec<u8>, i64, Vec<u8>, Vec<u8>, i64,
        ) = conn.query_row(
            "SELECT idempotency_key, message_id, conversation_id, sender_sequence,
                    logical_message, plaintext, created_at_ms
             FROM dm_messages WHERE message_id = ?1",
            params![result.message_id.as_slice()],
            |row| Ok((
                row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
                row.get(4)?, row.get(5)?, row.get(6)?,
            )),
        ).unwrap();
        assert_eq!(&stored_key, &idempotency_key, "idempotency_key mismatch");
        assert_eq!(&stored_msg_id, result.message_id.as_slice(), "message_id mismatch");
        assert_eq!(&stored_conv_id, result.conversation_id.as_slice(), "conversation_id mismatch");
        assert_eq!(stored_seq, 1, "sender_sequence should be 1");
        assert_eq!(stored_plaintext, plaintext, "plaintext mismatch");
        assert!(stored_created > 0, "created_at_ms should be set");

        // Deserialize the logical message and verify its contents.
        let logical: LogicalDirectMessage =
            postcard::from_bytes(&stored_logical).expect("valid logical message");
        assert_eq!(logical.conversation_id, result.conversation_id);
        assert_eq!(logical.sender, sender.public());
        assert_eq!(logical.recipient, recipient.public());
        assert_eq!(logical.sender_sequence, 1);
        assert_eq!(logical.plaintext, plaintext);
        // Signature must verify against the sender.
        let unsigned = postcard::to_stdvec(&(
            result.conversation_id,
            sender.public(),
            recipient.public(),
            1u64,
            plaintext.as_slice(),
        )).unwrap();
        sender
            .public()
            .verify(&unsigned, &iroh::Signature::from_bytes(&logical.signature.try_into().unwrap()))
            .expect("logical message signature should verify");

        // Check dm_outbox row.
        let (stored_outbox_msg_id, stored_recipient, stored_envelope_bytes, stored_next_attempt): (
            Vec<u8>, Vec<u8>, Vec<u8>, i64,
        ) = conn.query_row(
            "SELECT message_id, recipient_id, envelope, next_attempt_at_ms FROM dm_outbox WHERE message_id = ?1",
            params![result.message_id.as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).unwrap();
        assert_eq!(&stored_outbox_msg_id, result.message_id.as_slice());
        assert_eq!(stored_recipient, recipient.public().as_bytes());
        // Envelope must be decryptable by the recipient and yield the logical message.
        let envelope: crate::mailbox::MailboxEnvelope =
            postcard::from_bytes(&stored_envelope_bytes).expect("valid envelope bytes");
        let opened = mailbox.open(&envelope).expect("recipient can decrypt envelope");
        assert_eq!(opened, stored_logical, "envelope decrypts to the stored logical message");
        // next_attempt_at_ms should be now (immediate delivery attempt).
        assert!(
            stored_next_attempt <= now_ms() as i64 + 1000,
            "next_attempt_at_ms should be near now, got {stored_next_attempt}"
        );

        // Exactly one row in each table.
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_messages", [], |r| r.get::<_, i64>(0)).unwrap(),
            1,
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_outbox", [], |r| r.get::<_, i64>(0)).unwrap(),
            1,
        );
        drop(conn);
    }

    #[test]
    fn outgoing_dm_same_plaintext_no_duplication() {
        let storage = Storage::memory().unwrap();
        let sender = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let mailbox = crate::mailbox::MailboxIdentity::from_secret(&recipient);
        let plaintext = b"same content";

        // First call — unique idempotency key.
        let key1 = [0x1A; 32];
        let first = storage
            .queue_outgoing_dm(key1, &sender, recipient.public(), mailbox.public_key(), plaintext, u64::MAX)
            .unwrap();
        assert_eq!(first.sender_sequence, 1);

        // Second call — same idempotency key, same plaintext → idempotent return.
        let retry = storage
            .queue_outgoing_dm(key1, &sender, recipient.public(), mailbox.public_key(), plaintext, u64::MAX)
            .unwrap();
        assert_eq!(retry.message_id, first.message_id, "idempotent retry must return same message_id");
        assert_eq!(retry.sender_sequence, 1, "sequence must not increment on idempotent retry");
        // The envelope on retry comes from the stored row (not re-encrypted), so it's bitwise identical.
        assert_eq!(
            postcard::to_stdvec(&retry.envelope).unwrap(),
            postcard::to_stdvec(&first.envelope).unwrap(),
            "stored envelope must be returned verbatim on idempotent retry"
        );

        // Third call — different idempotency key, same plaintext → different message.
        let key2 = [0x2B; 32];
        let second = storage
            .queue_outgoing_dm(key2, &sender, recipient.public(), mailbox.public_key(), plaintext, u64::MAX)
            .unwrap();
        assert_ne!(second.message_id, first.message_id, "different key must produce different message_id");
        assert_eq!(second.sender_sequence, 2, "sequence must increment for each distinct key");

        // Exactly 2 rows in dm_messages — no duplication from the retry.
        let conn = storage.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_messages", [], |r| r.get::<_, i64>(0)).unwrap(),
            2,
            "expected exactly 2 dm_messages rows (no duplicate from idempotent retry)"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_outbox", [], |r| r.get::<_, i64>(0)).unwrap(),
            2,
            "expected exactly 2 dm_outbox rows"
        );
        // Opening both envelopes must yield the same logical plaintext.
        for msg_id in &[first.message_id, second.message_id] {
            let env_bytes: Vec<u8> = conn.query_row(
                "SELECT envelope FROM dm_outbox WHERE message_id = ?1",
                params![msg_id.as_slice()],
                |row| row.get(0),
            ).unwrap();
            let env: crate::mailbox::MailboxEnvelope = postcard::from_bytes(&env_bytes).unwrap();
            let logical_bytes = mailbox.open(&env).expect("recipient can decrypt");
            let logical: LogicalDirectMessage = postcard::from_bytes(&logical_bytes).unwrap();
            assert_eq!(&logical.plaintext, plaintext, "plaintext must match in both envelopes");
        }
        drop(conn);
    }

    #[test]
    fn outgoing_dm_sequence_preserved_across_restart_on_idempotent_retry() {
        let dir = crash_test_dir("outgoing-seq-retry");
        let sender = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let mailbox = crate::mailbox::MailboxIdentity::from_secret(&recipient);
        let key = [0xC0; 32];
        let plaintext = b"sequence across restart";

        // First session — queue.
        let first = {
            let storage = Storage::open(&dir).unwrap();
            storage
                .queue_outgoing_dm(key, &sender, recipient.public(), mailbox.public_key(), plaintext, u64::MAX)
                .unwrap()
        };
        assert_eq!(first.sender_sequence, 1);

        // Second session — retry with the same idempotency key.
        let retried = {
            let storage = Storage::open(&dir).unwrap();
            storage
                .queue_outgoing_dm(key, &sender, recipient.public(), mailbox.public_key(), plaintext, u64::MAX)
                .unwrap()
        };
        assert_eq!(
            retried.message_id, first.message_id,
            "idempotent retry after restart must return same message_id"
        );
        assert_eq!(
            retried.sender_sequence, 1,
            "sequence must NOT increment on idempotent retry after restart"
        );

        // Verify only one row exists.
        let storage = Storage::open(&dir).unwrap();
        let conn = storage.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_messages", [], |r| r.get::<_, i64>(0)).unwrap(),
            1,
            "no duplicate dm_messages after restart + idempotent retry"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_outbox", [], |r| r.get::<_, i64>(0)).unwrap(),
            1,
            "no duplicate dm_outbox after restart + idempotent retry"
        );
        drop(conn);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn outgoing_dm_new_instance_same_key_no_duplicate() {
        // Restart persistence: create a new Storage instance and call
        // queue_outgoing_dm again with the same parameters.
        let dir = crash_test_dir("outgoing-new-instance");
        let sender = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let mailbox = crate::mailbox::MailboxIdentity::from_secret(&recipient);
        let key = [0xD0; 32];
        let plaintext = b"restart then retry";

        let first = {
            let storage = Storage::open(&dir).unwrap();
            storage
                .queue_outgoing_dm(key, &sender, recipient.public(), mailbox.public_key(), plaintext, u64::MAX)
                .unwrap()
        };

        // Simulate full restart: open a brand-new Storage and call
        // queue_outgoing_dm again with the exact same arguments.
        let second = {
            let storage = Storage::open(&dir).unwrap();
            storage
                .queue_outgoing_dm(key, &sender, recipient.public(), mailbox.public_key(), plaintext, u64::MAX)
                .unwrap()
        };

        // Must return the same message_id (idempotent across restarts).
        assert_eq!(
            second.message_id, first.message_id,
            "queue_outgoing_dm with same key after restart must be idempotent"
        );
        assert_eq!(
            second.sender_sequence, 1,
            "sequence must remain 1 (not increment) on idempotent retry after restart"
        );

        // Verify the envelope from the retry is the stored one, not re-encrypted.
        let storage = Storage::open(&dir).unwrap();
        let stored = storage
            .get_queued_outgoing_dm(&first.message_id)
            .unwrap()
            .expect("message must survive restart");
        assert_eq!(
            postcard::to_stdvec(&second.envelope).unwrap(),
            postcard::to_stdvec(&stored.envelope).unwrap(),
            "retry envelope must match stored envelope"
        );

        // Only one row in each table.
        let conn = storage.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_messages", [], |r| r.get::<_, i64>(0)).unwrap(),
            1,
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_outbox", [], |r| r.get::<_, i64>(0)).unwrap(),
            1,
        );
        drop(conn);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn outgoing_dm_rolls_back_on_failure_after_sequence_allocation() {
        // Inject a SQLite RAISE trigger before INSERT on dm_messages.
        // This simulates a database failure (disk full / connection loss)
        // just after the sender sequence was allocated and the logical
        // message was built and encrypted, but before the visible
        // dm_messages row is written.
        let storage = Storage::memory().unwrap();
        let sender = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let mailbox = crate::mailbox::MailboxIdentity::from_secret(&recipient);

        {
            let conn = storage.conn.lock().unwrap();
            conn.execute_batch(
                "CREATE TRIGGER fail_msg_insert
                 BEFORE INSERT ON dm_messages
                 BEGIN
                     SELECT RAISE(ABORT, 'injected: disk full at dm_messages insert');
                 END;",
            )
            .unwrap();
        }

        assert!(
            storage
                .queue_outgoing_dm(
                    [0xF0; 32],
                    &sender,
                    recipient.public(),
                    mailbox.public_key(),
                    b"hello",
                    u64::MAX,
                )
                .is_err()
        );

        // Full transaction rollback — no orphaned rows in ANY dm_* table.
        let conn = storage.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_conversations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "dm_conversations should be empty after rollback"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_sender_sequences", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "dm_sender_sequences should be empty after rollback"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_messages", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "dm_messages should be empty after rollback"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_outbox", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "dm_outbox should be empty after rollback"
        );
        drop(conn);

        // Remove trigger and verify that a subsequent call succeeds,
        // proving no sequence gap or corrupted state.
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute_batch("DROP TRIGGER IF EXISTS fail_msg_insert")
                .unwrap();
        }
        let recovered = storage
            .queue_outgoing_dm(
                [0xF1; 32],
                &sender,
                recipient.public(),
                mailbox.public_key(),
                b"recovered",
                u64::MAX,
            )
            .unwrap();
        assert_eq!(recovered.sender_sequence, 1);
    }

    #[test]
    fn outgoing_dm_rolls_back_on_failure_after_message_insert() {
        // Inject a SQLite RAISE trigger before INSERT on dm_outbox.
        // This simulates a database failure after the visible dm_messages
        // row was inserted but before the durable dm_outbox entry is
        // written.  Without transaction atomicity this would leave an
        // orphaned message row.
        let storage = Storage::memory().unwrap();
        let sender = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let mailbox = crate::mailbox::MailboxIdentity::from_secret(&recipient);

        {
            let conn = storage.conn.lock().unwrap();
            conn.execute_batch(
                "CREATE TRIGGER fail_outbox_insert
                 BEFORE INSERT ON dm_outbox
                 BEGIN
                     SELECT RAISE(ABORT, 'injected: disk full at dm_outbox insert');
                 END;",
            )
            .unwrap();
        }

        assert!(
            storage
                .queue_outgoing_dm(
                    [0xF2; 32],
                    &sender,
                    recipient.public(),
                    mailbox.public_key(),
                    b"hello",
                    u64::MAX,
                )
                .is_err()
        );

        let conn = storage.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_conversations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "dm_conversations should be empty after rollback"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_sender_sequences", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "dm_sender_sequences should be empty after rollback"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_messages", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "dm_messages must have zero orphaned rows after rollback"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_outbox", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "dm_outbox should be empty after rollback"
        );
        drop(conn);

        // Remove trigger and verify recovery.
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute_batch("DROP TRIGGER IF EXISTS fail_outbox_insert")
                .unwrap();
        }
        let recovered = storage
            .queue_outgoing_dm(
                [0xF3; 32],
                &sender,
                recipient.public(),
                mailbox.public_key(),
                b"recovered",
                u64::MAX,
            )
            .unwrap();
        assert_eq!(recovered.sender_sequence, 1);
    }

    #[test]
    fn outgoing_dm_encryption_failure_leaves_no_orphaned_rows() {
        // Verify that an encryption failure (oversized plaintext rejected
        // by the mailbox layer) rolls back the entire transaction so that
        // not a single row is left behind in ANY dm_* table.
        let storage = Storage::memory().unwrap();
        let sender = iroh::SecretKey::generate();
        let recipient = iroh::SecretKey::generate();
        let mailbox = crate::mailbox::MailboxIdentity::from_secret(&recipient);
        let oversized =
            vec![0xEE; crate::abuse_controls::MAX_DECRYPTED_MESSAGE_SIZE + 1];

        assert!(
            storage
                .queue_outgoing_dm(
                    [0xE3; 32],
                    &sender,
                    recipient.public(),
                    mailbox.public_key(),
                    &oversized,
                    u64::MAX,
                )
                .is_err()
        );

        let conn = storage.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_conversations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "dm_conversations should be empty after encryption failure"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_sender_sequences", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "dm_sender_sequences should be empty after encryption failure"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_messages", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "dm_messages should be empty after encryption failure"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dm_outbox", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "dm_outbox should be empty after encryption failure"
        );
        drop(conn);

        // Verify that a subsequent message with a different key succeeds
        // and starts at sequence 1, confirming no leaked state.
        let ok = storage
            .queue_outgoing_dm(
                [0xE4; 32],
                &sender,
                recipient.public(),
                mailbox.public_key(),
                b"after encryption failure",
                u64::MAX,
            )
            .unwrap();
        assert_eq!(ok.sender_sequence, 1);
    }

    #[test]
    fn crash_sync_response_transaction_rolls_back_without_cursor() {
        let storage = Storage::memory().unwrap();
        let peer = random_public_key();
        let mut conn = storage.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute("INSERT INTO sync_cursor (peer_device_id, last_seen_msg_clock, last_sync_at_ms) VALUES (?1, ?2, 4)", params![peer.as_bytes(), b"response"]).unwrap();
        drop(tx);
        drop(conn);
        assert!(storage.list_sync_cursors().unwrap().is_empty());
    }
}
