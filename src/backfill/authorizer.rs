//! Authorization gate for the history backfill protocol.
//!
//! Decides whether a remote [`PublicKey`] may request history for a given
//! [`TopicId`].  The peer identity ALWAYS comes from the authenticated QUIC
//! connection — never from the request payload.

use std::sync::Arc;

use iroh::PublicKey;

use crate::contact::direct_topic;
use crate::proto::TopicId;
use crate::public_room::{public_lobby_topic, PublicNetwork};
use crate::storage::Storage;

// ── Authorization (server side) ───────────────────────────────────────────────

/// Centralized authorization for history backfill requests.
///
/// Decides whether a remote `peer` may currently request history for
/// `topic`.  The peer identity ALWAYS comes from the authenticated QUIC
/// connection ([`Connection::remote_id`]) — never from the request payload.
///
/// Policy:
/// - **Group epoch topics** — the peer must be an active member of the group
///   (state `Active`/`Member`/`Owner`) *and* the local node must still be an
///   active member, so a node removed from a group never serves stale group
///   history.
/// - **Direct-chat topics** — the peer must be the deterministic counterpart
///   of the local node for that topic (`direct_topic(peer, local) == topic`).
/// - **Public rooms** — the canonical public lobby and any topic advertised
///   in the public-room directory are readable by any authenticated peer.
/// - **Everything else** is denied without leaking whether the topic exists.
#[derive(Debug, Clone)]
pub struct BackfillAuthorizer {
    storage: Arc<Storage>,
    local_public: PublicKey,
}

impl BackfillAuthorizer {
    /// Create an authorizer for a node with the given local identity.
    pub fn new(storage: Arc<Storage>, local_public: PublicKey) -> Self {
        Self {
            storage,
            local_public,
        }
    }

    /// One authorization check: is `peer` currently allowed to backfill
    /// history for `topic`?
    ///
    /// This runs *before* any storage query that would reveal message
    /// IDs, counts, or metadata.  Unknown and forbidden topics both return
    /// `false` so an attacker cannot distinguish them externally.
    pub fn authorize(&self, peer: &PublicKey, topic: &TopicId) -> bool {
        // 0. The internal discovery topic is NOT a conversation store
        //    (BORU-DISC-13). Even if a peer somehow derived it, history
        //    backfill for the discovery mesh must never be served:
        //    discovery payloads are networking infrastructure, not chat
        //    history. This explicit exclusion keeps the policy honest even
        //    if a future storage layout made the topic look like a room.
        if crate::discovery_topic::is_discovery_topic(*topic) {
            return false;
        }

        // 1. Group epoch topic — membership is authoritative.  A group topic
        //    never falls through to the other checks even when the requester
        //    is not a member.
        if let Ok(Some(group)) = self.storage.find_group_by_topic(topic) {
            return is_active_group_member(&self.storage, &group.group_id, peer)
                && is_active_group_member(&self.storage, &group.group_id, &self.local_public);
        }

        // 2. Deterministic direct-chat topic — only the two participants can
        //    derive it, so the requester matching the topic IS the
        //    direct-chat relationship.
        if direct_topic(peer, &self.local_public) == *topic {
            return true;
        }

        // 3. Public-room policy — the canonical lobby and rooms advertised in
        //    the public-room directory are open to any authenticated peer.
        self.is_public_room_topic(topic)
    }

    fn is_public_room_topic(&self, topic: &TopicId) -> bool {
        if *topic == public_lobby_topic(PublicNetwork::Mainnet)
            || *topic == public_lobby_topic(PublicNetwork::Development)
            || *topic == public_lobby_topic(PublicNetwork::Test)
        {
            return true;
        }
        self.storage.is_public_room_topic(topic).unwrap_or(false)
    }
}

/// Active membership states — mirrored from the group UI filter in
/// `src/bin/boru/app.rs` (view_group_member_list).
fn is_active_group_member(storage: &Storage, group_id: &[u8; 32], peer: &PublicKey) -> bool {
    match storage.list_group_members(group_id) {
        Ok(members) => members.iter().any(|m| {
            m.public_key.as_slice() == peer.as_bytes()
                && (m.state == "Active" || m.state == "Member" || m.state == "Owner")
        }),
        Err(_) => false,
    }
}
