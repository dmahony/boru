//! Durable companion registrations, idempotent operation results, and sync records.

use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRegistration {
    pub registration_id: Vec<u8>,
    pub device_id: Vec<u8>,
    pub grant_revision: i64,
    pub revoked: bool,
}

/// Point-in-time authorization state for a companion request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionGrant {
    pub registration_id: Vec<u8>,
    pub device_id: Vec<u8>,
    pub grant_revision: i64,
    pub grant_scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationResult {
    pub request_digest: Vec<u8>,
    pub result: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionSnapshotToken {
    pub epoch: Vec<u8>,
    pub registration_id: Vec<u8>,
    pub device_id: Vec<u8>,
    pub grant_revision: i64,
    pub scope: String,
    pub watermark: i64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionChange {
    pub sequence: i64,
    pub change_id: Vec<u8>,
    pub entity_id: Vec<u8>,
    pub entity_revision: i64,
    pub kind: String,
    pub tombstone: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionChangePage {
    pub changes: Vec<CompanionChange>,
    pub next_sequence: i64,
    pub end: bool,
}

fn encode_snapshot(token: &CompanionSnapshotToken) -> Result<String> {
    let bytes = serde_json::to_vec(token).std_context("encode snapshot token")?;
    Ok(hex::encode(bytes))
}

fn decode_snapshot(value: &str) -> Result<CompanionSnapshotToken> {
    let bytes = hex::decode(value).map_err(|_| anyhow!("malformed snapshot token"))?;
    serde_json::from_slice(&bytes).std_context("decode snapshot token")
}

impl MessageStore {
    /// Validate registration, device binding, and revision in one read.
    pub fn authorize_companion(
        &self,
        registration_id: &[u8],
        device_id: &[u8],
        grant_revision: i64,
    ) -> Result<Option<CompanionGrant>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT registration_id, device_id, grant_revision, grant_scope FROM device_registrations
             WHERE registration_id=?1 AND device_id=?2 AND revoked=0 AND grant_revision=?3",
            params![registration_id, device_id, grant_revision],
            |row| {
                Ok(CompanionGrant {
                    registration_id: row.get(0)?,
                    device_id: row.get(1)?,
                    grant_revision: row.get(2)?,
                    grant_scope: row.get(3)?,
                })
            },
        )
        .optional()
        .std_context("authorize companion request")
    }

    pub fn register_device(&self, registration_id: &[u8], device_id: &[u8]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO device_registrations
             (registration_id, device_id, grant_revision, grant_scope, created_at_ms)
             VALUES (?1, ?2, 1, 'accessible', ?3)
             ON CONFLICT(registration_id) DO UPDATE SET
               device_id=excluded.device_id, revoked=0,
               grant_revision=device_registrations.grant_revision + 1,
               revoked_at_ms=NULL",
            params![registration_id, device_id, unix_now_ms() as i64],
        )
        .std_context("register companion device")?;
        Ok(())
    }

    /// Set the v1 conversation scope and rotate the grant revision.
    pub fn set_companion_scope(&self, registration_id: &[u8], scope: &str) -> Result<bool> {
        let changed = self
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE device_registrations SET grant_scope=?1, grant_revision=grant_revision+1
             WHERE registration_id=?2 AND revoked=0",
                params![scope, registration_id],
            )
            .std_context("set companion scope")?;
        Ok(changed != 0)
    }

    pub fn companion_scope(
        &self,
        registration_id: &[u8],
        device_id: &[u8],
        grant_revision: i64,
    ) -> Result<Option<String>> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT grant_scope FROM device_registrations
             WHERE registration_id=?1 AND device_id=?2 AND revoked=0 AND grant_revision=?3",
                params![registration_id, device_id, grant_revision],
                |row| row.get(0),
            )
            .optional()
            .std_context("read companion scope")
    }

    pub fn revoke_device(&self, registration_id: &[u8]) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "UPDATE device_registrations SET revoked=1, revoked_at_ms=?1,
                 grant_revision=grant_revision+1 WHERE registration_id=?2 AND revoked=0",
                params![unix_now_ms() as i64, registration_id],
            )
            .std_context("revoke companion device")?;
        Ok(changed != 0)
    }

    pub fn device_registration(
        &self,
        registration_id: &[u8],
    ) -> Result<Option<DeviceRegistration>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT registration_id, device_id, grant_revision, revoked
             FROM device_registrations WHERE registration_id=?1",
            [registration_id],
            |row| {
                Ok(DeviceRegistration {
                    registration_id: row.get(0)?,
                    device_id: row.get(1)?,
                    grant_revision: row.get(2)?,
                    revoked: row.get::<_, i64>(3)? != 0,
                })
            },
        )
        .optional()
        .std_context("read companion registration")
    }

    pub fn operation_result(
        &self,
        registration_id: &[u8],
        operation_id: &[u8],
    ) -> Result<Option<OperationResult>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT request_digest, result FROM operation_results
             WHERE registration_id=?1 AND operation_id=?2",
            params![registration_id, operation_id],
            |row| {
                Ok(OperationResult {
                    request_digest: row.get(0)?,
                    result: row.get(1)?,
                })
            },
        )
        .optional()
        .std_context("read operation result")
    }

    /// Read either a live or pruned result. Pruned ids remain reserved.
    pub fn operation_result_or_tombstone(
        &self,
        registration_id: &[u8],
        operation_id: &[u8],
    ) -> Result<Option<OperationResult>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT request_digest, result FROM operation_results
             WHERE registration_id=?1 AND operation_id=?2
             UNION ALL
             SELECT request_digest, result FROM operation_result_tombstones
             WHERE registration_id=?1 AND operation_id=?2 LIMIT 1",
            params![registration_id, operation_id],
            |row| Ok(OperationResult { request_digest: row.get(0)?, result: row.get(1)? }),
        )
        .optional()
        .std_context("read operation result or tombstone")
    }

    /// Read a cached result only while the same live grant is still valid.
    pub fn authorized_operation_result(
        &self,
        registration_id: &[u8],
        device_id: &[u8],
        grant_revision: i64,
        operation_id: &[u8],
    ) -> Result<Option<OperationResult>> {
        let conn = self.conn.lock().unwrap();
        let authorized: Option<i64> = conn
            .query_row(
                "SELECT grant_revision FROM device_registrations
                 WHERE registration_id=?1 AND device_id=?2 AND revoked=0 AND grant_revision=?3",
                params![registration_id, device_id, grant_revision],
                |row| row.get(0),
            )
            .optional()
            .std_context("authorize cached companion result")?;
        if authorized.is_none() {
            return Ok(None);
        }
        conn.query_row(
            "SELECT request_digest, result FROM operation_results
             WHERE registration_id=?1 AND operation_id=?2",
            params![registration_id, operation_id],
            |row| {
                Ok(OperationResult {
                    request_digest: row.get(0)?,
                    result: row.get(1)?,
                })
            },
        )
        .optional()
        .std_context("read authorized operation result")
    }

    pub fn sync_epoch(&self) -> Result<Vec<u8>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT epoch FROM sync_epoch WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .std_context("read sync epoch")
    }

    /// Rotate the epoch and invalidate every grant as one durable operation.
    pub fn reset_sync_state(&self, new_epoch: &[u8]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .std_context("begin sync reset")?;
        let now = unix_now_ms() as i64;
        tx.execute(
            "UPDATE device_registrations SET revoked=1, revoked_at_ms=?1,
             grant_revision=grant_revision+1 WHERE revoked=0",
            [now],
        )
        .std_context("invalidate companion grants")?;
        tx.execute(
            "UPDATE sync_epoch SET epoch=?1, updated_at_ms=?2 WHERE singleton=1",
            params![new_epoch, now],
        )
        .std_context("rotate sync epoch")?;
        tx.commit().std_context("commit sync reset")
    }

    /// Atomically admit a message, its optional delivery, idempotency result, and change reference.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_companion_mutation(
        &self,
        registration_id: &[u8],
        device_id: &[u8],
        grant_revision: i64,
        operation_id: &[u8],
        request_digest: &[u8],
        result: &[u8],
        change_id: &[u8],
        msg_hash: &[u8; 32],
        topic: &[u8; 32],
        sender: &[u8; 32],
        timestamp_ms: u64,
        body: &str,
        recipient_device: Option<PublicKey>,
    ) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .std_context("begin companion mutation")?;
        let authorized: Option<i64> = tx
            .query_row(
                "SELECT grant_revision FROM device_registrations
                 WHERE registration_id=?1 AND device_id=?2 AND revoked=0 AND grant_revision=?3",
                params![registration_id, device_id, grant_revision],
                |row| row.get(0),
            )
            .optional()
            .std_context("authorize companion mutation")?;
        if authorized.is_none() {
            return Err(anyhow!("companion grant revoked or stale").into());
        }
        let existing: Option<Vec<u8>> = tx
            .query_row(
                "SELECT request_digest FROM operation_results
                 WHERE registration_id=?1 AND operation_id=?2",
                params![registration_id, operation_id],
                |row| row.get(0),
            )
            .optional()
            .std_context("check operation deduplication")?;
        let pruned: Option<Vec<u8>> = tx
            .query_row(
                "SELECT request_digest FROM operation_result_tombstones
                 WHERE registration_id=?1 AND operation_id=?2",
                params![registration_id, operation_id],
                |row| row.get(0),
            )
            .optional()
            .std_context("check pruned operation deduplication")?;
        if let Some(digest) = existing.or(pruned) {
            if digest != request_digest {
                return Err(anyhow!("operation id reused with different request").into());
            }
            tx.commit()
                .std_context("commit duplicate companion operation")?;
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO messages(msg_hash,topic,sender,timestamp_ms,kind,body,delivery_state)
             VALUES (?1,?2,?3,?4,'text',?5,'host_accepted')",
            params![
                msg_hash.as_slice(),
                topic.as_slice(),
                sender.as_slice(),
                timestamp_ms as i64,
                body
            ],
        )
        .std_context("admit companion message")?;
        if let Some(recipient) = recipient_device {
            tx.execute(
                "INSERT INTO outbox(msg_id,recipient_device_id,status,attempts,next_attempt_at_ms)
                 VALUES (?1,?2,?3,0,?4)",
                params![
                    msg_hash.as_slice(),
                    recipient.as_bytes(),
                    DeliveryStatus::Pending as u8,
                    timestamp_ms as i64
                ],
            )
            .std_context("admit companion outbox")?;
            tx.execute(
                "UPDATE messages SET delivery_state='awaiting_recipient' WHERE msg_hash=?1",
                [msg_hash.as_slice()],
            )
            .std_context("mark companion message awaiting recipient")?;
        }
        let now = unix_now_ms() as i64;
        tx.execute(
            "INSERT INTO operation_results(registration_id,operation_id,request_digest,result,created_at_ms)
             VALUES (?1,?2,?3,?4,?5)",
            params![registration_id, operation_id, request_digest, result, now],
        )
        .std_context("record companion operation result")?;
        tx.execute("UPDATE change_sequence SET current=current+1 WHERE singleton=1", [])
            .std_context("advance change sequence")?;
        let sequence: i64 = tx
            .query_row("SELECT current FROM change_sequence WHERE singleton=1", [], |row| row.get(0))
            .std_context("read change sequence")?;
        tx.execute(
            "INSERT INTO change_references(change_id,registration_id,operation_id,message_hash,sequence,entity_revision,kind,tombstone,created_at_ms)
             VALUES (?1,?2,?3,?4,?5,?5,'message',0,?6)",
            params![change_id, registration_id, operation_id, msg_hash.as_slice(), sequence, now],
        )
        .std_context("record companion change reference")?;
        tx.commit().std_context("commit companion mutation")?;
        Ok(true)
    }

    /// Append a body-free read-state change to the C10 resumption stream.
    /// The read watermark itself is stored in `message_read_markers`; this
    /// reference lets an approved companion converge after reconnecting.
    pub fn record_companion_read_change(
        &self,
        registration_id: &[u8],
        operation_id: &[u8],
        change_id: &[u8],
        conversation_id: &[u8; 32],
    ) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .std_context("begin companion read change")?;
        tx.execute(
            "UPDATE change_sequence SET current=current+1 WHERE singleton=1",
            [],
        )
        .std_context("advance read change sequence")?;
        let sequence: i64 = tx
            .query_row("SELECT current FROM change_sequence WHERE singleton=1", [], |row| row.get(0))
            .std_context("read read-change sequence")?;
        tx.execute(
            "INSERT INTO change_references
             (change_id,registration_id,operation_id,message_hash,sequence,entity_revision,kind,tombstone,created_at_ms)
             VALUES (?1,?2,?3,?4,?5,?5,'read_state',0,?6)",
            params![
                change_id,
                registration_id,
                operation_id,
                conversation_id.as_slice(),
                sequence,
                unix_now_ms() as i64
            ],
        )
        .std_context("record companion read change")?;
        tx.commit().std_context("commit companion read change")
    }

    /// Capture a bounded snapshot watermark bound to the current grant and epoch.
    pub fn begin_companion_snapshot(
        &self,
        registration_id: &[u8],
        device_id: &[u8],
        grant_revision: i64,
        now_ms: u64,
    ) -> Result<String> {
        let grant = self.authorize_companion(registration_id, device_id, grant_revision)?
            .ok_or_else(|| anyhow!("companion grant revoked or stale"))?;
        let conn = self.conn.lock().unwrap();
        let watermark: i64 = conn
            .query_row("SELECT current FROM change_sequence WHERE singleton=1", [], |row| row.get(0))
            .std_context("read snapshot watermark")?;
        drop(conn);
        encode_snapshot(&CompanionSnapshotToken {
            epoch: self.sync_epoch()?,
            registration_id: grant.registration_id,
            device_id: grant.device_id,
            grant_revision,
            scope: grant.grant_scope,
            watermark,
            expires_at_ms: now_ms.saturating_add(15 * 60 * 1000),
        })
    }

    /// Resume body-free durable references strictly after the supplied cursor.
    pub fn resume_companion_changes(
        &self, token: &str, after_sequence: i64, limit: usize, now_ms: u64,
    ) -> Result<CompanionChangePage> {
        let snapshot = decode_snapshot(token)?;
        if now_ms > snapshot.expires_at_ms { return Err(anyhow!("snapshot expired").into()); }
        if self.sync_epoch()? != snapshot.epoch { return Err(anyhow!("snapshot invalidated by epoch change").into()); }
        if self.authorize_companion(&snapshot.registration_id, &snapshot.device_id, snapshot.grant_revision)?.is_none() {
            return Err(anyhow!("snapshot invalidated by authorization change").into());
        }
        let conn = self.conn.lock().unwrap();
        let oldest: Option<i64> = conn.query_row(
            "SELECT MIN(sequence) FROM change_references", [], |row| row.get(0))
            .std_context("read oldest change")?;
        if after_sequence < 0
            || oldest.is_some_and(|old| after_sequence < old - 1)
            || (oldest.is_none() && snapshot.watermark > after_sequence)
        {
            return Err(anyhow!("cursor pruned").into());
        }
        if after_sequence > snapshot.watermark { return Err(anyhow!("cursor beyond snapshot").into()); }
        let mut stmt = conn.prepare(
            "SELECT sequence,change_id,message_hash,entity_revision,kind,tombstone
             FROM change_references WHERE registration_id=?1 AND sequence>?2 AND sequence<=?3
             ORDER BY sequence LIMIT ?4")
            .std_context("prepare companion changes")?;
        let mut rows = stmt.query(params![snapshot.registration_id, after_sequence, snapshot.watermark, limit.clamp(1, 500) as i64])
            .std_context("query companion changes")?;
        let mut changes = Vec::new();
        while let Some(row) = rows.next().std_context("read companion change")? {
            changes.push(CompanionChange {
                sequence: row.get(0).std_context("read change sequence")?,
                change_id: row.get(1).std_context("read change id")?,
                entity_id: row.get(2).std_context("read change entity")?,
                entity_revision: row.get(3).std_context("read entity revision")?,
                kind: row.get(4).std_context("read change kind")?,
                tombstone: row.get::<_, i64>(5).std_context("read tombstone")? != 0,
            });
        }
        let next_sequence = changes.last().map_or(after_sequence, |c| c.sequence);
        Ok(CompanionChangePage { end: next_sequence >= snapshot.watermark || changes.is_empty(), changes, next_sequence })
    }

    pub fn prune_companion_changes(&self, through_sequence: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let removed = conn
            .execute("DELETE FROM change_references WHERE sequence<=?1", [through_sequence])
            .std_context("prune companion changes")?;
        Ok(removed)
    }

    pub fn prune_operation_results(&self, older_than_ms: u64, max_rows: usize) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().std_context("begin operation pruning")?;
        let removed = tx
            .execute(
                "INSERT OR REPLACE INTO operation_result_tombstones
             (registration_id, operation_id, request_digest, result, pruned_at_ms)
             SELECT registration_id, operation_id, request_digest, result, ?1
             FROM operation_results WHERE created_at_ms < ?2
             ORDER BY created_at_ms LIMIT ?3",
                params![unix_now_ms() as i64, older_than_ms as i64, max_rows as i64],
            )
            .std_context("archive operation results")?;
        tx.execute(
            "DELETE FROM operation_results WHERE created_at_ms < ?1
             AND rowid IN (SELECT rowid FROM operation_results ORDER BY created_at_ms LIMIT ?2)",
            params![older_than_ms as i64, max_rows as i64],
        )
        .std_context("prune operation results")?;
        tx.commit().std_context("commit operation pruning")?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_and_epoch_survive_restart_and_reset_revokes_grants() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("companion.db");
        {
            let store = MessageStore::open(&path).unwrap();
            store.register_device(b"registration", b"device").unwrap();
            assert_eq!(store.sync_epoch().unwrap(), vec![0; 32]);
        }
        let store = MessageStore::open(&path).unwrap();
        assert!(
            !store
                .device_registration(b"registration")
                .unwrap()
                .unwrap()
                .revoked
        );
        store.reset_sync_state(&[9; 32]).unwrap();
        assert_eq!(store.sync_epoch().unwrap(), vec![9; 32]);
        assert!(
            store
                .device_registration(b"registration")
                .unwrap()
                .unwrap()
                .revoked
        );
    }

    #[test]
    fn operation_dedup_is_atomic_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("companion.db");
        let store = MessageStore::open(&path).unwrap();
        store.register_device(b"r", b"device").unwrap();
        let hash = [7; 32];
        assert!(store
            .commit_companion_mutation(
                b"r", b"device", 1, b"op", b"digest", b"result", b"change", &hash, &[1; 32],
                &[2; 32], 10, "hello", None,
            )
            .unwrap());
        assert!(!store
            .commit_companion_mutation(
                b"r",
                b"device",
                1,
                b"op",
                b"digest",
                b"result",
                b"change-2",
                &hash,
                &[1; 32],
                &[2; 32],
                10,
                "hello",
                None,
            )
            .unwrap());
        drop(store);
        let reopened = MessageStore::open(&path).unwrap();
        assert_eq!(
            reopened
                .operation_result(b"r", b"op")
                .unwrap()
                .unwrap()
                .result,
            b"result"
        );
        let conn = reopened.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM change_references", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn snapshot_resumes_strictly_after_boundary_and_rejects_stale_state() {
        let store = MessageStore::memory().unwrap();
        let registration = b"snapshot-registration";
        let device = b"snapshot-device";
        store.register_device(registration, device).unwrap();
        let token = store
            .begin_companion_snapshot(registration, device, 1, 1_000)
            .unwrap();
        let hash = [7; 32];
        assert!(store
            .commit_companion_mutation(
                registration, device, 1, b"op-1", b"digest-1", b"result-1", b"change-1",
                &hash, &[1; 32], &[2; 32], 10, "hello", None,
            )
            .unwrap());
        let page = store.resume_companion_changes(&token, 0, 10, 1_001).unwrap();
        assert!(page.changes.is_empty());
        assert!(page.end);
        let token = store
            .begin_companion_snapshot(registration, device, 1, 1_002)
            .unwrap();
        let page = store.resume_companion_changes(&token, 0, 10, 1_003).unwrap();
        assert_eq!(page.changes.len(), 1);
        assert_eq!(page.changes[0].sequence, 1);
        assert!(page.changes[0].entity_id == hash);
        assert!(store.resume_companion_changes(&token, 2, 10, 1_003).is_err());
        store.prune_companion_changes(1).unwrap();
        assert!(store.resume_companion_changes(&token, 0, 10, 1_004).is_err());
    }

    #[test]
    fn snapshot_is_invalidated_by_grant_rotation_and_epoch_reset() {
        let store = MessageStore::memory().unwrap();
        let registration = b"r";
        let device = b"device";
        store.register_device(registration, device).unwrap();
        let token = store.begin_companion_snapshot(registration, device, 1, 0).unwrap();
        store.set_companion_scope(registration, "restricted").unwrap();
        assert!(store.resume_companion_changes(&token, 0, 10, 1).is_err());
        store.register_device(registration, device).unwrap();
        let token = store.begin_companion_snapshot(registration, device, 3, 2).unwrap();
        store.reset_sync_state(&[9; 32]).unwrap();
        assert!(store.resume_companion_changes(&token, 0, 10, 3).is_err());
    }


    #[test]
    fn revocation_blocks_queued_mutations_and_cached_results() {
        let store = MessageStore::memory().unwrap();
        store.register_device(b"r", b"device").unwrap();
        assert!(store
            .authorize_companion(b"r", b"device", 1)
            .unwrap()
            .is_some());
        store.revoke_device(b"r").unwrap();
        assert!(store
            .authorize_companion(b"r", b"device", 1)
            .unwrap()
            .is_none());
        assert!(store
            .commit_companion_mutation(
                b"r",
                b"device",
                1,
                b"queued",
                b"digest",
                b"result",
                b"change",
                &[8; 32],
                &[1; 32],
                &[2; 32],
                10,
                "must fail",
                None,
            )
            .is_err());
        assert!(store
            .authorized_operation_result(b"r", b"device", 1, b"queued")
            .unwrap()
            .is_none());
    }

    #[test]
    fn pruned_operation_id_stays_reserved() {
        let store = MessageStore::memory().unwrap();
        store.register_device(b"r", b"device").unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO operation_result_tombstones
                 (registration_id,operation_id,request_digest,result,pruned_at_ms)
                 VALUES (?1,?2,?3,?4,?5)",
                params![b"r", b"op", b"digest", b"result", 1i64],
            )
            .unwrap();
        assert_eq!(
            store
                .operation_result_or_tombstone(b"r", b"op")
                .unwrap()
                .unwrap()
                .result,
            b"result"
        );
        assert!(store
            .commit_companion_mutation(
                b"r", b"device", 1, b"op", b"other", b"new", b"change",
                &[8; 32], &[1; 32], &[2; 32], 10, "must conflict", None,
            )
            .is_err());
    }
}
