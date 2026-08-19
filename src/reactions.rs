//! Deterministic, actor-scoped reaction state.
//!
//! Reactions are projections keyed by `(message, actor, emoji)`.  A remove is
//! a durable tombstone, so delivery order cannot resurrect a reaction.  The
//! gossip envelope authenticates `actor`; this module deliberately does not
//! trust actor bytes carried in an unauthenticated UI payload.

use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Stable identifier of the message being reacted to.
pub type MessageId = [u8; 32];
/// Authenticated public-key bytes of the actor.
pub type ActorId = [u8; 32];

/// Operation represented by a reaction event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactionOp {
    /// Enable the actor's reaction.
    Add,
    /// Permanently disable the actor's reaction.
    Remove,
}

/// Authenticated reaction operation keyed by message, actor, and emoji.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReactionEvent {
    /// Stable identifier of the target message.
    pub message_id: MessageId,
    /// Public-key bytes of the authenticated actor.
    pub actor: ActorId,
    /// Emoji being added or removed.
    pub emoji: String,
    /// Operation to apply.
    pub op: ReactionOp,
}

impl ReactionEvent {
    /// Construct an add operation.
    pub fn add(message_id: MessageId, actor: ActorId, emoji: impl Into<String>) -> Self {
        Self { message_id, actor, emoji: emoji.into(), op: ReactionOp::Add }
    }

    /// Construct a remove operation.
    pub fn remove(message_id: MessageId, actor: ActorId, emoji: impl Into<String>) -> Self {
        Self { message_id, actor, emoji: emoji.into(), op: ReactionOp::Remove }
    }

    fn key(&self) -> (MessageId, ActorId, String) {
        (self.message_id, self.actor, self.emoji.clone())
    }
}

/// Materialized reaction projection.  B-tree collections make rendering and
/// replication deterministic regardless of arrival order or fan-out path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReactionState {
    active: BTreeMap<MessageId, BTreeSet<(ActorId, String)>>,
    removed: HashSet<(MessageId, ActorId, String)>,
}

impl ReactionState {
    /// Apply an event, returning whether the projection changed.
    pub fn apply(&mut self, event: ReactionEvent) -> bool {
        let key = event.key();
        match event.op {
            ReactionOp::Remove => {
                let changed = self.removed.insert(key.clone());
                if let Some(rows) = self.active.get_mut(&key.0) {
                    let removed_active = rows.remove(&(key.1, key.2.clone()));
                    changed || removed_active
                } else { changed }
            }
            ReactionOp::Add => {
                if self.removed.contains(&key) { return false; }
                self.active.entry(key.0).or_default().insert((key.1, key.2))
            }
        }
    }

    /// Return active `(actor, emoji)` pairs in deterministic order.
    pub fn for_message(&self, message_id: &MessageId) -> Vec<(ActorId, String)> {
        self.active.get(message_id).map(|v| v.iter().cloned().collect()).unwrap_or_default()
    }

    /// Test whether an actor currently has an emoji on a message.
    pub fn contains(&self, message_id: &MessageId, actor: &ActorId, emoji: &str) -> bool {
        self.active.get(message_id).is_some_and(|v| v.contains(&(*actor, emoji.to_owned())))
    }

    /// Test whether a remove tombstone exists for a reaction key.
    pub fn is_removed(&self, message_id: &MessageId, actor: &ActorId, emoji: &str) -> bool {
        self.removed.contains(&(*message_id, *actor, emoji.to_owned()))
    }

    /// Count active reactions on a message.
    pub fn active_count(&self, message_id: &MessageId) -> usize {
        self.active.get(message_id).map_or(0, BTreeSet::len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const M: MessageId = [7; 32];
    const A: ActorId = [3; 32];

    #[test]
    fn duplicate_add_and_remove_are_idempotent() {
        let mut state = ReactionState::default();
        assert!(state.apply(ReactionEvent::add(M, A, "👍")));
        assert!(!state.apply(ReactionEvent::add(M, A, "👍")));
        assert!(state.apply(ReactionEvent::remove(M, A, "👍")));
        assert!(!state.apply(ReactionEvent::remove(M, A, "👍")));
        assert_eq!(state.active_count(&M), 0);
    }

    #[test]
    fn remove_before_add_does_not_resurrect() {
        let mut state = ReactionState::default();
        assert!(state.apply(ReactionEvent::remove(M, A, "🔥")));
        assert!(!state.apply(ReactionEvent::add(M, A, "🔥")));
        assert!(state.is_removed(&M, &A, "🔥"));
    }

    #[test]
    fn projection_order_is_stable() {
        let mut state = ReactionState::default();
        state.apply(ReactionEvent::add(M, [2; 32], "😂"));
        state.apply(ReactionEvent::add(M, [1; 32], "👍"));
        assert_eq!(state.for_message(&M)[0], ([1; 32], "👍".into()));
    }
}
