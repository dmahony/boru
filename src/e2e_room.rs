//! Bounded room lifecycle adapter for the E2E ROOM lane.
//!
//! Join material is an in-process capability. It is intentionally not
//! serializable, returned by snapshots, or included in logs.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::e2e_control::{
    E2eError, E2eErrorCode, NodeAlias, RoomMarker, RoomSnapshot, RoomState, SchemaVersion,
    TestControlConfig,
};

const MAX_ALIASES: usize = 32;
const JOIN_HANDLE_TTL: Duration = Duration::from_secs(300);
const ASSERTION_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Process-local opaque capability used to join or rejoin a room.
#[derive(Debug, Clone)]
pub struct RoomJoinHandle {
    marker: RoomMarker,
    capability: u64,
    expires_at: Instant,
}

#[derive(Debug)]
struct RoomRecord {
    room_id_hash: String,
    state: RoomState,
    members: HashSet<NodeAlias>,
    last_transition: Option<String>,
    last_error: Option<E2eErrorCode>,
    capability: u64,
    rejoin_count: u32,
}

/// Lane-owned room lifecycle adapter.
#[derive(Debug)]
pub struct RoomAdapter {
    config: TestControlConfig,
    next_capability: u64,
    rooms: HashMap<RoomMarker, RoomRecord>,
}

impl RoomAdapter {
    /// Create an empty adapter using the supplied test-control gate.
    pub fn new(config: TestControlConfig) -> Self {
        Self {
            config,
            next_capability: 1,
            rooms: HashMap::new(),
        }
    }

    /// Create and locally join a room, returning its snapshot and join capability.
    pub fn create_room(
        &mut self,
        run_id: &str,
        node_alias: NodeAlias,
        workflow_id: &str,
        marker: RoomMarker,
    ) -> Result<(RoomSnapshot, RoomJoinHandle), E2eError> {
        self.require_mutation()?;
        if run_id.is_empty() || workflow_id.is_empty() || marker.as_ref().is_empty() {
            return Err(E2eError::new(
                E2eErrorCode::InvalidState,
                "room identifiers must not be empty",
            ));
        }
        if self.rooms.contains_key(&marker) {
            return Err(E2eError::new(
                E2eErrorCode::InvalidState,
                "room already exists",
            ));
        }
        let room_id_hash = stable_room_hash(run_id, workflow_id, marker.as_ref());
        let capability = self.allocate_capability();
        let mut members = HashSet::new();
        members.insert(node_alias);
        self.rooms.insert(
            marker.clone(),
            RoomRecord {
                room_id_hash,
                state: RoomState::Joined,
                members,
                last_transition: Some("created".to_owned()),
                last_error: None,
                capability,
                rejoin_count: 0,
            },
        );
        Ok((self.snapshot(&marker)?, self.handle(marker, capability)))
    }

    /// Join a room through a valid, unexpired process-local capability.
    pub fn join_room(
        &mut self,
        node_alias: NodeAlias,
        handle: &RoomJoinHandle,
    ) -> Result<RoomSnapshot, E2eError> {
        self.require_mutation()?;
        self.validate_handle(handle)?;
        let room = self.room_mut(&handle.marker)?;
        if matches!(room.state, RoomState::Joining | RoomState::Leaving) {
            return Err(E2eError::new(
                E2eErrorCode::InvalidState,
                "room transition already in progress",
            ));
        }
        let was_member = room.members.contains(&node_alias);
        room.members.insert(node_alias);
        room.state = RoomState::Joined;
        room.last_transition = Some(
            if was_member {
                "already_joined"
            } else {
                "joined"
            }
            .to_owned(),
        );
        room.last_error = None;
        self.snapshot(&handle.marker)
    }

    /// Leave a room through the normal lifecycle path.
    pub fn leave_room(
        &mut self,
        node_alias: &NodeAlias,
        marker: &RoomMarker,
    ) -> Result<RoomSnapshot, E2eError> {
        self.require_mutation()?;
        let room = self.room_mut(marker)?;
        room.members.remove(node_alias);
        room.state = RoomState::Left;
        room.last_transition = Some("left".to_owned());
        self.snapshot(marker)
    }

    /// Rejoin a room using the original process-local capability.
    pub fn rejoin_room(
        &mut self,
        node_alias: NodeAlias,
        handle: &RoomJoinHandle,
    ) -> Result<RoomSnapshot, E2eError> {
        self.require_mutation()?;
        self.validate_handle(handle)?;
        let room = self.room_mut(&handle.marker)?;
        room.members.insert(node_alias);
        room.rejoin_count = room.rejoin_count.saturating_add(1);
        room.state = RoomState::Joined;
        room.last_transition = Some("rejoined".to_owned());
        room.last_error = None;
        self.snapshot(&handle.marker)
    }

    /// Assert that a node is locally joined before `timeout` expires.
    pub fn assert_joined(
        &self,
        marker: &RoomMarker,
        node: &NodeAlias,
        timeout: Duration,
    ) -> Result<RoomSnapshot, E2eError> {
        self.assert_until(marker, node, timeout, "joined", |snapshot| {
            snapshot
                .safe_member_aliases
                .iter()
                .any(|alias| alias == node)
        })
    }

    /// Assert that a node is no longer locally joined before `timeout` expires.
    pub fn assert_left(
        &self,
        marker: &RoomMarker,
        node: &NodeAlias,
        timeout: Duration,
    ) -> Result<RoomSnapshot, E2eError> {
        self.assert_until(marker, node, timeout, "left", |snapshot| {
            !snapshot
                .safe_member_aliases
                .iter()
                .any(|alias| alias == node)
        })
    }

    /// Assert that `peer` is visible in the room membership snapshot.
    pub fn assert_peer_visible(
        &self,
        marker: &RoomMarker,
        node: &NodeAlias,
        peer: &NodeAlias,
        timeout: Duration,
    ) -> Result<RoomSnapshot, E2eError> {
        self.assert_until(marker, node, timeout, "peer-visible", |snapshot| {
            snapshot
                .safe_member_aliases
                .iter()
                .any(|alias| alias == peer)
        })
    }

    /// Assert that membership has held at `expected` for the whole stability window.
    pub fn assert_stable_membership_count(
        &self,
        marker: &RoomMarker,
        node: &NodeAlias,
        expected: u32,
        stable_for: Duration,
        timeout: Duration,
    ) -> Result<RoomSnapshot, E2eError> {
        let deadline = Instant::now() + timeout;
        let stable_deadline = Instant::now() + stable_for;
        let mut last = self.snapshot(marker)?;
        while Instant::now() < deadline || (timeout.is_zero() && last.member_count == expected) {
            last = self.snapshot(marker)?;
            if last.member_count == expected && Instant::now() >= stable_deadline {
                return Ok(last);
            }
            std::thread::sleep(ASSERTION_POLL_INTERVAL);
        }
        Err(assertion_timeout(node, "stable membership count", &last))
    }

    /// Assert that a node completed a leave/rejoin cycle and is joined again.
    pub fn assert_rejoined(
        &self,
        marker: &RoomMarker,
        node: &NodeAlias,
        timeout: Duration,
    ) -> Result<RoomSnapshot, E2eError> {
        self.assert_until(marker, node, timeout, "rejoined", |snapshot| {
            snapshot.state == RoomState::Joined
                && snapshot
                    .safe_member_aliases
                    .iter()
                    .any(|alias| alias == node)
                && snapshot.last_transition.as_deref() == Some("rejoined")
        })
    }

    fn assert_until<F>(
        &self,
        marker: &RoomMarker,
        node: &NodeAlias,
        timeout: Duration,
        expected: &str,
        predicate: F,
    ) -> Result<RoomSnapshot, E2eError>
    where
        F: Fn(&RoomSnapshot) -> bool,
    {
        let deadline = Instant::now() + timeout;
        let mut last;
        loop {
            last = self.snapshot(marker)?;
            if predicate(&last) {
                return Ok(last);
            }
            if Instant::now() >= deadline {
                return Err(assertion_timeout(node, expected, &last));
            }
            std::thread::sleep(ASSERTION_POLL_INTERVAL);
        }
    }

    /// Return a compact, secret-free snapshot of a room.
    pub fn snapshot(&self, marker: &RoomMarker) -> Result<RoomSnapshot, E2eError> {
        let room = self.rooms.get(marker).ok_or_else(|| not_found("room"))?;
        let mut aliases: Vec<_> = room.members.iter().cloned().collect();
        aliases.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
        aliases.truncate(MAX_ALIASES);
        Ok(RoomSnapshot {
            schema: SchemaVersion::default(),
            room_marker: marker.clone(),
            room_id_hash: room.room_id_hash.clone(),
            state: room.state,
            local_member: !room.members.is_empty(),
            member_count: room.members.len().min(u32::MAX as usize) as u32,
            safe_member_aliases: aliases,
            last_transition: room.last_transition.clone(),
            last_error: room.last_error.clone(),
        })
    }

    fn require_mutation(&self) -> Result<(), E2eError> {
        if self.config.allows_mutation() {
            Ok(())
        } else {
            Err(E2eError::new(
                E2eErrorCode::DisabledAction,
                "room test actions are disabled",
            ))
        }
    }

    fn validate_handle(&self, handle: &RoomJoinHandle) -> Result<(), E2eError> {
        if Instant::now() >= handle.expires_at {
            return Err(E2eError::new(
                E2eErrorCode::ExpiredJoinReference,
                "join reference expired",
            ));
        }
        let room = self
            .rooms
            .get(&handle.marker)
            .ok_or_else(|| not_found("room"))?;
        if room.capability != handle.capability {
            return Err(E2eError::new(
                E2eErrorCode::InvalidJoinReference,
                "invalid join reference",
            ));
        }
        Ok(())
    }

    fn room_mut(&mut self, marker: &RoomMarker) -> Result<&mut RoomRecord, E2eError> {
        self.rooms.get_mut(marker).ok_or_else(|| not_found("room"))
    }

    fn allocate_capability(&mut self) -> u64 {
        let value = self.next_capability;
        self.next_capability = self.next_capability.wrapping_add(1).max(1);
        value
    }

    fn handle(&self, marker: RoomMarker, capability: u64) -> RoomJoinHandle {
        RoomJoinHandle {
            marker,
            capability,
            expires_at: Instant::now() + JOIN_HANDLE_TTL,
        }
    }
}

fn stable_room_hash(run_id: &str, workflow_id: &str, marker: &str) -> String {
    blake3::hash(format!("boru-e2e-room/v1\0{run_id}\0{workflow_id}\0{marker}").as_bytes())
        .to_hex()
        .to_string()
}

fn not_found(kind: &str) -> E2eError {
    E2eError::new(E2eErrorCode::NotFound, format!("{kind} not found"))
}

fn assertion_timeout(node: &NodeAlias, expected: &str, observed: &RoomSnapshot) -> E2eError {
    E2eError::new(
        E2eErrorCode::Timeout,
        format!(
            "node '{}' expected {expected}; observed state={:?}, local_member={}, member_count={}",
            node.as_ref(),
            observed.state,
            observed.local_member,
            observed.member_count
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter() -> RoomAdapter {
        RoomAdapter::new(TestControlConfig {
            enabled: true,
            ..Default::default()
        })
    }

    #[test]
    fn create_join_leave_rejoin_converges() {
        let mut adapter = adapter();
        let marker: RoomMarker = "room-1".into();
        let (created, handle) = adapter
            .create_room("run-1", "node-a".into(), "workflow-1", marker.clone())
            .unwrap();
        assert_eq!(created.state, RoomState::Joined);
        let joined = adapter.join_room("node-b".into(), &handle).unwrap();
        assert_eq!(joined.member_count, 2);
        let left = adapter.leave_room(&"node-b".into(), &marker).unwrap();
        assert_eq!(left.member_count, 1);
        let rejoined = adapter.rejoin_room("node-b".into(), &handle).unwrap();
        assert_eq!(rejoined.member_count, 2);
        assert_eq!(rejoined.room_id_hash, created.room_id_hash);
    }

    #[test]
    fn invalid_and_expired_handles_are_structured() {
        let mut adapter = adapter();
        let marker: RoomMarker = "room-1".into();
        let (_, handle) = adapter
            .create_room("run-1", "node-a".into(), "workflow-1", marker)
            .unwrap();
        let invalid = RoomJoinHandle {
            capability: handle.capability + 1,
            ..handle.clone()
        };
        assert_eq!(
            adapter
                .join_room("node-b".into(), &invalid)
                .unwrap_err()
                .code,
            E2eErrorCode::InvalidJoinReference
        );
        let expired = RoomJoinHandle {
            expires_at: Instant::now() - Duration::from_secs(1),
            ..handle
        };
        assert_eq!(
            adapter
                .join_room("node-b".into(), &expired)
                .unwrap_err()
                .code,
            E2eErrorCode::ExpiredJoinReference
        );
    }

    #[test]
    fn assertions_cover_lifecycle_and_timeout_context() {
        let mut adapter = adapter();
        let marker: RoomMarker = "room-1".into();
        let (created, handle) = adapter
            .create_room("run-1", "node-a".into(), "workflow-1", marker.clone())
            .unwrap();
        assert!(adapter
            .assert_joined(&marker, &"node-a".into(), Duration::ZERO)
            .is_ok());
        assert!(adapter
            .assert_peer_visible(&marker, &"node-a".into(), &"node-b".into(), Duration::ZERO)
            .is_err());
        adapter.join_room("node-b".into(), &handle).unwrap();
        assert!(adapter
            .assert_peer_visible(&marker, &"node-a".into(), &"node-b".into(), Duration::ZERO)
            .is_ok());
        assert!(adapter
            .assert_stable_membership_count(
                &marker,
                &"node-a".into(),
                2,
                Duration::ZERO,
                Duration::ZERO
            )
            .is_ok());
        adapter.leave_room(&"node-b".into(), &marker).unwrap();
        assert!(adapter
            .assert_left(&marker, &"node-b".into(), Duration::ZERO)
            .is_ok());
        adapter.rejoin_room("node-b".into(), &handle).unwrap();
        let rejoined = adapter
            .assert_rejoined(&marker, &"node-b".into(), Duration::ZERO)
            .unwrap();
        assert_eq!(rejoined.room_id_hash, created.room_id_hash);
        let error = adapter
            .assert_joined(&marker, &"missing".into(), Duration::ZERO)
            .unwrap_err();
        assert_eq!(error.code, E2eErrorCode::Timeout);
        assert!(error.message.contains("missing") && error.message.contains("joined"));
    }

    #[test]
    fn duplicate_join_is_idempotent() {
        let mut adapter = adapter();
        let marker: RoomMarker = "room-1".into();
        let (_, handle) = adapter
            .create_room("run-1", "node-a".into(), "workflow-1", marker)
            .unwrap();
        let snapshot = adapter.join_room("node-a".into(), &handle).unwrap();
        assert_eq!(snapshot.member_count, 1);
        assert_eq!(snapshot.last_transition.as_deref(), Some("already_joined"));
    }

    #[test]
    fn mutation_is_disabled_by_default() {
        let mut adapter = RoomAdapter::new(TestControlConfig::default());
        let result = adapter.create_room("run", "node".into(), "workflow", "room".into());
        assert_eq!(result.unwrap_err().code, E2eErrorCode::DisabledAction);
    }
}
