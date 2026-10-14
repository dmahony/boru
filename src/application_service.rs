//! Shared application-facing conversation operations.
//!
//! The service is deliberately narrow: it owns text-message validation,
//! signing, and durable history projection while callers own transport and UI
//! projection.  This keeps companion clients and the desktop composer on the
//! same persistence path without creating another receive or retry worker.

use bytes::Bytes;
use iroh::{PublicKey, SecretKey};
use n0_error::{Result, StdResultExt};

use crate::chat_core::{message_hash, Message, MessageHash, SignedMessage};
use crate::store::{ConversationMeta, MessageStore};
use crate::storage::Storage;
use crate::threads::ThreadTarget;

/// A validated text-send request shared by frontends.
#[derive(Debug, Clone)]
pub struct TextSendRequest {
    /// Stable caller-owned identity for retries of the same command.
    pub command_key: String,
    /// Conversation/topic identifier.
    pub conversation_id: [u8; 32],
    /// User-visible text. Leading/trailing whitespace is rejected after trim.
    pub text: String,
    /// Optional thread target.
    pub thread_target: Option<ThreadTarget>,
}

/// Result of preparing and durably recording a local text message.
#[derive(Debug, Clone)]
pub struct PreparedTextMessage {
    /// Protocol message hash used by reactions, edits, and deduplication.
    pub message_hash: MessageHash,
    /// Hash of the signed envelope used by the durable message store.
    pub envelope_hash: [u8; 32],
    /// Signed bytes ready for the existing transport owner.
    pub encoded: Bytes,
    /// SQLite event id assigned by the durable outgoing admission.
    pub event_id: u64,
    /// Set when retry admission committed but a canonical history projection is pending.
    pub projection_error: Option<String>,
}

/// Stable safe conversation projection for companion clients.
#[derive(Debug, Clone, serde::Serialize)]
#[allow(missing_docs)]
pub struct ConversationRead {
    pub id: String,
    pub last_activity_at_ms: u64,
    pub last_message_preview: String,
    pub unread_count: u32,
    pub muted: bool,
    pub archived: bool,
}

/// Safe message projection; signed bytes and attachment paths are excluded.
#[derive(Debug, Clone, serde::Serialize)]
#[allow(missing_docs)]
pub struct MessageRead {
    pub id: String,
    pub conversation_id: String,
    pub sender_id: String,
    pub timestamp_ms: u64,
    pub kind: String,
    pub body: String,
    pub delivery_state: String,
}

/// Shared application service for conversation queries and local text sends.
#[derive(Debug, Clone)]
pub struct ConversationApplicationService {
    store: MessageStore,
    storage: Storage,
    secret_key: SecretKey,
    local_user_id: PublicKey,
}

impl ConversationApplicationService {
    /// Create a service over an existing message store.
    pub fn new(
        store: MessageStore,
        storage: Storage,
        secret_key: SecretKey,
        local_user_id: PublicKey,
    ) -> Self {
        Self {
            store,
            storage,
            secret_key,
            local_user_id,
        }
    }

    /// Return conversation metadata for sidebar/query clients.
    pub fn conversation_meta(
        &self,
        conversation_id: &[u8; 32],
    ) -> Result<Option<ConversationMeta>> {
        self.store.get_conversation_meta(conversation_id)
    }

    /// Return the unread count for a conversation.
    pub fn unread_count(&self, conversation_id: &[u8; 32]) -> Result<Option<u32>> {
        self.store.get_unread_count(conversation_id)
    }

    /// Mark a conversation read and return its previous unread count.
    pub fn mark_read(&self, conversation_id: &[u8; 32]) -> Result<Option<u32>> {
        self.store.mark_conversation_read(conversation_id)
    }

    /// Replay incomplete canonical projections from durable outgoing admission.
    pub fn repair_pending_text_projections(&self) -> Result<usize> {
        let pending = self.storage.pending_text_send_projections()?;
        let mut repaired = 0;
        for (event_id, payload) in pending {
            let (topic, sender, timestamp_ms, kind, body, signed_bytes, msg_hash, target): (
                [u8; 32],
                Vec<u8>,
                u64,
                String,
                String,
                Vec<u8>,
                [u8; 32],
                Option<ThreadTarget>,
            ) = postcard::from_bytes(&payload)
                .std_context("decode pending text projection")?;
            let sender: [u8; 32] = sender
                .try_into()
                .map_err(|_| n0_error::anyerr!("invalid text projection sender"))?;
            let envelope_hash = *blake3::hash(&signed_bytes).as_bytes();
            self.store
                .insert_chat_message(
                    &envelope_hash,
                    &topic,
                    &sender,
                    timestamp_ms,
                    &kind,
                    &body,
                    Some(&signed_bytes),
                    None,
                    &sender,
                )
                .std_context("repair local text history projection")?;
            if let Some(target) = target {
                self.store
                    .set_thread_target(&envelope_hash, &target)
                    .std_context("repair local thread target")?;
                self.storage
                    .insert_thread_message(
                        &msg_hash,
                        &topic,
                        &sender,
                        timestamp_ms,
                        &signed_bytes,
                        Some(target),
                    )
                    .std_context("repair local thread relation")?;
            }
            self.storage.mark_text_send_projection_complete(event_id)?;
            repaired += 1;
        }
        Ok(repaired)
    }

    /// List a bounded keyset page of safe conversation metadata.
    pub fn list_conversations(
        &self,
        after: Option<(u64, [u8; 32])>,
        limit: usize,
        allowed: Option<&[[u8; 32]]>,
    ) -> Result<Vec<ConversationRead>> {
        Ok(self
            .store
            .list_conversation_meta(after, None, limit)?
            .into_iter()
            .filter(|row| allowed.map_or(true, |ids| ids.contains(&row.conversation_id)))
            .map(|row| ConversationRead {
                id: hex::encode(row.conversation_id),
                last_activity_at_ms: row.last_activity_at_ms,
                last_message_preview: row.last_message_preview,
                unread_count: row.unread_count,
                muted: row.is_muted,
                archived: row.is_archived,
            })
            .collect())
    }

    /// Read a bounded chronological page. Unauthorized ids return no data.
    pub fn get_messages(
        &self,
        conversation_id: &[u8; 32],
        after: Option<(i64, i64)>,
        limit: usize,
        _max_bytes: usize,
        allowed: Option<&[[u8; 32]]>,
    ) -> Result<Vec<MessageRead>> {
        if !allowed.map_or(true, |ids| ids.contains(conversation_id)) {
            return Ok(Vec::new());
        }
        Ok(self
            .store
            .get_messages_keyset(
                conversation_id,
                after,
                self.store.message_history_snapshot(conversation_id)?,
                limit,
            )?
            .into_iter()
            .map(|row| MessageRead {
                id: hex::encode(row.msg_hash),
                conversation_id: hex::encode(row.topic),
                sender_id: hex::encode(row.sender),
                timestamp_ms: row.timestamp_ms.max(0) as u64,
                kind: row.kind,
                body: row.body,
                delivery_state: row.delivery_state,
            })
            .collect())
    }

    /// Prepare, sign, and durably project a local text message.
    ///
    /// Transport delivery remains owned by the existing outbox/broadcast
    /// runtime.  This method intentionally performs no network I/O.
    pub fn send_text(&self, request: TextSendRequest) -> Result<PreparedTextMessage> {
        let text = request.text.trim();
        if text.is_empty() {
            return Err(n0_error::anyerr!("message text must not be empty"));
        }

        let message = match request.thread_target {
            Some(target) => Message::ThreadMessage {
                text: text.to_string(),
                target,
            },
            None => Message::Message {
                text: text.to_string(),
            },
        };
        let message_hash = message_hash(&message);
        let request_hash = *blake3::hash(
            &[request.conversation_id.as_slice(), &postcard::to_allocvec(&message)
                .std_context("encode text-send request")?]
            .concat(),
        )
        .as_bytes();
        let mut encoded = SignedMessage::sign_and_encode(&self.secret_key, &message)
            .std_context("sign text message")?;
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .std_context("read system clock")?
            .as_millis() as u64;

        let projection_payload = postcard::to_allocvec(&(
            request.conversation_id,
            self.local_user_id.as_bytes().to_vec(),
            timestamp_ms,
            "text".to_string(),
            text.to_string(),
            encoded.to_vec(),
            message_hash,
            request.thread_target,
        ))
        .std_context("encode pending text projection")?;

        let topic = crate::TopicId::from_bytes(request.conversation_id);
        let event_id = self
            .storage
            .admit_outgoing_message(
                &topic,
                &crate::chat_history::blake3_hex(&encoded),
                &encoded,
                &request.command_key,
                &request_hash,
                &projection_payload,
            )
            .std_context("admit local text message for durable retry")?;

        let admitted = self
            .storage
            .get_outgoing_message(event_id)
            .std_context("read admitted text message")?
            .ok_or_else(|| n0_error::anyerr!("admitted text message disappeared"))?;
        encoded = Bytes::from(admitted.signed_bytes);
        let envelope_hash = *blake3::hash(&encoded).as_bytes();
        let projection_error = self.repair_pending_text_projections().err().map(|error| error.to_string());

        Ok(PreparedTextMessage {
            message_hash,
            envelope_hash,
            encoded,
            event_id,
            projection_error,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    fn service() -> ConversationApplicationService {
        let secret_key = SecretKey::from_bytes(&[7; 32]);
        ConversationApplicationService::new(
            MessageStore::memory().expect("memory store"),
            Storage::memory().expect("memory storage"),
            secret_key.clone(),
            secret_key.public(),
        )
    }

    #[test]
    fn fixture_send_and_read_share_the_same_store_path() {
        let service = service();
        let conversation_id = [9; 32];
        let prepared = service
            .send_text(TextSendRequest {
                command_key: "fixture-send".into(),
                conversation_id,
                text: " hello ".into(),
                thread_target: None,
            })
            .expect("send text");

        assert_ne!(prepared.message_hash, [0; 32]);
        assert_ne!(prepared.envelope_hash, [0; 32]);
        assert!(!prepared.encoded.is_empty());
        let outgoing = service
            .storage
            .list_outgoing_for_topic(&crate::TopicId::from_bytes(conversation_id))
            .unwrap();
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].event_id, prepared.event_id);
        assert_eq!(outgoing[0].signed_bytes, prepared.encoded);
        assert_eq!(service.unread_count(&conversation_id).unwrap(), Some(0));
        assert_eq!(service.mark_read(&conversation_id).unwrap(), Some(0));
        assert_eq!(
            service
                .conversation_meta(&conversation_id)
                .unwrap()
                .expect("metadata")
                .last_message_preview,
            "hello"
        );
    }

    #[test]
    fn blank_text_is_rejected_before_persistence() {
        let service = service();
        let error = service
            .send_text(TextSendRequest {
                command_key: "blank-text".into(),
                conversation_id: [1; 32],
                text: "  ".into(),
                thread_target: None,
            })
            .expect_err("blank text must fail");
        assert!(error.to_string().contains("must not be empty"));
    }

    #[test]
    fn distinct_identical_text_sends_get_distinct_admissions() {
        let service = service();
        let send = || {
            service
                .send_text(TextSendRequest {
                    command_key: format!("distinct-{}", rand::random::<u64>()),
                    conversation_id: [3; 32],
                    text: "same text".into(),
                    thread_target: None,
                })
                .expect("send text")
        };

        let first = send();
        let second = send();
        assert_ne!(first.event_id, second.event_id);
        assert_ne!(first.encoded, second.encoded);
        assert_eq!(first.message_hash, second.message_hash);
    }

    #[test]
    fn retrying_same_command_reuses_admitted_event_and_signed_bytes() {
        let service = service();
        let send = || {
            service.send_text(TextSendRequest {
                command_key: "stable-command".into(),
                conversation_id: [4; 32],
                text: "retry me".into(),
                thread_target: None,
            })
        };
        let first = send().unwrap();
        let retry = send().unwrap();
        assert_eq!(first.event_id, retry.event_id);
        assert_eq!(first.encoded, retry.encoded);
        assert_eq!(first.envelope_hash, retry.envelope_hash);
        assert_eq!(service.storage.list_recent_outgoing(10).unwrap().len(), 1);
    }

    #[test]
    fn command_key_cannot_be_reused_for_different_content() {
        let service = service();
        let request = |text: &str| TextSendRequest {
            command_key: "conflicting-command".into(),
            conversation_id: [6; 32],
            text: text.into(),
            thread_target: None,
        };
        service.send_text(request("first")).unwrap();
        let error = service
            .send_text(request("different"))
            .expect_err("same key with different body must be rejected");
        assert!(!error.to_string().is_empty());
        assert_eq!(service.storage.list_recent_outgoing(10).unwrap().len(), 1);
    }

    #[test]
    fn admitted_text_repairs_history_after_projection_failure() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("message_store.db");
        let storage = Storage::memory().unwrap();
        let service = {
            let secret_key = SecretKey::from_bytes(&[7; 32]);
            ConversationApplicationService::new(
                MessageStore::open(&store_path).unwrap(),
                storage.clone(),
                secret_key.clone(),
                secret_key.public(),
            )
        };
        let conn = rusqlite::Connection::open(&store_path).unwrap();
        conn.execute_batch(
                "CREATE TRIGGER fail_text_history BEFORE INSERT ON messages
                 BEGIN SELECT RAISE(ABORT, 'injected history failure'); END;",
            ).unwrap();
        let request = || TextSendRequest {
            command_key: "recover-history".into(),
            conversation_id: [10; 32],
            text: "recover me".into(),
            thread_target: None,
        };
        let prepared = service.send_text(request()).unwrap();
        assert!(prepared.projection_error.is_some());
        assert_eq!(storage.pending_text_send_projections().unwrap().len(), 1);
        // Projection repair is independent of transport lifecycle: a send
        // may advance while its history/thread projection remains pending.
        storage
            .update_outgoing_delivery_state(prepared.event_id, "sent")
            .unwrap();
        drop(service);
        conn.execute_batch("DROP TRIGGER fail_text_history;").unwrap();
        drop(conn);
        let secret_key = SecretKey::from_bytes(&[7; 32]);
        let restored = ConversationApplicationService::new(
            MessageStore::open(&store_path).unwrap(),
            storage.clone(),
            secret_key.clone(),
            secret_key.public(),
        );
        assert_eq!(restored.repair_pending_text_projections().unwrap(), 1);
        assert_eq!(restored.repair_pending_text_projections().unwrap(), 0);
        assert!(storage.pending_text_send_projections().unwrap().is_empty());
    }

    #[test]
    fn admitted_thread_reply_repairs_thread_projection_after_failure() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("message_store.db");
        let storage = Storage::memory().unwrap();
        let secret_key = SecretKey::from_bytes(&[8; 32]);
        let service = ConversationApplicationService::new(
            MessageStore::open(&store_path).unwrap(),
            storage.clone(),
            secret_key.clone(),
            secret_key.public(),
        );
        storage
            .with_conn(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER fail_thread_projection BEFORE INSERT ON chat_messages
                     WHEN NEW.thread_root_id IS NOT NULL
                     BEGIN SELECT RAISE(ABORT, 'injected thread projection failure'); END;",
                )
                .std_context("create thread projection failure trigger")?;
                Ok(())
            })
            .unwrap();
        let topic = [11; 32];
        let root = [12; 32];
        let request = || TextSendRequest {
            command_key: "recover-thread".into(),
            conversation_id: topic,
            text: "thread reply".into(),
            thread_target: Some(ThreadTarget::root(root)),
        };
        let prepared = service.send_text(request()).unwrap();
        assert!(prepared.projection_error.is_some());
        assert_eq!(storage.pending_text_send_projections().unwrap().len(), 1);
        drop(service);
        storage
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER fail_thread_projection;")
                    .std_context("drop thread projection failure trigger")?;
                Ok(())
            })
            .unwrap();
        let restored = ConversationApplicationService::new(
            MessageStore::open(&store_path).unwrap(),
            storage.clone(),
            secret_key.clone(),
            secret_key.public(),
        );
        assert_eq!(restored.repair_pending_text_projections().unwrap(), 1);
        assert_eq!(restored.repair_pending_text_projections().unwrap(), 0);
        let rows = storage
            .list_thread_messages_for_topic(&topic, Some(&root), false)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].thread_root_id, Some(root));
        assert!(storage.pending_text_send_projections().unwrap().is_empty());
    }

    #[test]
    fn completed_projections_are_safe_to_replay_when_completion_marker_fails() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("message_store.db");
        let storage = Storage::open(dir.path()).unwrap();
        let secret_key = SecretKey::from_bytes(&[9; 32]);
        let service = ConversationApplicationService::new(
            MessageStore::open(&store_path).unwrap(),
            storage.clone(),
            secret_key.clone(),
            secret_key.public(),
        );
        storage
            .with_conn(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER fail_projection_marker BEFORE UPDATE OF projection_complete ON text_send_commands
                     WHEN NEW.projection_complete = 1
                     BEGIN SELECT RAISE(ABORT, 'injected projection marker failure'); END;",
                )
                .std_context("create projection marker failure trigger")?;
                Ok(())
            })
            .unwrap();
        let topic = [13; 32];
        let prepared = service
            .send_text(TextSendRequest {
                command_key: "recover-marker".into(),
                conversation_id: topic,
                text: "marker retry".into(),
                thread_target: None,
            })
            .unwrap();
        assert!(prepared.projection_error.is_some());
        assert_eq!(storage.pending_text_send_projections().unwrap().len(), 1);
        drop(service);
        storage
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER fail_projection_marker;")
                    .std_context("drop projection marker failure trigger")?;
                Ok(())
            })
            .unwrap();
        drop(storage);
        let storage = Storage::open(dir.path()).unwrap();
        let restored = ConversationApplicationService::new(
            MessageStore::open(&store_path).unwrap(),
            storage.clone(),
            secret_key.clone(),
            secret_key.public(),
        );
        assert_eq!(restored.repair_pending_text_projections().unwrap(), 1);
        assert_eq!(restored.repair_pending_text_projections().unwrap(), 0);
        assert_eq!(restored.store.message_history_snapshot(&topic).unwrap(), 1);
        assert!(storage.pending_text_send_projections().unwrap().is_empty());
    }

    #[test]
    fn outgoing_admission_failure_does_not_project_successful_history() {
        let service = service();
        service
            .storage
            .with_conn(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER reject_text_admission BEFORE INSERT ON outgoing_messages
                     BEGIN SELECT RAISE(ABORT, 'injected admission failure'); END;",
                )
                .std_context("create admission failure trigger")?;
                Ok(())
            })
            .unwrap();
        let conversation_id = [5; 32];

        let error = service
            .send_text(TextSendRequest {
                command_key: "failed-admission".into(),
                conversation_id,
                text: "must not be admitted".into(),
                thread_target: None,
            })
            .expect_err("injected outbox admission failure");

        assert!(error.to_string().contains("admit local text message"));
        assert!(service
            .conversation_meta(&conversation_id)
            .unwrap()
            .is_none());
        assert!(service
            .storage
            .list_outgoing_for_topic(&crate::TopicId::from_bytes(conversation_id))
            .unwrap()
            .is_empty());
    }
}
