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
//! # Design rules
//!
//!  1. Chat attachments belong to messages (`message_attachments`).
//!  2. Profile file offers belong to a user profile (`shared_files`).
//!  3. Both reference the same content-addressed `file_objects` store.
//!  4. No local filesystem paths are exposed to remote peers.
//!  5. All large binary data lives in `file_objects`; relationship tables
//!     carry only foreign keys and metadata.

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::anyhow;
use iroh::{PublicKey, SecretKey};
use n0_error::{Result, StdResultExt};
use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::mailbox::{seal_for, MailboxAck, MailboxEnvelope, MailboxPublicKey};
use crate::store::{DeliveryStatus, MessageId, OutboxRow, StoredEnvelope};

// ── Current schema version ────────────────────────────────────────────────

/// Bump every time a new migration is added.
const CURRENT_SCHEMA_VERSION: u32 = 6;

/// Maximum number of rows inspected by a single outbox claim query.
pub const MAX_OUTBOX_CLAIM_LIMIT: u32 = 100;
/// Default lease duration for an outbox worker claim.
pub const DEFAULT_OUTBOX_LEASE_MS: u64 = 30_000;

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
#[derive(Debug, Clone)]
pub struct Download {
    /// Unique row id.
    pub id: i64,
    /// Links to `file_objects.content_hash` (the target).
    pub content_hash: String,
    /// The remote peer we are downloading from.
    pub remote_peer: String,
    /// Current state: "queued", "active", "paused", "completed", "failed".
    pub state: String,
    /// Bytes received so far.
    pub bytes_downloaded: u64,
    /// Total expected bytes.
    pub total_bytes: u64,
    /// When the download was created.
    pub created_at_ms: u64,
    /// When the state last changed.
    pub updated_at_ms: u64,
    /// Last error message (if state == "failed").
    pub last_error: Option<String>,
    /// Retry count.
    pub retry_count: u32,
    /// Next retry timestamp (ms since UNIX epoch).
    pub next_retry_at_ms: Option<u64>,
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

// ── Storage ───────────────────────────────────────────────────────────────

/// Relational storage backed by a single SQLite database.
///
/// Owns the connection, schema migrations, and provides repository-style
/// accessors for each logical group of tables.
#[derive(Debug, Clone)]
pub struct Storage {
    conn: Arc<Mutex<Connection>>,
}

/// Durable result of creating an outgoing direct message.
#[allow(missing_docs)]
#[derive(Debug, Clone)]
pub struct OutgoingDm {
    pub message_id: MessageId,
    pub sequence: u64,
    pub logical_message: Vec<u8>,
    pub envelope: MailboxEnvelope,
}

/// Deterministic failures used to verify outgoing-DM transaction rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutgoingDmFault {
    /// Fail while preparing mailbox encryption.
    Encryption,
    /// Fail after durable rows are written but before commit.
    Database,
}

#[allow(missing_docs)]
#[derive(Debug, Clone)]
pub struct DmMessageRow {
    pub message_id: MessageId,
    pub conversation_id: [u8; 32],
    pub sender: PublicKey,
    pub recipient: PublicKey,
    pub sequence: u64,
    pub request_key: String,
    pub plaintext: Vec<u8>,
    /// Local insertion time. This is informational only; it is deliberately
    /// not part of the history ordering key because remote clocks are untrusted.
    pub created_at_ms: u64,
}

#[allow(missing_docs)]
#[derive(Debug, Clone)]
pub struct DmOutboxRow {
    pub message_id: MessageId,
    pub recipient: PublicKey,
    pub envelope: MailboxEnvelope,
}

/// Deterministic failures used to verify acknowledgement rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckProcessingFault {
    /// Fail after acknowledgement state is written, before commit.
    Database,
}

#[derive(Debug, Serialize, Deserialize)]
struct LogicalDm {
    conversation_id: [u8; 32],
    sender: PublicKey,
    recipient: PublicKey,
    sequence: u64,
    message_id: MessageId,
    plaintext: Vec<u8>,
    signature: Vec<u8>,
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
            "PRAGMA journal_mode = WAL;\n             PRAGMA foreign_keys = ON;\n             PRAGMA busy_timeout = 5000;\n             PRAGMA synchronous = NORMAL;",
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

        Ok(storage)
    }

    /// Open an in-memory database (for tests).
    pub fn memory() -> Result<Self> {
        let conn = Connection::open_in_memory().std_context("open in-memory sqlite db")?;
        conn.execute_batch("PRAGMA foreign_keys = ON;\n             PRAGMA synchronous = NORMAL;")
            .std_context("set pragmas")?;
        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        storage.run_migrations()?;
        Ok(storage)
    }

    /// Run `PRAGMA integrity_check` and return a clear error on corruption.
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

    /// Recover state left dangling by a crash.
    ///
    /// 1. **Crash-left Sent outbox** — rows stuck in `Sent` (1) are reset to
    ///    `Pending` so the delivery engine retries them.
    /// 2. **Preserve ACKs** — rows with status `Acked` (2) are never touched.
    /// 3. **Stale Pending timestamps** — rows with `next_attempt_at_ms` in the
    ///    future are reset to now so they become due immediately.
    fn recover_crash_state(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = crate::chat_core::now_ms() as i64;

        // Reset crash-left "Sent" rows back to "Pending".
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

        // Reset stale Pending timestamps to now.
        conn.execute(
            "UPDATE outbox SET
                next_attempt_at_ms = ?1
             WHERE status = ?2 AND next_attempt_at_ms > ?3",
            params![now, crate::store::DeliveryStatus::Pending as u8, now,],
        )
        .std_context("recover stale Pending outbox timestamps")?;

        // Clear leases whose bounded deadline elapsed before restart.
        conn.execute(
            "UPDATE outbox SET lease_owner = NULL, locked_until_ms = NULL
             WHERE locked_until_ms IS NOT NULL AND locked_until_ms <= ?1",
            params![now],
        )
        .std_context("recover stale outbox leases")?;

        Ok(())
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
                6 => self.migrate_v6(&conn)?,
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

            CREATE TABLE dm_conversations (
                conversation_id BLOB PRIMARY KEY,
                peer_id BLOB NOT NULL,
                created_at_ms INTEGER NOT NULL
            );
            CREATE TABLE dm_sender_sequences (
                conversation_id BLOB NOT NULL,
                sender_id BLOB NOT NULL,
                next_sequence INTEGER NOT NULL,
                PRIMARY KEY (conversation_id, sender_id)
            );
            CREATE TABLE dm_messages (
                message_id BLOB PRIMARY KEY,
                conversation_id BLOB NOT NULL,
                sender_id BLOB NOT NULL,
                recipient_id BLOB NOT NULL,
                sequence INTEGER NOT NULL,
                request_key TEXT NOT NULL UNIQUE,
                plaintext BLOB NOT NULL,
                logical_message BLOB NOT NULL,
                created_at_ms INTEGER NOT NULL
            );
            CREATE TABLE dm_outbox (
                message_id BLOB PRIMARY KEY REFERENCES dm_messages(message_id),
                recipient_id BLOB NOT NULL,
                envelope BLOB NOT NULL,
                status INTEGER NOT NULL DEFAULT 0,
                created_at_ms INTEGER NOT NULL
            );
            CREATE UNIQUE INDEX dm_messages_sequence
                ON dm_messages(conversation_id, sender_id, sequence);
            ",
        )
        .std_context("migrate v2")?;
        Ok(())
    }

    /// V3 installs the outgoing direct-message tables for databases that
    /// already completed the original v2 migration.
    fn migrate_v3(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS dm_conversations (
                conversation_id BLOB PRIMARY KEY, peer_id BLOB NOT NULL,
                created_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS dm_sender_sequences (
                conversation_id BLOB NOT NULL, sender_id BLOB NOT NULL,
                next_sequence INTEGER NOT NULL,
                PRIMARY KEY (conversation_id, sender_id)
            );
            CREATE TABLE IF NOT EXISTS dm_messages (
                message_id BLOB PRIMARY KEY, conversation_id BLOB NOT NULL,
                sender_id BLOB NOT NULL, recipient_id BLOB NOT NULL,
                sequence INTEGER NOT NULL, request_key TEXT NOT NULL UNIQUE,
                plaintext BLOB NOT NULL, logical_message BLOB NOT NULL,
                created_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS dm_outbox (
                message_id BLOB PRIMARY KEY REFERENCES dm_messages(message_id),
                recipient_id BLOB NOT NULL, envelope BLOB NOT NULL,
                status INTEGER NOT NULL DEFAULT 0, created_at_ms INTEGER NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS dm_messages_sequence
                ON dm_messages(conversation_id, sender_id, sequence);",
        )
        .std_context("migrate v3")?;
        Ok(())
    }

    /// V4 adds durable worker leases to the message-delivery outbox.
    fn migrate_v4(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "ALTER TABLE outbox ADD COLUMN lease_owner TEXT;
             ALTER TABLE outbox ADD COLUMN locked_until_ms INTEGER;
             ALTER TABLE outbox ADD COLUMN expires_at_ms INTEGER;
             CREATE INDEX IF NOT EXISTS idx_outbox_next_attempt
                 ON outbox(next_attempt_at_ms);",
        )
        .std_context("migrate v4 outbox leases")?;
        Ok(())
    }

    /// V5 adds durable, idempotent acknowledgement records and a message
    /// acknowledgement timestamp.
    fn migrate_v5(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "ALTER TABLE dm_messages ADD COLUMN acknowledged_at_ms INTEGER;
             CREATE TABLE dm_acknowledgements (
                 message_id BLOB PRIMARY KEY,
                 original_sender_id BLOB NOT NULL,
                 recipient_id BLOB NOT NULL,
                 acknowledged_at_ms INTEGER NOT NULL,
                 status TEXT,
                 signature BLOB NOT NULL
             );",
        )
        .std_context("migrate v5 acknowledgements")?;
        Ok(())
    }

    /// V6 adds sync dedup tracking to prevent duplicate envelope delivery
    /// during repeat sync requests.  Every message id served via SyncResponse
    /// is recorded in sync_dedup.  The query_pending_outbound_for_recipient
    /// method filters out already-served ids so that subsequent sync requests
    /// from the same peer only receive newly-pending envelopes.
    fn migrate_v6(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sync_dedup (
                message_id BLOB NOT NULL,
                recipient_id BLOB NOT NULL,
                served_at_ms INTEGER NOT NULL,
                PRIMARY KEY (message_id, recipient_id)
            );
            CREATE INDEX idx_sync_dedup_recipient
                ON sync_dedup(recipient_id);",
        )
        .std_context("migrate v6 sync dedup")?;
        Ok(())
    }

    /// Atomically create and queue an outgoing direct message.
    pub fn queue_outgoing_dm(
        &self,
        conversation_id: [u8; 32],
        sender: PublicKey,
        request_key: &str,
        plaintext: &str,
        recipient: MailboxPublicKey,
        sender_secret: &SecretKey,
    ) -> Result<OutgoingDm> {
        self.queue_outgoing_dm_inner(
            conversation_id,
            sender,
            request_key,
            plaintext,
            recipient,
            sender_secret,
            None,
        )
    }

    /// Queue an outgoing DM while injecting a deterministic failure.
    pub fn queue_outgoing_dm_with_fault(
        &self,
        conversation_id: [u8; 32],
        sender: PublicKey,
        request_key: &str,
        plaintext: &str,
        recipient: MailboxPublicKey,
        sender_secret: &SecretKey,
        fault: OutgoingDmFault,
    ) -> Result<OutgoingDm> {
        self.queue_outgoing_dm_inner(
            conversation_id,
            sender,
            request_key,
            plaintext,
            recipient,
            sender_secret,
            Some(fault),
        )
    }

    fn queue_outgoing_dm_inner(
        &self,
        conversation_id: [u8; 32],
        sender: PublicKey,
        request_key: &str,
        plaintext: &str,
        recipient: MailboxPublicKey,
        sender_secret: &SecretKey,
        fault: Option<OutgoingDmFault>,
    ) -> Result<OutgoingDm> {
        if sender != sender_secret.public() {
            return Err(anyhow!("sender does not match sender secret key").into());
        }
        if request_key.is_empty() {
            return Err(anyhow!("request key must not be empty").into());
        }
        let plaintext = plaintext.as_bytes().to_vec();
        let message_id = *blake3::hash(
            &[
                b"boru-chat/dm/request/v1".as_slice(),
                sender.as_bytes(),
                &conversation_id,
                request_key.as_bytes(),
            ]
            .concat(),
        )
        .as_bytes();
        let recipient_id = recipient.identity;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .std_context("begin outgoing dm transaction")?;
        if let Some((
            stored_id,
            stored_conversation,
            stored_sender,
            stored_recipient,
            stored_plaintext,
            stored_logical,
            stored_envelope,
        )) = tx
            .query_row(
                "SELECT m.message_id, m.conversation_id, m.sender_id, m.recipient_id,
                    m.plaintext, m.logical_message, o.envelope
             FROM dm_messages m JOIN dm_outbox o USING (message_id)
             WHERE m.request_key = ?1",
                [request_key],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                        row.get::<_, Vec<u8>>(6)?,
                    ))
                },
            )
            .optional()
            .std_context("look up outgoing dm idempotency key")?
        {
            if stored_plaintext != plaintext
                || stored_id.as_slice() != message_id
                || stored_conversation.as_slice() != conversation_id
                || stored_sender.as_slice() != sender.as_bytes()
                || stored_recipient.as_slice() != recipient_id.as_bytes()
            {
                return Err(anyhow!("idempotency key is already bound to another message").into());
            }
            let mut id = [0; 32];
            id.copy_from_slice(&stored_id);
            let envelope: MailboxEnvelope = postcard::from_bytes(&stored_envelope)
                .std_context("decode stored mailbox envelope")?;
            let sequence = postcard::from_bytes::<LogicalDm>(&stored_logical)
                .std_context("decode stored logical message")?
                .sequence;
            tx.commit().std_context("commit idempotent outgoing dm")?;
            return Ok(OutgoingDm {
                message_id: id,
                sequence,
                logical_message: stored_logical.to_vec(),
                envelope,
            });
        }
        let sequence = tx.query_row("SELECT next_sequence FROM dm_sender_sequences WHERE conversation_id = ?1 AND sender_id = ?2", params![conversation_id.as_slice(), sender.as_bytes()], |row| row.get::<_, i64>(0)).optional().std_context("read outgoing dm sequence")?.unwrap_or(1) as u64;
        let unsigned = postcard::to_stdvec(&(
            conversation_id,
            sender,
            recipient_id,
            sequence,
            message_id,
            &plaintext,
        ))
        .std_context("encode logical dm")?;
        let logical = LogicalDm {
            conversation_id,
            sender,
            recipient: recipient_id,
            sequence,
            message_id,
            plaintext: plaintext.clone(),
            signature: sender_secret.sign(&unsigned).to_bytes().to_vec(),
        };
        let logical_message =
            postcard::to_stdvec(&logical).std_context("encode signed logical dm")?;
        if fault == Some(OutgoingDmFault::Encryption) {
            return Err(anyhow!("injected mailbox encryption failure").into());
        }
        let envelope = seal_for(sender_secret, recipient, &logical_message)?;
        let envelope_bytes =
            postcard::to_stdvec(&envelope).std_context("encode mailbox envelope")?;
        let now = now_ms() as i64;
        tx.execute("INSERT OR IGNORE INTO dm_conversations (conversation_id, peer_id, created_at_ms) VALUES (?1, ?2, ?3)", params![conversation_id.as_slice(), recipient_id.as_bytes(), now]).std_context("create dm conversation")?;
        tx.execute("INSERT INTO dm_sender_sequences (conversation_id, sender_id, next_sequence) VALUES (?1, ?2, ?3) ON CONFLICT(conversation_id, sender_id) DO UPDATE SET next_sequence = excluded.next_sequence", params![conversation_id.as_slice(), sender.as_bytes(), (sequence + 1) as i64]).std_context("advance dm sender sequence")?;
        tx.execute("INSERT INTO dm_messages (message_id, conversation_id, sender_id, recipient_id, sequence, request_key, plaintext, logical_message, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)", params![message_id.as_slice(), conversation_id.as_slice(), sender.as_bytes(), recipient_id.as_bytes(), sequence as i64, request_key, &plaintext, &logical_message, now]).std_context("insert visible dm message")?;
        tx.execute("INSERT INTO dm_outbox (message_id, recipient_id, envelope, created_at_ms) VALUES (?1, ?2, ?3, ?4)", params![message_id.as_slice(), recipient_id.as_bytes(), &envelope_bytes, now]).std_context("insert dm outbox envelope")?;
        if fault == Some(OutgoingDmFault::Database) {
            return Err(anyhow!("injected database failure").into());
        }
        tx.commit().std_context("commit outgoing dm transaction")?;
        Ok(OutgoingDm {
            message_id,
            sequence,
            logical_message,
            envelope,
        })
    }

    #[allow(missing_docs)]
    pub fn next_dm_sequence(&self, conversation_id: [u8; 32], sender: PublicKey) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT next_sequence FROM dm_sender_sequences WHERE conversation_id = ?1 AND sender_id = ?2", params![conversation_id.as_slice(), sender.as_bytes()], |row| row.get::<_, i64>(0)).optional().std_context("read next dm sequence")?.unwrap_or(1) as u64)
    }

    #[allow(missing_docs)]
    pub fn get_dm_message(&self, message_id: &MessageId) -> Result<Option<DmMessageRow>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT conversation_id, sender_id, recipient_id, sequence, request_key, plaintext, created_at_ms FROM dm_messages WHERE message_id = ?1", [message_id.as_slice()], |row| {
            let c: Vec<u8> = row.get(0)?; let mut conversation_id = [0; 32]; conversation_id.copy_from_slice(&c);
            let sender_bytes: Vec<u8> = row.get(1)?; let recipient_bytes: Vec<u8> = row.get(2)?;
            Ok(DmMessageRow { message_id: *message_id, conversation_id, sender: PublicKey::try_from(sender_bytes.as_slice()).map_err(|_| rusqlite::Error::InvalidQuery)?, recipient: PublicKey::try_from(recipient_bytes.as_slice()).map_err(|_| rusqlite::Error::InvalidQuery)?, sequence: row.get::<_, i64>(3)? as u64, request_key: row.get(4)?, plaintext: row.get(5)?, created_at_ms: row.get::<_, i64>(6)? as u64 })
        }).optional().std_context("get dm message")
    }

    /// List direct-message history using a clock-independent, deterministic
    /// order. A sender's persistent sequence is the primary key; sender and
    /// message id are stable tie-breakers for messages from different senders.
    ///
    /// `offset`/`limit` pagination is stable because this order never uses the
    /// local insertion time or a remote timestamp. Retries therefore remain a
    /// single row and cannot move an existing message in history.
    pub fn list_dm_messages(
        &self,
        conversation_id: [u8; 32],
        offset: u32,
        limit: Option<u32>,
    ) -> Result<Vec<DmMessageRow>> {
        let conn = self.conn.lock().unwrap();
        let pagination = match limit {
            Some(n) => format!(" LIMIT {n} OFFSET {offset}"),
            None => format!(" LIMIT -1 OFFSET {offset}"),
        };
        let sql = format!(
            "SELECT message_id, sender_id, recipient_id, sequence, request_key,
                    plaintext, created_at_ms
             FROM dm_messages
             WHERE conversation_id = ?1
             ORDER BY sequence ASC, sender_id ASC, message_id ASC{}",
            pagination
        );
        let mut stmt = conn.prepare(&sql).std_context("prepare list dm messages")?;
        let mut rows = stmt
            .query([conversation_id.as_slice()])
            .std_context("query dm messages")?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().std_context("next dm message")? {
            let message_id: Vec<u8> = row.get(0).map_err(|e| anyhow!(e))?;
            let sender_bytes: Vec<u8> = row.get(1).map_err(|e| anyhow!(e))?;
            let recipient_bytes: Vec<u8> = row.get(2).map_err(|e| anyhow!(e))?;
            let conversation_id_bytes = conversation_id;
            result.push(DmMessageRow {
                message_id: message_id
                    .try_into()
                    .map_err(|_| anyhow!("invalid stored dm message id"))?,
                conversation_id: conversation_id_bytes,
                sender: PublicKey::try_from(sender_bytes.as_slice())
                    .map_err(|_| anyhow!("invalid stored dm sender"))?,
                recipient: PublicKey::try_from(recipient_bytes.as_slice())
                    .map_err(|_| anyhow!("invalid stored dm recipient"))?,
                sequence: row.get::<_, i64>(3).map_err(|e| anyhow!(e))? as u64,
                request_key: row.get(4).map_err(|e| anyhow!(e))?,
                plaintext: row.get(5).map_err(|e| anyhow!(e))?,
                created_at_ms: row.get::<_, i64>(6).map_err(|e| anyhow!(e))? as u64,
            });
        }
        Ok(result)
    }

    #[allow(missing_docs)]
    pub fn get_dm_outbox(&self, message_id: &MessageId) -> Result<Option<DmOutboxRow>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT recipient_id, envelope FROM dm_outbox WHERE message_id = ?1",
            [message_id.as_slice()],
            |row| {
                let recipient_bytes: Vec<u8> = row.get(0)?;
                let envelope_bytes: Vec<u8> = row.get(1)?;
                Ok(DmOutboxRow {
                    message_id: *message_id,
                    recipient: PublicKey::try_from(recipient_bytes.as_slice())
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    envelope: postcard::from_bytes(&envelope_bytes)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                })
            },
        )
        .optional()
        .std_context("get dm outbox")
    }

    /// Process an acknowledgement from a recipient without exposing any
    /// partially-applied state.  The acknowledgement id is the stable
    /// mailbox-envelope id (not the logical DM id).
    pub fn process_outgoing_ack(&self, from: PublicKey, ack: &MailboxAck) -> Result<bool> {
        self.process_outgoing_ack_inner(from, ack, None)
    }

    /// Test-only fault injection point for acknowledgement transaction tests.
    pub fn process_outgoing_ack_with_fault(
        &self,
        from: PublicKey,
        ack: &MailboxAck,
        fault: AckProcessingFault,
    ) -> Result<bool> {
        self.process_outgoing_ack_inner(from, ack, Some(fault))
    }

    fn process_outgoing_ack_inner(
        &self,
        from: PublicKey,
        ack: &MailboxAck,
        fault: Option<AckProcessingFault>,
    ) -> Result<bool> {
        const MAX_ACK_MESSAGE_ID_LEN: usize = 128;
        if ack.message_id.len() > MAX_ACK_MESSAGE_ID_LEN {
            return Err(anyhow!("acknowledgement message id is too long").into());
        }
        // Verify the signed contract before taking the database lock.
        ack.verify(from)?;
        let id_bytes = hex::decode(&ack.message_id)
            .map_err(|e| anyhow!("invalid acknowledgement message id: {e}"))?;
        if id_bytes.len() != 32 {
            return Err(anyhow!("acknowledgement message id must be 32 bytes").into());
        }

        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .std_context("begin acknowledgement transaction")?;

        // Duplicate valid acknowledgements are harmless, including after the
        // sender has already removed its outbox row.
        let already_recorded: bool = tx
            .query_row(
                "SELECT 1 FROM dm_acknowledgements WHERE message_id = ?1",
                [id_bytes.as_slice()],
                |_| Ok(true),
            )
            .optional()
            .std_context("look up acknowledgement")?
            .unwrap_or(false);
        if already_recorded {
            tx.commit()
                .std_context("commit duplicate acknowledgement")?;
            return Ok(false);
        }

        // Find the outbox row by the mailbox envelope's stable id.  This
        // prevents an acknowledgement for a different envelope from being
        // attached to a merely similar logical message.
        let mut stmt = tx
            .prepare(
                "SELECT m.message_id, m.sender_id, m.recipient_id, o.recipient_id,
                        o.envelope
                 FROM dm_messages m
                 JOIN dm_outbox o ON o.message_id = m.message_id
                 WHERE m.acknowledged_at_ms IS NULL",
            )
            .std_context("prepare acknowledgement message lookup")?;
        let mut rows = stmt
            .query([])
            .std_context("query acknowledgement message lookup")?;
        let mut matched: Option<(MessageId, Vec<u8>, Vec<u8>, Vec<u8>)> = None;
        while let Some(row) = rows.next().std_context("next acknowledgement row")? {
            let logical_id: Vec<u8> = row
                .get(0)
                .std_context("get stored acknowledgement message id")?;
            let envelope_bytes: Vec<u8> = row
                .get(4)
                .std_context("get stored acknowledgement envelope")?;
            let envelope: MailboxEnvelope = postcard::from_bytes(&envelope_bytes)
                .std_context("decode stored acknowledgement envelope")?;
            if envelope.message_id().as_bytes() == ack.message_id.as_bytes() {
                let stored_sender: Vec<u8> =
                    row.get(1).std_context("get acknowledgement sender")?;
                let stored_recipient: Vec<u8> =
                    row.get(2).std_context("get acknowledgement recipient")?;
                let outbox_recipient: Vec<u8> = row.get(3).std_context("get outbox recipient")?;
                matched = Some((
                    logical_id
                        .try_into()
                        .map_err(|_| anyhow!("invalid stored message id"))?,
                    stored_sender,
                    stored_recipient,
                    outbox_recipient,
                ));
                break;
            }
        }
        drop(rows);
        drop(stmt);
        let Some((logical_id, sender_id, message_recipient, outbox_recipient)) = matched else {
            return Err(anyhow!("acknowledgement refers to an unknown message").into());
        };
        if sender_id.as_slice() != ack.original_sender.as_bytes() {
            return Err(anyhow!("acknowledgement original sender mismatch").into());
        }
        if message_recipient != outbox_recipient || message_recipient.as_slice() != from.as_bytes()
        {
            return Err(anyhow!("acknowledgement recipient mismatch").into());
        }

        let acked_at = ack.acknowledged_at_ms as i64;
        tx.execute(
            "INSERT INTO dm_acknowledgements
             (message_id, original_sender_id, recipient_id, acknowledged_at_ms, status, signature)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id_bytes.as_slice(),
                ack.original_sender.as_bytes(),
                ack.recipient.as_bytes(),
                acked_at,
                ack.status.as_deref(),
                ack.signature.as_slice(),
            ],
        )
        .std_context("insert acknowledgement")?;
        tx.execute(
            "UPDATE dm_messages SET acknowledged_at_ms = ?1 WHERE message_id = ?2",
            params![acked_at, &logical_id],
        )
        .std_context("mark message acknowledged")?;
        tx.execute("DELETE FROM dm_outbox WHERE message_id = ?1", [&logical_id])
            .std_context("remove acknowledged outbox entry")?;
        if fault == Some(AckProcessingFault::Database) {
            return Err(anyhow!("injected acknowledgement database failure").into());
        }
        tx.commit()
            .std_context("commit acknowledgement transaction")?;
        Ok(true)
    }

    /// Return whether a logical DM has been acknowledged.
    pub fn dm_acknowledged(&self, message_id: &MessageId) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT acknowledged_at_ms IS NOT NULL FROM dm_messages WHERE message_id = ?1",
                [message_id.as_slice()],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .std_context("check dm acknowledgement")?
            .unwrap_or(false))
    }

    /// Query pending outbound envelopes addressed to a specific recipient,
    /// bounded by count and total encoded size, ordered by creation time.
    ///
    /// Returns (envelopes, has_more). When the returned page is empty, has_more
    /// is always false. The caller uses the last envelope's `created_at` as a
    /// continuation cursor for the next page.
    ///
    /// Validation and replay protection:
    /// - Expired envelopes (older than DEFAULT_MAILBOX_TTL) are excluded
    /// - Already-served message IDs (via record_sync_served) are excluded
    /// - The requester-supplied since_ms is used for cursor-based pagination
    pub fn query_pending_outbound_for_recipient(
        &self,
        recipient: &PublicKey,
        since_ms: u64,
        max_count: usize,
        max_bytes: usize,
    ) -> Result<(Vec<MailboxEnvelope>, bool)> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms();
        let ttl_ms = crate::mailbox::DEFAULT_MAILBOX_TTL.as_millis() as u64;
        let expiry_cutoff = now.saturating_sub(ttl_ms);
        let effective_since = since_ms.max(expiry_cutoff);
        let mut stmt = conn
            .prepare(
                "SELECT o.message_id, o.envelope, o.created_at_ms
                 FROM dm_outbox o
                 WHERE o.recipient_id = ?1
                   AND o.created_at_ms >= ?2
                   AND o.created_at_ms >= ?4
                   AND NOT EXISTS (
                       SELECT 1 FROM sync_dedup d
                       WHERE d.message_id = o.message_id
                         AND d.recipient_id = o.recipient_id
                   )
                 ORDER BY o.created_at_ms ASC, o.message_id ASC
                 LIMIT ?3",
            )
            .std_context("prepare query_pending_outbound_for_recipient")?;
        // Query max_count + 1 to detect has_more
        let limit = (max_count + 1) as i64;
        let mut rows = stmt
            .query(params![recipient.as_bytes(), effective_since as i64, limit, expiry_cutoff as i64])
            .std_context("query pending outbound")?;

        let mut envelopes = Vec::with_capacity(max_count);
        let mut total_bytes = 0usize;
        let mut has_extra = false;

        while let Some(row) = rows.next().std_context("next outbound row")? {
            let message_id_blob: Vec<u8> = row
                .get(0)
                .std_context("get message_id")?;
            let envelope_bytes: Vec<u8> = row
                .get(1)
                .std_context("get envelope bytes")?;
            let _created_at_ms: i64 = row
                .get(2)
                .std_context("get created_at_ms")?;

            // If we already have a full page, just note there's an extra row
            if envelopes.len() >= max_count {
                has_extra = true;
                continue;
            }

            let envelope: MailboxEnvelope = postcard::from_bytes(&envelope_bytes)
                .std_context("decode envelope")?;
            let encoded_size = envelope_bytes.len();

            // Check size bound
            if total_bytes.saturating_add(encoded_size) > max_bytes && !envelopes.is_empty() {
                has_extra = true;
                continue;
            }

            total_bytes += encoded_size;
            envelopes.push(envelope);
        }

        let has_more = has_extra;
        Ok((envelopes, has_more))
    }

    /// Record that a set of message IDs were served via SyncResponse to a
    /// specific recipient.  Subsequent sync requests from the same recipient
    /// will exclude these envelopes, providing replay protection.
    pub fn record_sync_served(
        &self,
        recipient: &PublicKey,
        message_ids: &[[u8; 32]],
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        for msg_id in message_ids {
            conn.execute(
                "INSERT OR IGNORE INTO sync_dedup (message_id, recipient_id, served_at_ms)
                 VALUES (?1, ?2, ?3)",
                params![msg_id.as_slice(), recipient.as_bytes(), now],
            )
            .std_context("insert sync_dedup")?;
        }
        Ok(())
    }

    /// Remove sync dedup entries older than the retention window.  Call this
    /// periodically or during startup to keep the sync_dedup table from growing
    /// unboundedly as old envelopes expire and are naturally excluded.
    pub fn prune_sync_dedup(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let ttl_ms = crate::mailbox::DEFAULT_MAILBOX_TTL.as_millis() as u64;
        let cutoff = (now_ms() as i64).saturating_sub(ttl_ms as i64);
        conn.execute(
            "DELETE FROM sync_dedup WHERE served_at_ms < ?1",
            params![cutoff],
        )
        .std_context("prune sync_dedup")?;
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
            let mut msg_id = [0u8; 32];
            msg_id.copy_from_slice(&msg_id_blob);
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
            "UPDATE outbox SET status = ?1, lease_owner = NULL, locked_until_ms = NULL
             WHERE msg_id = ?2 AND recipient_device_id = ?3",
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
                        next_attempt_at_ms, last_error_code, last_attempt_at_ms,
                        lease_owner, locked_until_ms, expires_at_ms
                 FROM outbox
                 WHERE status != ?1 AND status != ?2 AND next_attempt_at_ms <= ?3
                   AND (locked_until_ms IS NULL OR locked_until_ms <= ?3)
                 ORDER BY next_attempt_at_ms, rowid
                 LIMIT ?4",
            )
            .std_context("prepare fetch_due_outbox")?;
        let mut rows = stmt
            .query(params![
                DeliveryStatus::Acked as u8,
                DeliveryStatus::Expired as u8,
                now_ms as i64,
                MAX_OUTBOX_CLAIM_LIMIT as i64,
            ])
            .std_context("query due outbox")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(row_to_outbox(row)?);
        }
        Ok(results)
    }

    /// Atomically claim the oldest due outbox row for a worker.
    ///
    /// The claim transaction is deliberately short: no network activity may
    /// occur while the SQLite write lock is held. Expired leases are eligible
    /// for recovery, and a bounded limit prevents an untrusted queue from
    /// producing an unbounded query.
    pub fn claim_due_outbox(
        &self,
        now_ms: u64,
        lease_owner: &str,
        lease_duration_ms: u64,
        limit: u32,
    ) -> Result<Option<OutboxRow>> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .std_context("begin outbox claim")?;
        let limit = limit.clamp(1, MAX_OUTBOX_CLAIM_LIMIT) as i64;
        let candidate: Option<(MessageId, Vec<u8>)> = tx
            .query_row(
                "SELECT msg_id, recipient_device_id FROM outbox
                 WHERE status != ?1 AND status != ?2 AND next_attempt_at_ms <= ?3
                   AND (locked_until_ms IS NULL OR locked_until_ms <= ?3)
                 ORDER BY next_attempt_at_ms, rowid LIMIT ?4",
                params![
                    DeliveryStatus::Acked as u8,
                    DeliveryStatus::Expired as u8,
                    now_ms as i64,
                    limit
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .std_context("select outbox claim candidate")?;
        let Some((msg_blob, recipient_blob)) = candidate else {
            tx.commit().std_context("commit empty outbox claim")?;
            return Ok(None);
        };
        let locked_until = now_ms.saturating_add(lease_duration_ms);
        let changed = tx
            .execute(
                "UPDATE outbox SET lease_owner = ?1, locked_until_ms = ?2
                 WHERE msg_id = ?3 AND recipient_device_id = ?4
                   AND (locked_until_ms IS NULL OR locked_until_ms <= ?5)",
                params![
                    lease_owner,
                    locked_until as i64,
                    &msg_blob,
                    &recipient_blob,
                    now_ms as i64
                ],
            )
            .std_context("claim outbox row")?;
        if changed != 1 {
            tx.rollback().std_context("rollback lost outbox claim")?;
            return Ok(None);
        }
        let mut stmt = tx
            .prepare(
                "SELECT msg_id, recipient_device_id, status, attempts,
                        next_attempt_at_ms, last_error_code, last_attempt_at_ms,
                        lease_owner, locked_until_ms, expires_at_ms
                 FROM outbox WHERE msg_id = ?1 AND recipient_device_id = ?2",
            )
            .std_context("prepare claimed outbox row")?;
        let mut rows = stmt
            .query(params![&msg_blob, &recipient_blob])
            .std_context("query claimed outbox row")?;
        let row_ref = rows
            .next()
            .std_context("next claimed outbox row")?
            .ok_or_else(|| anyhow!("claimed outbox row disappeared"))?;
        let row = row_to_outbox(row_ref)?;
        drop(rows);
        drop(stmt);
        tx.commit().std_context("commit outbox claim")?;
        Ok(Some(row))
    }

    /// Atomically claim the oldest due row addressed to one peer.
    pub fn claim_due_outbox_for_peer(
        &self,
        now_ms: u64,
        recipient_device_id: iroh::PublicKey,
        lease_owner: &str,
        lease_duration_ms: u64,
    ) -> Result<Option<OutboxRow>> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .std_context("begin peer outbox claim")?;
        let recipient = recipient_device_id.as_bytes();
        let candidate: Option<MessageId> = tx
            .query_row(
                "SELECT msg_id FROM outbox
             WHERE recipient_device_id = ?1 AND status != ?2 AND status != ?3
               AND next_attempt_at_ms <= ?4
               AND (locked_until_ms IS NULL OR locked_until_ms <= ?4)
             ORDER BY next_attempt_at_ms, rowid LIMIT 1",
                params![
                    recipient,
                    DeliveryStatus::Acked as u8,
                    DeliveryStatus::Expired as u8,
                    now_ms as i64
                ],
                |row| row.get(0),
            )
            .optional()
            .std_context("select peer outbox claim candidate")?;
        let Some(msg_id) = candidate else {
            tx.commit().std_context("commit empty peer outbox claim")?;
            return Ok(None);
        };
        let locked_until = now_ms.saturating_add(lease_duration_ms);
        let changed = tx
            .execute(
                "UPDATE outbox SET lease_owner = ?1, locked_until_ms = ?2
             WHERE msg_id = ?3 AND recipient_device_id = ?4
               AND (locked_until_ms IS NULL OR locked_until_ms <= ?5)",
                params![
                    lease_owner,
                    locked_until as i64,
                    msg_id.as_slice(),
                    recipient,
                    now_ms as i64
                ],
            )
            .std_context("claim peer outbox row")?;
        if changed != 1 {
            tx.rollback()
                .std_context("rollback lost peer outbox claim")?;
            return Ok(None);
        }
        let mut stmt = tx
            .prepare(
                "SELECT msg_id, recipient_device_id, status, attempts,
                    next_attempt_at_ms, last_error_code, last_attempt_at_ms,
                    lease_owner, locked_until_ms, expires_at_ms
             FROM outbox WHERE msg_id = ?1 AND recipient_device_id = ?2",
            )
            .std_context("prepare claimed peer outbox row")?;
        let mut rows = stmt
            .query(params![msg_id.as_slice(), recipient])
            .std_context("query claimed peer outbox row")?;
        let row_ref = rows
            .next()
            .std_context("next claimed peer outbox row")?
            .ok_or_else(|| anyhow!("claimed peer outbox row disappeared"))?;
        let row = row_to_outbox(row_ref)?;
        drop(rows);
        drop(stmt);
        tx.commit().std_context("commit peer outbox claim")?;
        Ok(Some(row))
    }

    /// Finish a claimed attempt and release its lease.
    pub fn finish_outbox_attempt(
        &self,
        msg_id: &MessageId,
        recipient_device_id: iroh::PublicKey,
        lease_owner: &str,
        success: bool,
        next_attempt_at_ms: u64,
        error_code: Option<&str>,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let status = if success {
            DeliveryStatus::Sent
        } else {
            DeliveryStatus::Pending
        };
        let changed = conn
            .execute(
                "UPDATE outbox SET attempts = attempts + 1, last_attempt_at_ms = ?1,
                        next_attempt_at_ms = ?2, last_error_code = ?3, status = ?4,
                        lease_owner = NULL, locked_until_ms = NULL
                 WHERE msg_id = ?5 AND recipient_device_id = ?6
                   AND lease_owner = ?7 AND status != ?8",
                params![
                    now_ms() as i64,
                    next_attempt_at_ms as i64,
                    error_code,
                    status as u8,
                    msg_id.as_slice(),
                    recipient_device_id.as_bytes(),
                    lease_owner,
                    DeliveryStatus::Acked as u8
                ],
            )
            .std_context("finish outbox attempt")?;
        Ok(changed == 1)
    }

    /// Extend a lease without opening a transaction during network activity.
    ///
    /// The caller supplies the new absolute deadline. Only the current owner
    /// may extend a live lease; an expired lease cannot be resurrected by its
    /// former owner and must be reclaimed first.
    pub fn extend_outbox_lease(
        &self,
        msg_id: &MessageId,
        recipient_device_id: iroh::PublicKey,
        lease_owner: &str,
        now_ms: u64,
        locked_until_ms: u64,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "UPDATE outbox SET locked_until_ms = ?1
                 WHERE msg_id = ?2 AND recipient_device_id = ?3
                   AND lease_owner = ?4 AND locked_until_ms > ?5",
                params![
                    locked_until_ms as i64,
                    msg_id.as_slice(),
                    recipient_device_id.as_bytes(),
                    lease_owner,
                    now_ms as i64
                ],
            )
            .std_context("extend outbox lease")?;
        Ok(changed == 1)
    }

    /// Release a lease without recording an attempt (for cancellation).
    pub fn release_outbox_lease(
        &self,
        msg_id: &MessageId,
        recipient_device_id: iroh::PublicKey,
        lease_owner: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "UPDATE outbox SET lease_owner = NULL, locked_until_ms = NULL
             WHERE msg_id = ?1 AND recipient_device_id = ?2 AND lease_owner = ?3",
                params![
                    msg_id.as_slice(),
                    recipient_device_id.as_bytes(),
                    lease_owner
                ],
            )
            .std_context("release outbox lease")?;
        Ok(changed == 1)
    }

    /// Expire leases whose deadlines have passed, making them immediately due.
    pub fn recover_stale_outbox_leases(&self, now_ms: u64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "UPDATE outbox SET lease_owner = NULL, locked_until_ms = NULL
             WHERE locked_until_ms IS NOT NULL AND locked_until_ms <= ?1
               AND status != ?2 AND status != ?3",
                params![
                    now_ms as i64,
                    DeliveryStatus::Acked as u8,
                    DeliveryStatus::Expired as u8
                ],
            )
            .std_context("recover stale outbox leases")?;
        Ok(changed)
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

    /// Get a single sync cursor for a specific peer.
    pub fn get_sync_cursor(&self, peer_device_id: &iroh::PublicKey) -> Result<Option<SyncCursorRow>> {
        let conn = self.conn.lock().unwrap();
        let result = conn
            .query_row(
                "SELECT peer_device_id, last_seen_msg_clock, last_sync_at_ms FROM sync_cursor WHERE peer_device_id = ?1",
                [peer_device_id.as_bytes()],
                |row| {
                    Ok(SyncCursorRow {
                        peer_device_id: row.get(0)?,
                        last_seen_msg_clock: row.get(1)?,
                        last_sync_at_ms: row.get::<_, i64>(2)? as u64,
                    })
                },
            )
            .optional()
            .std_context("get sync_cursor for peer")?;
        Ok(result)
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

    // ── Downloads (v2) ────────────────────────────────────────────────

    /// Create a download entry (queued state).
    pub fn create_download(
        &self,
        content_hash: &str,
        remote_peer: &str,
        total_bytes: u64,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "INSERT INTO downloads
                (content_hash, remote_peer, state, bytes_downloaded, total_bytes,
                 created_at_ms, updated_at_ms)
             VALUES (?1, ?2, 'queued', 0, ?3, ?4, ?4)",
            params![content_hash, remote_peer, total_bytes as i64, now],
        )
        .std_context("create download")?;
        Ok(conn.last_insert_rowid())
    }

    /// Update download progress.
    pub fn update_download_progress(
        &self,
        id: i64,
        bytes_downloaded: u64,
        state: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "UPDATE downloads SET bytes_downloaded = ?1, state = ?2, updated_at_ms = ?3
             WHERE id = ?4",
            params![bytes_downloaded as i64, state, now, id],
        )
        .std_context("update download progress")?;
        Ok(())
    }

    /// Mark a download as failed with an error message.
    pub fn fail_download(&self, id: i64, error: &str, next_retry_at_ms: Option<u64>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
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
        let mut stmt = conn
            .prepare(
                "SELECT id, content_hash, remote_peer, state, bytes_downloaded,
                        total_bytes, created_at_ms, updated_at_ms, last_error,
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

    /// List downloads in a given state.
    pub fn list_downloads_by_state(&self, state: &str) -> Result<Vec<Download>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, content_hash, remote_peer, state, bytes_downloaded,
                        total_bytes, created_at_ms, updated_at_ms, last_error,
                        retry_count, next_retry_at_ms
                 FROM downloads WHERE state = ?1
                 ORDER BY created_at_ms ASC",
            )
            .std_context("prepare list_downloads_by_state")?;
        let mut rows = stmt.query(params![state]).std_context("query downloads")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(row_to_download(row)?);
        }
        Ok(results)
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
                let mut msg_id = [0u8; 32];
                msg_id.copy_from_slice(&msg_id_blob);
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
}

// ── Helpers ───────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn row_to_envelope_bare(msg_id: &MessageId, row: &rusqlite::Row) -> Result<StoredEnvelope> {
    let mut conversation_id = [0u8; 32];
    let conv_blob: Vec<u8> = row.get(1).std_context("get conversation_id")?;
    conversation_id.copy_from_slice(&conv_blob);

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
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&signature_blob);
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
    let mut conversation_id = [0u8; 32];
    let conv_blob: Vec<u8> = row.get(0).std_context("get conversation_id")?;
    conversation_id.copy_from_slice(&conv_blob);

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
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&signature_blob);
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
    let mut msg_id = [0u8; 32];
    msg_id.copy_from_slice(&msg_blob);

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
        lease_owner: row.get(7).std_context("get lease_owner")?,
        locked_until_ms: row
            .get::<_, Option<i64>>(8)
            .std_context("get locked_until")?
            .map(|v| v as u64),
        expires_at_ms: row
            .get::<_, Option<i64>>(9)
            .std_context("get expires_at")?
            .map(|v| v as u64),
    })
}

fn row_to_download(row: &rusqlite::Row) -> Result<Download> {
    Ok(Download {
        id: row.get(0).std_context("get id")?,
        content_hash: row.get(1).std_context("get hash")?,
        remote_peer: row.get(2).std_context("get peer")?,
        state: row.get(3).std_context("get state")?,
        bytes_downloaded: row.get::<_, i64>(4).std_context("get bytes_down")? as u64,
        total_bytes: row.get::<_, i64>(5).std_context("get total_bytes")? as u64,
        created_at_ms: row.get::<_, i64>(6).std_context("get created")? as u64,
        updated_at_ms: row.get::<_, i64>(7).std_context("get updated")? as u64,
        last_error: row.get(8).std_context("get error")?,
        retry_count: row.get::<_, i64>(9).std_context("get retries")? as u32,
        next_retry_at_ms: row
            .get::<_, Option<i64>>(10)
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

    #[test]
    fn v1_get_sync_cursor_by_peer() {
        let storage = Storage::memory().unwrap();
        let peer = iroh::SecretKey::generate().public();
        let other = iroh::SecretKey::generate().public();

        // Returns None for unregistered peer.
        assert!(storage.get_sync_cursor(&peer).unwrap().is_none());

        // Upsert and verify per-peer lookup.
        storage
            .upsert_sync_cursor(&peer, Some(b"clock-1"), 1000)
            .unwrap();
        let cursor = storage.get_sync_cursor(&peer).unwrap().unwrap();
        assert_eq!(cursor.last_sync_at_ms, 1000);
        assert_eq!(cursor.last_seen_msg_clock, Some(b"clock-1".to_vec()));

        // Other peer still returns None.
        assert!(storage.get_sync_cursor(&other).unwrap().is_none());

        // Update and verify.
        storage
            .upsert_sync_cursor(&peer, Some(b"clock-2"), 2000)
            .unwrap();
        let cursor = storage.get_sync_cursor(&peer).unwrap().unwrap();
        assert_eq!(cursor.last_sync_at_ms, 2000);
        assert_eq!(cursor.last_seen_msg_clock, Some(b"clock-2".to_vec()));
    }

    #[test]
    fn v1_query_pending_outbound_for_recipient_pagination() {
        let storage = Storage::memory().unwrap();
        let sender_sk = iroh::SecretKey::generate();
        let sender = sender_sk.public();
        let recipient_sk = iroh::SecretKey::generate();
        let recipient_id = recipient_sk.public();
        let recipient = MailboxPublicKey {
            identity: recipient_id,
            encryption: [0u8; 32],
        };
        let conv_id = [1u8; 32];

        // Insert 5 outbound messages from sender to recipient.
        for i in 0..5u64 {
            let request_key = format!("req-{i}");
            let plaintext = format!("hello {i}");
            storage
                .queue_outgoing_dm(conv_id, sender, &request_key, &plaintext, recipient, &sender_sk)
                .unwrap();
        }

        // Query with max_count=3 should return 3 with has_more=true.
        let (page, has_more) = storage
            .query_pending_outbound_for_recipient(&recipient_id, 0, 3, 10_000_000)
            .unwrap();
        assert_eq!(page.len(), 3);
        assert!(has_more, "should have more pages");

        // Query with max_count=10 should return all 5 with has_more=false.
        let (page, has_more) = storage
            .query_pending_outbound_for_recipient(&recipient_id, 0, 10, 10_000_000)
            .unwrap();
        assert_eq!(page.len(), 5);
        assert!(!has_more, "should not have more pages");

        // Query scoped by recipient: other_recipient gets nothing.
        let other_recipient = iroh::SecretKey::generate().public();
        let (page, has_more) = storage
            .query_pending_outbound_for_recipient(&other_recipient, 0, 10, 10_000_000)
            .unwrap();
        assert_eq!(page.len(), 0);
        assert!(!has_more);

        // Query with since_ms beyond current time.
        let far_future_ms = now_ms() + 86_400_000; // 1 day in the future
        let (page, _) = storage
            .query_pending_outbound_for_recipient(&recipient_id, far_future_ms, 10, 10_000_000)
            .unwrap();
        assert_eq!(page.len(), 0);
    }

    #[test]
    fn v1_sync_dedup_replay_protection() {
        let storage = Storage::memory().unwrap();
        let sender_sk = iroh::SecretKey::generate();
        let sender = sender_sk.public();
        let recipient_sk = iroh::SecretKey::generate();
        let recipient_id = recipient_sk.public();
        let recipient = MailboxPublicKey {
            identity: recipient_id,
            encryption: [0u8; 32],
        };
        let conv_id = [2u8; 32];

        // Insert 3 outbound messages, capturing their raw message_id.
        let mut msg_ids: Vec<[u8; 32]> = Vec::new();
        for i in 0..3u64 {
            let request_key = format!("dedup-req-{i}");
            let plaintext = format!("dedup-test {i}");
            let outgoing = storage
                .queue_outgoing_dm(
                    conv_id, sender, &request_key, &plaintext, recipient, &sender_sk,
                )
                .unwrap();
            msg_ids.push(outgoing.message_id);
        }

        // First call returns all 3.
        let (page, _) = storage
            .query_pending_outbound_for_recipient(&recipient_id, 0, 10, 10_000_000)
            .unwrap();
        assert_eq!(page.len(), 3, "first call should return all 3");

        // Record first 2 message IDs as already served.
        storage.record_sync_served(&recipient_id, &msg_ids[..2]).unwrap();

        // Second call should return only the 3rd envelope.
        let (page2, _) = storage
            .query_pending_outbound_for_recipient(&recipient_id, 0, 10, 10_000_000)
            .unwrap();
        assert_eq!(page2.len(), 1, "second call should skip served envelopes");
        let second_mid = page2[0].message_id();
        let third_mid = page[2].message_id();
        assert_eq!(
            second_mid, third_mid,
            "remaining envelope should be the 3rd"
        );

        // Record the 3rd as served too.
        storage
            .record_sync_served(&recipient_id, &msg_ids[2..])
            .unwrap();

        // Third call should return nothing.
        let (page3, _) = storage
            .query_pending_outbound_for_recipient(&recipient_id, 0, 10, 10_000_000)
            .unwrap();
        assert_eq!(
            page3.len(),
            0,
            "third call should return nothing when all served"
        );

        // Stale dedup pruning should not affect recent entries.
        storage.prune_sync_dedup().unwrap();
        let (page4, _) = storage
            .query_pending_outbound_for_recipient(&recipient_id, 0, 10, 10_000_000)
            .unwrap();
        assert_eq!(
            page4.len(),
            0,
            "after prune, still nothing (entries are fresh)"
        );

        // Other recipient still sees nothing.
        let other_recipient = iroh::SecretKey::generate().public();
        let (page5, _) = storage
            .query_pending_outbound_for_recipient(&other_recipient, 0, 10, 10_000_000)
            .unwrap();
        assert_eq!(page5.len(), 0);
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
        assert!(!storage
            .check_permission("hash3", "bob", "download")
            .unwrap());
        assert!(storage
            .revoke_permission("hash3", "alice", "bob", "read")
            .unwrap());
        assert!(!storage.check_permission("hash3", "bob", "read").unwrap());
    }

    #[test]
    fn v2_downloads_state_machine() {
        let storage = Storage::memory().unwrap();
        // Insert a file_object first to satisfy the FK constraint.
        storage
            .put_file_object("hash4", 1024, "application/octet-stream", "large.bin", b"")
            .unwrap();
        let id = storage.create_download("hash4", "bob_peer", 1024).unwrap();
        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(dl.state, "queued");

        storage.update_download_progress(id, 512, "active").unwrap();
        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(dl.bytes_downloaded, 512);

        storage
            .fail_download(id, "connection reset", Some(5000))
            .unwrap();
        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(dl.state, "failed");
        assert_eq!(dl.last_error.as_deref(), Some("connection reset"));
        assert_eq!(dl.retry_count, 1);
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

    #[test]
    fn outbox_claim_is_exclusive_and_releases_on_failure() {
        let storage = Storage::memory().unwrap();
        let msg_id = [7u8; 32];
        let peer = random_public_key();
        storage.enqueue_outbox(&msg_id, peer, 100).unwrap();
        let claimed = storage
            .claim_due_outbox(100, "worker-a", 1_000, 1)
            .unwrap()
            .unwrap();
        assert_eq!(claimed.lease_owner.as_deref(), Some("worker-a"));
        assert!(storage
            .claim_due_outbox(100, "worker-b", 1_000, 1)
            .unwrap()
            .is_none());
        assert!(storage
            .finish_outbox_attempt(&msg_id, peer, "worker-a", false, 100, Some("reset"))
            .unwrap());
        assert!(storage
            .claim_due_outbox(100, "worker-b", 1_000, 1)
            .unwrap()
            .is_some());
    }

    #[test]
    fn outbox_stale_lease_is_reclaimable() {
        let storage = Storage::memory().unwrap();
        let msg_id = [8u8; 32];
        let peer = random_public_key();
        storage.enqueue_outbox(&msg_id, peer, 100).unwrap();
        storage.claim_due_outbox(100, "dead-worker", 10, 1).unwrap();
        assert!(storage
            .claim_due_outbox(109, "new-worker", 10, 1)
            .unwrap()
            .is_none());
        assert_eq!(storage.recover_stale_outbox_leases(110).unwrap(), 1);
        assert!(storage
            .claim_due_outbox(110, "new-worker", 10, 1)
            .unwrap()
            .is_some());
    }

    #[test]
    fn outbox_lease_can_be_extended_only_by_owner() {
        let storage = Storage::memory().unwrap();
        let msg_id = [10u8; 32];
        let peer = random_public_key();
        storage.enqueue_outbox(&msg_id, peer, 100).unwrap();
        storage.claim_due_outbox(100, "worker-a", 10, 1).unwrap();
        assert!(!storage
            .extend_outbox_lease(&msg_id, peer, "worker-b", 100, 200)
            .unwrap());
        assert!(storage
            .extend_outbox_lease(&msg_id, peer, "worker-a", 100, 300)
            .unwrap());
        assert!(storage
            .claim_due_outbox(299, "worker-b", 10, 1)
            .unwrap()
            .is_none());
        assert!(storage
            .claim_due_outbox(300, "worker-b", 10, 1)
            .unwrap()
            .is_some());
    }

    #[test]
    fn ack_clears_outbox_lease() {
        let storage = Storage::memory().unwrap();
        let msg_id = [11u8; 32];
        let peer = random_public_key();
        storage.enqueue_outbox(&msg_id, peer, 100).unwrap();
        storage.claim_due_outbox(100, "worker-a", 1_000, 1).unwrap();
        storage.mark_acked(&msg_id, peer).unwrap();
        let row = storage.fetch_due_outbox(100).unwrap();
        assert!(row.is_empty());
        let claimed = storage.claim_due_outbox(100, "worker-b", 10, 1).unwrap();
        assert!(claimed.is_none());
    }

    #[test]
    fn outbox_claim_survives_restart_with_lease_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let msg_id = [9u8; 32];
        let peer = random_public_key();
        // Use real-time-based timestamps with a long lease so that
        // recover_crash_state (called by Storage::open) does not clear
        // the lease before we can test that it survives restart.
        let t0 = now_ms();
        {
            let storage = Storage::open(dir.path()).unwrap();
            storage.enqueue_outbox(&msg_id, peer, t0).unwrap();
            storage
                .claim_due_outbox(t0, "crashed-worker", 30_000, 1)
                .unwrap();
            // locked_until_ms = t0 + 30_000
        }
        // After reopen, recover_crash_state sees locked_until_ms
        // is still in the future — does NOT clear it.
        let storage = Storage::open(dir.path()).unwrap();
        // Lease still valid: claim with a different owner should fail.
        assert!(storage
            .claim_due_outbox(t0 + 100, "replacement", 10, 1)
            .unwrap()
            .is_none());
        // After lease expires, claim should succeed.
        assert!(storage
            .claim_due_outbox(t0 + 30_001, "replacement", 10, 1)
            .unwrap()
            .is_some());
    }

    #[test]
    fn outbox_claim_query_is_bounded() {
        let storage = Storage::memory().unwrap();
        let peer = random_public_key();
        for id in 0..(MAX_OUTBOX_CLAIM_LIMIT + 5) {
            storage.enqueue_outbox(&[id as u8; 32], peer, 0).unwrap();
        }
        assert!(storage.fetch_due_outbox(0).unwrap().len() <= MAX_OUTBOX_CLAIM_LIMIT as usize);
    }

    // ── Comprehensive outbox claim/lease tests ──────────────────────────

    /// Single worker claims an entry, completes delivery successfully.
    /// Verifies: status → Sent, lease cleared, attempts incremented,
    /// last_attempt_at_ms set, next_attempt_at_ms set for future retry.
    #[test]
    fn test_outbox_single_worker_successful_delivery() {
        let storage = Storage::memory().unwrap();
        let msg_id = [20u8; 32];
        let peer = random_public_key();

        // Enqueue at t=100 with next_attempt=100 (immediately due)
        storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

        // Claim at t=100
        let claimed = storage
            .claim_due_outbox(100, "worker-1", 30_000, 1)
            .unwrap()
            .expect("should claim the due entry");
        assert_eq!(claimed.msg_id, msg_id);
        assert_eq!(claimed.recipient_device_id, peer);
        assert_eq!(claimed.status, DeliveryStatus::Pending);
        assert_eq!(claimed.attempts, 0);
        assert_eq!(claimed.lease_owner.as_deref(), Some("worker-1"));
        assert!(claimed.locked_until_ms.is_some());
        assert_eq!(claimed.locked_until_ms.unwrap(), 100 + 30_000);

        // Finish with success — schedule next attempt far in the future
        let done = storage
            .finish_outbox_attempt(
                &msg_id, peer, "worker-1", true,    // success
                200_000, // next_attempt_at_ms (far future)
                None,    // no error
            )
            .unwrap();
        assert!(done, "finish_outbox_attempt should succeed");

        // Verify: the entry should NOT appear in fetch_due_outbox at t=100
        // because next_attempt_at_ms=200_000 is in the future.
        let due = storage.fetch_due_outbox(100).unwrap();
        assert!(
            due.iter().find(|r| r.msg_id == msg_id).is_none(),
            "successfully sent entry should not be due at t=100"
        );

        // But at t=200_000 it should become due again (for retry if needed)
        let due2 = storage.fetch_due_outbox(200_000).unwrap();
        let row = due2
            .iter()
            .find(|r| r.msg_id == msg_id)
            .expect("entry should be due at t=200000");
        assert_eq!(row.status, DeliveryStatus::Sent);
        assert_eq!(row.attempts, 1);
        assert!(row.last_attempt_at_ms.is_some());
        assert_eq!(row.last_error_code, None);
        assert_eq!(row.lease_owner, None);
        assert_eq!(row.locked_until_ms, None);
    }

    /// Two workers race for the same entry. Only one wins.
    /// Verifies the losing worker's claim returns None and the winner's
    /// lease fields are correctly set.
    #[test]
    fn test_outbox_two_competitors_one_wins() {
        let storage = Storage::memory().unwrap();
        let msg_id = [21u8; 32];
        let peer = random_public_key();
        storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

        // Worker A claims
        let claimed_a = storage
            .claim_due_outbox(100, "worker-a", 30_000, 1)
            .unwrap()
            .expect("worker-a should claim");
        assert_eq!(claimed_a.lease_owner.as_deref(), Some("worker-a"));
        assert_eq!(claimed_a.locked_until_ms, Some(30_100));

        // Worker B tries to claim — must fail because the row is locked
        let claimed_b = storage
            .claim_due_outbox(100, "worker-b", 30_000, 1)
            .unwrap();
        assert!(claimed_b.is_none(), "worker-b must not claim locked row");

        // Worker A releases
        assert!(storage
            .release_outbox_lease(&msg_id, peer, "worker-a")
            .unwrap());

        // Now worker B can claim
        let claimed_b2 = storage
            .claim_due_outbox(100, "worker-b", 30_000, 1)
            .unwrap()
            .expect("worker-b should claim after release");
        assert_eq!(claimed_b2.lease_owner.as_deref(), Some("worker-b"));
    }

    /// Multiple stale leases are recovered in a single batch.
    #[test]
    fn test_outbox_recover_stale_leases_batch() {
        let storage = Storage::memory().unwrap();
        let peer = random_public_key();

        for i in 0..3 {
            let id = [30 + i as u8; 32];
            storage.enqueue_outbox(&id, peer, 100).unwrap();
        }

        // "dead-worker" claims all 3 with a short lease (10ms)
        for i in 0..3 {
            let id = [30 + i as u8; 32];
            storage
                .claim_due_outbox(100, "dead-worker", 10, 10)
                .unwrap();
        }

        // At t=109 lease still valid — nothing to recover
        assert_eq!(storage.recover_stale_outbox_leases(109).unwrap(), 0);

        // At t=110 all leases expired — recover all 3
        assert_eq!(storage.recover_stale_outbox_leases(110).unwrap(), 3);

        // Now a new worker can claim all 3
        for i in 0..3 {
            let claimed = storage
                .claim_due_outbox(110, "new-worker", 10, 10)
                .unwrap()
                .unwrap_or_else(|| panic!("entry {} should be claimable", i));
            assert_eq!(claimed.lease_owner.as_deref(), Some("new-worker"));
            // Release so the next loop iteration can claim too
            storage
                .release_outbox_lease(&[30 + i as u8; 32], peer, "new-worker")
                .unwrap();
        }
    }

    /// After the lease expires (locked_until_ms ≤ now_ms), a new worker
    /// can claim the entry *without* explicitly calling recover_stale.
    #[test]
    fn test_outbox_lease_expiry_claimable_without_recovery() {
        let storage = Storage::memory().unwrap();
        let msg_id = [22u8; 32];
        let peer = random_public_key();
        storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

        // Worker A claims with short lease (10ms)
        storage.claim_due_outbox(100, "worker-a", 10, 1).unwrap();

        // At t=109: still locked — worker B cannot claim
        assert!(storage
            .claim_due_outbox(109, "worker-b", 10, 1)
            .unwrap()
            .is_none());

        // At t=110: lease expired — worker B CAN claim (claim_due_outbox
        // checks locked_until_ms <= now_ms in its WHERE clause)
        let claimed = storage
            .claim_due_outbox(110, "worker-b", 10, 1)
            .unwrap()
            .expect("worker-b should claim expired lease at t=110");
        assert_eq!(claimed.lease_owner.as_deref(), Some("worker-b"));
    }

    /// After release_outbox_lease, the entry is immediately claimable
    /// at the same timestamp (no time advance needed).
    #[test]
    fn test_outbox_release_makes_immediately_claimable() {
        let storage = Storage::memory().unwrap();
        let msg_id = [23u8; 32];
        let peer = random_public_key();
        storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

        // Worker A claims
        storage
            .claim_due_outbox(100, "worker-a", 30_000, 1)
            .unwrap();

        // Worker A gracefully releases at t=100
        assert!(storage
            .release_outbox_lease(&msg_id, peer, "worker-a")
            .unwrap());

        // Worker B claims immediately at same t=100
        let claimed = storage
            .claim_due_outbox(100, "worker-b", 30_000, 1)
            .unwrap()
            .expect("worker-b should claim immediately after release");
        assert_eq!(claimed.lease_owner.as_deref(), Some("worker-b"));
    }

    /// Simulate a crash: a worker claims with a short lease, the process
    /// restarts, and recover_crash_state clears the expired lease so the
    /// entry becomes claimable.
    #[test]
    fn test_outbox_restart_clears_expired_lease() {
        let dir = tempfile::tempdir().unwrap();
        let msg_id = [24u8; 32];
        let peer = random_public_key();

        // First session: enqueue and claim with short lease (1ms)
        {
            let storage = Storage::open(dir.path()).unwrap();
            storage.enqueue_outbox(&msg_id, peer, 100).unwrap();
            storage.claim_due_outbox(100, "crash-worker", 1, 1).unwrap();
            // locked_until_ms = 100 + 1 = 101
        }
        // "crash" — process dies

        // Second session: recover_crash_state runs during Storage::open.
        // If the lease was set with locked_until_ms=101, and now real time
        // is much later, the lease should be cleared.
        {
            let storage = Storage::open(dir.path()).unwrap();
            // After recover_crash_state, the expired lease should be gone.
            // The entry should be claimable immediately.
            let claimed = storage
                .claim_due_outbox(
                    now_ms(), // use real current time
                    "recovery-worker",
                    30_000,
                    1,
                )
                .unwrap();
            assert!(
                claimed.is_some(),
                "expired lease should be cleared by recover_crash_state, making entry claimable"
            );
        }
    }

    /// finish_outbox_attempt must fail when called by a non-owner.
    #[test]
    fn test_outbox_finish_wrong_owner_rejected() {
        let storage = Storage::memory().unwrap();
        let msg_id = [25u8; 32];
        let peer = random_public_key();
        storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

        // Worker A claims
        storage
            .claim_due_outbox(100, "worker-a", 30_000, 1)
            .unwrap();

        // Worker B (non-owner) tries to finish — must fail
        let wrong = storage
            .finish_outbox_attempt(
                &msg_id, peer, "worker-b", // wrong owner
                true, 200_000, None,
            )
            .unwrap();
        assert!(!wrong, "non-owner must not finish the attempt");

        // Worker A (owner) finishes successfully
        let ok = storage
            .finish_outbox_attempt(&msg_id, peer, "worker-a", true, 200_000, None)
            .unwrap();
        assert!(ok, "owner must be able to finish the attempt");
    }

    /// release_outbox_lease must fail when called by a non-owner.
    #[test]
    fn test_outbox_release_wrong_owner_rejected() {
        let storage = Storage::memory().unwrap();
        let msg_id = [26u8; 32];
        let peer = random_public_key();
        storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

        // Worker A claims
        storage
            .claim_due_outbox(100, "worker-a", 30_000, 1)
            .unwrap();

        // Worker B tries to release — must fail
        assert!(!storage
            .release_outbox_lease(&msg_id, peer, "worker-b")
            .unwrap());

        // Worker A releases — must succeed
        assert!(storage
            .release_outbox_lease(&msg_id, peer, "worker-a")
            .unwrap());
    }

    /// Each call to finish_outbox_attempt increments the attempts counter.
    #[test]
    fn test_outbox_attempts_counter_increments() {
        let storage = Storage::memory().unwrap();
        let msg_id = [27u8; 32];
        let peer = random_public_key();
        storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

        // First attempt: attempts 0 → 1
        let claimed = storage
            .claim_due_outbox(100, "worker", 30_000, 1)
            .unwrap()
            .unwrap();
        assert_eq!(claimed.attempts, 0);
        storage
            .finish_outbox_attempt(
                &msg_id,
                peer,
                "worker",
                false, // failure
                200,   // retry at t=200
                Some("err1"),
            )
            .unwrap();

        // Second attempt: attempts 1 → 2 (re-claim after release)
        let claimed2 = storage
            .claim_due_outbox(200, "worker", 30_000, 1)
            .unwrap()
            .unwrap();
        assert_eq!(
            claimed2.attempts, 1,
            "attempts should be 1 after first finish"
        );
        assert_eq!(claimed2.last_error_code.as_deref(), Some("err1"));
        storage
            .finish_outbox_attempt(&msg_id, peer, "worker", false, 300, Some("err2"))
            .unwrap();

        // Third attempt: attempts 2 → 3
        let claimed3 = storage
            .claim_due_outbox(300, "worker", 30_000, 1)
            .unwrap()
            .unwrap();
        assert_eq!(
            claimed3.attempts, 2,
            "attempts should be 2 after second finish"
        );
        assert_eq!(claimed3.last_error_code.as_deref(), Some("err2"));
    }

    /// fetch_due_outbox must not return entries that hold a live lease.
    #[test]
    fn test_outbox_fetch_due_excludes_live_leased_entries() {
        let storage = Storage::memory().unwrap();
        let msg_id = [28u8; 32];
        let peer = random_public_key();
        storage.enqueue_outbox(&msg_id, peer, 100).unwrap();

        // Before claim: fetch_due_outbox returns the entry
        let before = storage.fetch_due_outbox(100).unwrap();
        assert!(before.iter().any(|r| r.msg_id == msg_id));

        // Claim with long lease
        storage.claim_due_outbox(100, "worker", 30_000, 1).unwrap();

        // After claim: fetch_due_outbox should NOT return it (live lease)
        let after = storage.fetch_due_outbox(100).unwrap();
        assert!(
            !after.iter().any(|r| r.msg_id == msg_id),
            "live-leased entry must not appear in fetch_due_outbox"
        );

        // After lease expires: fetch_due_outbox returns it again
        let expired = storage.fetch_due_outbox(130_001).unwrap();
        assert!(
            expired.iter().any(|r| r.msg_id == msg_id),
            "expired-lease entry must appear in fetch_due_outbox"
        );
    }

    /// Multiple entries are claimed in FIFO order by next_attempt_at_ms.
    #[test]
    fn test_outbox_multiple_entries_fifo_claim_order() {
        let storage = Storage::memory().unwrap();
        let peer = random_public_key();

        // Enqueue 3 entries with staggered next_attempt timestamps
        storage
            .enqueue_outbox(&[40u8; 32], peer, 300) // latest
            .unwrap();
        storage
            .enqueue_outbox(&[41u8; 32], peer, 200) // middle
            .unwrap();
        storage
            .enqueue_outbox(&[42u8; 32], peer, 100) // earliest
            .unwrap();

        // Claim should return earliest first
        let r1 = storage
            .claim_due_outbox(500, "worker", 30_000, 10)
            .unwrap()
            .expect("first claim");
        assert_eq!(
            r1.msg_id, [42u8; 32],
            "earliest next_attempt (100) should be first"
        );
        // Finish to push next_attempt far into the future so this entry
        // won't be picked up again by subsequent claims.
        storage
            .finish_outbox_attempt(&[42u8; 32], peer, "worker", false, 999_999, None)
            .unwrap();

        let r2 = storage
            .claim_due_outbox(500, "worker", 30_000, 10)
            .unwrap()
            .expect("second claim");
        assert_eq!(
            r2.msg_id, [41u8; 32],
            "middle next_attempt (200) should be second"
        );
        storage
            .finish_outbox_attempt(&[41u8; 32], peer, "worker", false, 999_999, None)
            .unwrap();

        let r3 = storage
            .claim_due_outbox(500, "worker", 30_000, 10)
            .unwrap()
            .expect("third claim");
        assert_eq!(
            r3.msg_id, [40u8; 32],
            "latest next_attempt (300) should be third"
        );
    }
}
