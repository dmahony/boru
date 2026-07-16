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
use n0_error::{Result, StdResultExt};
use rusqlite::{params, Connection};

use crate::store::{DeliveryStatus, MessageId, OutboxRow, StoredEnvelope};

// ── Current schema version ────────────────────────────────────────────────

/// Bump every time a new migration is added.
const CURRENT_SCHEMA_VERSION: u32 = 2;

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
                ).into());
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
            ",
        )
        .std_context("migrate v2")?;
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
}
