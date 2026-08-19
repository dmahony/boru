//! Small-group voice-room state and routing primitives.
//!
//! The direct call actor owns transport and codec lifetimes. This module owns
//! the group concerns that sit above it: durable room identity, convergent
//! membership, speaking policy, per-user mute, and bounded fan-out. It is
//! deliberately transport-agnostic so a slow peer cannot stall other peers.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use crate::proto::TopicId;

/// Ephemeral membership message exchanged on the containing room topic.
///
/// This is intentionally separate from [`VoiceRoom`]: a heartbeat must never
/// rewrite durable room metadata or history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MembershipSignal {
    /// Voice-room identity this update belongs to.
    pub room_id: VoiceRoomId,
    /// Authenticated participant publishing the update.
    pub peer: VoicePeerId,
    /// Monotonic incarnation/epoch for this participant.
    pub membership: Membership,
}

impl VoiceRoomId {
    /// Derive a voice-room identity from the existing chat topic.
    pub fn from_topic(topic: TopicId) -> Self {
        Self(*topic.as_bytes())
    }
}

impl VoicePeerId {
    /// Construct an authenticated participant identity from its wire bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

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

/// Point-in-time media health for a voice room.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoiceDiagnostics {
    /// Frames accepted for routing.
    pub packets_received: u64,
    /// Frames discarded because a destination queue was full.
    pub packets_dropped: u64,
    /// Frames suppressed by a destination mute gate.
    pub packets_muted: u64,
    /// Missing media sequence numbers inferred from received frames.
    pub packets_lost: u64,
    /// Current aggregate queued frames across all destinations.
    pub queued_frames: u64,
    /// Estimated receive jitter in milliseconds.
    pub jitter_ms: u64,
    /// Estimated encoded bitrate in bits per second.
    pub bitrate_bps: u64,
}

/// Transport-independent group voice session.
///
/// The session is the integration boundary between a chat room and the
/// authenticated call/media actors. Signalling owns [`MembershipSignal`]
/// values; the session owns ephemeral membership, VAD/PTT state, per-peer
/// isolation, and the metrics needed by the UI/diagnostics layer. A caller can
/// feed the returned frames into one authenticated media connection per peer.
#[derive(Debug)]
pub struct VoiceRoomSession {
    room: VoiceRoom,
    local_peer: VoicePeerId,
    membership: MembershipView,
    router: VoiceRouter,
    input: VoiceActivity,
    local_epoch: u64,
    local_sequence: u64,
    diagnostics: VoiceDiagnostics,
    received_sequence: Option<u64>,
    received_arrival_ms: Option<u64>,
}

impl VoiceRoomSession {
    /// Create a session for an existing chat room, capped at the V1 limit.
    pub fn new(room: VoiceRoom, local_peer: VoicePeerId, queue_capacity: usize) -> Self {
        let mut membership = MembershipView::default();
        membership.merge(
            local_peer,
            Membership {
                epoch: 1,
                last_seen_ms: 0,
                present: true,
            },
        );
        let mut router = VoiceRouter::new(queue_capacity);
        router.add_peer(local_peer);
        Self {
            room,
            local_peer,
            membership,
            router,
            input: VoiceActivity::new(VoiceInputMode::VoiceActivity, 300),
            local_epoch: 1,
            local_sequence: 0,
            diagnostics: VoiceDiagnostics::default(),
            received_sequence: None,
            received_arrival_ms: None,
        }
    }

    /// Durable room metadata used to bind this session to the room model.
    pub const fn room(&self) -> &VoiceRoom {
        &self.room
    }

    /// Apply a remote ephemeral membership update. Updates beyond the V1
    /// participant limit are rejected without evicting existing members.
    pub fn apply_membership(&mut self, signal: MembershipSignal) -> bool {
        if signal.room_id != self.room.id {
            return false;
        }
        if !self.membership.active().contains(&signal.peer)
            && signal.membership.present
            && self.membership.active().len() >= self.room.max_participants as usize
        {
            return false;
        }
        let changed = self.membership.merge(signal.peer, signal.membership);
        if signal.membership.present {
            self.router.add_peer(signal.peer);
        }
        changed
    }

    /// Return the local heartbeat/update for the room topic.
    pub fn local_membership(&self, now_ms: u64) -> MembershipSignal {
        MembershipSignal {
            room_id: self.room.id,
            peer: self.local_peer,
            membership: Membership {
                epoch: self.local_epoch,
                last_seen_ms: now_ms,
                present: true,
            },
        }
    }

    /// Mark the local participant as having left without touching room data.
    pub fn leave(&mut self, now_ms: u64) -> MembershipSignal {
        self.local_epoch = self.local_epoch.saturating_add(1);
        MembershipSignal {
            room_id: self.room.id,
            peer: self.local_peer,
            membership: Membership {
                epoch: self.local_epoch,
                last_seen_ms: now_ms,
                present: false,
            },
        }
    }

    /// Remove peers whose heartbeats have expired and return the count.
    pub fn expire_members(&mut self, now_ms: u64, timeout: Duration) -> usize {
        self.membership.expire(now_ms, timeout)
    }

    /// Change the local VAD/PTT policy.
    pub fn set_input_mode(&mut self, mode: VoiceInputMode) {
        self.input.set_mode(mode);
    }

    /// Update push-to-talk state.
    pub fn set_ptt_pressed(&mut self, pressed: bool) {
        self.input.set_ptt_pressed(pressed);
    }

    /// Feed a capture level and report whether the next frame may be sent.
    pub fn observe_input_level(&mut self, level: u16) -> bool {
        self.input.observe_level(level)
    }

    /// Update a per-participant mute gate.
    pub fn set_peer_muted(&mut self, peer: VoicePeerId, muted: bool) {
        self.router.set_muted(peer, muted);
    }

    /// Route one encoded frame to every other active participant.
    pub fn route_frame(&mut self, payload: Vec<u8>) -> Option<u64> {
        if !self.input.speaking() {
            return None;
        }
        let sequence = self.local_sequence;
        self.local_sequence = self.local_sequence.wrapping_add(1);
        let bytes = payload.len() as u64;
        self.router.route(VoiceFrame {
            sender: self.local_peer,
            sequence,
            payload,
        });
        self.diagnostics.packets_received = self.diagnostics.packets_received.saturating_add(1);
        self.diagnostics.bitrate_bps = bytes.saturating_mul(8).saturating_mul(50);
        self.refresh_queue_metrics();
        Some(sequence)
    }

    /// Drain frames for one peer, preserving per-peer isolation.
    pub fn drain_peer(&mut self, peer: VoicePeerId, limit: usize) -> Vec<VoiceFrame> {
        let frames = self.router.drain(peer, limit);
        self.refresh_queue_metrics();
        frames
    }

    /// Record a frame received from an authenticated peer for diagnostics.
    pub fn observe_received(&mut self, sequence: u64, payload_bytes: usize, arrival_ms: u64) {
        if let Some(previous) = self.received_sequence {
            let delta = sequence.wrapping_sub(previous);
            if delta > 1 && delta < (1u64 << 63) {
                self.diagnostics.packets_lost =
                    self.diagnostics.packets_lost.saturating_add(delta - 1);
            }
            if let Some(previous_arrival) = self.received_arrival_ms {
                let observed = arrival_ms.saturating_sub(previous_arrival);
                let deviation = observed.abs_diff(20);
                self.diagnostics.jitter_ms =
                    (self.diagnostics.jitter_ms.saturating_mul(7) + deviation) / 8;
            }
        }
        self.received_sequence = Some(sequence);
        self.received_arrival_ms = Some(arrival_ms);
        self.diagnostics.packets_received = self.diagnostics.packets_received.saturating_add(1);
        self.diagnostics.bitrate_bps = (payload_bytes as u64).saturating_mul(8).saturating_mul(50);
    }

    /// Read aggregate room media diagnostics.
    pub fn diagnostics(&self) -> VoiceDiagnostics {
        self.diagnostics
    }

    /// Read current active membership.
    pub fn active_members(&self) -> BTreeSet<VoicePeerId> {
        self.membership.active()
    }

    fn refresh_queue_metrics(&mut self) {
        self.diagnostics.queued_frames = self
            .active_members()
            .into_iter()
            .filter_map(|peer| self.router.metrics(peer))
            .map(|metrics| metrics.delivered.saturating_sub(metrics.dropped))
            .sum();
        self.diagnostics.packets_dropped = self
            .active_members()
            .into_iter()
            .filter_map(|peer| self.router.metrics(peer))
            .map(|metrics| metrics.dropped)
            .sum();
        self.diagnostics.packets_muted = self
            .active_members()
            .into_iter()
            .filter_map(|peer| self.router.metrics(peer))
            .map(|metrics| metrics.muted)
            .sum();
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

    #[test]
    fn three_peer_session_routes_simultaneous_frames_independently() {
        let room = VoiceRoom::new(VoiceRoomId([9; 32]), "team", [7; 32]);
        let mut session = VoiceRoomSession::new(room.clone(), peer(1), 4);
        for n in [2, 3] {
            assert!(session.apply_membership(MembershipSignal {
                room_id: room.id,
                peer: peer(n),
                membership: Membership {
                    epoch: 1,
                    last_seen_ms: 1,
                    present: true
                },
            }));
        }
        assert!(session.observe_input_level(900));
        assert_eq!(session.route_frame(vec![1, 2, 3]), Some(0));
        assert_eq!(session.drain_peer(peer(2), 1).len(), 1);
        assert_eq!(session.drain_peer(peer(3), 1).len(), 1);
        assert_eq!(session.diagnostics().packets_received, 1);
    }

    #[test]
    fn receive_metrics_track_loss_jitter_and_bitrate() {
        let room = VoiceRoom::new(VoiceRoomId([3; 32]), "metrics", [2; 32]);
        let mut session = VoiceRoomSession::new(room, peer(1), 2);
        session.observe_received(10, 20, 0);
        session.observe_received(12, 20, 30);
        let metrics = session.diagnostics();
        assert_eq!(metrics.packets_lost, 1);
        assert!(metrics.jitter_ms > 0);
        assert_eq!(metrics.bitrate_bps, 8_000);
    }

    #[test]
    fn session_rejects_ninth_member_and_expires_offline_peer() {
        let room = VoiceRoom::new(VoiceRoomId([8; 32]), "limit", [6; 32]);
        let mut session = VoiceRoomSession::new(room.clone(), peer(1), 2);
        for n in 2..=8 {
            assert!(session.apply_membership(MembershipSignal {
                room_id: room.id,
                peer: peer(n),
                membership: Membership {
                    epoch: 1,
                    last_seen_ms: 10,
                    present: true
                },
            }));
        }
        assert!(!session.apply_membership(MembershipSignal {
            room_id: room.id,
            peer: peer(9),
            membership: Membership {
                epoch: 1,
                last_seen_ms: 10,
                present: true
            },
        }));
        assert_eq!(session.active_members().len(), 8);
        assert_eq!(session.expire_members(1_000, Duration::from_millis(100)), 8);
        assert_eq!(session.active_members().len(), 0);
    }

    #[test]
    fn session_ptt_and_shutdown_membership_are_idempotent() {
        let room = VoiceRoom::new(VoiceRoomId([5; 32]), "ptt", [4; 32]);
        let mut session = VoiceRoomSession::new(room.clone(), peer(1), 2);
        session.set_input_mode(VoiceInputMode::PushToTalk);
        assert!(session.route_frame(vec![1]).is_none());
        session.set_ptt_pressed(true);
        assert_eq!(session.route_frame(vec![1]), Some(0));
        session.set_ptt_pressed(false);
        assert!(session.route_frame(vec![2]).is_none());
        let leave = session.leave(42);
        assert!(session.apply_membership(leave));
        assert_eq!(session.active_members().len(), 0);
        assert_eq!(session.local_membership(43).membership.epoch, 2);
    }
}
