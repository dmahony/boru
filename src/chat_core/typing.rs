//! Ephemeral, conversation-scoped typing state.
//!
//! Typing is deliberately separate from durable chat entries and storage.  A
//! peer refreshes its lease while typing; receivers expire leases locally when
//! no refresh/stop arrives (including on disconnect).

#![allow(missing_docs)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use iroh::PublicKey;

use crate::proto::TopicId;

/// Lease duration used by receivers when a typing refresh arrives.
pub const TYPING_LEASE: Duration = Duration::from_secs(4);
/// Minimum interval between locally emitted refreshes.
pub const TYPING_EMIT_INTERVAL: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypingLease {
    pub expires_at: Instant,
}

#[derive(Debug, Default)]
pub struct TypingState {
    leases: HashMap<(TopicId, PublicKey), TypingLease>,
}

impl TypingState {
    pub fn set(&mut self, topic: TopicId, peer: PublicKey, now: Instant) {
        self.leases.insert(
            (topic, peer),
            TypingLease {
                expires_at: now + TYPING_LEASE,
            },
        );
    }

    pub fn clear(&mut self, topic: TopicId, peer: &PublicKey) {
        self.leases.remove(&(topic, *peer));
    }

    pub fn clear_peer(&mut self, peer: &PublicKey) {
        self.leases.retain(|(_, current), _| current != peer);
    }

    pub fn expire(&mut self, now: Instant) {
        self.leases.retain(|_, lease| lease.expires_at > now);
    }

    pub fn active_peers(&mut self, topic: TopicId, now: Instant) -> Vec<PublicKey> {
        self.expire(now);
        let mut peers: Vec<_> = self
            .leases
            .iter()
            .filter_map(|((current_topic, peer), _)| (*current_topic == topic).then_some(*peer))
            .collect();
        peers.sort_by_key(|peer| peer.to_string());
        peers
    }

    pub fn contains(&mut self, topic: TopicId, peer: &PublicKey, now: Instant) -> bool {
        self.expire(now);
        self.leases.contains_key(&(topic, *peer))
    }

    pub fn len(&self) -> usize {
        self.leases.len()
    }

    /// Count leases for a conversation without mutating state; callers that
    /// own mutable state should call [`Self::expire`] before rendering.
    pub fn count_for_topic(&self, topic: TopicId) -> usize {
        self.leases
            .keys()
            .filter(|(current_topic, _)| *current_topic == topic)
            .count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypingEmitter {
    last_emit: Option<Instant>,
}

impl Default for TypingEmitter {
    fn default() -> Self {
        Self { last_emit: None }
    }
}

impl TypingEmitter {
    pub fn should_emit(&mut self, now: Instant) -> bool {
        if self
            .last_emit
            .is_some_and(|last| now.duration_since(last) < TYPING_EMIT_INTERVAL)
        {
            return false;
        }
        self.last_emit = Some(now);
        true
    }

    pub fn reset(&mut self) {
        self.last_emit = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topic(n: u8) -> TopicId {
        TopicId::from([n; 32])
    }
    fn peer(n: u8) -> PublicKey {
        iroh::SecretKey::from_bytes(&[n; 32]).public()
    }

    #[test]
    fn multiple_typers_expire_independently_and_are_room_scoped() {
        let now = Instant::now();
        let mut state = TypingState::default();
        state.set(topic(1), peer(1), now);
        state.set(topic(1), peer(2), now + Duration::from_secs(2));
        state.set(topic(2), peer(3), now);
        let active = state.active_peers(topic(1), now + Duration::from_secs(1));
        assert_eq!(active.len(), 2);
        assert!(active.contains(&peer(1)) && active.contains(&peer(2)));
        assert_eq!(
            state.active_peers(topic(1), now + Duration::from_secs(5)),
            vec![peer(2)]
        );
        assert!(state
            .active_peers(topic(2), now + Duration::from_secs(5))
            .is_empty());
    }

    #[test]
    fn emitter_is_bounded_and_resettable() {
        let now = Instant::now();
        let mut emitter = TypingEmitter::default();
        assert!(emitter.should_emit(now));
        assert!(!emitter.should_emit(now + Duration::from_millis(100)));
        assert!(emitter.should_emit(now + TYPING_EMIT_INTERVAL));
        emitter.reset();
        assert!(emitter.should_emit(now + Duration::from_millis(200)));
    }

    #[test]
    fn stop_and_disconnect_clear_only_the_relevant_state() {
        let now = Instant::now();
        let mut state = TypingState::default();
        state.set(topic(1), peer(1), now);
        state.set(topic(1), peer(2), now);
        state.clear(topic(1), &peer(1));
        assert!(!state.contains(topic(1), &peer(1), now));
        assert!(state.contains(topic(1), &peer(2), now));
        state.clear_peer(&peer(2));
        assert_eq!(state.len(), 0);
    }
}
