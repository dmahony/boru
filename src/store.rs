#![allow(missing_docs)]

use anyhow::anyhow;
use bytes::Bytes;
use iroh::PublicKey;
use n0_error::{Result, StdResultExt};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub type MessageId = [u8; 32];

/// Delivery status of an outbox message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DeliveryStatus {
    Pending = 0,
    Sent = 1,
    Acked = 2,
    Expired = 3,
}

impl TryFrom<u8> for DeliveryStatus {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        match value {
            0 => Ok(DeliveryStatus::Pending),
            1 => Ok(DeliveryStatus::Sent),
            2 => Ok(DeliveryStatus::Acked),
            3 => Ok(DeliveryStatus::Expired),
            _ => Err(anyhow!("invalid status code")),
        }
    }
}

/// A stored inbound or outbound envelope.
#[derive(Debug, Clone)]
pub struct StoredEnvelope {
    pub msg_id: MessageId,
    pub conversation_id: [u8; 32],
    pub author_user_id: PublicKey,
    pub author_device_id: PublicKey,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub ciphertext: Bytes,
    pub signature: [u8; 64],
    pub acked_at_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct OutboxRow {
    pub msg_id: MessageId,
    pub recipient_device_id: PublicKey,
    pub status: DeliveryStatus,
    pub attempts: u32,
    pub next_attempt_at_ms: u64,
    pub last_error_code: Option<String>,
    pub last_attempt_at_ms: Option<u64>,
}

/// Durable local storage for inbox and outbox messages.
#[derive(Debug, Clone)]
pub struct MessageStore {
    conn: Arc<Mutex<Connection>>,
}

impl MessageStore {
    /// Opens the message store at the given path, creating it if it doesn't exist.
    /// Sets restrictive permissions on Unix systems.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(parent) = path.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent).std_context("create store dir")?;
                }
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }

        let conn = Connection::open(path).std_context("open sqlite db")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Opens an in-memory message store for testing.
    pub fn memory() -> Result<Self> {
        let conn = Connection::open_in_memory().std_context("open in-memory sqlite db")?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
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
        .std_context("init schema")?;
        Ok(())
    }

    /// Inserts an envelope into the inbox idempotently.
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

    /// Get an inbox message by id.
    pub fn get_inbox(&self, msg_id: &MessageId) -> Result<Option<StoredEnvelope>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT conversation_id, author_user_id, author_device_id, created_at_ms, expires_at_ms, ciphertext, signature, acked_at_ms FROM inbox WHERE msg_id = ?1").std_context("prepare get_inbox")?;
        let mut rows = stmt
            .query([msg_id.as_slice()])
            .std_context("query get_inbox")?;

        if let Some(row) = rows.next().std_context("next row")? {
            let mut conversation_id = [0u8; 32];
            let conv_blob: Vec<u8> = row.get(0).std_context("get conversation_id")?;
            conversation_id.copy_from_slice(&conv_blob);

            let author_user_blob: Vec<u8> = row.get(1).std_context("get author_user_id")?;
            let author_user_id = PublicKey::try_from(author_user_blob.as_slice())
                .map_err(|e| anyhow!("invalid public key: {}", e))?;

            let author_device_blob: Vec<u8> = row.get(2).std_context("get author_device_id")?;
            let author_device_id = PublicKey::try_from(author_device_blob.as_slice())
                .map_err(|e| anyhow!("invalid public key: {}", e))?;

            let created_at_ms: i64 = row.get(3).std_context("get created_at_ms")?;
            let expires_at_ms: i64 = row.get(4).std_context("get expires_at_ms")?;

            let ciphertext_blob: Vec<u8> = row.get(5).std_context("get ciphertext")?;
            let ciphertext = Bytes::from(ciphertext_blob);

            let signature_blob: Vec<u8> = row.get(6).std_context("get signature")?;
            let mut signature = [0u8; 64];
            signature.copy_from_slice(&signature_blob);

            let acked_at_ms_i64: Option<i64> = row.get(7).std_context("get acked_at_ms")?;
            let acked_at_ms = acked_at_ms_i64.map(|v| v as u64);

            Ok(Some(StoredEnvelope {
                msg_id: *msg_id,
                conversation_id,
                author_user_id,
                author_device_id,
                created_at_ms: created_at_ms as u64,
                expires_at_ms: expires_at_ms as u64,
                ciphertext,
                signature,
                acked_at_ms,
            }))
        } else {
            Ok(None)
        }
    }

    /// Enqueue a message for direct delivery to a specific recipient device.
    pub fn enqueue_outbox(
        &self,
        msg_id: &MessageId,
        recipient_device_id: PublicKey,
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

    /// Update outbox state when an ACK is received.
    pub fn mark_acked(&self, msg_id: &MessageId, recipient_device_id: PublicKey) -> Result<()> {
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

    /// Update outbox state on delivery attempt.
    pub fn record_attempt(
        &self,
        msg_id: &MessageId,
        recipient_device_id: PublicKey,
        next_attempt_at_ms: u64,
        error_code: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
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

    /// Fetch pending messages that are due for a retry attempt.
    pub fn fetch_due_outbox(&self, now_ms: u64) -> Result<Vec<OutboxRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT msg_id, recipient_device_id, status, attempts, next_attempt_at_ms, last_error_code, last_attempt_at_ms 
             FROM outbox 
             WHERE status != ?1 AND status != ?2 AND next_attempt_at_ms <= ?3"
        ).std_context("prepare fetch due")?;

        let mut rows = stmt
            .query(params![
                DeliveryStatus::Acked as u8,
                DeliveryStatus::Expired as u8,
                now_ms as i64
            ])
            .std_context("query due outbox")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            let msg_blob: Vec<u8> = row.get(0).unwrap();
            let mut msg_id = [0u8; 32];
            msg_id.copy_from_slice(&msg_blob);

            let recipient_blob: Vec<u8> = row.get(1).unwrap();
            let recipient_device_id = PublicKey::try_from(recipient_blob.as_slice()).unwrap();

            let status_code: u8 = row.get(2).unwrap();
            let status = DeliveryStatus::try_from(status_code).unwrap();

            let attempts: u32 = row.get(3).unwrap();
            let next_attempt_at_ms: i64 = row.get(4).unwrap();
            let last_error_code: Option<String> = row.get(5).unwrap();
            let last_attempt_at_ms: Option<i64> = row.get(6).unwrap();

            results.push(OutboxRow {
                msg_id,
                recipient_device_id,
                status,
                attempts,
                next_attempt_at_ms: next_attempt_at_ms as u64,
                last_error_code,
                last_attempt_at_ms: last_attempt_at_ms.map(|v| v as u64),
            });
        }
        Ok(results)
    }

    /// Expire outbox messages that have exceeded their message expiry time.
    pub fn expire_outbox(&self, now_ms: u64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count = conn
            .execute(
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
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_public_key() -> PublicKey {
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        PublicKey::from_bytes(&bytes).unwrap()
    }

    #[test]
    fn test_store_idempotent_insert() {
        let store = MessageStore::memory().unwrap();

        let msg_id = [1u8; 32];
        let envelope = StoredEnvelope {
            msg_id,
            conversation_id: [2u8; 32],
            author_user_id: random_public_key(),
            author_device_id: random_public_key(),
            created_at_ms: 1000,
            expires_at_ms: 5000,
            ciphertext: Bytes::from(vec![1, 2, 3]),
            signature: [3u8; 64],
            acked_at_ms: None,
        };

        store.insert_inbox(&envelope).unwrap();
        store.insert_inbox(&envelope).unwrap(); // Should not error

        let fetched = store.get_inbox(&msg_id).unwrap().unwrap();
        assert_eq!(fetched.msg_id, envelope.msg_id);
    }

    #[test]
    fn test_outbox_flow() {
        let store = MessageStore::memory().unwrap();

        let msg_id = [1u8; 32];
        let recipient = random_public_key();

        store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();

        let due = store.fetch_due_outbox(500).unwrap();
        assert!(due.is_empty());

        let due = store.fetch_due_outbox(1500).unwrap();
        assert_eq!(due.len(), 1);

        store
            .record_attempt(&msg_id, recipient, 3000, Some("timeout"))
            .unwrap();

        let due = store.fetch_due_outbox(1500).unwrap();
        assert!(due.is_empty()); // Next attempt is 3000

        store.mark_acked(&msg_id, recipient).unwrap();
        let due = store.fetch_due_outbox(3500).unwrap();
        assert!(due.is_empty()); // Acked messages shouldn't be retried
    }
}
