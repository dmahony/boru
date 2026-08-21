//! Chat message history — content-addressed `messages` table accessors,
//! legacy JSON import, and group epoch-topic mapping queries.
//!
//! Each method is an `impl super::MessageStore` accessor over the shared
//! SQLite connection; no format or protocol changes live here (structural
//! split only, BORU-CORE-004).

use super::*;

impl super::MessageStore {
    /// Store an optional reply target for a message. The operation is
    /// idempotent, which is important for duplicate and reordered backfill.
    pub fn insert_reply_reference(
        &self,
        message_hash: &[u8; 32],
        reply_to_message_id: &[u8; 32],
        resolved: bool,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO message_replies (message_hash, reply_to_message_id, resolved)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(message_hash) DO UPDATE SET
               reply_to_message_id = excluded.reply_to_message_id,
               resolved = MAX(message_replies.resolved, excluded.resolved)",
            params![
                message_hash.as_slice(),
                reply_to_message_id.as_slice(),
                resolved as i32
            ],
        )
        .std_context("insert reply reference")?;
        Ok(conn.changes() > 0)
    }

    /// Return the parent id for a message, if it is a reply.
    pub fn reply_target(&self, message_hash: &[u8; 32]) -> Result<Option<([u8; 32], bool)>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT reply_to_message_id, resolved FROM message_replies WHERE message_hash = ?1",
            params![message_hash.as_slice()],
            |row| {
                let id: Vec<u8> = row.get(0)?;
                let id: [u8; 32] = id.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok((id, row.get::<_, i32>(1)? != 0))
            },
        )
        .optional()
        .std_context("read reply reference")
    }

    /// Mark references to a newly-arrived parent as resolved.
    pub fn resolve_reply_references(&self, parent_id: &[u8; 32]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE message_replies SET resolved = 1 WHERE reply_to_message_id = ?1",
            params![parent_id.as_slice()],
        )
        .std_context("resolve reply references")?;
        Ok(conn.changes() as usize)
    }

    /// Attach thread targeting metadata after the common message insert path.
    pub fn set_thread_target(
        &self,
        msg_hash: &[u8; 32],
        target: &crate::threads::ThreadTarget,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET thread_root_id = ?1, reply_to_message_id = ?2
             WHERE msg_hash = ?3",
            params![
                target.thread_root_id.as_slice(),
                target.reply_to_message_id.map(|id| id.to_vec()),
                msg_hash.as_slice(),
            ],
        )
        .std_context("set thread target")?;
        Ok(())
    }

    /// Insert a chat message into the `messages` table with deduplication.
    ///
    /// `msg_hash` is a blake3 hash of the signed message bytes (32 bytes),
    /// and serves as the content-addressed unique key.  `topic` and `sender`
    /// are 32-byte gossip TopicId and PublicKey, respectively.
    ///
    /// Returns `true` if a new row was inserted, `false` if a duplicate
    /// was silently ignored.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_chat_message(
        &self,
        msg_hash: &[u8; 32],
        topic: &[u8; 32],
        sender: &[u8; 32],
        timestamp_ms: u64,
        kind: &str,
        body: &str,
        signed_bytes: Option<&[u8]>,
        image_identifier: Option<&str>,
        local_user_id: &[u8; 32],
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO messages
             (msg_hash, topic, sender, timestamp_ms, kind, body, signed_bytes, delivery_state, image_identifier)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8)",
            params![
                msg_hash.as_slice(),
                topic.as_slice(),
                sender.as_slice(),
                timestamp_ms as i64,
                kind,
                body,
                signed_bytes,
                image_identifier,
            ],
        )
        .std_context("insert chat message")?;
        let is_new = conn.changes() > 0;

        // Update conversation_meta for the sidebar chat list.
        if is_new {
            let is_local = sender == local_user_id;
            let unread_increment = if is_local { 0 } else { 1 };
            conn.execute(
                "INSERT INTO conversation_meta
                 (conversation_id, last_message_id, last_activity_at_ms,
                  last_message_preview, last_author_user_id, unread_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(conversation_id) DO UPDATE SET
                    last_message_id = excluded.last_message_id,
                    last_activity_at_ms = excluded.last_activity_at_ms,
                    last_message_preview = excluded.last_message_preview,
                    last_author_user_id = excluded.last_author_user_id,
                    unread_count = conversation_meta.unread_count + excluded.unread_count",
                params![
                    topic.as_slice(),
                    msg_hash.as_slice(),
                    timestamp_ms as i64,
                    body,
                    sender.as_slice(),
                    unread_increment,
                ],
            )
            .std_context("update conversation meta for chat message")?;
        }

        Ok(is_new)
    }

    /// One-time legacy import of `chat_history.json` entries into the
    /// `messages` table.
    ///
    /// This is the migration entry point used by
    /// [`ChatHistoryStore::migrate_legacy_json`](crate::chat_history::ChatHistoryStore::migrate_legacy_json)
    /// — **not** a live write path.  It runs in a single transaction so a
    /// failure mid-import rolls back and leaves the legacy JSON file intact.
    ///
    /// Merge policy (documented, deterministic):
    /// - SQLite is authoritative.  Rows are keyed by content hash
    ///   (`msg_hash` UNIQUE + `INSERT OR IGNORE`), so an entry already in
    ///   SQLite always wins; legacy entries with new hashes are added.
    /// - After a successful import the caller renames the JSON file to a
    ///   backup (`chat_history.json.imported`), which doubles as the
    ///   "migration completed" marker.
    ///
    /// Returns the number of newly-inserted rows (duplicates are not counted).
    pub fn import_legacy_history(
        &self,
        entries: &[HistoryEntry],
        local_user_id: &[u8; 32],
    ) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .std_context("begin legacy chat history import")?;
        let mut inserted = 0usize;
        for entry in entries {
            let hash_vec = hex::decode(&entry.hash).unwrap_or_default();
            let hash = if hash_vec.len() == 32 {
                let mut value = [0u8; 32];
                value.copy_from_slice(&hash_vec);
                value
            } else {
                *blake3::hash(&entry.signed_bytes).as_bytes()
            };
            // Fail closed on a malformed sender: the JSON was written by the
            // app itself, so an unparseable key means the legacy file is
            // corrupt — roll back the whole import rather than write a
            // placeholder row.
            let sender = PublicKey::from_str(&entry.sender)
                .with_std_context(|_| format!("invalid legacy sender '{}'", entry.sender))?
                .as_bytes()
                .to_owned();
            let state = match entry.delivery_state {
                DeliveryState::Queued => "queued",
                DeliveryState::Sent => "sent",
                DeliveryState::Delivered => "delivered",
                DeliveryState::Seen => "seen",
                DeliveryState::Failed => "failed",
            };
            let is_new = tx
                .execute(
                    "INSERT OR IGNORE INTO messages
                     (msg_hash, topic, sender, timestamp_ms, kind, body, signed_bytes, delivery_state, image_identifier)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        hash.as_slice(),
                        entry.topic.as_bytes().as_slice(),
                        sender.as_slice(),
                        entry.timestamp as i64,
                        entry.kind.as_str(),
                        entry.text_preview.as_str(),
                        entry.signed_bytes.as_slice(),
                        state,
                        entry.image_identifier.as_deref(),
                    ],
                )
                .std_context("insert legacy chat message")?;
            if is_new > 0 {
                inserted += 1;
                // Mirror `insert_chat_message`: keep the sidebar chat list
                // (conversation_meta) in sync for imported history.
                let is_local = sender == *local_user_id;
                let unread_increment = if is_local { 0 } else { 1 };
                tx.execute(
                    "INSERT INTO conversation_meta
                     (conversation_id, last_message_id, last_activity_at_ms,
                      last_message_preview, last_author_user_id, unread_count)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(conversation_id) DO UPDATE SET
                        last_message_id = excluded.last_message_id,
                        last_activity_at_ms = excluded.last_activity_at_ms,
                        last_message_preview = excluded.last_message_preview,
                        last_author_user_id = excluded.last_author_user_id,
                        unread_count = conversation_meta.unread_count + excluded.unread_count",
                    params![
                        entry.topic.as_bytes().as_slice(),
                        hash.as_slice(),
                        entry.timestamp as i64,
                        entry.text_preview.as_str(),
                        sender.as_slice(),
                        unread_increment,
                    ],
                )
                .std_context("update conversation meta for legacy chat message")?;
            }
        }
        tx.commit()
            .std_context("commit legacy chat history import")?;
        Ok(inserted)
    }

    /// Register an epoch's topic for a stable group identity.
    ///
    /// This is intentionally a mapping-only operation: existing message rows
    /// retain their original topic and are never copied or rewritten.
    pub fn register_group_epoch(
        &self,
        group_id: [u8; 32],
        epoch: u64,
        topic: [u8; 32],
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO group_epoch_topics (group_id, epoch, topic)
             VALUES (?1, ?2, ?3)",
            params![group_id.as_slice(), epoch as i64, topic.as_slice()],
        )
        .std_context("register group epoch")?;
        Ok(conn.changes() > 0)
    }

    /// Return the known epoch/topic mappings for a group, oldest first.
    pub fn list_group_epochs(&self, group_id: &[u8; 32]) -> Result<Vec<(u64, [u8; 32])>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT epoch, topic FROM group_epoch_topics
                 WHERE group_id = ?1 ORDER BY epoch ASC",
            )
            .std_context("prepare list group epochs")?;
        let mut rows = stmt
            .query([group_id.as_slice()])
            .std_context("query list group epochs")?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().std_context("next group epoch")? {
            let topic_blob: Vec<u8> = row.get(1).std_context("get group epoch topic")?;
            let topic: [u8; 32] = topic_blob
                .try_into()
                .map_err(|_| anyhow!("invalid stored group epoch topic"))?;
            let epoch = row.get::<_, i64>(0).map_err(|error| anyhow!(error))? as u64;
            result.push((epoch, topic));
        }
        Ok(result)
    }

    /// Return chat history for every locally known epoch of a group.
    ///
    /// Results are chronological across epoch boundaries. Pagination is
    /// applied after merging the topics, so the UI can treat all epochs as a
    /// single conversation. Messages are not duplicated when an epoch mapping
    /// is registered repeatedly.
    pub fn get_messages_for_group(
        &self,
        group_id: &[u8; 32],
        offset: usize,
        limit: usize,
    ) -> Result<Vec<ChatMessageRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT m.msg_hash, m.topic, m.sender, m.timestamp_ms, m.kind,
                        m.body, m.signed_bytes, m.delivery_state,
                        m.image_identifier, m.id
                 FROM messages AS m
                 WHERE m.topic IN (
                     SELECT topic FROM group_epoch_topics WHERE group_id = ?1
                 )
                 ORDER BY m.timestamp_ms ASC, m.id ASC
                 LIMIT ?2 OFFSET ?3",
            )
            .std_context("prepare get messages for group")?;
        let mut rows = stmt
            .query(params![group_id.as_slice(), limit as i64, offset as i64])
            .std_context("query get messages for group")?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().std_context("next group message")? {
            result.push(row_to_chat_message(row)?);
        }
        Ok(result)
    }

    /// Count all stored messages across a group's known epochs.
    pub fn count_messages_for_group(&self, group_id: &[u8; 32]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages
                 WHERE topic IN (
                     SELECT topic FROM group_epoch_topics WHERE group_id = ?1
                 )",
                [group_id.as_slice()],
                |row| row.get(0),
            )
            .std_context("count messages for group")?;
        Ok(count as usize)
    }

    /// Return chat messages for a given topic, most recent first,
    /// with optional pagination (`limit` + `offset`).
    pub fn get_messages_for_topic(
        &self,
        topic: &[u8; 32],
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ChatMessageRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT msg_hash, topic, sender, timestamp_ms, kind, body,
                        signed_bytes, delivery_state, image_identifier, id
                 FROM messages
                 WHERE topic = ?1
                 ORDER BY timestamp_ms ASC
                 LIMIT ?2 OFFSET ?3",
            )
            .std_context("prepare get_messages_for_topic")?;
        let mut rows = stmt
            .query(params![topic.as_slice(), limit as i64, offset as i64])
            .std_context("query get_messages_for_topic")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(row_to_chat_message(row)?);
        }
        Ok(results)
    }

    /// Count messages for a topic.
    pub fn count_messages_for_topic(&self, topic: &[u8; 32]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE topic = ?1",
                [topic.as_slice()],
                |row| row.get(0),
            )
            .std_context("count messages for topic")?;
        Ok(count as usize)
    }

    /// Find a message by its blake3 hash.
    pub fn find_message_by_hash(&self, msg_hash: &[u8; 32]) -> Result<Option<ChatMessageRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT msg_hash, topic, sender, timestamp_ms, kind, body,
                        signed_bytes, delivery_state, image_identifier, id
                 FROM messages WHERE msg_hash = ?1",
            )
            .std_context("prepare find_message_by_hash")?;
        let mut rows = stmt
            .query([msg_hash.as_slice()])
            .std_context("query find_message_by_hash")?;
        if let Some(row) = rows.next().std_context("next row")? {
            Ok(Some(row_to_chat_message(row)?))
        } else {
            Ok(None)
        }
    }

    /// Update the delivery state of a message identified by its hash.
    pub fn update_message_delivery_state(
        &self,
        msg_hash: &[u8; 32],
        new_state: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn
            .execute(
                "UPDATE messages SET delivery_state = ?1 WHERE msg_hash = ?2",
                params![new_state, msg_hash.as_slice()],
            )
            .std_context("update message delivery state")?;
        Ok(affected > 0)
    }

    /// Remove all messages for a topic (used when a room is deleted).
    pub fn delete_messages_for_topic(&self, topic: &[u8; 32]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn
            .execute("DELETE FROM messages WHERE topic = ?1", [topic.as_slice()])
            .std_context("delete messages for topic")?;
        Ok(deleted)
    }

    /// Return up to `count` most recent messages across all topics.
    pub fn get_recent_messages(&self, count: usize) -> Result<Vec<ChatMessageRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT msg_hash, topic, sender, timestamp_ms, kind, body,
                        signed_bytes, delivery_state, image_identifier, id
                 FROM messages
                 ORDER BY timestamp_ms DESC
                 LIMIT ?1",
            )
            .std_context("prepare get_recent_messages")?;
        let mut rows = stmt
            .query([count as i64])
            .std_context("query get_recent_messages")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(row_to_chat_message(row)?);
        }
        results.reverse(); // oldest first for display
        Ok(results)
    }

    /// Return ALL messages ordered by timestamp (oldest first).
    pub fn get_all_messages(&self) -> Result<Vec<ChatMessageRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT msg_hash, topic, sender, timestamp_ms, kind, body,
                        signed_bytes, delivery_state, image_identifier, id
                 FROM messages
                 ORDER BY timestamp_ms ASC",
            )
            .std_context("prepare get_all_messages")?;
        let mut rows = stmt.query([]).std_context("query get_all_messages")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(row_to_chat_message(row)?);
        }
        Ok(results)
    }

    /// Query the `messages` table with optional hex-prefix filters on the
    /// sender public key and the topic, newest first.
    ///
    /// Exposed to the MCP diagnostic server (`boru_get_message_store` /
    /// `boru_wait_for_message_delivery`).  Prefixes must contain only hex
    /// characters (callers validate); the match is case-insensitive on the
    /// uppercase `hex()` rendering of the stored blob.  The limit is clamped
    /// to `[1, 500]` rows.
    pub fn query_messages(
        &self,
        sender_hex_prefix: Option<&str>,
        topic_hex_prefix: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ChatMessageRow>> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 500) as i64;

        let mut sql = String::from(
            "SELECT msg_hash, topic, sender, timestamp_ms, kind, body,\n\
                    signed_bytes, delivery_state, image_identifier, id\n\
             FROM messages",
        );
        let mut conditions: Vec<String> = Vec::new();
        if let Some(p) = sender_hex_prefix {
            if !p.is_empty() {
                conditions.push(format!("hex(sender) LIKE ?{}", conditions.len() + 1));
            }
        }
        if let Some(p) = topic_hex_prefix {
            if !p.is_empty() {
                conditions.push(format!("hex(topic) LIKE ?{}", conditions.len() + 1));
            }
        }
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        sql.push_str(" ORDER BY timestamp_ms DESC LIMIT ?");
        sql.push_str(&(conditions.len() + 1).to_string());

        let mut stmt = conn.prepare(&sql).std_context("prepare query_messages")?;
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(p) = sender_hex_prefix {
            if !p.is_empty() {
                params.push(Box::new(format!("{}%", p.to_ascii_uppercase())));
            }
        }
        if let Some(p) = topic_hex_prefix {
            if !p.is_empty() {
                params.push(Box::new(format!("{}%", p.to_ascii_uppercase())));
            }
        }
        params.push(Box::new(limit));

        let mut rows = stmt
            .query(
                params
                    .iter()
                    .map(|p| p.as_ref())
                    .collect::<Vec<_>>()
                    .as_slice(),
            )
            .std_context("query query_messages")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(row_to_chat_message(row)?);
        }
        Ok(results)
    }
}

#[cfg(test)]
mod reply_tests {
    use super::*;

    #[test]
    fn reply_reference_is_idempotent_and_resolves() {
        let store = MessageStore::memory().unwrap();
        let hash = [1u8; 32];
        let parent = [2u8; 32];
        assert!(store.insert_reply_reference(&hash, &parent, false).unwrap());
        assert!(store.insert_reply_reference(&hash, &parent, false).unwrap());
        assert_eq!(store.reply_target(&hash).unwrap(), Some((parent, false)));
        assert_eq!(store.resolve_reply_references(&parent).unwrap(), 1);
        assert_eq!(store.reply_target(&hash).unwrap(), Some((parent, true)));
    }
}
