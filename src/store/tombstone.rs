//! Message deletion and tombstone persistence — local and remote
//! deletion with backfill-resurrection prevention and pending-outbound
//! cancellation.
//!
//! Each method is an `impl super::MessageStore` accessor over the shared
//! SQLite connection; no format or protocol changes live here (structural
//! split only, BORU-CORE-004).

use super::*;

impl super::MessageStore {
    /// Locally delete a single message: insert a local tombstone to prevent
    /// backfill resurrection, remove the inbox row, and cancel any pending
    /// outbound deliveries for this message.
    ///
    /// This is a **local-only** operation — no protocol message is sent to
    /// peers.  Use [`insert_tombstone`](crate::store::MessageStore::insert_tombstone) for remote-initiated deletions.
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
