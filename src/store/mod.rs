#![allow(missing_docs)]

use anyhow::anyhow;
use bytes::Bytes;
use iroh::PublicKey;
use n0_error::{Result, StdResultExt};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::chat_core::DIAGNOSTICS;
use crate::chat_history::{DeliveryState, HistoryEntry};
use crate::diagnostics::DiagnosticEventKind;
use std::str::FromStr;

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
    /// A row currently being delivered by one worker.  If a row stays in
    /// ``Sending`` past a reasonable deadline, a recovery pass moves it
    /// back to ``Pending`` so another worker can retry.
    Sending = 4,
}

impl TryFrom<u8> for DeliveryStatus {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        match value {
            0 => Ok(DeliveryStatus::Pending),
            1 => Ok(DeliveryStatus::Sent),
            2 => Ok(DeliveryStatus::Acked),
            3 => Ok(DeliveryStatus::Expired),
            4 => Ok(DeliveryStatus::Sending),
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

            -- Chat message history: every locally-observed message
            -- (sent and received) is stored here with content-addressed
            -- deduplication.  Queried by topic for room views and
            -- by conversation_meta for the sidebar chat list.
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                msg_hash BLOB NOT NULL UNIQUE,
                topic BLOB NOT NULL,
                sender BLOB NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                kind TEXT NOT NULL,
                body TEXT NOT NULL,
                signed_bytes BLOB,
                delivery_state TEXT NOT NULL DEFAULT 'queued',
                image_identifier TEXT,
                thread_root_id BLOB,
                reply_to_message_id BLOB,
                deleted INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_messages_topic_ts
                ON messages(topic, timestamp_ms);
            CREATE INDEX IF NOT EXISTS idx_messages_hash
                ON messages(msg_hash);

            CREATE TABLE IF NOT EXISTS message_replies (
                message_hash BLOB PRIMARY KEY,
                reply_to_message_id BLOB NOT NULL,
                resolved INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_message_replies_parent
                ON message_replies(reply_to_message_id);

            -- Stable group identity to epoch-topic mapping. Messages remain
            -- keyed by their original topic; this table lets history queries
            -- span rotations without rewriting historical rows.
            CREATE TABLE IF NOT EXISTS group_epoch_topics (
                group_id BLOB NOT NULL,
                epoch INTEGER NOT NULL,
                topic BLOB NOT NULL UNIQUE,
                PRIMARY KEY (group_id, epoch)
            );
            CREATE INDEX IF NOT EXISTS idx_group_epoch_topics_group
                ON group_epoch_topics(group_id, epoch);

            -- Encryption key registry: maps PeerId -> serialized OneTimeKeyBundle.
            -- Added by group encryption Phase 2.
            CREATE TABLE IF NOT EXISTS identity_registry (
                peer_id BLOB PRIMARY KEY,
                key_bundle BLOB NOT NULL
            );

            -- One-time pre-key registry: unconsumed pre-key bundles per peer.
            -- Added by group encryption Phase 2.
            CREATE TABLE IF NOT EXISTS prekey_registry (
                peer_id BLOB NOT NULL,
                pre_key BLOB NOT NULL,
                used INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .std_context("init schema")?;
        // Forward-only compatibility for databases created before the thread
        // projection was added. SQLite has no IF NOT EXISTS form for columns,
        // so the duplicate-column errors are intentionally ignored.
        let _ = conn.execute("ALTER TABLE messages ADD COLUMN thread_root_id BLOB", []);
        let _ = conn.execute("ALTER TABLE messages ADD COLUMN reply_to_message_id BLOB", []);
        let _ = conn.execute(
            "ALTER TABLE messages ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0",
            [],
        );
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_messages_thread_root
             ON messages(topic, thread_root_id, timestamp_ms);",
        )
        .std_context("init thread message index")?;
        Ok(())
    }
}

// ── Chat message row type ────────────────────────────────────────────

/// A chat message row read from the `messages` table.
#[derive(Debug, Clone)]
pub struct ChatMessageRow {
    pub id: i64,
    pub msg_hash: [u8; 32],
    pub topic: [u8; 32],
    pub sender: [u8; 32],
    pub timestamp_ms: i64,
    pub kind: String,
    pub body: String,
    pub signed_bytes: Option<Vec<u8>>,
    pub delivery_state: String,
    pub image_identifier: Option<String>,
}

fn row_to_chat_message(row: &rusqlite::Row) -> Result<ChatMessageRow> {
    let hash_blob: Vec<u8> = row.get(0).std_context("get msg_hash")?;
    let mut msg_hash = [0u8; 32];
    msg_hash.copy_from_slice(&hash_blob);

    let topic_blob: Vec<u8> = row.get(1).std_context("get topic")?;
    let mut topic = [0u8; 32];
    topic.copy_from_slice(&topic_blob);

    let sender_blob: Vec<u8> = row.get(2).std_context("get sender")?;
    let mut sender = [0u8; 32];
    sender.copy_from_slice(&sender_blob);

    let timestamp_ms: i64 = row.get(3).std_context("get timestamp_ms")?;
    let kind: String = row.get(4).std_context("get kind")?;
    let body: String = row.get(5).std_context("get body")?;
    let signed_bytes: Option<Vec<u8>> = row.get(6).std_context("get signed_bytes")?;
    let delivery_state: String = row.get(7).std_context("get delivery_state")?;
    let image_identifier: Option<String> = row.get(8).std_context("get image_identifier")?;

    Ok(ChatMessageRow {
        id: row.get::<_, i64>(9).unwrap_or(0),
        msg_hash,
        topic,
        sender,
        timestamp_ms,
        kind,
        body,
        signed_bytes,
        delivery_state,
        image_identifier,
    })
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

// ── Submodules ──────────────────────────────────────────────────────────

mod conversation;
mod history;
mod inbox;
mod outbox;
#[cfg(test)]
mod tests;
mod tombstone;
