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
use crate::threads::ThreadTarget;

/// A validated text-send request shared by frontends.
#[derive(Debug, Clone)]
pub struct TextSendRequest {
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
    secret_key: SecretKey,
    local_user_id: PublicKey,
}

impl ConversationApplicationService {
    /// Create a service over an existing message store.
    pub fn new(store: MessageStore, secret_key: SecretKey, local_user_id: PublicKey) -> Self {
        Self {
            store,
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
        let encoded = SignedMessage::sign_and_encode(&self.secret_key, &message)
            .std_context("sign text message")?;
        let envelope_hash = *blake3::hash(&encoded).as_bytes();
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .std_context("read system clock")?
            .as_millis() as u64;

        self.store
            .insert_chat_message(
                &envelope_hash,
                &request.conversation_id,
                self.local_user_id.as_bytes(),
                timestamp_ms,
                "text",
                text,
                Some(&encoded),
                None,
                self.local_user_id.as_bytes(),
            )
            .std_context("persist local text message")?;
        if let Some(target) = request.thread_target {
            self.store
                .set_thread_target(&envelope_hash, &target)
                .std_context("persist local thread target")?;
        }

        Ok(PreparedTextMessage {
            message_hash,
            envelope_hash,
            encoded,
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
                conversation_id,
                text: " hello ".into(),
                thread_target: None,
            })
            .expect("send text");

        assert_ne!(prepared.message_hash, [0; 32]);
        assert_ne!(prepared.envelope_hash, [0; 32]);
        assert!(!prepared.encoded.is_empty());
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
                conversation_id: [1; 32],
                text: "  ".into(),
                thread_target: None,
            })
            .expect_err("blank text must fail");
        assert!(error.to_string().contains("must not be empty"));
    }
}
