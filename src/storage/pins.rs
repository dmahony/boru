//! Durable pinned-message projection.

use super::*;
use crate::chat_core::MessageHash;

/// A durable pin projection row loaded from SQLite.
#[derive(Debug, Clone)]
pub struct PinnedMessageRow {
    pub topic: TopicId,
    pub message_hash: MessageHash,
    pub pinned_by: PublicKey,
    pub action: String,
    pub sent_at: u64,
}

impl super::Storage {
    /// Reconcile a verified pin operation using timestamp then author bytes as
    /// the deterministic tie-break. The operation is idempotent on retries.
    pub fn reconcile_pinned_message(
        &self,
        topic: TopicId,
        message_hash: MessageHash,
        author: PublicKey,
        action: &str,
        sent_at: u64,
    ) -> Result<()> {
        if !matches!(action, "pin" | "unpin") {
            return Err(anyhow!("invalid pin action").into());
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO pinned_messages
                (topic, message_hash, pinned_by, action, sent_at, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(topic, message_hash) DO UPDATE SET
                pinned_by=excluded.pinned_by,
                action=excluded.action,
                sent_at=excluded.sent_at,
                updated_at_ms=excluded.updated_at_ms
             WHERE excluded.sent_at > pinned_messages.sent_at
                OR (excluded.sent_at = pinned_messages.sent_at
                    AND excluded.pinned_by > pinned_messages.pinned_by)",
            params![
                topic.as_bytes().as_slice(),
                message_hash.as_slice(),
                author.as_bytes().as_slice(),
                action,
                sent_at as i64,
                crate::chat_core::now_ms() as i64,
            ],
        )
        .std_context("reconcile pinned message")?;
        Ok(())
    }

    /// Load the reconciled pin projection for one conversation.
    pub fn pinned_messages_for_topic(&self, topic: TopicId) -> Result<Vec<PinnedMessageRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT topic, message_hash, pinned_by, action, sent_at
                 FROM pinned_messages WHERE topic = ?1 ORDER BY sent_at, pinned_by",
            )
            .std_context("prepare pinned messages query")?;
        let rows = stmt.query_map(params![topic.as_bytes().as_slice()], |row| {
            let topic_bytes: Vec<u8> = row.get(0)?;
            let hash_bytes: Vec<u8> = row.get(1)?;
            let author_bytes: Vec<u8> = row.get(2)?;
            let topic_bytes: [u8; 32] = topic_bytes
                .try_into()
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            let hash_bytes: [u8; 32] = hash_bytes
                .try_into()
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            let author_bytes: [u8; 32] = author_bytes
                .try_into()
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok(PinnedMessageRow {
                topic: TopicId::from_bytes(topic_bytes),
                message_hash: hash_bytes,
                pinned_by: PublicKey::from_bytes(&author_bytes)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                action: row.get(3)?,
                sent_at: row.get::<_, i64>(4)?.max(0) as u64,
            })
        })
        .std_context("query pinned messages")?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .std_context("load pinned messages")
    }
}
