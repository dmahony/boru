//! Conversation reconciliation after a reconnect (PDF Phase 3, Task 3.2 /
//! BORU-CP-08).
//!
//! When a peer becomes reachable again, the app must restore **only** the
//! communication state the local user is already entitled to. This module
//! owns the pure decision: given the local identity, the reconnected peer,
//! the peer's friend record (if any), and the local conversation store,
//! compute the set of direct topics that must be (re)subscribed.
//!
//! # Design rules (PDF Task 3.2 + cross-cutting guardrails)
//!
//! * **Restore only existing entitlement.** Topics come exclusively from
//!   local metadata — the friend record's designated direct conversation
//!   and existing non-archived direct `ConversationEntry` records whose
//!   `peer_id` matches the reconnected peer. Discovery advertisements are
//!   never a source of topics.
//! * **No authorisation by presence.** A peer being discoverable does not
//!   make them a friend, group member, tunnel client, or file recipient.
//!   A missing friend record yields nothing; a `Blocked` relationship
//!   yields nothing.
//! * **Deleted/blocked relationships are not resurrected.** An
//!   `Archived` designated direct conversation is skipped, archived
//!   conversation-store entries are skipped, and `Blocked` friends are
//!   skipped entirely.
//! * **No auto-join of new groups/public chats.** Only direct topics are
//!   ever returned. Group membership is owned by the group subsystem
//!   (deterministic topic ownership); discovery-driven reconnection never
//!   re-joins a group.
//! * **Idempotence / no duplicates.** Topics are deduplicated into a
//!   `BTreeSet`, so repeated announcements cannot duplicate
//!   subscriptions. This module never mutates stores — the caller decides
//!   what to do with the returned topics.
//! * **No control-plane/chat coupling.** This is a pure function over
//!   metadata; it never touches chat rendering, history, or backfill
//!   paths (message synchronisation stays out of discovery).

use std::collections::BTreeSet;

use iroh_base::PublicKey;

use crate::contact::direct_topic;
use crate::conversations::{ConversationEntry, ConversationKind};
use crate::friends::{DirectConversationState, FriendRecord, FriendRelationship};
use crate::proto::TopicId;

/// Compute the direct topics that must be (re)subscribed after `peer`
/// becomes reachable again.
///
/// Only topics the local user is already entitled to are returned:
///
/// 1. The friend record's designated direct conversation, if the friend
///    relationship allows messaging (`relationship.can_message()`) and the
///    designated conversation is not `Archived`. When the record carries no
///    designated conversation, the deterministic
///    [`direct_topic`](crate::contact::direct_topic) for the pair is used —
///    that is the stable direct topic the app subscribes for friends.
/// 2. Every existing, non-archived direct `ConversationEntry` whose
///    `peer_id` equals the reconnected peer.
///
/// Returns nothing for: non-friends (no record), `Blocked` friends,
/// `Archived` designated conversations, archived store entries, and group
/// or public topics. Results are deduplicated and sorted.
pub fn required_reconnect_topics(
    local: &PublicKey,
    peer: &PublicKey,
    friend: Option<&FriendRecord>,
    conversations: &[ConversationEntry],
) -> Vec<TopicId> {
    let mut topics = BTreeSet::new();

    if let Some(record) = friend {
        // A blocked relationship is never resurrected — no topic at all,
        // even if stale direct records exist.
        if record.relationship == FriendRelationship::Blocked {
            return Vec::new();
        }
        if record.relationship.can_message() {
            match record.direct_conversation() {
                Some(dc) if dc.state != DirectConversationState::Archived => {
                    topics.insert(dc.topic);
                }
                // Archived = the user deleted the designated direct
                // conversation; do not resurrect it.
                Some(_) => {}
                // No explicit direct-conversation metadata: the
                // deterministic direct topic is the friend's entitlement.
                None => {
                    topics.insert(direct_topic(local, peer));
                }
            }
        }
    }

    // Existing local direct conversations that require this peer. Archived
    // entries (deleted conversations) are never resurrected; group/public
    // entries are never auto-joined from discovery.
    for entry in conversations {
        if entry.kind != ConversationKind::Direct || entry.archived {
            continue;
        }
        if entry.peer_id == peer.to_string() {
            topics.insert(entry.topic);
        }
    }

    topics.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> PublicKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed).public()
    }

    fn topic(byte: u8) -> TopicId {
        TopicId::from_bytes([byte; 32])
    }

    fn friend_with(
        relationship: FriendRelationship,
        state: Option<DirectConversationState>,
    ) -> FriendRecord {
        let mut record = FriendRecord::default();
        record.relationship = relationship;
        if let Some(state) = state {
            record.set_direct_conversation(topic(0xAA), state);
        }
        record
    }

    fn direct_entry(peer: &PublicKey, archived: bool) -> ConversationEntry {
        let mut entry = ConversationEntry::new(topic(0xBB), peer.to_string(), "peer");
        entry.archived = archived;
        entry
    }

    /// A current friend with an active designated direct conversation must
    /// have that topic restored (existing direct chat recovers).
    #[test]
    fn friend_with_active_direct_conversation_restores_topic() {
        let local = key(0x01);
        let peer = key(0x02);
        let friend = friend_with(
            FriendRelationship::Friends,
            Some(DirectConversationState::Active),
        );

        let topics = required_reconnect_topics(&local, &peer, Some(&friend), &[]);

        assert_eq!(topics, vec![topic(0xAA)]);
    }

    /// A current friend without explicit direct-conversation metadata still
    /// restores the deterministic direct topic (the app's stable friend
    /// topic).
    #[test]
    fn friend_without_metadata_restores_deterministic_topic() {
        let local = key(0x01);
        let peer = key(0x02);
        let friend = friend_with(FriendRelationship::Friends, None);

        let topics = required_reconnect_topics(&local, &peer, Some(&friend), &[]);

        assert_eq!(topics, vec![direct_topic(&local, &peer)]);
    }

    /// A peer with no friend record has no entitlement from discovery alone
    /// (no authorisation by presence).
    #[test]
    fn non_friend_yields_nothing() {
        let local = key(0x01);
        let peer = key(0x02);

        let topics = required_reconnect_topics(&local, &peer, None, &[]);

        assert!(topics.is_empty());
    }

    /// A blocked relationship is never resurrected, even when stale direct
    /// records exist in the store.
    #[test]
    fn blocked_friend_yields_nothing_even_with_stale_records() {
        let local = key(0x01);
        let peer = key(0x02);
        let friend = friend_with(
            FriendRelationship::Blocked,
            Some(DirectConversationState::Active),
        );
        let entries = [direct_entry(&peer, false)];

        let topics = required_reconnect_topics(&local, &peer, Some(&friend), &entries);

        assert!(topics.is_empty());
    }

    /// An archived (deleted) designated direct conversation is not
    /// resurrected.
    #[test]
    fn archived_designated_conversation_not_resurrected() {
        let local = key(0x01);
        let peer = key(0x02);
        let friend = friend_with(
            FriendRelationship::Friends,
            Some(DirectConversationState::Archived),
        );

        let topics = required_reconnect_topics(&local, &peer, Some(&friend), &[]);

        assert!(topics.is_empty());
    }

    /// Existing non-archived direct conversation records for the peer are
    /// restored even without a friend record (a previously opened direct
    /// chat is an existing entitlement).
    #[test]
    fn existing_direct_store_entry_is_restored() {
        let local = key(0x01);
        let peer = key(0x02);
        let entries = [direct_entry(&peer, false)];

        let topics = required_reconnect_topics(&local, &peer, None, &entries);

        assert_eq!(topics, vec![topic(0xBB)]);
    }

    /// Archived store entries (deleted conversations) are never resurrected.
    #[test]
    fn archived_store_entry_not_resurrected() {
        let local = key(0x01);
        let peer = key(0x02);
        let entries = [direct_entry(&peer, true)];

        let topics = required_reconnect_topics(&local, &peer, None, &entries);

        assert!(topics.is_empty());
    }

    /// Direct records for a different peer are not restored.
    #[test]
    fn other_peer_store_entry_not_restored() {
        let local = key(0x01);
        let peer = key(0x02);
        let other = key(0x03);
        let entries = [direct_entry(&other, false)];

        let topics = required_reconnect_topics(&local, &peer, None, &entries);

        assert!(topics.is_empty());
    }

    /// Group/public conversation entries are never auto-joined from
    /// discovery, even when they belong to the reconnected peer's topic
    /// space.
    #[test]
    fn group_entries_never_auto_joined() {
        let local = key(0x01);
        let peer = key(0x02);
        let mut group = ConversationEntry::new_group(topic(0xCC), "a group");
        // A group record should not be treated as a direct entitlement.
        group.peer_id = peer.to_string();

        let topics = required_reconnect_topics(&local, &peer, None, &[group]);

        assert!(topics.is_empty());
    }

    /// Duplicate sources (friend designated topic == store entry topic) are
    /// deduplicated — reconnection never produces duplicate topics.
    #[test]
    fn duplicates_are_deduplicated() {
        let local = key(0x01);
        let peer = key(0x02);
        let friend = friend_with(
            FriendRelationship::Friends,
            Some(DirectConversationState::Active),
        );
        // Store entry with the same topic as the friend's designated topic.
        let mut entry = ConversationEntry::new(topic(0xAA), peer.to_string(), "peer");
        entry.archived = false;
        let entries = [entry];

        let topics = required_reconnect_topics(&local, &peer, Some(&friend), &entries);

        assert_eq!(topics, vec![topic(0xAA)]);
    }
}
