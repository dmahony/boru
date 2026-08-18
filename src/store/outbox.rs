//! Outbox delivery persistence — enqueue, attempts, ACK, expiry, due-pull
//! and removal of the `outbox` delivery queue.
//!
//! Each method is an `impl super::MessageStore` accessor over the shared
//! SQLite connection; no format or protocol changes live here (structural
//! split only, BORU-CORE-004).

use super::*;

impl super::MessageStore {
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
    /// Excludes ``Acked``, ``Expired``, and ``Sending`` rows.
    pub fn fetch_due_outbox(&self, now_ms: u64) -> Result<Vec<OutboxRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT msg_id, recipient_device_id, status, attempts, next_attempt_at_ms, last_error_code, last_attempt_at_ms
             FROM outbox
             WHERE status != ?1 AND status != ?2 AND status != ?3 AND next_attempt_at_ms <= ?4"
        ).std_context("prepare fetch due")?;

        let mut rows = stmt
            .query(params![
                DeliveryStatus::Acked as u8,
                DeliveryStatus::Expired as u8,
                DeliveryStatus::Sending as u8,
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
