//! Identity and configuration persistence — key/value settings, room hidden
//! preferences, the contact roster, sync cursors, and profile-manifest
//! revision tracking.
//!
//! Each method is an `impl super::Storage` accessor over the shared SQLite
//! connection; no format or protocol changes live here (structural split
//! only, BORU-CORE-001).

use super::*;

impl super::Storage {
    /// KV key under which the hidden room-id set is persisted (BORU-DIR-12).
    /// `pub(super)` so the storage test suite can exercise malformed payloads.
    pub(super) const ROOM_HIDDEN_IDS_KEY: &str = "room_hidden_ids";

    /// Ensure the key-value table exists (idempotent).
    fn ensure_kv_table(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS kv_store (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );",
        )
        .std_context("create kv_store table")
    }
    /// Read a value from the key-value store.
    pub fn kv_get(&self, key: &str) -> Result<Option<String>> {
        self.ensure_kv_table()?;
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT value FROM kv_store WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .std_context("kv_get")
    }
    /// Write a value to the key-value store, upserting if the key exists.
    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        self.ensure_kv_table()?;
        let conn = self.conn.lock().unwrap();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        conn.execute(
            "INSERT INTO kv_store (key, value, updated_at_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at_ms = ?3",
            rusqlite::params![key, value, now_ms],
        )
        .std_context("kv_set")?;
        Ok(())
    }
    /// Read the persisted set of room ids the user has hidden/blocked
    /// from the public-room directory (BORU-DIR-12, PDF Task 4.3 step 3).
    ///
    /// The hide preference survives advertisement refreshes and
    /// application restarts: the app layer loads it at startup and feeds
    /// it into the directory via
    /// [`crate::room_directory::RoomDirectory::sync_local_states`], so a
    /// hidden room is never re-shown unless the user explicitly resets
    /// the preference (BORU-DIR-20 adds the user-facing controls; this
    /// is the persistence hook they write through).
    pub fn room_hidden_ids(&self) -> Result<Vec<[u8; 32]>> {
        let Some(raw) = self.kv_get(Self::ROOM_HIDDEN_IDS_KEY)? else {
            return Ok(Vec::new());
        };
        let hex_ids: Vec<String> =
            serde_json::from_str(&raw).std_context("parse room_hidden_ids payload")?;
        let mut out = Vec::with_capacity(hex_ids.len());
        for hex_id in hex_ids {
            if let Ok(bytes) = hex::decode(&hex_id) {
                if let Ok(arr) = <[u8; 32]>::try_from(bytes) {
                    out.push(arr);
                }
            }
        }
        Ok(out)
    }
    /// Persist a hide/block preference for a room (BORU-DIR-12, PDF Task
    /// 4.3 step 3). `hidden = true` adds the room id to the persisted
    /// hidden set; `false` removes it (the explicit reset path).
    ///
    /// This is the persistence hook BORU-DIR-20's user-facing Hide/Block
    /// controls write through; the directory cache reads the preference
    /// via [`Self::room_hidden_ids`] + `sync_local_states`.
    pub fn set_room_hidden(&self, room_id: &[u8; 32], hidden: bool) -> Result<()> {
        let mut ids = self.room_hidden_ids()?;
        let present = ids.iter().any(|id| id == room_id);
        match (hidden, present) {
            (true, false) => ids.push(*room_id),
            (false, true) => ids.retain(|id| id != room_id),
            _ => return Ok(()), // no-op: already in desired state
        }
        let hex_ids: Vec<String> = ids.iter().map(hex::encode).collect();
        let raw =
            serde_json::to_string(&hex_ids).std_context("serialize room_hidden_ids payload")?;
        self.kv_set(Self::ROOM_HIDDEN_IDS_KEY, &raw)
    }
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
    pub fn get_sync_cursor(
        &self,
        peer_device_id: &iroh::PublicKey,
    ) -> Result<Option<SyncCursorRow>> {
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
}
