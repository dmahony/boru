//! Conversation metadata persistence — unread counts, archived/muted and
//! soft/hard delete flags, and the sidebar chat-list queries over the
//! `conversation_meta` table.
//!
//! Each method is an `impl super::MessageStore` accessor over the shared
//! SQLite connection; no format or protocol changes live here (structural
//! split only, BORU-CORE-004).

use super::*;

impl super::MessageStore {
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
    /// delivered.  Use `delete_outgoing_for_conversation` for the
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

    /// Hard-delete a conversation: removes every message-related row for
    /// this conversation and removes the metadata row entirely.
    ///
    /// This is the explicit "delete everything" path.  Only use this when
    /// the user explicitly confirms they want to discard pending outgoing
    /// messages as well.
    pub fn hard_delete_conversation(&self, conversation_id: &[u8; 32]) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .std_context("begin hard delete conversation transaction")?;

        // Capture ids before deleting.  These ids are shared by inbox,
        // tombstones, replay bookkeeping, and the delivery outbox.
        let mut stmt = tx
            .prepare(
                "SELECT msg_id FROM inbox WHERE conversation_id = ?1
                 UNION
                 SELECT msg_id FROM message_tombstones WHERE conversation_id = ?1",
            )
            .std_context("prepare select msg_ids for hard delete")?;
        let msg_ids: Vec<Vec<u8>> = stmt
            .query_map([conversation_id.as_slice()], |row| row.get(0))
            .std_context("query msg_ids for hard delete")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .std_context("collect msg_ids")?;
        drop(stmt);

        // Delete all durable message projections for this conversation.
        let removed_inbox = tx
            .execute(
                "DELETE FROM inbox WHERE conversation_id = ?1",
                [conversation_id.as_slice()],
            )
            .std_context("hard delete inbox messages")?;

        tx.execute(
            "DELETE FROM message_tombstones WHERE conversation_id = ?1",
            [conversation_id.as_slice()],
        )
        .std_context("hard delete message tombstones")?;
        tx.execute(
            "DELETE FROM messages WHERE topic = ?1",
            [conversation_id.as_slice()],
        )
        .std_context("hard delete chat history messages")?;

        // Delete corresponding outbox rows
        let mut delete_outbox = tx
            .prepare("DELETE FROM outbox WHERE msg_id = ?1")
            .std_context("prepare hard delete outbox")?;
        for msg_blob in &msg_ids {
            tx.execute(
                "DELETE FROM incoming_replay WHERE msg_id = ?1",
                [msg_blob.as_slice()],
            )
            .std_context("hard delete incoming replay row")?;
            delete_outbox
                .execute([msg_blob.as_slice()])
                .std_context("hard delete outbox row")?;
        }
        drop(delete_outbox);

        // Remove metadata row entirely
        tx.execute(
            "DELETE FROM conversation_meta WHERE conversation_id = ?1",
            [conversation_id.as_slice()],
        )
        .std_context("delete conversation meta row")?;

        tx.commit()
            .std_context("commit hard delete conversation transaction")?;
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

    /// Remove the `conversation_meta` sidebar row for a topic, if present.
    ///
    /// Used when purging a room's persisted state entirely (e.g. the
    /// BORU-DISC-18 lobby migration): `insert_chat_message` /
    /// `import_legacy_history` create one of these rows per topic, so a
    /// removed room would otherwise leave a stale unread/preview row behind.
    /// Returns the number of rows deleted (0 when none existed).
    pub fn delete_conversation_meta_row(&self, conversation_id: &[u8; 32]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn
            .execute(
                "DELETE FROM conversation_meta WHERE conversation_id = ?1",
                [conversation_id.as_slice()],
            )
            .std_context("delete conversation meta row")?;
        Ok(deleted)
    }
}
