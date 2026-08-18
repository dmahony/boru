//! Peer registry and dedup logic for the internal discovery subsystem.
//!
//! Extracted from [`DiscoveryService`](crate::discovery_service::DiscoveryService)
//! (BORU-DISC-004). This module owns the in-process registry of peers seen
//! on the discovery topic plus the `(node_id, event_id)` dedup policy
//! (BORU-DISC-17).
//!
//! # Architecture
//!
//! * [`PeerRegistry`] is the pure, owned state — a `HashMap<PublicKey,
//!   PeerRegistryEntry>`. All insert / refresh / dedup / prune / clear /
//!   accessor logic lives here and is unit-testable in isolation (no
//!   network, no peers).
//! * `DiscoveryService` remains the facade/coordinator: it owns the
//!   `Arc<Mutex<PeerRegistry>>` locking and the receive-path wiring that
//!   feeds this registry. A given broker never mutates the registry
//!   directly except through this module's [`PeerRegistry`] API, so there
//!   is no duplicate mutable state.
//! * [`PeerSource`] classifies which discovery message kind announced a
//!   peer. It lives here because it types the per-peer metadata; the
//!   service re-exports it (and this module's other types) so the public
//!   path `boru_core::discovery_service::PeerSource` stays stable.
//!
//! # Invariants enforced here
//!
//! * A node is registered **once** — the `node_id` key dominates, so the
//!   same peer discovered on two paths is represented as a single entry.
//! * Dedup is keyed by `(node_id, event_id)` — a re-delivered message with
//!   the same id leaves the entry untouched ([`UpsertOutcome::Duplicate`]).
//! * Legacy senders (no event id on the wire) always refresh and are
//!   **never** deduplicated (BORU-DISC-06 behaviour preserved).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use iroh_base::PublicKey;

use crate::discovery_message::DiscoveryMessage;
use crate::proto::TopicId;

/// Which discovery message kind announced a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeerSource {
    /// The peer announced itself with a `Hello` after joining the topic.
    Hello,
    /// The peer sent a periodic `Presence` heartbeat.
    Presence,
    /// The peer was the sender of a `PeerAdvertisement`.
    PeerAdvertisement,
}

impl PeerSource {
    /// Classify a discovery message by its kind.
    pub fn from_message(message: &DiscoveryMessage) -> Self {
        match message {
            DiscoveryMessage::Hello { .. } => Self::Hello,
            DiscoveryMessage::Presence { .. } => Self::Presence,
            DiscoveryMessage::PeerAdvertisement { .. } => Self::PeerAdvertisement,
        }
    }
}

/// Outcome of [`PeerRegistry::upsert`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    /// The peer was not registered before — a fresh entry was created.
    New,
    /// The peer was already registered and its metadata was refreshed
    /// (last-seen / source / source-topic). A distinct event id from a
    /// known node updates last-seen.
    Refreshed,
    /// The peer was already registered AND the incoming event id equals the
    /// last accepted event id — the same advertisement delivered twice (e.g.
    /// over two discovery paths). The registry was **not** modified.
    Duplicate,
}

/// Per-peer metadata held in the [`PeerRegistry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRegistryEntry {
    /// When this peer was last heard from on the discovery topic.
    pub last_seen: Instant,
    /// Which discovery message kind most recently announced this peer.
    pub source: PeerSource,
    /// The gossip topic this peer was heard on.
    pub source_topic: TopicId,
    /// The event id of the most recently accepted message from this peer
    /// (`None` = only legacy, event-id-less messages seen). The second half
    /// of the dedup key: a re-delivered message with the same id is a
    /// duplicate and does not refresh the entry (BORU-DISC-17).
    pub last_event_id: Option<u64>,
}

/// In-process registry of peers seen on the internal discovery topic.
///
/// Maps `node_id` → last-seen / source-topic metadata. This is the dedup
/// anchor: a node that has already been registered is not re-announced as
/// new. Dedup is keyed by `(node_id, event_id)` (BORU-DISC-17) — the same
/// peer discovered on two paths is represented once; a duplicate event id
/// leaves the entry untouched.
#[derive(Debug, Default, Clone)]
pub struct PeerRegistry {
    peers: HashMap<PublicKey, PeerRegistryEntry>,
}

impl PeerRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or refresh a peer entry, deduplicating by `(node_id, event_id)`.
    ///
    /// * **New peer** — creates a fresh entry ([`UpsertOutcome::New`]).
    /// * **Known peer, new event id** — refreshes `last_seen`/`source`/
    ///   `source_topic` ([`UpsertOutcome::Refreshed`]).
    /// * **Known peer, same event id** — duplicate delivery; the entry is
    ///   **not** modified ([`UpsertOutcome::Duplicate`]).
    ///
    /// Legacy senders (no event id on the wire, `event_id == None`) always
    /// refresh — they are never deduplicated, preserving BORU-DISC-06
    /// behaviour.
    pub fn upsert(
        &mut self,
        node_id: PublicKey,
        source: PeerSource,
        source_topic: TopicId,
        event_id: Option<u64>,
    ) -> UpsertOutcome {
        if let Some(entry) = self.peers.get_mut(&node_id) {
            // Duplicate event id from an already-known node: the same event
            // re-delivered (e.g. over two discovery paths). Leave the entry
            // untouched.
            if let Some(id) = event_id {
                if entry.last_event_id == Some(id) {
                    return UpsertOutcome::Duplicate;
                }
            }
            entry.last_seen = Instant::now();
            entry.source = source;
            entry.source_topic = source_topic;
            if event_id.is_some() {
                entry.last_event_id = event_id;
            }
            UpsertOutcome::Refreshed
        } else {
            self.peers.insert(
                node_id,
                PeerRegistryEntry {
                    last_seen: Instant::now(),
                    source,
                    source_topic,
                    last_event_id: event_id,
                },
            );
            UpsertOutcome::New
        }
    }

    /// Whether `node_id` is currently registered.
    pub fn contains(&self, node_id: &PublicKey) -> bool {
        self.peers.contains_key(node_id)
    }

    /// Look up the entry for `node_id`.
    pub fn get(&self, node_id: &PublicKey) -> Option<&PeerRegistryEntry> {
        self.peers.get(node_id)
    }

    /// Last time `node_id` was heard from, if it is registered.
    pub fn last_seen(&self, node_id: &PublicKey) -> Option<Instant> {
        self.peers.get(node_id).map(|entry| entry.last_seen)
    }

    /// Iterate over all registered peers.
    pub fn peers(&self) -> impl Iterator<Item = (&PublicKey, &PeerRegistryEntry)> {
        self.peers.iter()
    }

    /// Number of registered peers.
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Whether the registry has no peers.
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Remove peers not heard from within `max_age`, returning their ids.
    ///
    /// Used by the (later) presence-expiry loop; kept here so the expiry
    /// policy is unit-testable in isolation.
    pub fn prune_older_than(&mut self, max_age: Duration) -> Vec<PublicKey> {
        let cutoff = Instant::now() - max_age;
        let mut removed = Vec::new();
        self.peers.retain(|node_id, entry| {
            let keep = entry.last_seen >= cutoff;
            if !keep {
                removed.push(*node_id);
            }
            keep
        });
        removed
    }

    /// Remove every peer, returning the removed ids.
    pub fn clear(&mut self) -> Vec<PublicKey> {
        let removed: Vec<PublicKey> = self.peers.keys().copied().collect();
        self.peers.clear();
        removed
    }

    /// Refresh an existing entry's last-seen / source after a RESTART
    /// re-discovery (BORU-CP-07).
    ///
    /// The registry's `(node_id, event_id)` dedup would otherwise classify
    /// a restarted node's first HELLO (which reuses event id 0) as a
    /// duplicate delivery and silently drop the announcement that must
    /// trigger automatic reconnection. This method updates the entry's
    /// metadata WITHOUT touching `last_event_id` — the restarted node's
    /// counter produces fresh ids next, and the entry is no longer stale.
    /// Returns `false` (no-op) if the peer is not registered.
    pub fn refresh_after_restart(
        &mut self,
        node_id: PublicKey,
        source: PeerSource,
        source_topic: TopicId,
    ) -> bool {
        let Some(entry) = self.peers.get_mut(&node_id) else {
            return false;
        };
        entry.last_seen = Instant::now();
        entry.source = source;
        entry.source_topic = source_topic;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::public_room::PublicNetwork;

    /// Deterministic test identity: a `SecretKey` seeded from a single byte
    /// produces a valid Ed25519 public key.
    fn test_key(byte: u8) -> PublicKey {
        test_secret_key(byte).public()
    }

    /// Deterministic test secret key seeded from a single byte (matches
    /// [`test_key`]).
    fn test_secret_key(byte: u8) -> iroh_base::SecretKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed)
    }

    fn test_topic() -> TopicId {
        crate::discovery_topic::discovery_topic(PublicNetwork::Test)
    }

    // ── Registry ──────────────────────────────────────────────────────

    #[test]
    fn registry_upsert_and_accessors() {
        let mut registry = PeerRegistry::new();
        assert!(registry.is_empty());

        let node = test_key(0x01);
        let topic = test_topic();
        assert!(!registry.contains(&node));
        registry.upsert(node, PeerSource::Hello, topic, None);

        assert!(registry.contains(&node));
        assert_eq!(registry.len(), 1);
        let entry = registry.get(&node).unwrap();
        assert_eq!(entry.source, PeerSource::Hello);
        assert_eq!(entry.source_topic, topic);
        assert!(registry.last_seen(&node).is_some());
        assert_eq!(entry.last_event_id, None);

        // Refresh with a different source updates the entry, not the count.
        registry.upsert(node, PeerSource::Presence, topic, None);
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.get(&node).unwrap().source, PeerSource::Presence);

        let collected: Vec<PublicKey> = registry.peers().map(|(id, _)| *id).collect();
        assert_eq!(collected, vec![node]);
    }

    #[test]
    fn registry_prune_older_than_removes_only_stale() {
        let mut registry = PeerRegistry::new();
        let topic = test_topic();
        let fresh = test_key(0x10);
        let stale = test_key(0x11);

        // Insert the "stale" peer, then backdate it (tests are a child
        // module, so they can reach the private map directly).
        registry.upsert(stale, PeerSource::Hello, topic, None);
        registry.peers.get_mut(&stale).unwrap().last_seen =
            Instant::now() - Duration::from_secs(3600);
        registry.upsert(fresh, PeerSource::Presence, topic, None);

        let removed = registry.prune_older_than(Duration::from_secs(60));
        assert_eq!(removed, vec![stale]);
        assert!(!registry.contains(&stale));
        assert!(registry.contains(&fresh));
    }

    // ── Dedup by node id + event id (BORU-DISC-17) ────────────────────

    /// Same peer advertised on two topics yields ONE registry entry: the
    /// node-identity key dominates, and the entry's source-topic metadata
    /// updates to the latest hop.
    #[test]
    fn registry_same_peer_two_topics_is_one_entry() {
        let mut registry = PeerRegistry::new();
        let node = test_key(0x21);
        let topic_a = test_topic();
        let topic_b =
            crate::discovery_topic::discovery_topic(crate::public_room::PublicNetwork::Development);
        assert_ne!(topic_a, topic_b);

        assert_eq!(
            registry.upsert(node, PeerSource::Hello, topic_a, Some(1)),
            UpsertOutcome::New
        );
        // The same peer arrives on a second discovery path with a new event.
        assert_eq!(
            registry.upsert(node, PeerSource::Presence, topic_b, Some(2)),
            UpsertOutcome::Refreshed
        );

        // Still exactly ONE entry — a peer discovered on both paths is
        // represented once.
        assert_eq!(registry.len(), 1);
        let entry = registry.get(&node).unwrap();
        assert_eq!(entry.source, PeerSource::Presence);
        assert_eq!(entry.source_topic, topic_b);
        assert_eq!(entry.last_event_id, Some(2));
    }

    /// A duplicate event id from the same node is ignored: the entry is
    /// untouched (no last-seen/source/source-topic change).
    #[test]
    fn registry_duplicate_event_id_ignored() {
        let mut registry = PeerRegistry::new();
        let node = test_key(0x22);
        let topic = test_topic();

        assert_eq!(
            registry.upsert(node, PeerSource::Hello, topic, Some(7)),
            UpsertOutcome::New
        );
        let first_seen = registry.get(&node).unwrap().last_seen;

        // Same node, same event id re-delivered (e.g. the same advertisement
        // forwarded over two paths) — must NOT refresh the entry.
        assert_eq!(
            registry.upsert(node, PeerSource::Presence, topic, Some(7)),
            UpsertOutcome::Duplicate
        );
        assert_eq!(registry.len(), 1);
        let entry = registry.get(&node).unwrap();
        assert_eq!(entry.source, PeerSource::Hello, "source must not change");
        assert_eq!(entry.source_topic, topic);
        assert_eq!(
            entry.last_seen, first_seen,
            "last_seen must not change on duplicate"
        );
        assert_eq!(entry.last_event_id, Some(7));
    }

    /// Distinct event ids from the same node update last-seen.
    #[test]
    fn registry_distinct_event_ids_update_last_seen() {
        let mut registry = PeerRegistry::new();
        let node = test_key(0x23);
        let topic = test_topic();

        assert_eq!(
            registry.upsert(node, PeerSource::Hello, topic, Some(1)),
            UpsertOutcome::New
        );
        let first_seen = registry.get(&node).unwrap().last_seen;

        // A new event id (a real refresh/presence) updates last_seen.
        std::thread::sleep(Duration::from_millis(2));
        assert_eq!(
            registry.upsert(node, PeerSource::Presence, topic, Some(2)),
            UpsertOutcome::Refreshed
        );
        let entry = registry.get(&node).unwrap();
        assert_eq!(entry.source, PeerSource::Presence);
        assert!(entry.last_seen > first_seen);
        assert_eq!(entry.last_event_id, Some(2));
    }

    /// Legacy senders (no event id on the wire) always refresh — they are
    /// never deduplicated. Two distinct legacy messages from the same node
    /// produce Refreshed, never Duplicate.
    #[test]
    fn registry_legacy_messages_never_deduplicated() {
        let mut registry = PeerRegistry::new();
        let node = test_key(0x24);
        let topic = test_topic();

        assert_eq!(
            registry.upsert(node, PeerSource::Hello, topic, None),
            UpsertOutcome::New
        );
        assert_eq!(
            registry.upsert(node, PeerSource::Presence, topic, None),
            UpsertOutcome::Refreshed
        );
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.get(&node).unwrap().source, PeerSource::Presence);
        assert_eq!(registry.get(&node).unwrap().last_event_id, None);
    }

    /// Mixing: a legacy message does not erase the tracked event id, and a
    /// later duplicate of a previously-seen event id is still ignored.
    #[test]
    fn registry_event_id_survives_legacy_message() {
        let mut registry = PeerRegistry::new();
        let node = test_key(0x25);
        let topic = test_topic();

        assert_eq!(
            registry.upsert(node, PeerSource::Hello, topic, Some(5)),
            UpsertOutcome::New
        );
        // A legacy (event-id-less) message refreshes but keeps the id.
        assert_eq!(
            registry.upsert(node, PeerSource::Presence, topic, None),
            UpsertOutcome::Refreshed
        );
        assert_eq!(registry.get(&node).unwrap().last_event_id, Some(5));
        // The duplicate of event 5 is still ignored even after the legacy
        // refresh.
        assert_eq!(
            registry.upsert(node, PeerSource::PeerAdvertisement, topic, Some(5)),
            UpsertOutcome::Duplicate
        );
        let entry = registry.get(&node).unwrap();
        // A duplicate never mutates the entry: `source` stays Presence (from
        // the last accepted legacy refresh) and the tracked id is untouched.
        assert_eq!(entry.source, PeerSource::Presence);
        assert_eq!(entry.last_event_id, Some(5));
    }
}
