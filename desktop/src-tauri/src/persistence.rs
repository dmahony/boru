//! Desktop persistence adapter over Boru's existing SQLite message store.
//!
//! This deliberately delegates schema ownership, message identity, delivery
//! transitions, and unread accounting to `boru_core::store::MessageStore`.
use std::path::Path;

use anyhow::{Context, Result};
use boru_core::store::{ChatMessageRow, MessageStore};

/// Input projection for inserting a persisted chat message.
#[derive(Debug, Clone, Copy)]
pub struct MessageRecord<'a> {
    pub hash: &'a [u8; 32],
    pub room: &'a [u8; 32],
    pub sender: &'a [u8; 32],
    pub timestamp_ms: u64,
    pub kind: &'a str,
    pub body: &'a str,
    pub signed_bytes: Option<&'a [u8]>,
    pub local_user: &'a [u8; 32],
}

/// Narrow persistence seam shared by the desktop command and event layers.
#[derive(Clone, Debug)]
pub struct MessagePersistence {
    store: MessageStore,
}

impl MessagePersistence {
    /// Open (or create) the message database for a profile. Reopen the same
    /// path after dropping this adapter to exercise process-restart semantics.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            store: MessageStore::open(path).context("open Boru message persistence")?,
        })
    }

    pub fn insert_message(&self, message: MessageRecord<'_>) -> Result<bool> {
        self.store
            .insert_chat_message(
                message.hash,
                message.room,
                message.sender,
                message.timestamp_ms,
                message.kind,
                message.body,
                message.signed_bytes,
                None,
                message.local_user,
            )
            .context("persist desktop chat message")
    }

    pub fn history(
        &self,
        room: &[u8; 32],
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ChatMessageRow>> {
        self.store
            .get_messages_for_topic(room, limit, offset)
            .context("read desktop room history")
    }

    pub fn set_delivery_state(&self, message_hash: &[u8; 32], state: &str) -> Result<bool> {
        self.store
            .update_message_delivery_state_monotonic(message_hash, state)
            .context("persist desktop delivery state")
    }

    pub fn record_ack(
        &self,
        message_hash: &[u8; 32],
        recipient_device: &[u8],
        observed_at_ms: u64,
    ) -> Result<bool> {
        self.store
            .record_recipient_ack(message_hash, recipient_device, observed_at_ms)
            .context("persist desktop delivery acknowledgement")
    }

    pub fn unread_count(&self, room: &[u8; 32]) -> Result<Option<u32>> {
        self.store
            .get_unread_count(room)
            .context("read desktop unread count")
    }

    pub fn mark_read(&self, room: &[u8; 32]) -> Result<Option<u32>> {
        self.store
            .mark_conversation_read(room)
            .context("persist desktop read marker")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn message_ack_and_unread_survive_reopening_profile() {
        let profile = tempdir().unwrap();
        let database = profile.path().join("messages.sqlite");
        let room = [1; 32];
        let local = [2; 32];
        let remote = [3; 32];
        let hash = [4; 32];
        let peer = [5; 32];
        {
            let persistence = MessagePersistence::open(&database).unwrap();
            persistence
                .insert_message(MessageRecord {
                    hash: &hash,
                    room: &room,
                    sender: &remote,
                    timestamp_ms: 10,
                    kind: "text",
                    body: "hello",
                    signed_bytes: None,
                    local_user: &local,
                })
                .unwrap();
            assert_eq!(persistence.unread_count(&room).unwrap(), Some(1));
            persistence.record_ack(&hash, &peer, 20).unwrap();
            persistence.set_delivery_state(&hash, "delivered").unwrap();
        }
        let reopened = MessagePersistence::open(&database).unwrap();
        let rows = reopened.history(&room, 10, 0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].body, "hello");
        assert_eq!(rows[0].delivery_state, "recipient_acknowledged");
        assert_eq!(reopened.unread_count(&room).unwrap(), Some(1));
        assert_eq!(reopened.mark_read(&room).unwrap(), Some(1));
        assert_eq!(reopened.unread_count(&room).unwrap(), Some(0));
    }
}
