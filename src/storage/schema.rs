//! Schema and managed migrations for the relational store.
//!
//! Owns the SQLite connection construction (`open`/`memory`), integrity
//! checking, crash-state recovery, and the versioned `migrate_v1..v20`
//! steps that lift a database to [`super::CURRENT_SCHEMA_VERSION`].

use super::*;

impl super::Storage {
    /// Open (or create) the database at `data_dir / `[`DB_FILE_NAME`]`.
    ///
    /// Runs schema migrations automatically so the database is always at
    /// `CURRENT_SCHEMA_VERSION` after this call returns.
    /// Runs integrity check and crash-state recovery automatically.
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_catalogue_limits(data_dir, CatalogueLimitsConfig::default())
    }
    /// Open (or create) the database with explicit catalogue admission limits.
    ///
    /// This is the same as [`open`](Self::open), but allows tests and
    /// deployments to override the catalogue count caps without changing the
    /// rest of the storage configuration.
    pub fn open_with_catalogue_limits(
        data_dir: impl AsRef<Path>,
        catalogue_limits: CatalogueLimitsConfig,
    ) -> Result<Self> {
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
            catalogue_limits,
            #[cfg(feature = "net")]
            activity: Arc::new(DbActivity::default()),
        };

        // Check DB integrity before touching any data.
        storage.check_integrity()?;

        // Run migrations (handles partial migration recovery internally).
        storage.run_migrations()?;

        // Recover any state left dangling by a crash.
        storage.recover_crash_state()?;
        // Recover interrupted file transfers after the schema is ready.
        storage.recover_downloads_from_restart()?;

        info!(db_path = %db_path.display(), "storage opened successfully");
        Ok(storage)
    }
    /// Open an in-memory database (for tests).
    pub fn memory() -> Result<Self> {
        Self::memory_with_catalogue_limits(CatalogueLimitsConfig::default())
    }
    /// Open an in-memory database (for tests) with explicit catalogue limits.
    pub fn memory_with_catalogue_limits(catalogue_limits: CatalogueLimitsConfig) -> Result<Self> {
        let conn = Connection::open_in_memory().std_context("open in-memory sqlite db")?;
        conn.execute_batch("PRAGMA foreign_keys = ON;\n             PRAGMA synchronous = NORMAL;")
            .std_context("set pragmas")?;
        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
            catalogue_limits,
            #[cfg(feature = "net")]
            activity: Arc::new(DbActivity::default()),
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
        debug!("database integrity check passed");
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
        debug!("recovering crash state");
        let conn = self.conn.lock().unwrap();
        let now = crate::chat_core::now_ms() as i64;

        // Recover crash-left Sent rows back to Pending.
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

        // Recover crash-left Sending rows back to Pending.
        conn.execute(
            "UPDATE outbox SET
                status = ?1,
                next_attempt_at_ms = ?2,
                last_error_code = 'crash_recovered'
             WHERE status = ?3",
            params![
                crate::store::DeliveryStatus::Pending as u8,
                now,
                crate::store::DeliveryStatus::Sending as u8,
            ],
        )
        .std_context("recover crash-left Sending outbox")?;

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
    fn add_column_if_missing(
        conn: &Connection,
        table: &str,
        column: &str,
        definition: &str,
    ) -> Result<()> {
        let present: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2 LIMIT 1",
                params![table, column],
                |row| row.get(0),
            )
            .optional()
            .std_context("inspect migration column")?;
        if present.is_none() {
            conn.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                [],
            )
            .std_context("add migration column")?;
        }
        Ok(())
    }
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
            debug!(version = start, "database already at latest schema");
            return Ok(());
        }

        info!(
            from_version = start,
            to_version = CURRENT_SCHEMA_VERSION,
            "running database migrations"
        );

        // Run each migration in its own transaction.
        for v in (start + 1)..=CURRENT_SCHEMA_VERSION {
            match v {
                1 => self.migrate_v1(&conn)?,
                2 => self.migrate_v2(&conn)?,
                3 => self.migrate_v3(&conn)?,
                4 => self.migrate_v4(&conn)?,
                5 => self.migrate_v5(&conn)?,
                6 => self.migrate_v6(&conn)?,
                7 => self.migrate_v7(&conn)?,
                8 => self.migrate_v8(&conn)?,
                9 => self.migrate_v9(&conn)?,
                10 => self.migrate_v10(&conn)?,
                11 => self.migrate_v11(&conn)?,
                12 => self.migrate_v12(&conn)?,
                13 => self.migrate_v13(&conn)?,
                14 => self.migrate_v14(&conn)?,
                15 => self.migrate_v15(&conn)?,
                16 => self.migrate_v16(&conn)?,
                17 => self.migrate_v17(&conn)?,
                18 => self.migrate_v18(&conn)?,
                19 => self.migrate_v19(&conn)?,
                20 => self.migrate_v20(&conn)?,
                21 => self.migrate_v21(&conn)?,
                22 => self.migrate_v22(&conn)?,
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
            -- The row id is the insertion sequence and disambiguates
            -- downloads created during the same millisecond.
            CREATE INDEX idx_downloads_queue_order
                ON downloads(state, created_at_ms, id);
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
    /// V7 adds file verification tracking and file replacement history.
    fn migrate_v7(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS file_verification (
                content_hash TEXT NOT NULL REFERENCES file_objects(content_hash),
                profile_user_id TEXT NOT NULL,
                availability TEXT NOT NULL DEFAULT 'Unknown',
                verified_at_ms INTEGER,
                expected_content_hash TEXT NOT NULL DEFAULT '',
                expected_size INTEGER NOT NULL DEFAULT 0,
                updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY (content_hash, profile_user_id)
            );
            CREATE TABLE IF NOT EXISTS file_replacements (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                old_content_hash TEXT NOT NULL,
                new_content_hash TEXT NOT NULL,
                profile_user_id TEXT NOT NULL,
                replaced_at_ms INTEGER NOT NULL
            );",
        )
        .std_context("migrate v7 file verification and replacements")?;
        Ok(())
    }
    /// V8 adds the filesystem paths needed to recover an interrupted
    /// download without guessing where its partial output belongs.
    fn migrate_v8(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "ALTER TABLE downloads ADD COLUMN temp_path TEXT;
             ALTER TABLE downloads ADD COLUMN destination_path TEXT;",
        )
        .std_context("migrate v8 download paths")?;
        Ok(())
    }
    /// V9 adds `source_path` to `file_objects` for locally-referenced files
    /// (files on disk that are served without importing into iroh-blobs).
    fn migrate_v9(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch("ALTER TABLE file_objects ADD COLUMN source_path TEXT;")
            .std_context("migrate v9 source_path")?;
        Ok(())
    }
    /// V10 creates the `outgoing_messages` table so the GUI can read/write
    /// delivery state from SQLite instead of `outbox.json`.
    fn migrate_v10(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS outgoing_messages (
                event_id INTEGER PRIMARY KEY,
                topic_blob BLOB NOT NULL,
                hash TEXT NOT NULL,
                signed_bytes BLOB NOT NULL,
                delivery_state TEXT NOT NULL DEFAULT 'queued',
                retry_count INTEGER NOT NULL DEFAULT 0,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_outgoing_topic
                ON outgoing_messages(topic_blob);",
        )
        .std_context("migrate v10 outgoing_messages")?;
        Ok(())
    }
    /// V11 creates the durable public-room directory advertisement table.
    fn migrate_v11(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS directory_ads (
                topic BLOB NOT NULL,
                author BLOB NOT NULL,
                room_name TEXT NOT NULL,
                description TEXT NOT NULL,
                ticket TEXT NOT NULL,
                member_count INTEGER NOT NULL,
                last_activity INTEGER NOT NULL,
                received_at_ms INTEGER NOT NULL,
                PRIMARY KEY (topic, author)
            );",
        )
        .std_context("migrate v11 directory_ads")?;
        Ok(())
    }
    /// V12 adds durable group metadata, membership, epochs, and invites.
    fn migrate_v12(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS groups (
                group_id BLOB PRIMARY KEY, name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
                owner_public_key BLOB NOT NULL, current_epoch INTEGER NOT NULL DEFAULT 0,
                created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL, archived INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS group_members (
                group_id BLOB NOT NULL REFERENCES groups(group_id) ON DELETE CASCADE,
                public_key BLOB NOT NULL, role TEXT NOT NULL, joined_at_ms INTEGER NOT NULL,
                invited_by BLOB, epoch_joined INTEGER NOT NULL, state TEXT NOT NULL,
                PRIMARY KEY (group_id, public_key)
            );
            CREATE INDEX IF NOT EXISTS idx_group_members_state ON group_members(group_id, state);
            CREATE TABLE IF NOT EXISTS group_epochs (
                group_id BLOB NOT NULL REFERENCES groups(group_id) ON DELETE CASCADE,
                epoch INTEGER NOT NULL, topic_id BLOB NOT NULL, discovery_secret BLOB NOT NULL,
                created_at_ms INTEGER NOT NULL, PRIMARY KEY (group_id, epoch)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_group_epochs_topic ON group_epochs(topic_id);
            CREATE TABLE IF NOT EXISTS group_invites (
                invite_id BLOB PRIMARY KEY, group_id BLOB NOT NULL REFERENCES groups(group_id) ON DELETE CASCADE,
                inviter_public_key BLOB NOT NULL, recipient_public_key BLOB NOT NULL, epoch INTEGER NOT NULL,
                status TEXT NOT NULL, created_at_ms INTEGER NOT NULL, expires_at_ms INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_group_invites_recipient_status ON group_invites(recipient_public_key, status);
            CREATE INDEX IF NOT EXISTS idx_group_invites_group ON group_invites(group_id);",
        )
        .std_context("migrate v12 groups")?;
        Ok(())
    }
    /// V13: per-group encryption state for end-to-end encrypted messaging.
    fn migrate_v13(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );
            ",
        )
        .std_context("migrate v13 group_encryption_state")?;
        Ok(())
    }
    /// V14 adds a `ticket` column to `group_invites` so pending invites carry
    /// the room ticket needed to accept and join the group's gossip room.
    fn migrate_v14(&self, conn: &Connection) -> Result<()> {
        Self::add_column_if_missing(conn, "group_invites", "ticket", "TEXT NOT NULL DEFAULT ''")
    }
    /// v15 — persist group name from whisper invites so the recipient can
    /// create a ConversationEntry with the correct display name at accept time.
    fn migrate_v15(&self, conn: &Connection) -> Result<()> {
        Self::add_column_if_missing(
            conn,
            "group_invites",
            "group_name",
            "TEXT NOT NULL DEFAULT ''",
        )
    }
    /// v16 adds shared-file revisions and a bounded, idempotent activity
    /// projection. Activity stores only privacy-filtered telemetry metadata.
    fn migrate_v16(&self, conn: &Connection) -> Result<()> {
        Self::add_column_if_missing(
            conn,
            "shared_files",
            "version",
            "INTEGER NOT NULL DEFAULT 1",
        )?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS transfer_activity (
                 event_id TEXT PRIMARY KEY,
                 transfer_id TEXT NOT NULL,
                 event_name TEXT NOT NULL,
                 sequence INTEGER NOT NULL,
                 occurred_at_ms INTEGER NOT NULL,
                 attempt INTEGER NOT NULL,
                 payload_json TEXT,
                 UNIQUE(transfer_id, sequence)
             );
             CREATE INDEX IF NOT EXISTS idx_transfer_activity_time
                 ON transfer_activity(occurred_at_ms DESC, event_id DESC);",
        )
        .std_context("migrate v16 file revisions and transfer activity")?;
        Ok(())
    }
    /// v17 adds a `direction` column to the transfer activity projection so the
    /// Activity Log can deterministically distinguish downloads to this node
    /// (`inbound`) from uploads served to remote peers (`outbound`) without
    /// re-deriving direction from transfer-id prefixes or row resolvability.
    /// Existing rows predate outbound recording and are all inbound.
    fn migrate_v17(&self, conn: &Connection) -> Result<()> {
        Self::add_column_if_missing(
            conn,
            "transfer_activity",
            "direction",
            "TEXT NOT NULL DEFAULT 'inbound'",
        )
    }
    /// v18 adds named-ring permission groups (iroh-rings borrow).
    ///
    /// A ring is a named set of peers sharing typed Read/Write/Delete
    /// permissions on file resources.  Three tables:
    ///
    /// - `rings` — the named ring definitions, owned by a profile.  The
    ///   built-in open ring has `is_open = 1` and grants its associated
    ///   permissions to any authenticated peer (no membership row).
    /// - `ring_members` — which peers belong to which rings.
    /// - `ring_resource_permissions` — typed permission associations
    ///   between a ring and a file resource (by content hash).
    ///
    /// Ring grants are additive with the existing friend-relationship and
    /// per-peer `shared_file_permissions` checks; a resource with no ring
    /// association is implicitly denied by the ring model (rings only grant
    /// what is explicitly associated).
    fn migrate_v18(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS rings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                owner_user_id TEXT NOT NULL,
                name TEXT NOT NULL,
                is_open INTEGER NOT NULL DEFAULT 0,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                UNIQUE(owner_user_id, name)
            );
            CREATE INDEX IF NOT EXISTS idx_rings_owner
                ON rings(owner_user_id);

            CREATE TABLE IF NOT EXISTS ring_members (
                ring_id INTEGER NOT NULL REFERENCES rings(id)
                    ON DELETE CASCADE,
                member_user_id TEXT NOT NULL,
                joined_at_ms INTEGER NOT NULL,
                PRIMARY KEY (ring_id, member_user_id)
            );
            CREATE INDEX IF NOT EXISTS idx_ring_members_member
                ON ring_members(member_user_id);

            CREATE TABLE IF NOT EXISTS ring_resource_permissions (
                ring_id INTEGER NOT NULL REFERENCES rings(id)
                    ON DELETE CASCADE,
                content_hash TEXT NOT NULL,
                permission TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                PRIMARY KEY (ring_id, content_hash, permission)
            );
            CREATE INDEX IF NOT EXISTS idx_ring_resource_perms_hash
                ON ring_resource_permissions(content_hash);
            ",
        )
        .std_context("migrate v18 rings")?;
        Ok(())
    }
    /// v19 creates the durable chat-message history table used by the
    /// backfill service.
    ///
    /// The storage CRUD methods (`insert_chat_message`,
    /// `get_recent_chat_messages_for_topic`, `count_chat_messages_for_topic`)
    /// have always referenced `chat_messages`, but no migration ever created
    /// the table — so every backfill query silently failed and returned
    /// empty history.  This migration adds the table with the exact columns
    /// the CRUD layer uses (`msg_hash` unique for `INSERT OR IGNORE` dedup,
    /// `id` for newest-first ordering).
    fn migrate_v19(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS chat_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                msg_hash BLOB NOT NULL,
                topic BLOB NOT NULL,
                sender BLOB NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                signed_bytes BLOB NOT NULL,
                UNIQUE(msg_hash)
            );
            CREATE INDEX IF NOT EXISTS idx_chat_messages_topic
                ON chat_messages(topic, timestamp_ms);
            ",
        )
        .std_context("migrate v19 chat_messages")?;
        Ok(())
    }
    /// v20 adds the advertisement TTL column to the durable public-room
    /// directory table (BORU-DIR-08, PDF Task 3.2).
    ///
    /// `DirectoryStore` now tracks each advertisement's `expires_after_secs`
    /// so directory clients can evict stale entries when no valid refresh
    /// arrives.  Existing rows (created by migrate_v11) default to the
    /// protocol TTL (`DEFAULT_ADVERT_TTL_SECS`, 300 s); rows whose TTL
    /// elapsed while the app was offline are dropped at load time.
    fn migrate_v20(&self, conn: &Connection) -> Result<()> {
        Self::add_column_if_missing(
            conn,
            "directory_ads",
            "expires_after_secs",
            "INTEGER NOT NULL DEFAULT 300",
        )
        .std_context("migrate v20 directory_ads expires_after_secs")?;
        Ok(())
    }
    fn migrate_v21(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS message_replies (
                message_hash BLOB PRIMARY KEY,
                reply_to_message_id BLOB NOT NULL,
                resolved INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_message_replies_parent
                ON message_replies(reply_to_message_id);",
        )
        .std_context("migrate v21 message replies")?;
        Ok(())
    }

    /// v21 stores reaction projections and remove tombstones independently
    /// from message bodies, allowing restart and backfill convergence.
    fn migrate_v22(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS reaction_events (
                message_id BLOB NOT NULL,
                actor BLOB NOT NULL,
                emoji TEXT NOT NULL,
                removed INTEGER NOT NULL DEFAULT 0,
                updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY (message_id, actor, emoji)
            );
            CREATE INDEX IF NOT EXISTS idx_reaction_events_message
                ON reaction_events(message_id, removed);",
        )
        .std_context("migrate v22 reaction_events")?;
        Ok(())
    }
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
}
