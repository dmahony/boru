#![allow(missing_docs)]

use anyhow::anyhow;
use bytes::Bytes;
use iroh::PublicKey;
use n0_error::{Result, StdResultExt};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::chat_core::DIAGNOSTICS;
use crate::diagnostics::DiagnosticEventKind;

/// Helper: produce a stable 8-char hex prefix from a 32-byte hash.
fn short_id(id: &[u8; 32]) -> String {
    hex::encode(&id[..4])
}

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
    pub lease_owner: Option<String>,
    pub locked_until_ms: Option<u64>,
    pub expires_at_ms: Option<u64>,
}

/// Per-conversation metadata tracked in SQLite.
///
/// Added by Step 11 of the storage redesign — lives in the
/// `conversation_meta` table alongside the inbox/outbox tables.
#[derive(Debug, Clone)]
pub struct ConversationMeta {
    /// Conversation identifier (gossip topic bytes).
    pub conversation_id: [u8; 32],
    /// Message id of the most recent message, if any.
    pub last_message_id: Option<MessageId>,
    /// Unix-epoch milliseconds of the most recent activity.
    pub last_activity_at_ms: u64,
    /// Short text preview of the most recent message.
    pub last_message_preview: String,
    /// Public key of the author of the most recent message, if any.
    pub last_author_user_id: Option<PublicKey>,
    /// Number of unread messages in this conversation.
    pub unread_count: u32,
    /// Whether notifications for new messages are muted.
    pub is_muted: bool,
    /// Whether the conversation is archived (hidden from the default list).
    pub is_archived: bool,
    /// Whether the conversation has been locally deleted (soft delete).
    pub is_deleted: bool,
}

/// Outcome of accepting an incoming message into durable local storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncomingMessageResult {
    Inserted,
    Duplicate,
    Conflict,
    Rejected,
}

/// Durable replay bookkeeping for an incoming message id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IncomingReplayMetadata {
    pub first_received_at_ms: u64,
    pub last_received_at_ms: u64,
    pub receive_count: u64,
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

            -- Conversation metadata: unread counts, last message, archive/mute/deleted flags.
            -- Added by storage redesign Step 11.
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

            -- Message tombstones: tracks locally-deleted and remote-deleted messages
            -- so they are not resurrected by backfill, duplicates, or restarts.
            -- Added by storage redesign Step 12.
            CREATE TABLE IF NOT EXISTS message_tombstones (
                msg_id BLOB PRIMARY KEY,
                conversation_id BLOB NOT NULL,
                deleted_at_ms INTEGER NOT NULL,
                deleted_by BLOB NOT NULL,
                signature BLOB NOT NULL,
                is_local INTEGER NOT NULL DEFAULT 1
            );

            -- Durable replay bookkeeping for incoming acceptance.  This is
            -- separate from inbox so duplicate deliveries remain observable
            -- without mutating message history or conversation ordering.
            CREATE TABLE IF NOT EXISTS incoming_replay (
                msg_id BLOB PRIMARY KEY,
                first_received_at_ms INTEGER NOT NULL,
                last_received_at_ms INTEGER NOT NULL,
                receive_count INTEGER NOT NULL DEFAULT 1
            );
            ",
        )
        .std_context("init schema")?;
        Ok(())
    }

    /// Accept an incoming message and all derived conversation state in one
    /// SQLite transaction.  The message id is stable and immutable: reusing
    /// it with different envelope fields is a conflict, never an update.
    pub fn accept_incoming_message(
        &self,
        env: &StoredEnvelope,
        local_user_id: &PublicKey,
    ) -> Result<IncomingMessageResult> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .std_context("begin incoming acceptance transaction")?;

        let tombstoned: bool = tx
            .query_row(
                "SELECT 1 FROM message_tombstones WHERE msg_id = ?1",
                [env.msg_id.as_slice()],
                |row| row.get::<_, i32>(0).map(|v| v != 0),
            )
            .unwrap_or(false);
        if tombstoned {
            tx.commit()
                .std_context("commit rejected incoming message")?;
            return Ok(IncomingMessageResult::Rejected);
        }

        let existing = tx
            .query_row(
                "SELECT conversation_id, author_user_id, author_device_id,
                        created_at_ms, expires_at_ms, ciphertext, signature, acked_at_ms
                 FROM inbox WHERE msg_id = ?1",
                [env.msg_id.as_slice()],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                        row.get::<_, Vec<u8>>(6)?,
                        row.get::<_, Option<i64>>(7)?,
                    ))
                },
            )
            .optional()
            .std_context("lookup incoming message id")?;

        if let Some((
            conversation_id,
            author_user_id,
            author_device_id,
            created_at,
            expires_at,
            ciphertext,
            signature,
            acked_at,
        )) = existing
        {
            let matches = conversation_id == env.conversation_id.as_slice()
                && author_user_id == env.author_user_id.as_bytes()
                && author_device_id == env.author_device_id.as_bytes()
                && created_at == env.created_at_ms as i64
                && expires_at == env.expires_at_ms as i64
                && ciphertext == env.ciphertext.as_ref()
                && signature == env.signature.as_slice()
                && acked_at == env.acked_at_ms.map(|v| v as i64);
            if !matches {
                tx.commit()
                    .std_context("commit conflicting incoming message")?;
                return Ok(IncomingMessageResult::Conflict);
            }

            tx.execute(
                "UPDATE incoming_replay
                 SET last_received_at_ms = ?2, receive_count = receive_count + 1
                 WHERE msg_id = ?1",
                params![env.msg_id.as_slice(), unix_now_ms() as i64],
            )
            .std_context("update incoming replay metadata")?;
            tx.commit()
                .std_context("commit duplicate incoming message")?;
            return Ok(IncomingMessageResult::Duplicate);
        }

        let acked = env.acked_at_ms.map(|v| v as i64);
        tx.execute(
            "INSERT INTO inbox (
                msg_id, conversation_id, author_user_id, author_device_id,
                created_at_ms, expires_at_ms, ciphertext, signature, acked_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
        .std_context("insert incoming message")?;

        let now = unix_now_ms();
        tx.execute(
            "INSERT INTO incoming_replay
             (msg_id, first_received_at_ms, last_received_at_ms, receive_count)
             VALUES (?1, ?2, ?2, 1)",
            params![env.msg_id.as_slice(), now as i64],
        )
        .std_context("insert incoming replay metadata")?;

        let preview = format!("[{} bytes]", env.ciphertext.len());
        let unread_increment = if env.author_user_id != *local_user_id {
            1
        } else {
            0
        };
        tx.execute(
            "INSERT INTO conversation_meta (
                conversation_id, last_message_id, last_activity_at_ms,
                last_message_preview, last_author_user_id, unread_count
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(conversation_id) DO UPDATE SET
                last_message_id = excluded.last_message_id,
                last_activity_at_ms = excluded.last_activity_at_ms,
                last_message_preview = excluded.last_message_preview,
                last_author_user_id = excluded.last_author_user_id,
                unread_count = conversation_meta.unread_count + excluded.unread_count",
            params![
                env.conversation_id.as_slice(),
                env.msg_id.as_slice(),
                now as i64,
                preview,
                env.author_user_id.as_bytes(),
                unread_increment,
            ],
        )
        .std_context("update incoming conversation metadata")?;
        tx.commit().std_context("commit incoming acceptance")?;
        Ok(IncomingMessageResult::Inserted)
    }

    /// Return durable replay metadata for a message id.
    pub fn get_incoming_replay_metadata(
        &self,
        msg_id: &MessageId,
    ) -> Result<Option<IncomingReplayMetadata>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT first_received_at_ms, last_received_at_ms, receive_count
             FROM incoming_replay WHERE msg_id = ?1",
            [msg_id.as_slice()],
            |row| {
                Ok(IncomingReplayMetadata {
                    first_received_at_ms: row.get::<_, i64>(0)? as u64,
                    last_received_at_ms: row.get::<_, i64>(1)? as u64,
                    receive_count: row.get::<_, i64>(2)? as u64,
                })
            },
        )
        .optional()
        .std_context("get incoming replay metadata")
    }

    // ── Basic inbox/outbox operations (existing) ──────────────────────

    /// Inserts an envelope into the inbox idempotently.
    ///
    /// Returns `true` if a new row was inserted, `false` if a duplicate
    /// was silently ignored or the message has been tombstoned.
    pub fn insert_inbox(&self, env: &StoredEnvelope) -> Result<bool> {
        let conn = self.conn.lock().unwrap();

        // Reject tombstoned messages — they can't be resurrected by backfill
        // or duplicate delivery.
        let tombstoned: bool = conn
            .query_row(
                "SELECT 1 FROM message_tombstones WHERE msg_id = ?1",
                [env.msg_id.as_slice()],
                |row| row.get::<_, i32>(0).map(|v| v != 0),
            )
            .unwrap_or(false);
        if tombstoned {
            return Ok(false);
        }

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
        let is_new = conn.changes() > 0;
        // Record diagnostic event.
        let msg_short = short_id(&env.msg_id);
        let conv_prefix = short_id(&env.conversation_id);
        let peer = Some(env.author_user_id.to_string());
        if is_new {
            DIAGNOSTICS.record_with_peer(
                None,
                peer.as_deref(),
                DiagnosticEventKind::IncomingPersisted {
                    message_id_short: Some(msg_short),
                    conversation_id_prefix: Some(conv_prefix),
                    peer_id: peer.clone(),
                    delivery_state: "Inbox".to_string(),
                },
            );
        } else {
            DIAGNOSTICS.record_with_peer(
                None,
                peer.as_deref(),
                DiagnosticEventKind::DuplicateReceived {
                    message_id_short: Some(msg_short),
                    conversation_id_prefix: Some(conv_prefix),
                    peer_id: peer.clone(),
                },
            );
        }
        Ok(is_new)
    }

    /// Inserts an envelope and atomically updates conversation metadata,
    /// including the unread count.
    ///
    /// `local_user_id` is the local user's [`PublicKey`]; messages authored
    /// by the local user do **not** increment the unread count.
    ///
    /// Returns `true` if a new row was inserted, `false` if a duplicate
    /// was silently ignored or the message has been tombstoned.
    pub fn insert_inbox_with_conversation_update(
        &self,
        env: &StoredEnvelope,
        local_user_id: &PublicKey,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();

        // Reject tombstoned messages — they can't be resurrected by backfill
        // or duplicate delivery.
        let tombstoned: bool = conn
            .query_row(
                "SELECT 1 FROM message_tombstones WHERE msg_id = ?1",
                [env.msg_id.as_slice()],
                |row| row.get::<_, i32>(0).map(|v| v != 0),
            )
            .unwrap_or(false);
        if tombstoned {
            return Ok(false);
        }

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
        .std_context("insert inbox with conversation update")?;

        let is_new = conn.changes() > 0;

        // Build a short preview from the ciphertext length (we can't decrypt here).
        let preview = format!("[{} bytes]", env.ciphertext.len());

        // Upsert conversation_meta: ensure a row exists, update last message fields.
        conn.execute(
            "INSERT INTO conversation_meta (
                conversation_id, last_message_id, last_activity_at_ms,
                last_message_preview, last_author_user_id, unread_count
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(conversation_id) DO UPDATE SET
                last_message_id = ?2,
                last_activity_at_ms = ?3,
                last_message_preview = ?4,
                last_author_user_id = ?5,
                unread_count = CASE
                    WHEN ?6 = 1 THEN unread_count + 1
                    ELSE unread_count
                END",
            params![
                env.conversation_id.as_slice(),
                env.msg_id.as_slice(),
                env.created_at_ms as i64,
                preview,
                env.author_user_id.as_bytes(),
                // Increment unread only if this is a new message AND
                // the author is NOT the local user.
                if is_new && env.author_user_id != *local_user_id {
                    1i32
                } else {
                    0i32
                },
            ],
        )
        .std_context("upsert conversation meta")?;

        // Record diagnostic event.
        let msg_short = short_id(&env.msg_id);
        let conv_prefix = short_id(&env.conversation_id);
        let peer = Some(env.author_user_id.to_string());
        if is_new {
            DIAGNOSTICS.record_with_peer(
                None,
                peer.as_deref(),
                DiagnosticEventKind::IncomingPersisted {
                    message_id_short: Some(msg_short),
                    conversation_id_prefix: Some(conv_prefix),
                    peer_id: peer.clone(),
                    delivery_state: "Inbox".to_string(),
                },
            );
        } else {
            DIAGNOSTICS.record_with_peer(
                None,
                peer.as_deref(),
                DiagnosticEventKind::DuplicateReceived {
                    message_id_short: Some(msg_short),
                    conversation_id_prefix: Some(conv_prefix),
                    peer_id: peer.clone(),
                },
            );
        }

        Ok(is_new)
    }

    /// Update the last-message preview text for a conversation.
    ///
    /// This is a separate operation because the actual plaintext is only
    /// available after decryption, which may happen at a different time
    /// than the initial inbox insert.  The initial insert uses a
    /// placeholder preview (`[N bytes]`); this method replaces it with
    /// the actual text once decrypted.
    pub fn update_last_message_preview(
        &self,
        conversation_id: &[u8; 32],
        preview: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE conversation_meta SET last_message_preview = ?1
             WHERE conversation_id = ?2",
            params![preview, conversation_id.as_slice()],
        )
        .std_context("update last message preview")?;
        Ok(())
    }

    /// Get an inbox message by id.
    ///
    /// Returns `None` if the message doesn't exist or has been tombstoned.
    pub fn get_inbox(&self, msg_id: &MessageId) -> Result<Option<StoredEnvelope>> {
        let conn = self.conn.lock().unwrap();

        // Check tombstone first — tombstoned messages are treated as non-existent.
        let tombstoned: bool = conn
            .query_row(
                "SELECT 1 FROM message_tombstones WHERE msg_id = ?1",
                [msg_id.as_slice()],
                |row| row.get::<_, i32>(0).map(|v| v != 0),
            )
            .unwrap_or(false);
        if tombstoned {
            return Ok(None);
        }

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

    // ── Conversation metadata operations ──────────────────────────────

    /// Atomically reset the unread count for a conversation to zero.
    ///
    /// Returns the previous unread count, or `None` if no metadata row
    /// exists for this conversation.
    pub fn mark_conversation_read(&self, conversation_id: &[u8; 32]) -> Result<Option<u32>> {
        let conn = self.conn.lock().unwrap();
        // Read current unread count
        let prev: Option<u32> = conn
            .query_row(
                "SELECT unread_count FROM conversation_meta WHERE conversation_id = ?1",
                [conversation_id.as_slice()],
                |row| row.get(0),
            )
            .std_context("query current unread count")
            .ok(); // None if no row yet

        conn.execute(
            "UPDATE conversation_meta SET unread_count = 0 WHERE conversation_id = ?1",
            [conversation_id.as_slice()],
        )
        .std_context("reset unread count")?;

        Ok(prev)
    }

    /// Get the unread count for a conversation, or `None` if the
    /// conversation has no metadata row yet.
    pub fn get_unread_count(&self, conversation_id: &[u8; 32]) -> Result<Option<u32>> {
        let conn = self.conn.lock().unwrap();
        let count: Option<u32> = conn
            .query_row(
                "SELECT unread_count FROM conversation_meta WHERE conversation_id = ?1",
                [conversation_id.as_slice()],
                |row| row.get(0),
            )
            .std_context("query unread count")
            .ok();
        Ok(count)
    }

    /// Get the total (summed) unread count across all non-deleted
    /// conversations.
    pub fn total_unread_count(&self) -> Result<u32> {
        let conn = self.conn.lock().unwrap();
        let count: u32 = conn
            .query_row(
                "SELECT COALESCE(SUM(unread_count), 0) FROM conversation_meta
                 WHERE is_deleted = 0",
                [],
                |row| row.get(0),
            )
            .std_context("query total unread count")?;
        Ok(count)
    }

    /// Retrieve the full [`ConversationMeta`] for a conversation, or
    /// `None` if no metadata row exists.
    pub fn get_conversation_meta(
        &self,
        conversation_id: &[u8; 32],
    ) -> Result<Option<ConversationMeta>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT conversation_id, last_message_id, last_activity_at_ms,
                        last_message_preview, last_author_user_id,
                        unread_count, is_muted, is_archived, is_deleted
                 FROM conversation_meta WHERE conversation_id = ?1",
            )
            .std_context("prepare get_conversation_meta")?;

        let mut rows = stmt
            .query([conversation_id.as_slice()])
            .std_context("query get_conversation_meta")?;

        if let Some(row) = rows.next().std_context("next row")? {
            Ok(Some(row_to_conversation_meta(row)?))
        } else {
            Ok(None)
        }
    }

    /// Set the archived flag for a conversation.
    pub fn set_conversation_archived(
        &self,
        conversation_id: &[u8; 32],
        archived: bool,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO conversation_meta (conversation_id, last_activity_at_ms, is_archived)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(conversation_id) DO UPDATE SET is_archived = ?3",
            params![
                conversation_id.as_slice(),
                unix_now_ms() as i64,
                archived as i32,
            ],
        )
        .std_context("set conversation archived")?;
        Ok(())
    }

    /// Set the muted flag for a conversation.
    pub fn set_conversation_muted(&self, conversation_id: &[u8; 32], muted: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO conversation_meta (conversation_id, last_activity_at_ms, is_muted)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(conversation_id) DO UPDATE SET is_muted = ?3",
            params![
                conversation_id.as_slice(),
                unix_now_ms() as i64,
                muted as i32,
            ],
        )
        .std_context("set conversation muted")?;
        Ok(())
    }

    /// Locally delete a conversation: removes all inbox messages for the
    /// conversation and soft-deletes the metadata row.
    ///
    /// **Does NOT touch outbox/outgoing messages** — pending outgoing
    /// messages for this conversation are preserved so they can still be
    /// delivered.  Use [`delete_outgoing_for_conversation`] for the
    /// explicit "delete everything" path.
    ///
    /// Returns the number of inbox messages removed.
    pub fn delete_conversation(&self, conversation_id: &[u8; 32]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        // Remove inbox messages for this conversation
        let removed = conn
            .execute(
                "DELETE FROM inbox WHERE conversation_id = ?1",
                [conversation_id.as_slice()],
            )
            .std_context("delete inbox messages for conversation")?;

        // Soft-delete the metadata row
        conn.execute(
            "INSERT INTO conversation_meta (conversation_id, last_activity_at_ms, is_deleted)
             VALUES (?1, ?2, 1)
             ON CONFLICT(conversation_id) DO UPDATE SET is_deleted = 1",
            params![conversation_id.as_slice(), unix_now_ms() as i64],
        )
        .std_context("soft-delete conversation meta")?;

        Ok(removed)
    }

    /// Hard-delete a conversation: removes inbox messages AND pending
    /// outgoing messages for this conversation, and removes the metadata
    /// row entirely.
    ///
    /// This is the explicit "delete everything" path.  Only use this when
    /// the user explicitly confirms they want to discard pending outgoing
    /// messages as well.
    pub fn hard_delete_conversation(&self, conversation_id: &[u8; 32]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();

        // Capture the msg_ids before deleting, so we can also remove
        // corresponding outbox rows.
        let mut stmt = conn
            .prepare("SELECT msg_id FROM inbox WHERE conversation_id = ?1")
            .std_context("prepare select msg_ids for hard delete")?;
        let msg_ids: Vec<Vec<u8>> = stmt
            .query_map([conversation_id.as_slice()], |row| row.get(0))
            .std_context("query msg_ids for hard delete")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .std_context("collect msg_ids")?;

        // Delete inbox messages for this conversation
        let removed_inbox = conn
            .execute(
                "DELETE FROM inbox WHERE conversation_id = ?1",
                [conversation_id.as_slice()],
            )
            .std_context("hard delete inbox messages")?;

        // Delete corresponding outbox rows
        for msg_blob in &msg_ids {
            conn.execute(
                "DELETE FROM outbox WHERE msg_id = ?1",
                [msg_blob.as_slice()],
            )
            .std_context("hard delete outbox row")?;
        }

        // Remove metadata row entirely
        conn.execute(
            "DELETE FROM conversation_meta WHERE conversation_id = ?1",
            [conversation_id.as_slice()],
        )
        .std_context("delete conversation meta row")?;

        Ok(removed_inbox)
    }

    /// Delete pending outgoing messages for a specific conversation.
    ///
    /// Only removes messages with status `Pending` or `Sent` — already
    /// acked messages are left alone.
    ///
    /// Returns the number of outbox rows removed.
    pub fn delete_pending_outgoing_for_conversation(
        &self,
        conversation_id: &[u8; 32],
    ) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let removed = conn
            .execute(
                "DELETE FROM outbox WHERE msg_id IN (
                    SELECT msg_id FROM inbox WHERE conversation_id = ?1
                ) AND status NOT IN (?2, ?3)",
                params![
                    conversation_id.as_slice(),
                    DeliveryStatus::Acked as u8,
                    DeliveryStatus::Expired as u8,
                ],
            )
            .std_context("delete pending outgoing for conversation")?;
        Ok(removed)
    }

    /// List all non-deleted conversations, ordered by most recent activity
    /// first.
    ///
    /// If `include_archived` is `true`, archived conversations are included;
    /// otherwise they are filtered out.
    pub fn list_conversations(&self, include_archived: bool) -> Result<Vec<ConversationMeta>> {
        let conn = self.conn.lock().unwrap();
        let sql = if include_archived {
            "SELECT conversation_id, last_message_id, last_activity_at_ms,
                    last_message_preview, last_author_user_id,
                    unread_count, is_muted, is_archived, is_deleted
             FROM conversation_meta
             WHERE is_deleted = 0
             ORDER BY last_activity_at_ms DESC"
        } else {
            "SELECT conversation_id, last_message_id, last_activity_at_ms,
                    last_message_preview, last_author_user_id,
                    unread_count, is_muted, is_archived, is_deleted
             FROM conversation_meta
             WHERE is_deleted = 0 AND is_archived = 0
             ORDER BY last_activity_at_ms DESC"
        };
        let mut stmt = conn
            .prepare(sql)
            .std_context("prepare list_conversations")?;
        let mut rows = stmt.query([]).std_context("query list_conversations")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(row_to_conversation_meta(row)?);
        }
        Ok(results)
    }

    // ── Deletion and tombstone methods (Step 12) ──────────────────────

    /// Locally delete a single message: insert a local tombstone to prevent
    /// backfill resurrection, remove the inbox row, and cancel any pending
    /// outbound deliveries for this message.
    ///
    /// This is a **local-only** operation — no protocol message is sent to
    /// peers.  Use [`insert_tombstone`] for remote-initiated deletions.
    ///
    /// Returns `true` if the message was found and deleted.
    pub fn delete_message(&self, msg_id: &MessageId) -> Result<bool> {
        let conn = self.conn.lock().unwrap();

        // Read conversation_id before deleting.
        let conv_blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT conversation_id FROM inbox WHERE msg_id = ?1",
                [msg_id.as_slice()],
                |row| row.get(0),
            )
            .std_context("query conversation_id for delete")
            .ok();

        let conversation_id = match conv_blob {
            Some(ref blob) => {
                let mut id = [0u8; 32];
                id.copy_from_slice(blob);
                id
            }
            None => return Ok(false), // Message not found
        };

        let now = unix_now_ms();

        // Insert tombstone.
        conn.execute(
            "INSERT OR IGNORE INTO message_tombstones (msg_id, conversation_id, deleted_at_ms, deleted_by, signature, is_local)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![
                msg_id.as_slice(),
                conversation_id.as_slice(),
                now as i64,
                // deleted_by is the local message author (zeros for local)
                [0u8; 32].as_slice(),
                // signature is empty for local deletions
                [0u8; 0].as_slice(),
            ],
        )
        .std_context("insert local tombstone")?;

        // Remove from inbox.
        conn.execute("DELETE FROM inbox WHERE msg_id = ?1", [msg_id.as_slice()])
            .std_context("delete inbox message")?;

        // Cancel any pending outbound deliveries for this message.
        conn.execute(
            "UPDATE outbox SET status = ?1
             WHERE msg_id = ?2 AND status NOT IN (?3, ?4)",
            params![
                DeliveryStatus::Expired as u8,
                msg_id.as_slice(),
                DeliveryStatus::Acked as u8,
                DeliveryStatus::Expired as u8,
            ],
        )
        .std_context("cancel outbound for deleted message")?;

        Ok(true)
    }

    /// Cancel pending outbound delivery for a message, removing it from
    /// retry scheduling.
    ///
    /// Returns the number of outbox rows affected (0 if the message had
    /// no pending outbound entries).
    pub fn cancel_pending_outbound(&self, msg_id: &MessageId) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count = conn
            .execute(
                "UPDATE outbox SET status = ?1
                 WHERE msg_id = ?2 AND status NOT IN (?3, ?4)",
                params![
                    DeliveryStatus::Expired as u8,
                    msg_id.as_slice(),
                    DeliveryStatus::Acked as u8,
                    DeliveryStatus::Expired as u8,
                ],
            )
            .std_context("cancel pending outbound")?;
        Ok(count)
    }

    /// Insert a remote-delete tombstone (from a protocol message).
    ///
    /// This records that the message's author authorized its deletion.
    /// The signature must be validated by the caller before calling this.
    ///
    /// Also removes the inbox row so the message is no longer visible.
    /// Returns `true` if a new tombstone was inserted, `false` if the
    /// message was already tombstoned.
    pub fn insert_tombstone(
        &self,
        msg_id: &MessageId,
        conversation_id: &[u8; 32],
        deleted_by: &PublicKey,
        signature: &[u8],
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let now = unix_now_ms();

        conn.execute(
            "INSERT OR IGNORE INTO message_tombstones (msg_id, conversation_id, deleted_at_ms, deleted_by, signature, is_local)
             VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            params![
                msg_id.as_slice(),
                conversation_id.as_slice(),
                now as i64,
                deleted_by.as_bytes(),
                signature,
            ],
        )
        .std_context("insert remote tombstone")?;

        let is_new = conn.changes() > 0;

        // Remove the inbox row if it exists.
        conn.execute("DELETE FROM inbox WHERE msg_id = ?1", [msg_id.as_slice()])
            .std_context("delete inbox for tombstoned message")?;

        // Cancel pending outbound deliveries.
        conn.execute(
            "UPDATE outbox SET status = ?1
             WHERE msg_id = ?2 AND status NOT IN (?3, ?4)",
            params![
                DeliveryStatus::Expired as u8,
                msg_id.as_slice(),
                DeliveryStatus::Acked as u8,
                DeliveryStatus::Expired as u8,
            ],
        )
        .std_context("cancel outbound for tombstoned message")?;

        Ok(is_new)
    }

    /// Check whether a message has been tombstoned (locally or remotely deleted).
    ///
    /// Returns `true` if a tombstone exists for this message id.
    pub fn is_tombstoned(&self, msg_id: &MessageId) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM message_tombstones WHERE msg_id = ?1",
                [msg_id.as_slice()],
                |row| row.get::<_, i32>(0).map(|v| v != 0),
            )
            .std_context("check tombstone")
            .unwrap_or(false);
        Ok(exists)
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn row_to_conversation_meta(row: &rusqlite::Row) -> Result<ConversationMeta> {
    let conv_blob: Vec<u8> = row.get(0).std_context("get conversation_id")?;
    let mut conversation_id = [0u8; 32];
    conversation_id.copy_from_slice(&conv_blob);

    let last_msg_blob: Option<Vec<u8>> = row.get(1).std_context("get last_message_id")?;
    let last_message_id = last_msg_blob.map(|blob| {
        let mut id = [0u8; 32];
        id.copy_from_slice(&blob);
        id
    });

    let last_activity_at_ms: i64 = row.get(2).std_context("get last_activity_at_ms")?;
    let last_message_preview: String = row.get(3).std_context("get last_message_preview")?;

    let last_author_blob: Option<Vec<u8>> = row.get(4).std_context("get last_author_user_id")?;
    let last_author_user_id =
        last_author_blob.map(|blob| PublicKey::try_from(blob.as_slice()).unwrap());

    let unread_count: u32 = row.get(5).std_context("get unread_count")?;
    let is_muted: bool = row.get::<_, i32>(6).std_context("get is_muted")? != 0;
    let is_archived: bool = row.get::<_, i32>(7).std_context("get is_archived")? != 0;
    let is_deleted: bool = row.get::<_, i32>(8).std_context("get is_deleted")? != 0;

    Ok(ConversationMeta {
        conversation_id,
        last_message_id,
        last_activity_at_ms: last_activity_at_ms as u64,
        last_message_preview,
        last_author_user_id,
        unread_count,
        is_muted,
        is_archived,
        is_deleted,
    })
}

// ── Outbox operations ──────────────────────────────────────────────────────

impl MessageStore {
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
        let is_new = conn.changes() > 0;
        if is_new {
            let peer = recipient_device_id.to_string();
            DIAGNOSTICS.record_with_peer(
                None,
                Some(&peer),
                DiagnosticEventKind::MessageQueued {
                    message_id_short: Some(short_id(msg_id)),
                    conversation_id_prefix: None,
                    peer_id: Some(peer.clone()),
                    delivery_state: "Pending".to_string(),
                },
            );
        }
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
        if conn.changes() > 0 {
            let peer = recipient_device_id.to_string();
            DIAGNOSTICS.record_with_peer(
                None,
                Some(&peer),
                DiagnosticEventKind::AckReceived {
                    message_id_short: Some(short_id(msg_id)),
                    conversation_id_prefix: None,
                    peer_id: Some(peer.clone()),
                    attempt_count: 0,
                    elapsed_ms: None,
                },
            );
        }
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
             WHERE msg_id = ?5 AND recipient_device_id = ?6 AND status != ?7 AND status != ?8",
            params![
                next_attempt_at_ms as i64,
                error_code,
                now_ms as i64,
                DeliveryStatus::Sent as u8,
                msg_id.as_slice(),
                recipient_device_id.as_bytes(),
                DeliveryStatus::Acked as u8,
                DeliveryStatus::Expired as u8,
            ],
        )
        .std_context("record attempt")?;
        if conn.changes() > 0 {
            let peer = recipient_device_id.to_string();
            let msg_short = short_id(msg_id);
            let delay = next_attempt_at_ms.saturating_sub(now_ms);
            if let Some(err) = error_code {
                let category = if err.contains("timeout") || err.contains("Connection") {
                    "connection".to_string()
                } else if err.contains("reject") || err.contains("unauthorized") {
                    "rejected".to_string()
                } else if err.contains("expir") {
                    "expired".to_string()
                } else {
                    "transient".to_string()
                };
                DIAGNOSTICS.record_with_peer(
                    None,
                    Some(&peer),
                    DiagnosticEventKind::RetryScheduled {
                        message_id_short: Some(msg_short),
                        conversation_id_prefix: None,
                        peer_id: Some(peer.clone()),
                        attempt_count: 0, // actual count read from DB separately
                        retry_delay_ms: delay,
                        failure_category: category,
                    },
                );
            } else {
                DIAGNOSTICS.record_with_peer(
                    None,
                    Some(&peer),
                    DiagnosticEventKind::DeliveryAttemptStarted {
                        message_id_short: Some(msg_short),
                        conversation_id_prefix: None,
                        peer_id: Some(peer.clone()),
                        attempt_count: 0,
                        retry_delay_ms: None,
                    },
                );
            }
        }
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
                lease_owner: None,
                locked_until_ms: None,
                expires_at_ms: None,
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
        if count > 0 {
            DIAGNOSTICS.record(
                None,
                DiagnosticEventKind::MessageExpired {
                    message_id_short: None,
                    conversation_id_prefix: None,
                    peer_id: None,
                    delivery_state: format!("{:?}", DeliveryStatus::Expired),
                },
            );
        }
        Ok(count)
    }

    /// Remove an outbox entry entirely (e.g. sender cancellation).
    ///
    /// Returns `true` if a row was deleted.
    pub fn remove_outbox_entry(&self, msg_id: &MessageId) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM outbox WHERE msg_id = ?1", [msg_id.as_slice()])
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    /// Remove all outbox entries for a specific recipient.
    ///
    /// Returns the number of rows deleted.
    pub fn remove_outbox_for_recipient(&self, recipient: &PublicKey) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM outbox WHERE recipient_device_id = ?1",
            [recipient.as_bytes()],
        )
        .unwrap_or(0)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incoming_acceptance_persists_replay_metadata_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let local = other_public_key(8);
        let env = make_envelope([8u8; 32], [6u8; 32], random_public_key());
        {
            let store = MessageStore::open(dir.path().join("messages.db")).unwrap();
            assert_eq!(
                store.accept_incoming_message(&env, &local).unwrap(),
                IncomingMessageResult::Inserted
            );
            assert_eq!(
                store.accept_incoming_message(&env, &local).unwrap(),
                IncomingMessageResult::Duplicate
            );
        }
        let reopened = MessageStore::open(dir.path().join("messages.db")).unwrap();
        let replay = reopened
            .get_incoming_replay_metadata(&env.msg_id)
            .unwrap()
            .unwrap();
        assert_eq!(replay.receive_count, 2);
        assert_eq!(
            reopened.get_unread_count(&env.conversation_id).unwrap(),
            Some(1)
        );
    }

    #[test]
    fn incoming_acceptance_rejects_tombstoned_message() {
        let store = MessageStore::memory().unwrap();
        let env = make_envelope([4u8; 32], [5u8; 32], random_public_key());
        store
            .insert_tombstone(&env.msg_id, &env.conversation_id, &env.author_user_id, &[])
            .unwrap();
        assert_eq!(
            store
                .accept_incoming_message(&env, &other_public_key(8))
                .unwrap(),
            IncomingMessageResult::Rejected
        );
        assert!(store
            .get_conversation_meta(&env.conversation_id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn incoming_acceptance_is_idempotent_and_detects_conflicts() {
        let store = MessageStore::memory().unwrap();
        let local = other_public_key(8);
        let remote = random_public_key();
        let conv = [9u8; 32];
        let env = make_envelope([7u8; 32], conv, remote);

        assert_eq!(
            store.accept_incoming_message(&env, &local).unwrap(),
            IncomingMessageResult::Inserted
        );
        let first = store.get_conversation_meta(&conv).unwrap().unwrap();
        assert_eq!(first.unread_count, 1);
        assert_eq!(
            store.accept_incoming_message(&env, &local).unwrap(),
            IncomingMessageResult::Duplicate
        );
        let second = store.get_conversation_meta(&conv).unwrap().unwrap();
        assert_eq!(second.unread_count, 1);
        assert_eq!(second.last_activity_at_ms, first.last_activity_at_ms);

        let mut conflict = env.clone();
        conflict.ciphertext = Bytes::from(vec![99, 98]);
        assert_eq!(
            store.accept_incoming_message(&conflict, &local).unwrap(),
            IncomingMessageResult::Conflict
        );
        assert_eq!(store.get_unread_count(&conv).unwrap(), Some(1));
    }

    fn random_public_key() -> PublicKey {
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        PublicKey::from_bytes(&bytes).unwrap()
    }

    fn other_public_key(base: u8) -> PublicKey {
        // Derive a deterministic but valid Ed25519 public key from a seed.
        use iroh::SecretKey;
        let seed = [base; 32];
        SecretKey::from_bytes(&seed).public()
    }

    fn make_envelope(
        msg_id: [u8; 32],
        conversation_id: [u8; 32],
        author: PublicKey,
    ) -> StoredEnvelope {
        StoredEnvelope {
            msg_id,
            conversation_id,
            author_user_id: author,
            author_device_id: author,
            created_at_ms: 1000,
            expires_at_ms: 5000,
            ciphertext: Bytes::from(vec![1, 2, 3]),
            signature: [3u8; 64],
            acked_at_ms: None,
        }
    }

    // ── Existing tests ─────────────────────────────────────────────────

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

        assert!(store.insert_inbox(&envelope).unwrap()); // new insert
        assert!(!store.insert_inbox(&envelope).unwrap()); // duplicate

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

    // ── Conversation meta table tests (Step 11) ────────────────────────

    #[test]
    fn test_conversation_meta_table_created_on_init() {
        let store = MessageStore::memory().unwrap();
        // Schema init should not error; meta table exists implicitly.
        // Verify by inserting a meta row manually.
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO conversation_meta (conversation_id, last_activity_at_ms)
             VALUES (X'aa', 1000)",
            [],
        )
        .unwrap();
        let count: u32 = conn
            .query_row("SELECT COUNT(*) FROM conversation_meta", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_insert_inbox_returns_new_vs_duplicate() {
        let store = MessageStore::memory().unwrap();
        let env = make_envelope([1u8; 32], [2u8; 32], random_public_key());

        // First insert should return true
        assert!(store.insert_inbox(&env).unwrap());
        // Duplicate insert should return false
        assert!(!store.insert_inbox(&env).unwrap());
    }

    #[test]
    fn test_insert_with_conversation_update_and_unread() {
        let store = MessageStore::memory().unwrap();
        let local = random_public_key();
        let remote = other_public_key(42);
        let conv_id = [2u8; 32];
        let msg_id1 = [1u8; 32];
        let msg_id2 = [2u8; 32];

        // 1. Remote sends a message → unread should be 1
        let env1 = make_envelope(msg_id1, conv_id, remote);
        assert!(store
            .insert_inbox_with_conversation_update(&env1, &local)
            .unwrap());
        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert_eq!(meta.unread_count, 1);
        assert!(!meta.is_muted);
        assert!(!meta.is_archived);
        assert!(!meta.is_deleted);

        // 2. Duplicate of same message → unread should NOT increment
        assert!(!store
            .insert_inbox_with_conversation_update(&env1, &local)
            .unwrap());
        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert_eq!(meta.unread_count, 1);

        // 3. Second remote message → unread should be 2
        let env2 = make_envelope(msg_id2, conv_id, remote);
        assert!(store
            .insert_inbox_with_conversation_update(&env2, &local)
            .unwrap());
        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert_eq!(meta.unread_count, 2);
    }

    #[test]
    fn test_self_sent_does_not_increment_unread() {
        let store = MessageStore::memory().unwrap();
        let local = random_public_key();
        let conv_id = [2u8; 32];

        // Local user sends a message → unread should be 0
        let env = make_envelope([1u8; 32], conv_id, local);
        assert!(store
            .insert_inbox_with_conversation_update(&env, &local)
            .unwrap());
        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert_eq!(meta.unread_count, 0);
    }

    #[test]
    fn test_mark_conversation_read() {
        let store = MessageStore::memory().unwrap();
        let local = random_public_key();
        let remote = other_public_key(42);
        let conv_id = [2u8; 32];

        // Send two messages from remote
        let env1 = make_envelope([1u8; 32], conv_id, remote);
        let env2 = make_envelope([2u8; 32], conv_id, remote);
        store
            .insert_inbox_with_conversation_update(&env1, &local)
            .unwrap();
        store
            .insert_inbox_with_conversation_update(&env2, &local)
            .unwrap();

        // Mark read
        let prev = store.mark_conversation_read(&conv_id).unwrap().unwrap();
        assert_eq!(prev, 2);

        // Unread should now be 0
        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert_eq!(meta.unread_count, 0);
    }

    #[test]
    fn test_mark_read_non_existent_conversation() {
        let store = MessageStore::memory().unwrap();
        let conv_id = [99u8; 32];
        // No meta row yet → returns None
        let prev = store.mark_conversation_read(&conv_id).unwrap();
        assert!(prev.is_none());
    }

    #[test]
    fn test_get_unread_count() {
        let store = MessageStore::memory().unwrap();
        let local = random_public_key();
        let remote = other_public_key(42);
        let conv_id = [2u8; 32];

        // No messages → no meta row → returns None
        assert!(store.get_unread_count(&conv_id).unwrap().is_none());

        // After a remote message → returns Some(1)
        let env = make_envelope([1u8; 32], conv_id, remote);
        store
            .insert_inbox_with_conversation_update(&env, &local)
            .unwrap();
        assert_eq!(store.get_unread_count(&conv_id).unwrap().unwrap(), 1);
    }

    #[test]
    fn test_total_unread_count() {
        let store = MessageStore::memory().unwrap();
        let local = random_public_key();
        let remote = other_public_key(42);

        let conv_a = [1u8; 32];
        let conv_b = [2u8; 32];

        assert_eq!(store.total_unread_count().unwrap(), 0);

        // 2 unread in conv_a, 1 in conv_b
        store
            .insert_inbox_with_conversation_update(
                &make_envelope([1u8; 32], conv_a, remote),
                &local,
            )
            .unwrap();
        store
            .insert_inbox_with_conversation_update(
                &make_envelope([2u8; 32], conv_a, remote),
                &local,
            )
            .unwrap();
        store
            .insert_inbox_with_conversation_update(
                &make_envelope([3u8; 32], conv_b, remote),
                &local,
            )
            .unwrap();

        assert_eq!(store.total_unread_count().unwrap(), 3);
    }

    #[test]
    fn test_archive_and_unarchive() {
        let store = MessageStore::memory().unwrap();
        let conv_id = [2u8; 32];

        // Archive
        store.set_conversation_archived(&conv_id, true).unwrap();
        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert!(meta.is_archived);

        // Unarchive
        store.set_conversation_archived(&conv_id, false).unwrap();
        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert!(!meta.is_archived);
    }

    #[test]
    fn test_mute_and_unmute() {
        let store = MessageStore::memory().unwrap();
        let conv_id = [2u8; 32];

        // Mute
        store.set_conversation_muted(&conv_id, true).unwrap();
        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert!(meta.is_muted);

        // Unmute
        store.set_conversation_muted(&conv_id, false).unwrap();
        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert!(!meta.is_muted);
    }

    #[test]
    fn test_delete_conversation_removes_inbox_but_not_pending_outgoing() {
        let store = MessageStore::memory().unwrap();
        let local = random_public_key();
        let remote = other_public_key(42);
        let conv_id = [2u8; 32];
        let recipient = random_public_key();

        // Insert inbox messages
        let env1 = make_envelope([1u8; 32], conv_id, remote);
        let env2 = make_envelope([2u8; 32], conv_id, remote);
        store
            .insert_inbox_with_conversation_update(&env1, &local)
            .unwrap();
        store
            .insert_inbox_with_conversation_update(&env2, &local)
            .unwrap();

        // Enqueue a pending outgoing message for the same conversation
        store.enqueue_outbox(&[3u8; 32], recipient, 1000).unwrap();

        // Delete conversation
        let removed = store.delete_conversation(&conv_id).unwrap();
        assert_eq!(removed, 2); // Two inbox messages removed

        // Verify inbox is empty for this conversation
        assert!(store.get_inbox(&[1u8; 32]).unwrap().is_none());
        assert!(store.get_inbox(&[2u8; 32]).unwrap().is_none());

        // Verify outbox still has the pending message
        let due = store.fetch_due_outbox(2000).unwrap();
        assert_eq!(due.len(), 1);

        // Verify meta is soft-deleted
        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert!(meta.is_deleted);
    }

    #[test]
    fn test_list_conversations_filters_correctly() {
        let store = MessageStore::memory().unwrap();
        let local = random_public_key();
        let remote = other_public_key(42);

        let conv_active = [1u8; 32];
        let conv_archived = [2u8; 32];

        // Create two conversations
        store
            .insert_inbox_with_conversation_update(
                &make_envelope([1u8; 32], conv_active, remote),
                &local,
            )
            .unwrap();
        store
            .insert_inbox_with_conversation_update(
                &make_envelope([2u8; 32], conv_archived, remote),
                &local,
            )
            .unwrap();

        // Archive the second one
        store
            .set_conversation_archived(&conv_archived, true)
            .unwrap();

        // Without archived → only 1
        let list = store.list_conversations(false).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].conversation_id, conv_active);

        // With archived → 2
        let list = store.list_conversations(true).unwrap();
        assert_eq!(list.len(), 2);

        // Delete one → it should disappear from both lists
        store.delete_conversation(&conv_active).unwrap();
        let list = store.list_conversations(true).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].conversation_id, conv_archived);
    }

    #[test]
    fn test_conversation_state_survives_reopen() {
        // Use a temp file so we can re-open
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("store.db");

        let local = random_public_key();
        let remote = other_public_key(42);
        let conv_id = [2u8; 32];

        // First session
        {
            let store = MessageStore::open(&db_path).unwrap();
            store
                .insert_inbox_with_conversation_update(
                    &make_envelope([1u8; 32], conv_id, remote),
                    &local,
                )
                .unwrap();
            store
                .insert_inbox_with_conversation_update(
                    &make_envelope([2u8; 32], conv_id, remote),
                    &local,
                )
                .unwrap();

            // Mark one as read with preview
            store.mark_conversation_read(&conv_id).unwrap();
            store.set_conversation_archived(&conv_id, true).unwrap();
            store.set_conversation_muted(&conv_id, true).unwrap();

            let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
            assert_eq!(meta.unread_count, 0);
            assert!(meta.is_archived);
            assert!(meta.is_muted);
        }

        // Second session — reopen
        {
            let store = MessageStore::open(&db_path).unwrap();

            // All state should be restored
            let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
            assert_eq!(meta.unread_count, 0);
            assert!(meta.is_archived);
            assert!(meta.is_muted);
            assert!(!meta.is_deleted);

            // Inbox messages should be present
            assert!(store.get_inbox(&[1u8; 32]).unwrap().is_some());
            assert!(store.get_inbox(&[2u8; 32]).unwrap().is_some());
        }
    }

    #[test]
    fn test_hard_delete_conversation_removes_everything() {
        let store = MessageStore::memory().unwrap();
        let local = random_public_key();
        let remote = other_public_key(42);
        let conv_id = [2u8; 32];
        let recipient = random_public_key();

        // Insert messages
        let env = make_envelope([1u8; 32], conv_id, remote);
        store
            .insert_inbox_with_conversation_update(&env, &local)
            .unwrap();

        // Enqueue a pending outgoing
        store.enqueue_outbox(&[1u8; 32], recipient, 1000).unwrap();

        // Hard delete
        let removed = store.hard_delete_conversation(&conv_id).unwrap();
        assert_eq!(removed, 1);

        // Meta row is gone
        assert!(store.get_conversation_meta(&conv_id).unwrap().is_none());

        // Inbox is empty
        assert!(store.get_inbox(&[1u8; 32]).unwrap().is_none());

        // Outbox is empty too (the msg_id matched the inbox query)
        let due = store.fetch_due_outbox(2000).unwrap();
        assert_eq!(due.len(), 0);
    }

    #[test]
    fn test_duplicate_remote_does_not_increment_unread() {
        let store = MessageStore::memory().unwrap();
        let local = random_public_key();
        let remote = other_public_key(42);
        let conv_id = [2u8; 32];

        let env = make_envelope([1u8; 32], conv_id, remote);

        // First insert → unread = 1
        store
            .insert_inbox_with_conversation_update(&env, &local)
            .unwrap();
        assert_eq!(store.get_unread_count(&conv_id).unwrap().unwrap(), 1);

        // Duplicate (e.g. from restart replay) → unread stays 1
        store
            .insert_inbox_with_conversation_update(&env, &local)
            .unwrap();
        assert_eq!(store.get_unread_count(&conv_id).unwrap().unwrap(), 1);
    }

    #[test]
    fn test_delete_pending_outgoing_explicit() {
        let store = MessageStore::memory().unwrap();
        let local = random_public_key();
        let remote = other_public_key(42);
        let conv_id = [2u8; 32];
        let recipient = random_public_key();

        // Insert inbox message to establish the conversation
        let env = make_envelope([1u8; 32], conv_id, remote);
        store
            .insert_inbox_with_conversation_update(&env, &local)
            .unwrap();

        // Enqueue pending outgoing
        store.enqueue_outbox(&[1u8; 32], recipient, 1000).unwrap();

        // Explicitly delete pending outgoing for this conversation
        let removed = store
            .delete_pending_outgoing_for_conversation(&conv_id)
            .unwrap();
        assert_eq!(removed, 1);

        // Outbox should be empty now
        let due = store.fetch_due_outbox(2000).unwrap();
        assert_eq!(due.len(), 0);
    }

    #[test]
    fn test_update_last_message_preview() {
        let store = MessageStore::memory().unwrap();
        let local = random_public_key();
        let remote = other_public_key(42);
        let conv_id = [2u8; 32];

        let env = make_envelope([1u8; 32], conv_id, remote);
        store
            .insert_inbox_with_conversation_update(&env, &local)
            .unwrap();

        // Initial preview is "[3 bytes]"
        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert_eq!(meta.last_message_preview, "[3 bytes]");

        // Update to actual decrypted preview
        store
            .update_last_message_preview(&conv_id, "Hello, world!")
            .unwrap();
        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert_eq!(meta.last_message_preview, "Hello, world!");
    }

    #[test]
    fn test_last_author_and_message_id_tracking() {
        let store = MessageStore::memory().unwrap();
        let local = random_public_key();
        let remote = other_public_key(42);
        let conv_id = [2u8; 32];

        // First message from remote
        let env1 = make_envelope([1u8; 32], conv_id, remote);
        store
            .insert_inbox_with_conversation_update(&env1, &local)
            .unwrap();

        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert_eq!(meta.last_message_id, Some([1u8; 32]));
        assert_eq!(meta.last_author_user_id, Some(remote));

        // Second message overwrites
        let env2 = make_envelope([2u8; 32], conv_id, remote);
        store
            .insert_inbox_with_conversation_update(&env2, &local)
            .unwrap();

        let meta = store.get_conversation_meta(&conv_id).unwrap().unwrap();
        assert_eq!(meta.last_message_id, Some([2u8; 32]));
    }

    // ── Deletion and tombstone tests (Step 12) ────────────────────────

    #[test]
    fn test_delete_message_local() {
        let store = MessageStore::memory().unwrap();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];
        let author = random_public_key();

        // Insert a message
        let env = make_envelope(msg_id, conv_id, author);
        assert!(store.insert_inbox(&env).unwrap());
        assert!(store.get_inbox(&msg_id).unwrap().is_some());

        // Delete it locally
        assert!(store.delete_message(&msg_id).unwrap());

        // Inbox should be gone
        assert!(store.get_inbox(&msg_id).unwrap().is_none());

        // Tombstone should exist
        assert!(store.is_tombstoned(&msg_id).unwrap());

        // Cannot re-insert a tombstoned message
        assert!(!store.insert_inbox(&env).unwrap());

        // Deleting a non-existent message returns false
        assert!(!store.delete_message(&[99u8; 32]).unwrap());
    }

    #[test]
    fn test_delete_message_with_pending_outbound() {
        let store = MessageStore::memory().unwrap();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];
        let author = random_public_key();
        let recipient = random_public_key();

        let env = make_envelope(msg_id, conv_id, author);
        assert!(store.insert_inbox(&env).unwrap());

        // Enqueue pending outbound
        store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();

        // Delete should cancel the pending outbound
        assert!(store.delete_message(&msg_id).unwrap());

        // Outbox should not have this message as due (it's now Expired)
        let due = store.fetch_due_outbox(2000).unwrap();
        assert!(!due.iter().any(|r| r.msg_id == msg_id));
    }

    #[test]
    fn test_cancel_pending_outbound() {
        let store = MessageStore::memory().unwrap();
        let msg_id = [1u8; 32];
        let recipient = other_public_key(1);
        let recipient2 = other_public_key(2);

        // Enqueue pending outbounds for same message to two recipients
        store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();
        store.enqueue_outbox(&msg_id, recipient2, 1000).unwrap();

        // Cancel pending outbound
        let affected = store.cancel_pending_outbound(&msg_id).unwrap();
        assert_eq!(affected, 2);

        // Should not appear in due outbox
        let due = store.fetch_due_outbox(2000).unwrap();
        assert!(!due.iter().any(|r| r.msg_id == msg_id));
    }

    #[test]
    fn test_cancel_pending_outbound_already_acked() {
        let store = MessageStore::memory().unwrap();
        let msg_id = [1u8; 32];
        let recipient = random_public_key();

        store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();
        store.mark_acked(&msg_id, recipient).unwrap();

        // Canceling an already-acked message should not affect it
        let affected = store.cancel_pending_outbound(&msg_id).unwrap();
        assert_eq!(affected, 0);

        // Should still not appear in due (it's acked)
        let due = store.fetch_due_outbox(2000).unwrap();
        assert!(!due.iter().any(|r| r.msg_id == msg_id));
    }

    #[test]
    fn test_insert_tombstone_remote() {
        let store = MessageStore::memory().unwrap();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];
        let author = random_public_key();
        let signature = [4u8; 64];

        // Insert a message
        let env = make_envelope(msg_id, conv_id, author);
        assert!(store.insert_inbox(&env).unwrap());
        assert!(store.get_inbox(&msg_id).unwrap().is_some());

        // Insert a remote tombstone
        let is_new = store
            .insert_tombstone(&msg_id, &conv_id, &author, &signature)
            .unwrap();
        assert!(is_new);

        // Inbox should be gone
        assert!(store.get_inbox(&msg_id).unwrap().is_none());

        // Tombstone should exist
        assert!(store.is_tombstoned(&msg_id).unwrap());

        // Cannot re-insert
        assert!(!store.insert_inbox(&env).unwrap());

        // Duplicate tombstone returns false
        let is_new2 = store
            .insert_tombstone(&msg_id, &conv_id, &author, &signature)
            .unwrap();
        assert!(!is_new2);
    }

    #[test]
    fn test_tombstone_rejects_backfill_after_restart() {
        // Simulate: message deleted, DB reopened, backfill tries to re-insert.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("store.db");

        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];
        let author = random_public_key();

        // First session: insert, then delete
        {
            let store = MessageStore::open(&db_path).unwrap();
            let env = make_envelope(msg_id, conv_id, author);
            assert!(store.insert_inbox(&env).unwrap());
            assert!(store.delete_message(&msg_id).unwrap());
        }

        // Second session: try to re-insert (simulating backfill after restart)
        {
            let store = MessageStore::open(&db_path).unwrap();
            let env = make_envelope(msg_id, conv_id, author);

            // Tombstone should block re-insertion
            assert!(!store.insert_inbox(&env).unwrap());
            assert!(!store
                .insert_inbox_with_conversation_update(&env, &author)
                .unwrap());

            // get_inbox should return None
            assert!(store.get_inbox(&msg_id).unwrap().is_none());

            // is_tombstoned should still be true
            assert!(store.is_tombstoned(&msg_id).unwrap());
        }
    }

    #[test]
    fn test_get_inbox_returns_none_for_tombstoned() {
        let store = MessageStore::memory().unwrap();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];
        let author = random_public_key();

        let env = make_envelope(msg_id, conv_id, author);
        assert!(store.insert_inbox(&env).unwrap());
        assert!(store.get_inbox(&msg_id).unwrap().is_some());

        // Delete
        assert!(store.delete_message(&msg_id).unwrap());

        // get_inbox should return None even though the msg still exists in tombstone table
        assert!(store.get_inbox(&msg_id).unwrap().is_none());
    }

    #[test]
    fn test_is_tombstoned_non_existent() {
        let store = MessageStore::memory().unwrap();
        // Non-existent message is not tombstoned
        assert!(!store.is_tombstoned(&[42u8; 32]).unwrap());
    }

    #[test]
    fn test_record_attempt_guards_against_expired() {
        let store = MessageStore::memory().unwrap();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];
        let author = random_public_key();
        let recipient = random_public_key();

        // Insert and enqueue
        let env = make_envelope(msg_id, conv_id, author);
        assert!(store.insert_inbox(&env).unwrap());
        store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();

        // Cancel (set to Expired)
        store.cancel_pending_outbound(&msg_id).unwrap();

        // record_attempt should not resurrect an Expired message
        store
            .record_attempt(&msg_id, recipient, 2000, Some("timeout"))
            .unwrap();

        // Should still not appear as due
        let due = store.fetch_due_outbox(3000).unwrap();
        assert!(!due.iter().any(|r| r.msg_id == msg_id));
    }

    // ── Additional edge-case tests (Step 12) ──────────────────────────

    #[test]
    fn test_tombstone_survives_reopen() {
        // Verify tombstones persist across store reopens.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("store.db");
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];
        let author = random_public_key();

        // First session: insert and delete
        {
            let store = MessageStore::open(&db_path).unwrap();
            let env = make_envelope(msg_id, conv_id, author);
            assert!(store.insert_inbox(&env).unwrap());
            assert!(store.delete_message(&msg_id).unwrap());
            assert!(store.is_tombstoned(&msg_id).unwrap());
        }

        // Second session: tombstone should still block re-insertion
        {
            let store = MessageStore::open(&db_path).unwrap();
            assert!(store.is_tombstoned(&msg_id).unwrap());
            let env = make_envelope(msg_id, conv_id, author);
            assert!(!store.insert_inbox(&env).unwrap());
            assert!(!store
                .insert_inbox_with_conversation_update(&env, &author)
                .unwrap());
            assert!(store.get_inbox(&msg_id).unwrap().is_none());
        }
    }

    #[test]
    fn test_remote_tombstone_survives_reopen() {
        // Verify remote tombstones persist across store reopens.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("store.db");
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];
        let author = random_public_key();
        let signature = [4u8; 64];

        // First session: insert and apply remote tombstone
        {
            let store = MessageStore::open(&db_path).unwrap();
            let env = make_envelope(msg_id, conv_id, author);
            assert!(store.insert_inbox(&env).unwrap());
            assert!(store
                .insert_tombstone(&msg_id, &conv_id, &author, &signature)
                .unwrap());
            assert!(store.is_tombstoned(&msg_id).unwrap());
        }

        // Second session: tombstone persists
        {
            let store = MessageStore::open(&db_path).unwrap();
            assert!(store.is_tombstoned(&msg_id).unwrap());
            assert!(store.get_inbox(&msg_id).unwrap().is_none());
        }
    }

    #[test]
    fn test_remote_tombstone_cancels_pending_outbound() {
        // A remote delete tombstone should cancel pending outbound deliveries.
        let store = MessageStore::memory().unwrap();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];
        let author = random_public_key();
        let recipient = random_public_key();
        let signature = [4u8; 64];

        let env = make_envelope(msg_id, conv_id, author);
        assert!(store.insert_inbox(&env).unwrap());

        // Enqueue pending outbound
        store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();

        // Apply remote tombstone
        assert!(store
            .insert_tombstone(&msg_id, &conv_id, &author, &signature)
            .unwrap());

        // Outbound should be cancelled (not due for retry)
        let due = store.fetch_due_outbox(2000).unwrap();
        assert!(!due.iter().any(|r| r.msg_id == msg_id));
    }

    #[test]
    fn test_ack_tombstoned_message_is_safe() {
        // ACKing a message that has been tombstoned should not resurrect it.
        let store = MessageStore::memory().unwrap();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];
        let author = random_public_key();
        let recipient = random_public_key();

        let env = make_envelope(msg_id, conv_id, author);
        assert!(store.insert_inbox(&env).unwrap());
        store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();

        // Delete locally (tombstone)
        assert!(store.delete_message(&msg_id).unwrap());
        assert!(store.is_tombstoned(&msg_id).unwrap());

        // ACK should still work on the outbox row (doesn't touch tombstone)
        store.mark_acked(&msg_id, recipient).unwrap();

        // Message should remain tombstoned
        assert!(store.is_tombstoned(&msg_id).unwrap());
        assert!(store.get_inbox(&msg_id).unwrap().is_none());

        // Outbox should still not show as due (it's acked)
        let due = store.fetch_due_outbox(2000).unwrap();
        assert!(!due.iter().any(|r| r.msg_id == msg_id));
    }

    #[test]
    fn test_local_and_remote_tombstones_coexist() {
        // Verify the store handles both local and remote tombstones.
        let store = MessageStore::memory().unwrap();
        let author = random_public_key();
        let conv_id = [2u8; 32];

        // Two messages in same conversation
        let msg_local = [1u8; 32];
        let msg_remote = [2u8; 32];
        let env_local = make_envelope(msg_local, conv_id, author);
        let env_remote = make_envelope(msg_remote, conv_id, author);
        assert!(store.insert_inbox(&env_local).unwrap());
        assert!(store.insert_inbox(&env_remote).unwrap());

        // Delete one locally
        assert!(store.delete_message(&msg_local).unwrap());

        // Tombstone the other remotely
        let signature = [5u8; 64];
        assert!(store
            .insert_tombstone(&msg_remote, &conv_id, &author, &signature)
            .unwrap());

        // Both should be tombstoned
        assert!(store.is_tombstoned(&msg_local).unwrap());
        assert!(store.is_tombstoned(&msg_remote).unwrap());

        // Neither should be re-insertable
        assert!(!store.insert_inbox(&env_local).unwrap());
        assert!(!store.insert_inbox(&env_remote).unwrap());
    }

    #[test]
    fn test_durable_replay_rejects_tombstoned_message() {
        // Simulate: message is received, then a duplicate arrives after
        // the message was locally deleted. The duplicate must be rejected.
        let store = MessageStore::memory().unwrap();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];
        let author = random_public_key();

        let env = make_envelope(msg_id, conv_id, author);

        // Insert and then delete
        assert!(store.insert_inbox(&env).unwrap());
        assert!(store.delete_message(&msg_id).unwrap());

        // A duplicate arriving later should be rejected (tombstone check)
        assert!(!store.insert_inbox(&env).unwrap());
    }

    #[test]
    fn test_backfill_after_local_delete_is_rejected() {
        // Simulate: backfill tries to re-insert a message that was
        // locally deleted before a restart.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("store.db");
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];
        let author = random_public_key();

        // Session 1: insert and locally delete
        {
            let store = MessageStore::open(&db_path).unwrap();
            let env = make_envelope(msg_id, conv_id, author);
            assert!(store.insert_inbox(&env).unwrap());
            assert!(store.delete_message(&msg_id).unwrap());
        }

        // Session 2: backfill tries to insert the same message
        {
            let store = MessageStore::open(&db_path).unwrap();
            let env = make_envelope(msg_id, conv_id, author);

            // Both insert paths must reject
            assert!(!store.insert_inbox(&env).unwrap());
            assert!(!store
                .insert_inbox_with_conversation_update(&env, &author)
                .unwrap());
        }
    }

    #[test]
    fn test_redelivery_after_remote_tombstone_is_rejected() {
        // Simulate: a remote delete tombstone is received, then the
        // same message is redelivered (e.g. from another device's sync).
        let store = MessageStore::memory().unwrap();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];
        let author = random_public_key();
        let signature = [6u8; 64];

        let env = make_envelope(msg_id, conv_id, author);

        // Insert, then apply remote tombstone
        assert!(store.insert_inbox(&env).unwrap());
        assert!(store
            .insert_tombstone(&msg_id, &conv_id, &author, &signature)
            .unwrap());

        // Redelivery attempt should be rejected
        assert!(!store.insert_inbox(&env).unwrap());
        assert!(!store
            .insert_inbox_with_conversation_update(&env, &author)
            .unwrap());
    }

    #[test]
    fn test_cancel_pending_outbound_on_tombstoned_message() {
        // Cancelling pending outbound on a message that was already
        // tombstoned should work (idempotent).
        let store = MessageStore::memory().unwrap();
        let msg_id = [1u8; 32];
        let conv_id = [2u8; 32];
        let author = random_public_key();
        let recipient = random_public_key();

        let env = make_envelope(msg_id, conv_id, author);
        assert!(store.insert_inbox(&env).unwrap());
        store.enqueue_outbox(&msg_id, recipient, 1000).unwrap();

        // Delete (creates tombstone + cancels outbound)
        assert!(store.delete_message(&msg_id).unwrap());

        // Cancel again — should be a no-op (already Expired)
        let affected = store.cancel_pending_outbound(&msg_id).unwrap();
        assert_eq!(affected, 0);
    }

    #[test]
    fn test_get_inbox_preserves_non_tombstoned_messages() {
        // Verify that get_inbox still works for non-tombstoned messages
        // when other messages in the same conversation are tombstoned.
        let store = MessageStore::memory().unwrap();
        let author = random_public_key();
        let conv_id = [2u8; 32];

        let msg_alive = [1u8; 32];
        let msg_dead = [2u8; 32];

        let env_alive = make_envelope(msg_alive, conv_id, author);
        let env_dead = make_envelope(msg_dead, conv_id, author);

        assert!(store.insert_inbox(&env_alive).unwrap());
        assert!(store.insert_inbox(&env_dead).unwrap());

        // Delete one
        assert!(store.delete_message(&msg_dead).unwrap());

        // Alive message should still be retrievable
        assert!(store.get_inbox(&msg_alive).unwrap().is_some());

        // Dead message should not
        assert!(store.get_inbox(&msg_dead).unwrap().is_none());
    }
}
