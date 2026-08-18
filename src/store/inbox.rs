//! Inbound envelope persistence — durable inbox acceptance with replay
//! bookkeeping, idempotent inbox inserts, and last-message-preview updates.
//!
//! Each method is an `impl super::MessageStore` accessor over the shared
//! SQLite connection; no format or protocol changes live here (structural
//! split only, BORU-CORE-004).

use super::*;

impl super::MessageStore {
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
}
