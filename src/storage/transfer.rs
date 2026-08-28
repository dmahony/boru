//! Transfer persistence — content-addressed file objects, shared files and
//! collections, per-peer permissions, rings, the download state machine,
//! remote-file catalogue caching, and transfer-activity audit rows.
//!
//! Each method is an `impl super::Storage` accessor over the shared SQLite
//! connection; no format or protocol changes live here (structural split
//! only, BORU-CORE-001).

use super::*;

impl super::Storage {
    /// Store a file object. If the content hash already exists, returns the
    /// existing row without modifying it (idempotent).
    /// Optionally records the `source_path` on disk for referenced files.
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
    /// Store a chat upload and make it an offered file in the owner's profile.
    ///
    /// Chat uploads are content-addressed just like files added through the
    /// catalogue UI. Keeping object insertion, the profile offer, and the
    /// manifest revision together prevents a successful chat send from being
    /// invisible to the owner's profile catalogue.
    pub fn register_chat_upload(
        &self,
        profile_user_id: &str,
        filename: &str,
        mime_type: &str,
        data: &[u8],
    ) -> Result<String> {
        let content_hash = blake3::hash(data).to_hex().to_string();
        self.put_file_object(&content_hash, data.len() as u64, mime_type, filename, data)?;
        self.upsert_shared_file(
            &content_hash,
            profile_user_id,
            &content_hash,
            filename,
            None,
            true,
        )?;
        self.bump_manifest_revision(profile_user_id, &format!("chat-upload:{content_hash}"))?;
        Ok(content_hash)
    }
    /// Set (or clear) the `source_path` of an existing file object.
    /// Used when a local file is referenced on disk rather than imported
    /// into iroh-blobs.
    pub fn set_file_object_source_path(
        &self,
        content_hash: &str,
        source_path: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE file_objects SET source_path = ?1 WHERE content_hash = ?2",
            params![source_path, content_hash],
        )
        .std_context("set file_object source_path")?;
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
                "SELECT content_hash, size, mime_type, filename, created_at_ms, data, source_path
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
                data: row.get(5).std_context("get data")?,
                source_path: row.get(6).std_context("get source_path")?,
            }))
        } else {
            Ok(None)
        }
    }
    /// Look up the content hash of a file object previously recorded with
    /// this exact source path. Used by the chat send fast path to skip
    /// re-ingesting a file whose blob is already in the store.
    pub fn file_object_hash_by_source_path(&self, source_path: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let result = conn
            .query_row(
                "SELECT content_hash FROM file_objects WHERE source_path = ?1 LIMIT 1",
                params![source_path],
                |row| row.get(0),
            )
            .optional()
            .std_context("file_object hash by source path")?;
        Ok(result)
    }
    /// Record that a local file lives at `source_path` and is present in
    /// the iroh-blobs store as `blob_hash`, so a later share of the same
    /// path can skip re-ingesting the blob.
    ///
    /// Idempotent upsert: the row keeps any existing inline `data` (the
    /// blob store owns the content for chat-sent files, so `data` stays
    /// NULL here). The `blob_hash` is what the peer file-access handler
    /// needs to serve the content from the store.
    pub fn record_local_file_object(
        &self,
        content_hash: &str,
        size: u64,
        mime_type: &str,
        filename: &str,
        source_path: &str,
        blob_hash: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "INSERT INTO file_objects
                (content_hash, size, mime_type, filename, created_at_ms, source_path, blob_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(content_hash) DO UPDATE SET
                size = excluded.size,
                mime_type = excluded.mime_type,
                filename = excluded.filename,
                source_path = excluded.source_path,
                blob_hash = excluded.blob_hash",
            params![
                content_hash,
                size as i64,
                mime_type,
                filename,
                now,
                source_path,
                blob_hash,
            ],
        )
        .std_context("record local file object")?;
        Ok(())
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
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .std_context("begin shared file upsert")?;
        let existing_offered: Option<bool> = tx
            .query_row(
                "SELECT offered FROM shared_files
                 WHERE content_hash = ?1 AND profile_user_id = ?2",
                params![content_hash, profile_user_id],
                |row| row.get::<_, i64>(0).map(|v| v != 0),
            )
            .optional()
            .std_context("check existing shared file")?;
        if offered && existing_offered != Some(true) {
            let shared_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM shared_files
                     WHERE profile_user_id = ?1 AND offered = 1",
                    params![profile_user_id],
                    |row| row.get(0),
                )
                .std_context("count shared files")?;
            if shared_count as usize >= self.catalogue_limits.max_files_per_catalogue {
                return Err(anyhow!(
                    "catalogue has {} files, exceeds maximum of {}",
                    shared_count,
                    self.catalogue_limits.max_files_per_catalogue
                )
                .into());
            }
        }
        let now = now_ms() as i64;
        tx.execute(
            "INSERT INTO shared_files
                (content_hash, profile_user_id, metadata_id, display_filename,
                 description, offered, created_at_ms, updated_at_ms, version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, 1)
             ON CONFLICT(content_hash, profile_user_id) DO UPDATE SET
                metadata_id = excluded.metadata_id,
                display_filename = excluded.display_filename,
                description = excluded.description,
                offered = excluded.offered,
                version = shared_files.version + 1,
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
        tx.commit().std_context("commit shared file upsert")?;
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
                    description, offered, created_at_ms, updated_at_ms, version
             FROM shared_files
             WHERE profile_user_id = ?1 AND offered = 1
             ORDER BY updated_at_ms DESC"
        } else {
            "SELECT content_hash, profile_user_id, metadata_id, display_filename,
                    description, offered, created_at_ms, updated_at_ms, version
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
                version: row.get::<_, i64>(8).std_context("get version")? as u64,
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
                        description, offered, created_at_ms, updated_at_ms, version
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
                version: row.get::<_, i64>(8).std_context("get version")? as u64,
            }))
        } else {
            Ok(None)
        }
    }
    /// Return the requester-filtered view used by the remote catalogue.
    pub fn catalogue_entries_for_peer(
        &self,
        profile_user_id: &str,
        requester: &PublicKey,
        friends: &FriendsStore,
    ) -> Result<CatalogueView> {
        let requester_id = crate::friends::FriendId::from_public_key(*requester);
        let is_friend = friends
            .get(&requester_id)
            .is_some_and(|r| r.relationship == FriendRelationship::Friends);
        let permissions = self.list_permissions_for_grantee(requester_id.as_str())?;
        let now_ms = now_ms();
        let mut used_ids = std::collections::HashSet::new();
        let mut files = Vec::new();
        for row in self.list_shared_files(profile_user_id, true)? {
            if !self.file_object_exists(&row.content_hash)? {
                continue;
            }
            let mut denied = false;
            let mut granted = false;
            for permission in &permissions {
                if permission.grantor_user_id == profile_user_id
                    && permission.content_hash == row.content_hash
                    // Expired grants are inert: they neither deny nor authorize.
                    && permission.is_active_at(now_ms)
                {
                    denied |= permission.permission == "deny";
                    granted |= permission.permission == "read";
                }
            }
            let has_restricted_permissions =
                self.has_active_permissions_for_file(&row.content_hash, profile_user_id)?;
            if denied || (!granted && (has_restricted_permissions || !is_friend)) {
                continue;
            }
            // A shared-file row without its content record is corrupt or
            // stale.  Skip it rather than advertising guessed metadata
            // (size 0, empty MIME) in a signed catalogue (BORU-AUDIT-07).
            // `file_object_exists` above is the fast path; this strict
            // match closes the race and the record-without-object case.
            let Some(object) = self.get_file_object(&row.content_hash)? else {
                continue;
            };
            let shared_file_id = if used_ids.insert(row.metadata_id.clone()) {
                row.metadata_id
            } else {
                row.content_hash.clone()
            };
            files.push(RemoteSharedFile {
                shared_file_id,
                display_name: row.display_filename,
                description: row.description,
                mime_type: object.mime_type,
                size_bytes: object.size,
                content_hash: row.content_hash,
                version_number: row.version.min(u32::MAX as u64) as u32,
                updated_at_ms: row.updated_at_ms,
                collection_ids: Vec::new(),
                children: vec![],
            });
        }
        Ok(CatalogueView {
            collections: Vec::new(),
            files,
        })
    }
    /// Look up a shared file by its stable metadata identifier.
    pub fn get_shared_file_by_metadata_id(
        &self,
        profile_user_id: &str,
        metadata_id: &str,
    ) -> Result<Option<SharedFileRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT content_hash, profile_user_id, metadata_id, display_filename,
                    description, offered, created_at_ms, updated_at_ms, version
             FROM shared_files WHERE profile_user_id = ?1 AND metadata_id = ?2",
            )
            .std_context("prepare get_shared_file_by_metadata_id")?;
        let mut rows = stmt
            .query(params![profile_user_id, metadata_id])
            .std_context("query shared_file_by_metadata_id")?;
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
                version: row.get::<_, i64>(8).std_context("get version")? as u64,
            }))
        } else {
            Ok(None)
        }
    }
    /// Count active read grants for a file owner.
    pub fn count_read_grants_for_file(
        &self,
        content_hash: &str,
        grantor_user_id: &str,
    ) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM shared_file_permissions
             WHERE content_hash = ?1 AND grantor_user_id = ?2 AND permission = 'read'
               AND (expires_at_ms IS NULL OR expires_at_ms > ?3)",
                params![content_hash, grantor_user_id, now_ms() as i64],
                |row| row.get(0),
            )
            .std_context("count read grants")?;
        Ok(count as u64)
    }
    /// Create or get a named collection for a profile.
    pub fn ensure_collection(
        &self,
        profile_user_id: &str,
        name: &str,
        description: Option<&str>,
    ) -> Result<i64> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .std_context("begin collection ensure")?;
        if let Some(id) = tx
            .query_row(
                "SELECT id FROM file_collections WHERE profile_user_id = ?1 AND name = ?2",
                params![profile_user_id, name],
                |row| row.get(0),
            )
            .optional()
            .std_context("lookup existing collection")?
        {
            tx.commit()
                .std_context("commit existing collection lookup")?;
            return Ok(id);
        }
        let collection_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM file_collections WHERE profile_user_id = ?1",
                params![profile_user_id],
                |row| row.get(0),
            )
            .std_context("count collections")?;
        if collection_count as usize >= self.catalogue_limits.max_collections {
            return Err(anyhow!(
                "catalogue has {} collections, exceeds maximum of {}",
                collection_count,
                self.catalogue_limits.max_collections
            )
            .into());
        }
        let now = now_ms() as i64;
        tx.execute(
            "INSERT INTO file_collections
                (profile_user_id, name, description, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![profile_user_id, name, description, now],
        )
        .std_context("ensure collection")?;
        let id = tx.last_insert_rowid();
        tx.commit().std_context("commit collection ensure")?;
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
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .std_context("begin collection item add")?;
        let existing_item: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM file_collection_items
                 WHERE collection_id = ?1 AND content_hash = ?2",
                params![collection_id, content_hash],
                |row| row.get(0),
            )
            .optional()
            .std_context("lookup existing collection item")?;
        if existing_item.is_none() {
            let item_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM file_collection_items WHERE collection_id = ?1",
                    params![collection_id],
                    |row| row.get(0),
                )
                .std_context("count collection items")?;
            if item_count as usize >= self.catalogue_limits.max_entries_per_collection {
                return Err(anyhow!(
                    "collection {collection_id} has more than {} entries",
                    self.catalogue_limits.max_entries_per_collection
                )
                .into());
            }
        }
        let now = now_ms() as i64;
        tx.execute(
            "INSERT INTO file_collection_items
                (collection_id, content_hash, position, added_at_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(collection_id, content_hash) DO UPDATE SET
                position = excluded.position,
                added_at_ms = excluded.added_at_ms",
            params![collection_id, content_hash, position as i64, now],
        )
        .std_context("add to collection")?;
        tx.commit().std_context("commit collection item add")?;
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
    /// List every permission grant this profile has made on its shared files
    /// (grantor-side). Used by the "Files I'm Sharing" dashboard projection so
    /// the UI can show recipients, expiry, and deny grants without scanning
    /// the whole table or trusting remote display strings.
    pub fn list_permissions_for_grantor(
        &self,
        grantor_user_id: &str,
    ) -> Result<Vec<SharedFilePermission>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT content_hash, grantor_user_id, grantee_user_id, permission,
                        created_at_ms, expires_at_ms
                 FROM shared_file_permissions
                 WHERE grantor_user_id = ?1
                 ORDER BY created_at_ms DESC",
            )
            .std_context("prepare list_permissions_for_grantor")?;
        let mut rows = stmt
            .query(params![grantor_user_id])
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
    /// Return whether a file has any active explicit permissions.
    pub fn has_active_permissions_for_file(
        &self,
        content_hash: &str,
        grantor_user_id: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        let has: bool = conn
            .query_row(
                "SELECT 1 FROM shared_file_permissions
                 WHERE content_hash = ?1 AND grantor_user_id = ?2
                   AND (expires_at_ms IS NULL OR expires_at_ms > ?3)
                 LIMIT 1",
                params![content_hash, grantor_user_id, now],
                |_| Ok(true),
            )
            .optional()
            .std_context("check file permissions")?
            .unwrap_or(false);
        Ok(has)
    }
    /// Create a named ring owned by a profile.
    ///
    /// Returns the new ring's row id.  `is_open = true` creates the
    /// built-in open ring, which grants its associated permissions to any
    /// authenticated peer without a membership row.  By convention the open
    /// ring is read-only: [`set_ring_permission`](Self::set_ring_permission)
    /// rejects non-`Read` grants on it.
    ///
    /// The ring name must be unique per owner (`UNIQUE(owner_user_id, name)`).
    pub fn create_ring(&self, owner_user_id: &str, name: &str, is_open: bool) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "INSERT INTO rings (owner_user_id, name, is_open, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![owner_user_id, name, is_open, now],
        )
        .std_context("create ring")?;
        Ok(conn.last_insert_rowid())
    }
    /// Get a ring by id.
    pub fn get_ring(&self, ring_id: i64) -> Result<Option<Ring>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, owner_user_id, name, is_open, created_at_ms, updated_at_ms
                 FROM rings WHERE id = ?1",
                params![ring_id],
                |row| {
                    Ok(Ring {
                        id: row.get(0)?,
                        owner_user_id: row.get(1)?,
                        name: row.get(2)?,
                        is_open: row.get::<_, i64>(3)? != 0,
                        created_at_ms: row.get::<_, i64>(4)? as u64,
                        updated_at_ms: row.get::<_, i64>(5)? as u64,
                    })
                },
            )
            .optional()
            .std_context("get ring")?;
        Ok(row)
    }
    /// List all rings owned by a profile.
    pub fn list_rings(&self, owner_user_id: &str) -> Result<Vec<Ring>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, owner_user_id, name, is_open, created_at_ms, updated_at_ms
                 FROM rings WHERE owner_user_id = ?1 ORDER BY name",
            )
            .std_context("prepare list_rings")?;
        let mut rows = stmt
            .query(params![owner_user_id])
            .std_context("query rings")?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().std_context("next ring")? {
            out.push(Ring {
                id: row.get(0).std_context("ring id")?,
                owner_user_id: row.get(1).std_context("ring owner")?,
                name: row.get(2).std_context("ring name")?,
                is_open: row.get::<_, i64>(3).std_context("ring open")? != 0,
                created_at_ms: row.get::<_, i64>(4).std_context("ring created")? as u64,
                updated_at_ms: row.get::<_, i64>(5).std_context("ring updated")? as u64,
            });
        }
        Ok(out)
    }
    /// Rename a ring (bumping its `updated_at_ms`).
    ///
    /// The new name must be unique per owner.  Returns an error if the ring
    /// does not exist.
    pub fn rename_ring(&self, ring_id: i64, new_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        let rows = conn
            .execute(
                "UPDATE rings SET name = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![new_name, now, ring_id],
            )
            .std_context("rename ring")?;
        if rows == 0 {
            return Err(anyhow!("ring {ring_id} not found").into());
        }
        Ok(())
    }
    /// Delete a ring and all its memberships and resource permissions
    /// (cascaded by `ON DELETE CASCADE`).
    pub fn delete_ring(&self, ring_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM rings WHERE id = ?1", params![ring_id])
            .std_context("delete ring")?;
        Ok(())
    }
    /// Add a peer to a ring (idempotent).
    pub fn add_ring_member(&self, ring_id: i64, member_user_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "INSERT OR IGNORE INTO ring_members (ring_id, member_user_id, joined_at_ms)
             VALUES (?1, ?2, ?3)",
            params![ring_id, member_user_id, now],
        )
        .std_context("add ring member")?;
        Ok(())
    }
    /// Remove a peer from a ring.
    pub fn remove_ring_member(&self, ring_id: i64, member_user_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM ring_members WHERE ring_id = ?1 AND member_user_id = ?2",
            params![ring_id, member_user_id],
        )
        .std_context("remove ring member")?;
        Ok(())
    }
    /// List the member ids of a ring.
    pub fn list_ring_members(&self, ring_id: i64) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT member_user_id FROM ring_members
                 WHERE ring_id = ?1 ORDER BY joined_at_ms",
            )
            .std_context("prepare list_ring_members")?;
        let mut rows = stmt
            .query(params![ring_id])
            .std_context("query ring members")?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().std_context("next member")? {
            out.push(row.get(0).std_context("member id")?);
        }
        Ok(out)
    }
    /// Associate a typed permission between a ring and a file resource.
    ///
    /// Upserts (replaces) an existing association for the same
    /// (ring, resource, permission) triple.  Returns an error if the ring
    /// does not exist, or if the ring is the open ring and the permission is
    /// not [`RingPermission::Read`] (open rings are read-only by design).
    pub fn set_ring_permission(
        &self,
        ring_id: i64,
        content_hash: &str,
        permission: RingPermission,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let is_open: bool = conn
            .query_row(
                "SELECT is_open FROM rings WHERE id = ?1",
                params![ring_id],
                |row| row.get::<_, i64>(0).map(|v| v != 0),
            )
            .optional()
            .std_context("check ring open flag")?
            .unwrap_or(false);
        if is_open && permission != RingPermission::Read {
            return Err(
                anyhow!("open ring {ring_id} is read-only: cannot grant {permission}").into(),
            );
        }
        let now = now_ms() as i64;
        conn.execute(
            "INSERT INTO ring_resource_permissions
                (ring_id, content_hash, permission, created_at_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(ring_id, content_hash, permission)
             DO UPDATE SET created_at_ms = excluded.created_at_ms",
            params![ring_id, content_hash, permission.as_str(), now],
        )
        .std_context("set ring permission")?;
        Ok(())
    }
    /// Remove a typed permission association between a ring and a resource.
    pub fn remove_ring_permission(
        &self,
        ring_id: i64,
        content_hash: &str,
        permission: RingPermission,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM ring_resource_permissions
             WHERE ring_id = ?1 AND content_hash = ?2 AND permission = ?3",
            params![ring_id, content_hash, permission.as_str()],
        )
        .std_context("remove ring permission")?;
        Ok(())
    }
    /// List all resource-permission associations for a ring.
    pub fn list_ring_permissions(&self, ring_id: i64) -> Result<Vec<RingResourcePermission>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT ring_id, content_hash, permission, created_at_ms
                 FROM ring_resource_permissions
                 WHERE ring_id = ?1 ORDER BY content_hash, permission",
            )
            .std_context("prepare list_ring_permissions")?;
        let mut rows = stmt
            .query(params![ring_id])
            .std_context("query ring permissions")?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().std_context("next perm")? {
            let perm_str: String = row.get(2).std_context("perm string")?;
            let permission = RingPermission::from_str(&perm_str).ok_or_else(|| {
                anyhow!("unknown ring permission {perm_str:?} for ring {ring_id}")
            })?;
            out.push(RingResourcePermission {
                ring_id: row.get(0).std_context("ring id")?,
                content_hash: row.get(1).std_context("content hash")?,
                permission,
                created_at_ms: row.get::<_, i64>(3).std_context("created")? as u64,
            });
        }
        Ok(out)
    }
    /// Request-time ring authorization check.
    ///
    /// Returns `true` if `requester_user_id` is authorized by a ring to
    /// perform `permission` on the resource identified by `content_hash`.
    /// A peer is authorized when they are a member of a ring that holds the
    /// typed permission on that resource, or when the resource is associated
    /// with the owner's open ring (`is_open = 1` — grants to any
    /// authenticated peer).
    ///
    /// This is a **live** SQLite query — membership changes revoke access at
    /// request time; there is no cached catalogue state that could grant
    /// stale access.  Resources with no ring association are implicitly
    /// denied by the ring model (returns `false`).
    pub fn check_ring_access(
        &self,
        owner_user_id: &str,
        requester_user_id: &str,
        content_hash: &str,
        permission: RingPermission,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let allowed: bool = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM ring_resource_permissions rp
                    JOIN rings r ON r.id = rp.ring_id
                    WHERE r.owner_user_id = ?1
                      AND rp.content_hash = ?2
                      AND rp.permission = ?3
                      AND (r.is_open = 1 OR EXISTS(
                          SELECT 1 FROM ring_members m
                          WHERE m.ring_id = r.id
                            AND m.member_user_id = ?4
                      ))
                )",
                params![
                    owner_user_id,
                    content_hash,
                    permission.as_str(),
                    requester_user_id
                ],
                |row| row.get(0),
            )
            .std_context("check ring access")?;
        Ok(allowed)
    }
    /// Return the row id of the owner's open ring, if one exists.
    pub fn find_open_ring(&self, owner_user_id: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        let id = conn
            .query_row(
                "SELECT id FROM rings WHERE owner_user_id = ?1 AND is_open = 1 LIMIT 1",
                params![owner_user_id],
                |row| row.get(0),
            )
            .optional()
            .std_context("find open ring")?;
        Ok(id)
    }
    /// Persist one privacy-filtered lifecycle event. Replays are ignored by
    /// event id and by the transfer/sequence pair. Direction is read from the
    /// allow-listed `direction` payload key ("outbound" for uploads served to
    /// remote peers); events without it are inbound by default.
    pub fn record_transfer_activity(&self, event: &TransferLifecycleEvent) -> Result<()> {
        let payload_json = event.payload.as_ref().and_then(sanitize_activity_payload);
        let direction = event
            .payload
            .as_ref()
            .and_then(|value| value.get("direction"))
            .and_then(serde_json::Value::as_str)
            .filter(|value| *value == "outbound" || *value == "inbound")
            .unwrap_or("inbound");
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO transfer_activity
                (event_id, transfer_id, event_name, sequence, occurred_at_ms,
                 attempt, payload_json, direction)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event.event_id,
                event.transfer_id,
                event.event_name,
                event.sequence as i64,
                event.occurred_at_ms as i64,
                event.attempt as i64,
                payload_json,
                direction,
            ],
        )
        .std_context("record transfer activity")?;
        Ok(())
    }
    /// List newest activity first, bounded by the caller's requested limit.
    pub fn list_transfer_activity(&self, limit: usize) -> Result<Vec<TransferActivityRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT event_id, transfer_id, event_name, sequence, occurred_at_ms,
                        attempt, payload_json, direction
                 FROM transfer_activity
                 ORDER BY occurred_at_ms DESC, event_id DESC
                 LIMIT ?1",
            )
            .std_context("prepare transfer activity list")?;
        let rows = stmt
            .query_map(params![limit.min(1000) as i64], |row| {
                Ok(TransferActivityRow {
                    event_id: row.get(0)?,
                    transfer_id: row.get(1)?,
                    event_name: row.get(2)?,
                    sequence: row.get::<_, i64>(3)? as u64,
                    occurred_at_ms: row.get::<_, i64>(4)? as u64,
                    attempt: row.get::<_, i64>(5)? as u32,
                    payload_json: row.get(6)?,
                    direction: row.get(7)?,
                })
            })
            .std_context("query transfer activity")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .std_context("collect transfer activity")
    }
    /// Delete activity older than the supplied event timestamp.
    pub fn prune_transfer_activity(&self, before_occurred_at_ms: u64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM transfer_activity WHERE occurred_at_ms < ?1",
            params![before_occurred_at_ms as i64],
        )
        .std_context("prune transfer activity")
    }
    /// Clear the local activity projection entirely.
    ///
    /// This is the retention-aware "Clear History" primitive: it deletes only
    /// the `transfer_activity` projection rows. It never touches shared files,
    /// downloaded files, permissions, security records, or any other table, so
    /// the underlying share/download state machine stays authoritative.
    pub fn clear_transfer_activity(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM transfer_activity", [])
            .std_context("clear transfer activity")
    }
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
    /// Persist the temporary and final filesystem paths for restart recovery.
    pub fn set_download_paths(
        &self,
        id: i64,
        temp_path: impl AsRef<Path>,
        destination_path: impl AsRef<Path>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET temp_path = ?1, destination_path = ?2,
                    updated_at_ms = ?3 WHERE id = ?4",
            params![
                temp_path.as_ref().to_string_lossy().as_ref(),
                destination_path.as_ref().to_string_lossy().as_ref(),
                now_ms() as i64,
                id,
            ],
        )
        .std_context("set download paths")?;
        Ok(())
    }
    /// Recover download rows left in an in-progress state by a restart.
    ///
    /// Resolution and permission negotiation are retried from the beginning
    /// when no partial file exists, while a partial file is retained behind a
    /// paused row. Active transfers are always paused. Verification is never
    /// trusted from the database: an existing destination or temporary file
    /// is hashed again before it can become complete.
    pub fn recover_downloads_from_restart(&self) -> Result<()> {
        let rows: Vec<RecoveryRow> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT id, state, total_bytes, content_hash, temp_path, destination_path
                 FROM downloads
                 WHERE state IN ('resolving_peer', 'requesting_permission',
                                 'downloading', 'verifying')",
                )
                .std_context("prepare download restart recovery")?;
            let mut query = stmt
                .query([])
                .std_context("query download restart recovery")?;
            let mut rows = Vec::new();
            while let Some(row) = query.next().std_context("read download restart row")? {
                rows.push(RecoveryRow {
                    id: row.get(0).std_context("get download id")?,
                    state: row.get(1).std_context("get download state")?,
                    total_bytes: row.get::<_, i64>(2).std_context("get download size")? as u64,
                    content_hash: row.get(3).std_context("get download hash")?,
                    temp_path: row.get(4).std_context("get temporary path")?,
                    destination_path: row.get(5).std_context("get destination path")?,
                });
            }
            rows
        };

        for row in rows {
            let temp_exists = row
                .temp_path
                .as_deref()
                .is_some_and(|path| Path::new(path).is_file());
            match row.state.as_str() {
                "resolving_peer" | "requesting_permission" => {
                    let next = if temp_exists { "paused" } else { "queued" };
                    self.set_download_state_for_recovery(row.id, next)?;
                }
                "downloading" => self.set_download_state_for_recovery(row.id, "paused")?,
                "verifying" => {
                    let destination_valid = row.destination_path.as_deref().is_some_and(|path| {
                        Path::new(path).is_file()
                            && crate::download::verify_download_file(
                                path,
                                row.total_bytes,
                                &row.content_hash,
                            )
                            .is_ok()
                    });
                    let temp_valid = !destination_valid
                        && row.temp_path.as_deref().is_some_and(|path| {
                            Path::new(path).is_file()
                                && crate::download::verify_download_file(
                                    path,
                                    row.total_bytes,
                                    &row.content_hash,
                                )
                                .is_ok()
                        });
                    if destination_valid || temp_valid {
                        if destination_valid {
                            if let Some(temp) = row.temp_path.as_deref() {
                                if Path::new(temp).is_file() {
                                    let _ = std::fs::remove_file(temp);
                                }
                            }
                        } else if let (Some(temp), Some(destination)) =
                            (row.temp_path.as_deref(), row.destination_path.as_deref())
                        {
                            std::fs::rename(temp, destination)
                                .std_context("install verified download during recovery")?;
                        }
                        self.complete_download(row.id, row.total_bytes)?;
                    } else {
                        self.set_download_state_for_recovery(row.id, "downloading")?;
                    }
                }
                _ => unreachable!("filtered download state"),
            }
        }
        Ok(())
    }
    fn set_download_state_for_recovery(&self, id: i64, state: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET state = ?1, updated_at_ms = ?2 WHERE id = ?3",
            params![state, now_ms() as i64, id],
        )
        .std_context("set recovered download state")?;
        Ok(())
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
        let changed = conn
            .execute(
                "UPDATE downloads SET bytes_downloaded = ?1, state = ?2, updated_at_ms = ?3
             WHERE id = ?4 AND state != 'paused'",
                params![bytes_downloaded as i64, state, now, id],
            )
            .std_context("update download progress")?;
        if changed == 0 {
            let state: Option<String> = conn
                .query_row(
                    "SELECT state FROM downloads WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()
                .std_context("check download state after progress update")?;
            match state.as_deref() {
                Some("paused") => {
                    return Err(anyhow!("download is paused; active work must be cancelled").into());
                }
                Some(_) => return Err(anyhow!("download progress update affected no rows").into()),
                None => return Err(anyhow!("download not found").into()),
            }
        }
        Ok(())
    }
    /// Write multiple progress updates in a single SQLite transaction.
    ///
    /// Each entry is `(download_id, bytes_downloaded, state)`.  Updates
    /// are applied via the same logic as [`update_download_progress`](crate::storage::Storage::update_download_progress),
    /// but wrapped in a single `BEGIN` / `COMMIT` so that N concurrent
    /// downloaders do not issue N separate transactions.
    ///
    /// Paused or missing downloads are silently skipped (the batch
    /// should not fail because one download was paused mid-transfer).
    pub fn flush_progress_batch(&self, batch: &[(i64, u64, &str)]) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().std_context("start progress batch")?;
        let now = now_ms() as i64;
        for &(id, bytes, state) in batch {
            let changed = tx
                .execute(
                    "UPDATE downloads SET bytes_downloaded = ?1, state = ?2, updated_at_ms = ?3
                     WHERE id = ?4 AND state != 'paused'",
                    params![bytes as i64, state, now, id],
                )
                .std_context("batch progress update")?;
            if changed == 0 {
                // Paused / missing — skip silently rather than failing the
                // whole batch.  A concurrent pause or cancellation is normal.
                let _: Option<String> = tx
                    .query_row(
                        "SELECT state FROM downloads WHERE id = ?1",
                        params![id],
                        |row| row.get(0),
                    )
                    .optional()
                    .std_context("check download state")?;
            }
        }
        tx.commit().std_context("commit progress batch")?;
        Ok(())
    }
    /// Pause a non-terminal download without discarding its durable state.
    ///
    /// The update is deliberately narrow: it changes only the state and
    /// timestamp.  The expected content hash, peer/file metadata, byte count,
    /// retry information, and any transfer temporary data owned by the
    /// download worker are therefore retained for a later resume.  A worker
    /// that races with this call is rejected by [`Self::update_download_progress`]
    /// once the row is paused, which prevents stale transfer progress from
    /// moving the download out of `paused`.
    ///
    /// Pausing an already-paused download is idempotent.  Terminal downloads
    /// cannot be paused because doing so would make their durable outcome
    /// ambiguous.
    pub fn pause_download(&self, id: i64) -> Result<()> {
        const TERMINAL_STATES: &[&str] = &[
            "complete",
            "completed",
            "cancelled",
            "failed",
            "version_mismatch",
        ];

        let conn = self.conn.lock().unwrap();
        let current: Option<String> = conn
            .query_row(
                "SELECT state FROM downloads WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .std_context("look up download before pause")?;
        let Some(current) = current else {
            return Err(anyhow!("download not found").into());
        };
        if current == "paused" {
            return Ok(());
        }
        if TERMINAL_STATES.contains(&current.as_str()) {
            return Err(anyhow!("cannot pause terminal download in state {current}").into());
        }

        let now = now_ms() as i64;
        conn.execute(
            "UPDATE downloads SET state = 'paused', updated_at_ms = ?1
             WHERE id = ?2 AND state = ?3",
            params![now, id, current],
        )
        .std_context("pause download")?;
        Ok(())
    }
    /// Begin a truthful resume of a paused download.
    ///
    /// Resuming never jumps directly back to byte transfer. The worker must
    /// first resolve the peer and obtain a new permission/descriptor, because
    /// descriptors are short-lived and permissions may have changed while the
    /// download was paused or the process was stopped. Persisted progress and
    /// the expected content hash are retained for content-addressed chunk reuse.
    pub fn resume_download(&self, id: i64) -> Result<()> {
        const RESUME_IN_PROGRESS: &[&str] = &[
            "resolving_peer",
            "requesting_permission",
            "downloading",
            "verifying",
        ];

        let conn = self.conn.lock().unwrap();
        let current: Option<String> = conn
            .query_row(
                "SELECT state FROM downloads WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .std_context("look up download before resume")?;
        let Some(current) = current else {
            return Err(anyhow!("download not found").into());
        };
        if current == "paused" {
            let now = now_ms() as i64;
            conn.execute(
                "UPDATE downloads SET state = 'resolving_peer', last_error = NULL,
                        next_retry_at_ms = NULL, updated_at_ms = ?1
                 WHERE id = ?2 AND state = 'paused'",
                params![now, id],
            )
            .std_context("resume download")?;
            return Ok(());
        }
        if RESUME_IN_PROGRESS.contains(&current.as_str()) {
            return Ok(());
        }
        Err(anyhow!("cannot resume download in state {current}").into())
    }
    /// Accept a freshly authorised resume only when its descriptor still
    /// names the original content. A changed hash is recorded as a terminal
    /// version mismatch rather than silently downloading different content.
    ///
    /// Callers that have an expiry value should prefer
    /// [`Self::accept_resumed_descriptor_at`], which rejects stale descriptors
    /// before transfer starts.
    pub fn accept_resumed_descriptor(
        &self,
        id: i64,
        descriptor_content_hash: &str,
        total_bytes: u64,
    ) -> Result<()> {
        self.accept_resumed_descriptor_at(id, descriptor_content_hash, total_bytes, u64::MAX, 0)
    }
    /// Accept a fresh descriptor while checking its expiry at a supplied time.
    ///
    /// Supplying the clock makes expiry handling deterministic in tests and
    /// keeps a previously issued descriptor from being reused after it has
    /// expired. An expired descriptor is recorded as a failed resume and the
    /// download remains paused so a later retry must obtain another grant.
    pub fn accept_resumed_descriptor_at(
        &self,
        id: i64,
        descriptor_content_hash: &str,
        total_bytes: u64,
        expires_at_ms: u64,
        now_ms: u64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let expected: Option<(String, String)> = conn
            .query_row(
                "SELECT content_hash, state FROM downloads WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .std_context("look up download before accepting descriptor")?;
        let Some((expected_hash, state)) = expected else {
            return Err(anyhow!("download not found").into());
        };
        if state != "resolving_peer" && state != "requesting_permission" {
            return Err(anyhow!("download is not awaiting resume authorisation").into());
        }
        if now_ms >= expires_at_ms {
            conn.execute(
                "UPDATE downloads SET state = 'paused',
                        last_error = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![
                    "resume descriptor expired; fresh permission required",
                    now_ms as i64,
                    id
                ],
            )
            .std_context("record expired resume descriptor")?;
            return Err(anyhow!("resume descriptor expired").into());
        }
        let now = now_ms as i64;
        if expected_hash != descriptor_content_hash {
            conn.execute(
                "UPDATE downloads SET state = 'version_mismatch',
                        last_error = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![
                    format!("resume descriptor hash mismatch: expected {expected_hash}, got {descriptor_content_hash}"),
                    now,
                    id
                ],
            )
            .std_context("record resume version mismatch")?;
            return Err(anyhow!("resume descriptor content hash mismatch").into());
        }
        conn.execute(
            "UPDATE downloads SET state = 'downloading', total_bytes = ?1,
                    last_error = NULL, next_retry_at_ms = NULL, updated_at_ms = ?2
             WHERE id = ?3",
            params![total_bytes as i64, now, id],
        )
        .std_context("accept resumed descriptor")?;
        Ok(())
    }
    /// Mark a download as failed with an error message.
    pub fn fail_download(&self, id: i64, error: &str, next_retry_at_ms: Option<u64>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        let changed = conn
            .execute(
                "UPDATE downloads SET state = 'failed', last_error = ?1,
                    retry_count = retry_count + 1, next_retry_at_ms = ?2,
                    updated_at_ms = ?3
             WHERE id = ?4 AND state != 'paused'",
                params![error, next_retry_at_ms.map(|v| v as i64), now, id,],
            )
            .std_context("fail download")?;
        if changed == 0 {
            let state: Option<String> = conn
                .query_row(
                    "SELECT state FROM downloads WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()
                .std_context("check download state after failure")?;
            if state.as_deref() == Some("paused") {
                return Err(anyhow!("download is paused; active work must be cancelled").into());
            }
            if state.is_none() {
                return Err(anyhow!("download not found").into());
            }
        }
        Ok(())
    }
    /// Mark a download complete after its temporary output has been verified
    /// and atomically installed by the download worker.
    pub fn complete_download(&self, id: i64, bytes_downloaded: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        let changed = conn
            .execute(
                "UPDATE downloads SET state = 'complete', bytes_downloaded = ?1,
                        last_error = NULL, next_retry_at_ms = NULL, updated_at_ms = ?2
                 WHERE id = ?3 AND state NOT IN ('complete', 'cancelled', 'paused')",
                params![bytes_downloaded as i64, now, id],
            )
            .std_context("complete download")?;
        if changed == 0 {
            return Err(
                anyhow!("download {id} does not exist or cannot transition to complete").into(),
            );
        }
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
                 ORDER BY created_at_ms ASC, id ASC",
            )
            .std_context("prepare list_downloads_by_state")?;
        let mut rows = stmt.query(params![state]).std_context("query downloads")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(row_to_download(row)?);
        }
        Ok(results)
    }
    /// List every download row regardless of state, oldest first.
    ///
    /// This is the authoritative all-time view for the Sharing Summary card:
    /// each row is one durable download record, so total and active counts can
    /// be derived without scanning the filesystem or the rendered UI.
    pub fn list_downloads(&self) -> Result<Vec<Download>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, content_hash, remote_peer, state, bytes_downloaded,
                        total_bytes, created_at_ms, updated_at_ms, last_error,
                        retry_count, next_retry_at_ms
                 FROM downloads
                 ORDER BY created_at_ms ASC, id ASC",
            )
            .std_context("prepare list_downloads")?;
        let mut rows = stmt.query([]).std_context("query all downloads")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next download row")? {
            results.push(row_to_download(row)?);
        }
        Ok(results)
    }
    /// List the distinct peers a profile has explicitly shared files with,
    /// as recorded by durable permission grants (grantor → grantee).
    ///
    /// Unique peers are identified by their hex-encoded public key. The list
    /// is ordered deterministically by peer id so projections are stable
    /// across refreshes and restarts. A peer granted access to several files
    /// counts once.
    pub fn list_shared_peer_ids(&self, grantor_user_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT grantee_user_id
                 FROM shared_file_permissions
                 WHERE grantor_user_id = ?1
                 ORDER BY grantee_user_id ASC",
            )
            .std_context("prepare list_shared_peer_ids")?;
        let mut rows = stmt
            .query(params![grantor_user_id])
            .std_context("query shared peer ids")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next peer row")? {
            results.push(row.get(0).std_context("get grantee id")?);
        }
        Ok(results)
    }
    /// List completed downloads (terminal `complete` state), newest first,
    /// joined with their file-object display metadata and recorded destination
    /// paths. Used by the Downloaded dashboard tab; it never scans arbitrary
    /// download directories — history comes only from durable records.
    pub fn list_completed_downloads(&self) -> Result<Vec<CompletedDownloadRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT d.id, d.content_hash, d.remote_peer, d.total_bytes,
                        d.updated_at_ms, d.destination_path, fo.filename, fo.mime_type
                 FROM downloads d
                 JOIN file_objects fo ON fo.content_hash = d.content_hash
                 WHERE d.state IN ('complete', 'completed')
                 ORDER BY d.updated_at_ms DESC, d.id DESC",
            )
            .std_context("prepare list completed downloads")?;
        let mut rows = stmt.query([]).std_context("query completed downloads")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next completed download row")? {
            results.push(CompletedDownloadRecord {
                id: row.get(0).std_context("get download id")?,
                content_hash: row.get(1).std_context("get content hash")?,
                remote_peer: row.get(2).std_context("get remote peer")?,
                total_bytes: row.get::<_, i64>(3).std_context("get total bytes")? as u64,
                completed_at_ms: row.get::<_, i64>(4).std_context("get completed at")? as u64,
                destination_path: row.get(5).std_context("get destination path")?,
                display_filename: row.get(6).std_context("get display filename")?,
                mime_type: row.get(7).std_context("get mime type")?,
            });
        }
        Ok(results)
    }
    /// Remove a completed download's history record only. Never deletes the
    /// local file and never touches rows that are still part of the transfer
    /// state machine (queued/active/paused/verifying). Returns true when a
    /// terminal row was removed.
    pub fn delete_download_history(&self, id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "DELETE FROM downloads
                 WHERE id = ?1 AND state IN
                     ('complete', 'completed', 'failed', 'cancelled', 'version_mismatch')",
                params![id],
            )
            .std_context("delete download history record")?;
        Ok(changed > 0)
    }
    /// Find downloads targeting a specific content_hash, optionally filtered
    /// by remote_peer.  Returns all matching rows (in any state).
    ///
    /// Used by the download initiation logic to detect conflicting completed
    /// downloads before creating a new one.
    pub fn find_downloads_for_file(
        &self,
        content_hash: &str,
        remote_peer: Option<&str>,
    ) -> Result<Vec<Download>> {
        let conn = self.conn.lock().unwrap();
        let mut results = Vec::new();

        let sql = "SELECT id, content_hash, remote_peer, state, bytes_downloaded, \
                    total_bytes, created_at_ms, updated_at_ms, last_error, \
                    retry_count, next_retry_at_ms \
                   FROM downloads WHERE content_hash = ?1";

        if let Some(peer) = remote_peer {
            let mut stmt = conn
                .prepare(&format!(
                    "{sql} AND remote_peer = ?2 ORDER BY created_at_ms ASC, id ASC"
                ))
                .std_context("prepare find_downloads_for_file")?;
            let mut rows = stmt
                .query(rusqlite::params![content_hash, peer])
                .std_context("query find_downloads_for_file")?;
            while let Some(row) = rows.next().std_context("next row")? {
                results.push(row_to_download(row)?);
            }
        } else {
            let mut stmt = conn
                .prepare(&format!("{sql} ORDER BY created_at_ms ASC, id ASC"))
                .std_context("prepare find_downloads_for_file")?;
            let mut rows = stmt
                .query(rusqlite::params![content_hash])
                .std_context("query find_downloads_for_file")?;
            while let Some(row) = rows.next().std_context("next row")? {
                results.push(row_to_download(row)?);
            }
        }

        Ok(results)
    }
    /// Cancel a non-terminal download — sets state to 'cancelled'.
    ///
    /// Terminal downloads (complete, failed, version_mismatch) are rejected
    /// because their outcome is already durably recorded.  An already-cancelled
    /// download is treated as idempotent (no-op), matching the pattern used
    /// by [`Self::pause_download`].
    pub fn cancel_download(&self, id: i64) -> Result<()> {
        const TERMINAL_STATES: &[&str] = &["complete", "completed", "failed", "version_mismatch"];

        let conn = self.conn.lock().unwrap();
        let current: Option<String> = conn
            .query_row(
                "SELECT state FROM downloads WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .std_context("look up download before cancel")?;
        let Some(current) = current else {
            return Err(anyhow!("download not found").into());
        };
        if current == "cancelled" {
            return Ok(());
        }
        if TERMINAL_STATES.contains(&current.as_str()) {
            return Err(anyhow!("cannot cancel terminal download in state {current}").into());
        }
        let now = now_ms() as i64;
        conn.execute(
            "UPDATE downloads SET state = 'cancelled', updated_at_ms = ?1
             WHERE id = ?2 AND state = ?3",
            params![now, id, current],
        )
        .std_context("cancel download")?;
        Ok(())
    }
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
    /// Store a remote peer's catalogue locally.
    ///
    /// Inserts metadata (revision, generation time), file entries, and
    /// collection entries so they can be queried later via the
    /// `get_remote_*` methods.
    pub fn replace_remote_catalogue(&self, catalogue: &SignedFileCatalogue) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let peer = catalogue.owner_id.to_string();
        let now = now_ms() as i64;
        let tx = conn
            .transaction()
            .std_context("begin remote catalogue transaction")?;

        // Store catalogue meta in profile_manifest_state (reused as
        // remote-catalogue meta store for the stub implementation).
        tx.execute(
            "INSERT OR REPLACE INTO profile_manifest_state
                (user_id, revision, manifest_hash, created_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                peer,
                catalogue.revision as i64,
                catalogue.generated_at_ms.to_string(),
                now,
            ],
        )
        .std_context("store remote catalogue meta")?;

        // A catalogue is a complete snapshot. Remove entries from the
        // previous revision before inserting the current one so deleted
        // offers do not remain visible in the remote cache.
        tx.execute(
            "DELETE FROM shared_files WHERE profile_user_id = ?1",
            params![peer],
        )
        .std_context("clear remote catalogue files")?;
        tx.execute(
            "DELETE FROM file_collections WHERE profile_user_id = ?1",
            params![peer],
        )
        .std_context("clear remote catalogue collections")?;

        // Store each file from the catalogue.
        for file in &catalogue.files {
            // Satisfy FK: ensure a file_objects row exists.
            tx.execute(
                "INSERT OR IGNORE INTO file_objects
                    (content_hash, size, mime_type, filename, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    &file.content_hash,
                    file.size_bytes as i64,
                    &file.mime_type,
                    &file.display_name,
                    now,
                ],
            )
            .std_context("store remote catalogue file object")?;

            // Upsert into shared_files keyed by remote peer.
            tx.execute(
                "INSERT OR REPLACE INTO shared_files
                    (content_hash, profile_user_id, metadata_id, display_filename,
                     description, offered, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)",
                params![
                    &file.content_hash,
                    peer,
                    &file.shared_file_id,
                    &file.display_name,
                    &file.description,
                    now,
                ],
            )
            .std_context("store remote catalogue shared file")?;
        }

        // Store each collection from the catalogue.
        for collection in &catalogue.collections {
            tx.execute(
                "INSERT OR REPLACE INTO file_collections
                    (profile_user_id, name, description, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![peer, &collection.name, &collection.description, now],
            )
            .std_context("store remote catalogue collection")?;
        }

        tx.commit()
            .std_context("commit remote catalogue transaction")?;
        Ok(())
    }
    /// Read back catalogue metadata for a remote peer.
    pub fn get_remote_catalogue_meta(
        &self,
        peer: &iroh::PublicKey,
    ) -> Result<Option<RemoteCatalogueMeta>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT user_id, revision, manifest_hash, created_at_ms
             FROM profile_manifest_state WHERE user_id = ?1",
            params![peer.to_string()],
            |row| {
                Ok(RemoteCatalogueMeta {
                    peer: row.get(0)?,
                    revision: row.get::<_, i64>(1)? as u64,
                    generated_at_ms: row.get::<_, String>(2)?.parse().unwrap_or(0),
                    fetched_at_ms: row.get::<_, i64>(3)? as u64,
                })
            },
        )
        .optional()
        .std_context("get remote catalogue meta")
    }
    /// Read back shared files stored from a remote peer's catalogue.
    pub fn get_remote_shared_files(
        &self,
        peer: &iroh::PublicKey,
    ) -> Result<Vec<RemoteSharedFileRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT sf.content_hash, sf.display_filename, fo.mime_type, fo.size
                 FROM shared_files sf
                 JOIN file_objects fo ON fo.content_hash = sf.content_hash
                 WHERE sf.profile_user_id = ?1
                 ORDER BY sf.updated_at_ms DESC",
            )
            .std_context("prepare get_remote_shared_files")?;
        let mut rows = stmt
            .query(params![peer.to_string()])
            .std_context("query remote shared files")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(RemoteSharedFileRow {
                content_hash: row.get(0).std_context("get content_hash")?,
                display_filename: row.get(1).std_context("get display_filename")?,
                mime_type: row.get(2).std_context("get mime_type")?,
                size_bytes: row.get::<_, i64>(3).std_context("get size_bytes")? as u64,
            });
        }
        Ok(results)
    }
    /// Read back collections stored from a remote peer's catalogue.
    pub fn get_remote_collections(
        &self,
        peer: &iroh::PublicKey,
    ) -> Result<Vec<RemoteCollectionRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, name
                 FROM file_collections
                 WHERE profile_user_id = ?1
                 ORDER BY name",
            )
            .std_context("prepare get_remote_collections")?;
        let mut rows = stmt
            .query(params![peer.to_string()])
            .std_context("query remote collections")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(RemoteCollectionRow {
                id: row.get(0).std_context("get id")?,
                name: row.get(1).std_context("get name")?,
            });
        }
        Ok(results)
    }
    /// Update metadata fields of a shared file entry.
    ///
    /// Args: content_hash, profile_user_id, display_filename, description (optional), metadata_id.
    pub fn update_shared_file_metadata(
        &self,
        content_hash: &str,
        profile_user_id: &str,
        display_filename: &str,
        description: Option<&str>,
        metadata_id: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "UPDATE shared_files SET
                display_filename = ?1,
                description = ?2,
                metadata_id = ?3,
                updated_at_ms = ?4
             WHERE content_hash = ?5 AND profile_user_id = ?6",
            params![
                display_filename,
                description,
                metadata_id,
                now,
                content_hash,
                profile_user_id
            ],
        )
        .std_context("update shared file metadata")?;
        Ok(())
    }
    /// Set the offered flag on a shared file entry.
    pub fn set_shared_file_offered(
        &self,
        content_hash: &str,
        profile_user_id: &str,
        offered: bool,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "UPDATE shared_files SET offered = ?1, updated_at_ms = ?2
             WHERE content_hash = ?3 AND profile_user_id = ?4",
            params![offered as i64, now, content_hash, profile_user_id],
        )
        .std_context("set shared file offered")?;
        Ok(())
    }
    /// Delete a shared file entry.
    pub fn delete_shared_file(&self, content_hash: &str, profile_user_id: &str) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .std_context("begin delete shared file")?;
        tx.execute(
            "DELETE FROM shared_file_permissions
             WHERE content_hash = ?1 AND grantor_user_id = ?2",
            params![content_hash, profile_user_id],
        )
        .std_context("delete shared file permissions")?;
        let n = tx
            .execute(
                "DELETE FROM shared_files
                 WHERE content_hash = ?1 AND profile_user_id = ?2",
                params![content_hash, profile_user_id],
            )
            .std_context("delete shared file")?;
        tx.commit().std_context("commit delete shared file")?;
        Ok(n > 0)
    }
    /// Record file verification/availability state for a specific profile.
    ///
    /// Args: content_hash, profile_user_id, availability_status, verified_at_ms,
    ///       expected_content_hash, expected_size.
    pub fn set_file_availability(
        &self,
        content_hash: &str,
        profile_user_id: &str,
        availability: &str,
        verified_at_ms: Option<u64>,
        expected_content_hash: &str,
        expected_size: u64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "INSERT OR REPLACE INTO file_verification
                (content_hash, profile_user_id, availability, verified_at_ms,
                 expected_content_hash, expected_size, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                content_hash,
                profile_user_id,
                availability,
                verified_at_ms.map(|v| v as i64),
                expected_content_hash,
                expected_size as i64,
                now,
            ],
        )
        .std_context("set file availability")?;
        Ok(())
    }
    /// Get the current file verification/availability state for a profile.
    pub fn get_file_availability(
        &self,
        content_hash: &str,
        profile_user_id: &str,
    ) -> Result<Option<FileAvailability>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT content_hash, profile_user_id, availability, verified_at_ms,
                    expected_content_hash, expected_size, updated_at_ms
             FROM file_verification
             WHERE content_hash = ?1 AND profile_user_id = ?2",
            params![content_hash, profile_user_id],
            |row| {
                Ok(FileAvailability {
                    content_hash: row.get(0)?,
                    profile_user_id: row.get(1)?,
                    availability: row.get(2)?,
                    verified_at_ms: row.get::<_, Option<i64>>(3)?.map(|v| v as u64),
                    expected_content_hash: row.get(4)?,
                    expected_size: row.get::<_, i64>(5)? as u64,
                    updated_at_ms: row.get::<_, i64>(6)? as u64,
                })
            },
        )
        .optional()
        .std_context("get file availability")
    }
    /// Record that a file was replaced with a new hash.
    pub fn record_file_replacement(
        &self,
        old_content_hash: &str,
        new_content_hash: &str,
        profile_user_id: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        conn.execute(
            "INSERT INTO file_replacements
                (old_content_hash, new_content_hash, profile_user_id, replaced_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![old_content_hash, new_content_hash, profile_user_id, now],
        )
        .std_context("record file replacement")?;
        Ok(())
    }
    /// Increment the revision counter for a shared file.
    pub fn increment_shared_file_revision(
        &self,
        _content_hash: &str,
        _profile_user_id: &str,
    ) -> Result<()> {
        // Currently a no-op — revision tracking for individual shared files
        // is not yet implemented in the schema. The per-file version_number
        // is primarily a wire-format concept in RemoteSharedFile.
        // This stub satisfies the API contract for file_library_ops.
        Ok(())
    }
    /// Rename a file collection.
    pub fn rename_collection(&self, collection_id: i64, new_name: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        let n = conn
            .execute(
                "UPDATE file_collections SET name = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![new_name, now, collection_id],
            )
            .std_context("rename collection")?;
        Ok(n > 0)
    }
    /// Delete a file collection (cascade removes items via ON DELETE CASCADE).
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
    /// Check whether any foreign-key references exist for a file object.
    ///
    /// Returns `true` if at least one other table (message_attachments,
    /// shared_files, file_collection_items, shared_file_permissions, or
    /// downloads) references the given `content_hash`.
    pub fn file_object_has_references(&self, content_hash: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let has: bool = conn
            .query_row(
                "SELECT 1 FROM (
                    SELECT 1 FROM message_attachments WHERE content_hash = ?1
                    UNION ALL
                    SELECT 1 FROM shared_files WHERE content_hash = ?1
                    UNION ALL
                    SELECT 1 FROM file_collection_items WHERE content_hash = ?1
                    UNION ALL
                    SELECT 1 FROM shared_file_permissions WHERE content_hash = ?1
                    UNION ALL
                    SELECT 1 FROM downloads WHERE content_hash = ?1
                ) LIMIT 1",
                params![content_hash],
                |_| Ok(true),
            )
            .optional()
            .std_context("check file object references")?
            .unwrap_or(false);
        Ok(has)
    }
}
