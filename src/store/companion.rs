//! Durable companion registrations, idempotent operation results, and sync records.

use super::*;

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
        if let Some(digest) = existing {
            if digest != request_digest {
                return Err(anyhow!("operation id reused with different request").into());
            }
            tx.commit()
                .std_context("commit duplicate companion operation")?;
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO messages(msg_hash,topic,sender,timestamp_ms,kind,body,delivery_state)
             VALUES (?1,?2,?3,?4,'text',?5,'queued')",
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
        }
        let now = unix_now_ms() as i64;
        tx.execute(
            "INSERT INTO operation_results(registration_id,operation_id,request_digest,result,created_at_ms)
             VALUES (?1,?2,?3,?4,?5)",
            params![registration_id, operation_id, request_digest, result, now],
        )
        .std_context("record companion operation result")?;
        tx.execute(
            "INSERT INTO change_references(change_id,registration_id,operation_id,message_hash,created_at_ms)
             VALUES (?1,?2,?3,?4,?5)",
            params![change_id, registration_id, operation_id, msg_hash.as_slice(), now],
        )
        .std_context("record companion change reference")?;
        tx.commit().std_context("commit companion mutation")?;
        Ok(true)
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
}
