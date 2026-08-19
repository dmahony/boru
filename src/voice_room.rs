//! Small-group voice-room state and routing primitives.
//!
//! The direct call actor owns transport and codec lifetimes. This module owns
//! the group concerns that sit above it: durable room identity, convergent
//! membership, speaking policy, per-user mute, and bounded fan-out. It is
//! deliberately transport-agnostic so a slow peer cannot stall other peers.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

/// Stable identifier for a voice room.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub struct VoiceRoomId(pub [u8; 32]);

/// Stable identifier for an authenticated participant.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub struct VoicePeerId(pub [u8; 32]);

/// Durable room metadata. Membership is intentionally not stored here.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VoiceRoom {
    /// Stable room identity shared by all members.
    pub id: VoiceRoomId,
    /// User-visible room name.
    pub name: String,
    /// Gossip topic used by the containing room model.
    pub topic: [u8; 32],
    /// V1 fan-out limit. Larger rooms should use a future SFU/forwarder.
    pub max_participants: u16,
}

impl VoiceRoom {
    /// V1 limit for a fully connected small-group room.
    pub const V1_MAX_PARTICIPANTS: u16 = 8;

    /// Create durable metadata with the documented V1 limit.
    pub fn new(id: VoiceRoomId, name: impl Into<String>, topic: [u8; 32]) -> Self {
        Self {
            id,
            name: name.into(),
            topic,
            max_participants: Self::V1_MAX_PARTICIPANTS,
        }
    }
}

/// Input policy selected by the local participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VoiceInputMode {
    /// Speak when the input level crosses the configured threshold.
    VoiceActivity,
    /// Speak only while the push-to-talk key is held.
    PushToTalk,
}

/// Deterministic voice activity detector with hysteresis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceActivity {
    mode: VoiceInputMode,
    threshold: u16,
    release_threshold: u16,
    speaking: bool,
    ptt_pressed: bool,
}

impl VoiceActivity {
    /// Construct a detector. Levels are normalized to 0..=1000.
    pub fn new(mode: VoiceInputMode, threshold: u16) -> Self {
        let threshold = threshold.min(1000);
        Self {
            mode,
            threshold,
            release_threshold: threshold.saturating_mul(3) / 4,
            speaking: false,
            ptt_pressed: false,
        }
    }
    /// Change input mode without carrying stale PTT state into VAD mode.
    pub fn set_mode(&mut self, mode: VoiceInputMode) {
        self.mode = mode;
        self.speaking = false;
    }
    /// Update the PTT key state.
    pub fn set_ptt_pressed(&mut self, pressed: bool) {
        self.ptt_pressed = pressed;
        if self.mode == VoiceInputMode::PushToTalk {
            self.speaking = pressed;
        }
    }
    /// Observe one input level and return whether audio should be sent.
    pub fn observe_level(&mut self, level: u16) -> bool {
        self.speaking = match self.mode {
            VoiceInputMode::PushToTalk => self.ptt_pressed,
            VoiceInputMode::VoiceActivity => {
                if self.speaking {
                    level >= self.release_threshold
                } else {
                    level >= self.threshold
                }
            }
        };
        self.speaking
    }
    /// Whether the local participant is currently speaking.
    pub const fn speaking(&self) -> bool {
        self.speaking
    }
}

/// Last-known ephemeral membership state for one participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Membership {
    /// Monotonic membership epoch chosen by the room authority.
    pub epoch: u64,
    /// Last heartbeat observed from this peer, in monotonic application time.
    pub last_seen_ms: u64,
    /// Whether the peer intends to be in the room at this epoch.
    pub present: bool,
}

/// Convergent room membership view. The highest epoch wins; ties resolve by
/// present=true so a delayed leave cannot remove a newer join at the same epoch.
#[derive(Debug, Clone, Default)]
pub struct MembershipView {
    members: BTreeMap<VoicePeerId, Membership>,
}

impl MembershipView {
    /// Merge one remote membership update.
    pub fn merge(&mut self, peer: VoicePeerId, update: Membership) -> bool {
        let changed = self.members.get(&peer).is_none_or(|old| {
            update.epoch > old.epoch
                || (update.epoch == old.epoch && update.present && !old.present)
        });
        if changed {
            self.members.insert(peer, update);
        }
        changed
    }
    /// Remove absent or expired peers using a bounded heartbeat timeout.
    pub fn expire(&mut self, now_ms: u64, timeout: Duration) -> usize {
        let before = self.members.len();
        let timeout = timeout.as_millis() as u64;
        self.members.retain(|_, state| {
            state.present && now_ms.saturating_sub(state.last_seen_ms) <= timeout
        });
        before - self.members.len()
    }
    /// Return the current active members in deterministic order.
    pub fn active(&self) -> BTreeSet<VoicePeerId> {
        self.members
            .iter()
            .filter_map(|(peer, state)| state.present.then_some(*peer))
            .collect()
    }
    /// Return a participant's last-known membership.
    pub fn get(&self, peer: VoicePeerId) -> Option<Membership> {
        self.members.get(&peer).copied()
    }
}

/// One encoded audio frame routed to a participant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceFrame {
    /// Authenticated sender.
    pub sender: VoicePeerId,
    /// Monotonic media sequence.
    pub sequence: u64,
    /// Encoded Opus payload.
    pub payload: Vec<u8>,
}

/// Per-peer delivery and quality counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PeerMetrics {
    /// Frames delivered.
    pub delivered: u64,
    /// Frames discarded because the peer queue was full.
    pub dropped: u64,
    /// Frames discarded due to local mute.
    pub muted: u64,
}

#[derive(Debug)]
struct PeerQueue {
    queue: VecDeque<VoiceFrame>,
    metrics: PeerMetrics,
}

/// Bounded fan-out router. Each peer has an independent queue and metrics.
#[derive(Debug)]
pub struct VoiceRouter {
    capacity: usize,
    muted: BTreeSet<VoicePeerId>,
    queues: BTreeMap<VoicePeerId, PeerQueue>,
}

impl VoiceRouter {
    /// Create a router with a bounded queue per destination.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            muted: BTreeSet::new(),
            queues: BTreeMap::new(),
        }
    }
    /// Register a destination peer.
    pub fn add_peer(&mut self, peer: VoicePeerId) {
        self.queues.entry(peer).or_insert_with(|| PeerQueue {
            queue: VecDeque::new(),
            metrics: PeerMetrics::default(),
        });
    }
    /// Set a local per-user mute, which suppresses delivery to that user.
    pub fn set_muted(&mut self, peer: VoicePeerId, muted: bool) {
        if muted {
            self.muted.insert(peer);
        } else {
            self.muted.remove(&peer);
        }
    }
    /// Fan out a frame without waiting on any destination.
    pub fn route(&mut self, frame: VoiceFrame) {
        for (peer, destination) in &mut self.queues {
            if *peer == frame.sender {
                continue;
            }
            if self.muted.contains(peer) {
                destination.metrics.muted += 1;
                continue;
            }
            if destination.queue.len() >= self.capacity {
                destination.metrics.dropped += 1;
                continue;
            }
            destination.queue.push_back(frame.clone());
            destination.metrics.delivered += 1;
        }
    }
    /// Drain at most `limit` frames for a destination.
    pub fn drain(&mut self, peer: VoicePeerId, limit: usize) -> Vec<VoiceFrame> {
        self.queues
            .get_mut(&peer)
            .map(|q| q.queue.drain(..limit.min(q.queue.len())).collect())
            .unwrap_or_default()
    }
    /// Read quality counters for a destination.
    pub fn metrics(&self, peer: VoicePeerId) -> Option<PeerMetrics> {
        self.queues.get(&peer).map(|q| q.metrics)
    }
    /// Number of registered destinations.
    pub fn peer_count(&self) -> usize {
        self.queues.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn peer(n: u8) -> VoicePeerId {
        VoicePeerId([n; 32])
    }

    #[test]
    fn vad_has_hysteresis_and_ptt_is_predictable() {
        let mut vad = VoiceActivity::new(VoiceInputMode::VoiceActivity, 100);
        assert!(!vad.observe_level(99));
        assert!(vad.observe_level(100));
        assert!(vad.observe_level(76));
        assert!(!vad.observe_level(74));
        vad.set_mode(VoiceInputMode::PushToTalk);
        assert!(!vad.observe_level(1000));
        vad.set_ptt_pressed(true);
        assert!(vad.observe_level(0));
        vad.set_ptt_pressed(false);
        assert!(!vad.speaking());
    }

    #[test]
    fn membership_merge_and_expiry_converge() {
        let mut view = MembershipView::default();
        assert!(view.merge(
            peer(1),
            Membership {
                epoch: 2,
                last_seen_ms: 100,
                present: false
            }
        ));
        assert!(!view.merge(
            peer(1),
            Membership {
                epoch: 1,
                last_seen_ms: 200,
                present: true
            }
        ));
        assert!(view.merge(
            peer(1),
            Membership {
                epoch: 3,
                last_seen_ms: 200,
                present: true
            }
        ));
        assert_eq!(view.active().len(), 1);
        assert_eq!(view.expire(1_000, Duration::from_millis(100)), 1);
    }

    #[test]
    fn slow_peer_isolated_and_mute_is_per_user() {
        let mut router = VoiceRouter::new(1);
        router.add_peer(peer(2));
        router.add_peer(peer(3));
        router.route(VoiceFrame {
            sender: peer(1),
            sequence: 1,
            payload: vec![1],
        });
        router.route(VoiceFrame {
            sender: peer(1),
            sequence: 2,
            payload: vec![2],
        });
        assert_eq!(router.metrics(peer(2)).unwrap().dropped, 1);
        assert_eq!(router.drain(peer(3), 8).len(), 1);
        router.set_muted(peer(3), true);
        router.route(VoiceFrame {
            sender: peer(1),
            sequence: 3,
            payload: vec![3],
        });
        assert_eq!(router.metrics(peer(3)).unwrap().muted, 1);
    }
}
