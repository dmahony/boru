//! Reply references shared by protocol, storage, and frontends.
//!
//! A reply is intentionally only an address. The parent body is looked up from
//! the message store and is never copied into the reply payload.

use std::collections::HashMap;

use crate::chat_core::MessageId;

/// A reply target retained by the UI while its parent is unavailable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyReference {
    /// Stable id of the parent message.
    pub message_id: MessageId,
    /// Whether the target is currently present in the local projection.
    pub resolved: bool,
}

impl ReplyReference {
    /// Construct an unresolved reference.
    pub fn unresolved(message_id: MessageId) -> Self {
        Self { message_id, resolved: false }
    }
}

/// Small in-memory resolver used by frontends and backfill handlers.
#[derive(Clone, Debug, Default)]
pub struct ReplyResolver {
    bodies: HashMap<MessageId, String>,
    pending: HashMap<MessageId, Vec<MessageId>>,
}

impl ReplyResolver {
    /// Record a message body and return replies that became resolvable.
    pub fn insert(&mut self, message_id: MessageId, body: impl Into<String>) -> Vec<MessageId> {
        self.bodies.insert(message_id, body.into());
        self.pending.remove(&message_id).unwrap_or_default()
    }

    /// Register a reply and return the parent body when already available.
    pub fn reference(&mut self, reply_id: MessageId, parent_id: MessageId) -> Option<&str> {
        if let Some(body) = self.bodies.get(&parent_id) {
            return Some(body.as_str());
        }
        self.pending.entry(parent_id).or_default().push(reply_id);
        None
    }

    /// Look up a resolved parent body.
    pub fn body(&self, message_id: &MessageId) -> Option<&str> {
        self.bodies.get(message_id).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unresolved_reply_resolves_when_parent_arrives() {
        let parent = [7; 32];
        let reply = [8; 32];
        let mut resolver = ReplyResolver::default();
        assert_eq!(resolver.reference(reply, parent), None);
        assert_eq!(resolver.insert(parent, "parent"), vec![reply]);
        assert_eq!(resolver.body(&parent), Some("parent"));
    }

    #[test]
    fn reply_wire_roundtrip_keeps_only_parent_id() {
        let parent = [9; 32];
        let message = crate::chat_core::Message::reply("answer", parent);
        let bytes = postcard::to_stdvec(&message).unwrap();
        let decoded: crate::chat_core::Message = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.reply_to_message_id(), Some(parent));
        assert!(matches!(decoded, crate::chat_core::Message::Reply { text, .. } if text == "answer"));
    }
}
