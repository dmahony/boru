//! Test: online user list tracking via NeighborUp/NeighborDown.
//!
//! Verifies that:
//! 1. NeighborUp adds peers to the neighbors set (visible as "online").
//! 2. NeighborDown removes peers.
//! 3. The list correctly reflects the current set of neighbors.
//! 4. Regression: after a peer leaves (count → 0) and a new peer joins
//!    (count → 1 again), the new peer appears in the list. This was
//!    broken by a `current_ncount > 0` guard in a previous backend
//!    implementation that prevented re-emitting after the count
//!    dropped to 0.

use std::collections::{HashMap, HashSet};

use iroh::PublicKey;
use iroh_gossip::{
    chat_core::{handle_net_event, ChatCallbacks, MessageHash, NetEvent},
    friends::FriendId,
};
use rand::SeedableRng;

/// A mock frontend that captures neighbor changes as snapshots.
struct OnlineUserTracker {
    local_public: PublicKey,
    neighbors: HashSet<PublicKey>,
    names: HashMap<PublicKey, String>,
    snapshots: Vec<Vec<PublicKey>>,
}

impl OnlineUserTracker {
    fn new(local_pk: PublicKey) -> Self {
        Self {
            local_public: local_pk,
            neighbors: HashSet::new(),
            names: HashMap::new(),
            snapshots: Vec::new(),
        }
    }

    /// Take a snapshot of the current neighbor list (sorted by key string).
    fn snapshot(&mut self) {
        let mut keys: Vec<PublicKey> = self.neighbors.iter().copied().collect();
        keys.sort_by_key(|k| k.to_string());
        self.snapshots.push(keys);
    }

    fn peer_snapshots(&self) -> Vec<Vec<String>> {
        self.snapshots
            .iter()
            .map(|snap| snap.iter().map(|k| k.to_string()).collect())
            .collect()
    }
}

impl ChatCallbacks for OnlineUserTracker {
    fn local_public(&self) -> PublicKey {
        self.local_public
    }
    fn set_name(&mut self, _peer: PublicKey, _name: String) -> Option<String> {
        None
    }
    fn is_friend(&self, _peer: &PublicKey) -> bool {
        false
    }
    fn friend_mark_online(&mut self, _fid: FriendId) {}
    fn friend_mark_offline(&mut self, _fid: FriendId) {}
    fn friend_set_name(&mut self, _fid: FriendId, _name: String) {}
    fn mark_friends_dirty(&mut self) {}
    fn push_system(&mut self, _text: String) {}
    fn push_remote(
        &mut self,
        _peer: PublicKey,
        _label: String,
        _text: String,
        _hash: Option<MessageHash>,
        _sent_at: Option<u64>,
    ) {
    }
    fn set_pending_file(&mut self, _name: String, _ticket: String) {}
    fn set_pending_image(&mut self, _name: String, _hash: MessageHash, _from: PublicKey) {}
    fn has_message(&self, _hash: &MessageHash) -> bool {
        false
    }
    fn edit_message(&mut self, _hash: &MessageHash, _new_text: String) {}
    fn delete_message(&mut self, _hash: &MessageHash) {}
    fn add_reaction(&mut self, _hash: &MessageHash, _emoji: String) {}
    fn on_neighbor_up(&mut self, peer: PublicKey) {
        self.neighbors.insert(peer);
    }
    fn on_neighbor_down(&mut self, peer: PublicKey) {
        self.neighbors.remove(&peer);
    }
    fn record_activity(&mut self, _peer: PublicKey) {}
    fn request_quit(&mut self) {}
    fn resolve_name(&self, peer: &PublicKey) -> String {
        self.names
            .get(peer)
            .cloned()
            .unwrap_or_else(|| peer.fmt_short().to_string())
    }
}

/// Create valid PublicKeys from SecretKeys (the only way to
/// produce iroh-valid public keys without I/O).
fn make_peer_key(rng_seed: u64) -> PublicKey {
    use iroh::SecretKey;
    let mut rng = rand::rngs::StdRng::seed_from_u64(rng_seed);
    let mut bytes = [0u8; 32];
    rand::Rng::fill_bytes(&mut rng, &mut bytes);
    let sk = SecretKey::from_bytes(&bytes);
    sk.public()
}

/// Simulate the core bug pattern: NeighborUp → NeighborDown → NeighborUp
/// with the *same* count (0→1→0→1) to ensure each event correctly updates.
#[tokio::test]
async fn test_online_list_tracks_peer_lifecycle() {
    let local_pk = make_peer_key(0xDEAD);
    let peer_a = make_peer_key(0xAAAA);
    let peer_b = make_peer_key(0xBBBB);

    let mut tracker = OnlineUserTracker::new(local_pk);

    // Phase 1: NeighborUp for peer A — count 0→1
    handle_net_event(NetEvent::NeighborUp { peer: peer_a }, &mut tracker)
        .expect("handle NeighborUp");
    tracker.snapshot();

    assert!(
        tracker.neighbors.contains(&peer_a),
        "Peer A should be in the neighbor list after NeighborUp"
    );
    assert!(
        !tracker.neighbors.contains(&peer_b),
        "Peer B should NOT be in the neighbor list yet"
    );

    // Phase 2: NeighborDown for peer A — count 1→0
    handle_net_event(NetEvent::NeighborDown { peer: peer_a }, &mut tracker)
        .expect("handle NeighborDown");
    tracker.snapshot();

    assert!(
        tracker.neighbors.is_empty(),
        "Neighbors should be empty after the last peer leaves"
    );

    // Phase 3: NeighborUp for peer B — count 0→1 again (REGRESSION TEST)
    handle_net_event(NetEvent::NeighborUp { peer: peer_b }, &mut tracker)
        .expect("handle NeighborUp");
    tracker.snapshot();

    assert!(
        !tracker.neighbors.contains(&peer_a),
        "Peer A should NOT be in the neighbor list after disconnect"
    );
    assert!(
        tracker.neighbors.contains(&peer_b),
        "Peer B SHOULD be in the neighbor list after NeighborUp — this is the regression!"
    );

    // Verify we got all three snapshots: [A], [], [B]
    let snaps = tracker.peer_snapshots();
    assert_eq!(snaps.len(), 3, "Should have 3 neighbor snapshots");
    assert_eq!(
        snaps[0].len(),
        1,
        "Snapshot 0 should have 1 peer (A connected)"
    );
    assert!(
        snaps[0][0].contains(&peer_a.to_string()),
        "Snapshot 0 should contain peer A, got: {:?}",
        snaps[0]
    );
    assert_eq!(
        snaps[1].len(),
        0,
        "Snapshot 1 should be empty (A disconnected)"
    );
    assert_eq!(
        snaps[2].len(),
        1,
        "Snapshot 2 should have 1 peer (B connected)"
    );
    assert!(
        snaps[2][0].contains(&peer_b.to_string()),
        "Snapshot 2 should contain peer B, got: {:?}",
        snaps[2]
    );

    println!("✓ Online user list lifecycle test passed");
}

/// Test that multiple peers in the neighbor list are tracked.
#[tokio::test]
async fn test_online_list_multiple_peers() {
    let local_pk = make_peer_key(0xDEAD);
    let peer_a = make_peer_key(0xAAAA);
    let peer_b = make_peer_key(0xBBBB);
    let peer_c = make_peer_key(0xCCCC);

    let mut tracker = OnlineUserTracker::new(local_pk);

    // Three peers join one by one
    for peer in &[peer_a, peer_b, peer_c] {
        handle_net_event(NetEvent::NeighborUp { peer: *peer }, &mut tracker)
            .expect("handle NeighborUp");
    }

    assert_eq!(
        tracker.neighbors.len(),
        3,
        "All three peers should be in neighbor list"
    );
    assert!(tracker.neighbors.contains(&peer_a));
    assert!(tracker.neighbors.contains(&peer_b));
    assert!(tracker.neighbors.contains(&peer_c));

    // One leaves
    handle_net_event(NetEvent::NeighborDown { peer: peer_b }, &mut tracker)
        .expect("handle NeighborDown");

    assert_eq!(
        tracker.neighbors.len(),
        2,
        "Two peers should remain after B leaves"
    );
    assert!(tracker.neighbors.contains(&peer_a));
    assert!(!tracker.neighbors.contains(&peer_b));
    assert!(tracker.neighbors.contains(&peer_c));

    println!("✓ Multiple peers test passed");
}

/// Test that NeighborUp for an already-known peer is idempotent.
#[tokio::test]
async fn test_online_list_duplicate_neighbor_up() {
    let local_pk = make_peer_key(0xDEAD);
    let peer = make_peer_key(0xAAAA);

    let mut tracker = OnlineUserTracker::new(local_pk);

    // Same peer sends NeighborUp twice
    handle_net_event(NetEvent::NeighborUp { peer }, &mut tracker).expect("first NeighborUp");
    handle_net_event(NetEvent::NeighborUp { peer }, &mut tracker)
        .expect("second NeighborUp (duplicate)");

    assert_eq!(
        tracker.neighbors.len(),
        1,
        "Duplicate NeighborUp should not increase count"
    );

    println!("✓ Duplicate NeighborUp idempotency test passed");
}
