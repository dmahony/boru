//! Durable pinned-message projection.

use super::*;
use crate::chat_core::MessageHash;

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
}
