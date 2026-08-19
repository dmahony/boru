//! Deterministic pinned-message state and authenticated operation helpers.
//!
//! Pins are references to message hashes, not copies of message content. This
//! means a pin can be received before its message and can remain visible after
//! history pruning; the UI decides how an unavailable reference is rendered.

use std::collections::HashMap;

use iroh::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};

use crate::chat_core::MessageHash;
use crate::proto::TopicId;

/// Canonical signing domain for pin operations.
pub const PIN_OPERATION_PROTOCOL: &str = "boru/pinned-message";
/// Version of the pin-operation framing.
pub const PIN_OPERATION_VERSION: u16 = 1;

/// Whether an operation adds or removes a pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PinAction {
    /// Make the referenced message pinned.
    Pin,
    /// Remove the referenced message from the pinned set.
    Unpin,
}

/// Stable, authenticated pin operation metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinOperation {
    /// Conversation in which the operation applies.
    pub topic: TopicId,
    /// Referenced message hash.
    pub message_hash: MessageHash,
    /// Add or remove the pin.
    pub action: PinAction,
    /// Sender's public identity.
    pub author: PublicKey,
    /// Unix epoch seconds from the signed envelope.
    pub sent_at: u64,
    /// Ed25519 signature over the canonical operation framing.
    pub signature: Vec<u8>,
}

impl PinOperation {
    /// Sign a pin operation using the sender's identity key.
    pub fn sign(
        secret_key: &SecretKey,
        topic: TopicId,
        message_hash: MessageHash,
        action: PinAction,
        sent_at: u64,
    ) -> Self {
        let author = secret_key.public();
        let framing = canonical_bytes(&topic, &message_hash, action, &author, sent_at);
        Self {
            topic,
            message_hash,
            action,
            author,
            sent_at,
            signature: secret_key.sign(&framing).to_bytes().to_vec(),
        }
    }

    /// Verify the operation's signature and basic identity binding.
    pub fn verify(&self) -> bool {
        crate::protocol_signing::verify(
            &self.author,
            &self.signature,
            &canonical_bytes(
                &self.topic,
                &self.message_hash,
                self.action,
                &self.author,
                self.sent_at,
            ),
        )
    }
}

fn canonical_bytes(
    topic: &TopicId,
    message_hash: &MessageHash,
    action: PinAction,
    author: &PublicKey,
    sent_at: u64,
) -> Vec<u8> {
    crate::protocol_signing::canonical_signed_bytes(
        PIN_OPERATION_PROTOCOL,
        PIN_OPERATION_VERSION,
        &(topic, message_hash, action, author, sent_at),
    )
    .expect("pin operation framing is serializable")
}

#[derive(Debug, Clone)]
struct PinRecord {
    action: PinAction,
    sent_at: u64,
    author: PublicKey,
}

/// In-memory, deterministic reconciliation state for one or more topics.
#[derive(Debug, Default, Clone)]
pub struct PinState {
    records: HashMap<(TopicId, MessageHash), PinRecord>,
}

impl PinState {
    /// Replace the in-memory projection with rows loaded from SQLite.
    pub fn load_rows(
        &mut self,
        rows: impl IntoIterator<Item = (TopicId, MessageHash, PublicKey, PinAction, u64)>,
    ) {
        self.records.clear();
        for (topic, message_hash, author, action, sent_at) in rows {
            self.apply_authenticated(topic, message_hash, action, author, sent_at);
        }
    }

    /// Apply a pin operation whose enclosing `SignedMessage` has already been
    /// verified by the chat receiver.
    pub fn apply_authenticated(
        &mut self,
        topic: TopicId,
        message_hash: MessageHash,
        action: PinAction,
        author: PublicKey,
        sent_at: u64,
    ) -> bool {
        let key = (topic, message_hash);
        let newer = self.records.get(&key).is_none_or(|old| {
            (sent_at, author.as_bytes()) > (old.sent_at, old.author.as_bytes())
        });
        if newer {
            self.records.insert(key, PinRecord { action, sent_at, author });
        }
        newer
    }

    /// Apply a verified operation, ignoring stale arrival-order updates.
    pub fn apply(&mut self, operation: &PinOperation) -> bool {
        if !operation.verify() {
            return false;
        }
        let key = (operation.topic, operation.message_hash);
        let newer = self.records.get(&key).is_none_or(|old| {
            (operation.sent_at, operation.author.as_bytes())
                > (old.sent_at, old.author.as_bytes())
        });
        if newer {
            self.records.insert(
                key,
                PinRecord {
                    action: operation.action,
                    sent_at: operation.sent_at,
                    author: operation.author,
                },
            );
        }
        newer
    }

    /// Return whether a message is currently pinned.
    pub fn is_pinned(&self, topic: TopicId, message_hash: &MessageHash) -> bool {
        self.records
            .get(&(topic, *message_hash))
            .is_some_and(|record| record.action == PinAction::Pin)
    }

    /// Return all currently pinned hashes in deterministic order.
    pub fn pinned(&self, topic: TopicId) -> Vec<MessageHash> {
        let mut hashes: Vec<_> = self
            .records
            .iter()
            .filter_map(|((record_topic, hash), record)| {
                (record_topic == &topic && record.action == PinAction::Pin).then_some(*hash)
            })
            .collect();
        hashes.sort_unstable();
        hashes
    }

    /// Forget all state for a conversation being permanently removed.
    pub fn clear_topic(&mut self, topic: TopicId) {
        self.records.retain(|(record_topic, _), _| *record_topic != topic);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_are_deterministic_and_authenticated() {
        let key = SecretKey::generate();
        let topic = TopicId::from_bytes([7; 32]);
        let hash = [9; 32];
        let first = PinOperation::sign(&key, topic, hash, PinAction::Pin, 10);
        let second = PinOperation::sign(&key, topic, hash, PinAction::Pin, 10);
        assert_eq!(first.signature, second.signature);
        assert!(first.verify());
        let mut tampered = first.clone();
        tampered.sent_at = 11;
        assert!(!tampered.verify());
    }

    #[test]
    fn stale_reconnect_delivery_does_not_override_newer_state() {
        let key = SecretKey::generate();
        let topic = TopicId::from_bytes([1; 32]);
        let hash = [2; 32];
        let pin = PinOperation::sign(&key, topic, hash, PinAction::Pin, 20);
        let unpin = PinOperation::sign(&key, topic, hash, PinAction::Unpin, 21);
        let mut state = PinState::default();
        assert!(state.apply(&unpin));
        assert!(!state.apply(&pin));
        assert!(!state.is_pinned(topic, &hash));
    }

    #[test]
    fn references_can_exist_without_message_content() {
        let key = SecretKey::generate();
        let topic = TopicId::from_bytes([3; 32]);
        let hash = [4; 32];
        let op = PinOperation::sign(&key, topic, hash, PinAction::Pin, 1);
        let mut state = PinState::default();
        assert!(state.apply(&op));
        assert_eq!(state.pinned(topic), vec![hash]);
    }

    #[test]
    fn storage_reload_reconciles_reordered_rows_and_unpins() {
        let key = SecretKey::generate();
        let topic = TopicId::from_bytes([8; 32]);
        let hash = [6; 32];
        let mut state = PinState::default();
        state.load_rows([
            (topic, hash, key.public(), PinAction::Unpin, 12),
            (topic, hash, key.public(), PinAction::Pin, 11),
        ]);
        assert!(!state.is_pinned(topic, &hash));
        state.load_rows([(topic, hash, key.public(), PinAction::Pin, 13)]);
        assert!(state.is_pinned(topic, &hash));
    }
}
