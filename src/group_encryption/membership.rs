//! Dynamic group-membership management and handshake.
//!
//! Implements [`AckedGroupMembership<PeerId, OpId>`] by bridging boru's
//! existing owner-centric membership model (from
//! [`group_events`](crate::group_events) + [`group_epoch`](crate::group_epoch))
//! to p2panda-encryption's operation-based trait.
//!
//! # Bridge design
//!
//! Rather than adopting p2panda-auth's full CRDT membership, this module uses
//! boru's existing owner-centric model:
//!
//! - The owner performs add/remove via [`GroupEvent`](crate::group_events::GroupEvent)s.
//! - Each operation maps to a p2panda [`OpId`].
//! - The member set is authoritative from boru's group state.
//! - On member removal, encryption keys are rotated (delegated to
//!   p2panda-encryption's [`MessageGroup::remove`]).
//!
//! # State
//!
//! [`MembershipState`] tracks the current member set, the owner, and a
//! sequenced operation log.  All operations from the owner are immediately
//! effective — acknowledgements are tracked for p2panda-encryption
//! compatibility but do not gate membership visibility.

use std::collections::{HashMap, HashSet};

use p2panda_encryption::traits::AckedGroupMembership;
use serde::{Deserialize, Serialize};

use super::types::{OpId, PeerId};

// ── MembershipError ─────────────────────────────────────────────────────────

/// Errors that can occur during membership operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MembershipError {
    /// The specified operation was not found in the operation log.
    OperationNotFound,
    /// The target member is already in the group.
    AlreadyMember,
    /// The target member is not in the group.
    NotMember,
    /// The operation ID was expected to be one kind but is another.
    OperationTypeMismatch,
}

impl std::fmt::Display for MembershipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MembershipError::OperationNotFound => {
                write!(f, "operation not found in membership log")
            }
            MembershipError::AlreadyMember => {
                write!(f, "member is already in the group")
            }
            MembershipError::NotMember => {
                write!(f, "member is not in the group")
            }
            MembershipError::OperationTypeMismatch => {
                write!(f, "operation type does not match expected kind")
            }
        }
    }
}

impl std::error::Error for MembershipError {}

// ── OperationEntry ──────────────────────────────────────────────────────────

/// A recorded membership operation in the sequenced log.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperationEntry {
    /// Unique identifier for this operation.
    pub op_id: OpId,
    /// Whether this was an add or a remove.
    pub kind: OpKind,
    /// The peer being added or removed.
    pub peer: PeerId,
}

/// Whether a membership operation is an add or a remove.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum OpKind {
    /// A member was added to the group.
    Add,
    /// A member was removed from the group.
    Remove,
}

// ── MembershipState ─────────────────────────────────────────────────────────

/// Owner-centric membership state that bridges boru's group events to
/// p2panda-encryption's [`AckedGroupMembership`] trait.
///
/// # Fields
///
/// * `owner` — the peer that is the authority for membership changes.
/// * `members` — the current set of active members (including the owner).
/// * `operations` — sequenced log of all membership operations.
/// * `add_ops` — quick lookup: operation ID → whether it was an add.
/// * `remove_ops` — quick lookup: operation ID → whether it was a remove.
/// * `acks` — for each operation ID, the set of peers that have acknowledged it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MembershipState {
    owner: PeerId,
    members: HashSet<PeerId>,
    operations: Vec<OperationEntry>,
    add_ops: HashSet<OpId>,
    remove_ops: HashSet<OpId>,
    acks: HashMap<OpId, HashSet<PeerId>>,
}

impl MembershipState {
    /// Create a new membership state with the given owner and initial members.
    ///
    /// The owner is always an implicit member.  If `initial_members` does not
    /// contain the owner, they are added automatically.
    pub fn new(owner: PeerId, initial_members: &[PeerId]) -> Self {
        let mut members = HashSet::with_capacity(initial_members.len() + 1);
        members.insert(owner);
        for m in initial_members {
            members.insert(*m);
        }
        Self {
            owner,
            members,
            operations: Vec::new(),
            add_ops: HashSet::new(),
            remove_ops: HashSet::new(),
            acks: HashMap::new(),
        }
    }

    /// The owner of this group.
    pub fn owner(&self) -> PeerId {
        self.owner
    }

    /// Current set of active members (including the owner).
    pub fn members(&self) -> &HashSet<PeerId> {
        &self.members
    }

    /// Check whether a given peer is an active member.
    pub fn is_member(&self, peer: &PeerId) -> bool {
        self.members.contains(peer)
    }

    /// Number of operations in the sequenced log.
    pub fn operation_count(&self) -> usize {
        self.operations.len()
    }

    /// Iterate over all recorded operations in order.
    pub fn operations(&self) -> impl Iterator<Item = &OperationEntry> {
        self.operations.iter()
    }

    /// Return the set of peers that have acknowledged a given operation.
    pub fn acks_for(&self, op_id: &OpId) -> Option<&HashSet<PeerId>> {
        self.acks.get(op_id)
    }
}

// ── AckedGroupMembership trait implementation ───────────────────────────────

/// Unit struct that carries the [`AckedGroupMembership`] trait implementation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Membership;

impl AckedGroupMembership<PeerId, OpId> for Membership {
    type State = MembershipState;
    type Error = MembershipError;

    /// Creates a new group with the caller (`my_id`) as owner and the given
    /// `initial_members`.
    ///
    /// The owner is always included as a member even if not in the slice.
    fn create(my_id: PeerId, initial_members: &[PeerId]) -> Result<Self::State, Self::Error> {
        Ok(MembershipState::new(my_id, initial_members))
    }

    /// Processes a received welcome state from another peer.
    ///
    /// For the owner-centric model, the welcome state from the owner is
    /// authoritative.  We take all members and operations from the remote
    /// state, retaining only our own ownership view.
    fn from_welcome(y: Self::State, y_welcome: Self::State) -> Result<Self::State, Self::Error> {
        // Merge members from the welcome state into our own.
        let mut merged = y;
        merged.members.extend(&y_welcome.members);

        // Merge operation history.
        let existing_ops: HashSet<OpId> = merged.operations.iter().map(|o| o.op_id).collect();
        for op in &y_welcome.operations {
            if !existing_ops.contains(&op.op_id) {
                merged.operations.push(op.clone());
                merged
                    .add_ops
                    .extend(std::iter::once(op.op_id).filter(|_| op.kind == OpKind::Add));
                merged
                    .remove_ops
                    .extend(std::iter::once(op.op_id).filter(|_| op.kind == OpKind::Remove));
            }
        }

        // Merge acknowledgement state.
        for (op_id, ackers) in &y_welcome.acks {
            merged.acks.entry(*op_id).or_default().extend(ackers);
        }

        Ok(merged)
    }

    /// Adds a member to the group.
    ///
    /// The `adder` should be the owner (or a delegate with authority).  The
    /// operation is recorded with the given `operation_id` and takes effect
    /// immediately.
    ///
    /// Adding an already-present member is idempotent (no error): the DCGKA
    /// `process_welcome` flow calls `from_welcome` (which merges the welcome
    /// history already containing the new member) and then `add` for the
    /// same member, mirroring p2panda's reference DGM where `add` is a plain
    /// set insertion.
    fn add(
        mut y: Self::State,
        _adder: PeerId,
        added: PeerId,
        operation_id: OpId,
    ) -> Result<Self::State, Self::Error> {
        if !y.members.contains(&added) {
            y.operations.push(OperationEntry {
                op_id: operation_id,
                kind: OpKind::Add,
                peer: added,
            });
        }
        y.members.insert(added);
        y.add_ops.insert(operation_id);
        Ok(y)
    }

    /// Removes a member from the group.
    ///
    /// The `remover` should be the owner.  The operation is recorded with the
    /// given `operation_id` and takes effect immediately.
    fn remove(
        mut y: Self::State,
        _remover: PeerId,
        removed: &PeerId,
        operation_id: OpId,
    ) -> Result<Self::State, Self::Error> {
        if !y.members.contains(removed) {
            return Err(MembershipError::NotMember);
        }
        y.members.remove(removed);
        y.operations.push(OperationEntry {
            op_id: operation_id,
            kind: OpKind::Remove,
            peer: *removed,
        });
        y.remove_ops.insert(operation_id);
        Ok(y)
    }

    /// Records that `acker` has acknowledged the operation identified by
    /// `operation_id`.
    ///
    /// In the owner-centric model, operations from the owner do not require
    /// acknowledgements to take effect.  However we track them for
    /// compatibility with the p2panda-encryption DCKGA protocol.
    fn ack(
        mut y: Self::State,
        acker: PeerId,
        operation_id: OpId,
    ) -> Result<Self::State, Self::Error> {
        if !y.add_ops.contains(&operation_id) && !y.remove_ops.contains(&operation_id) {
            return Err(MembershipError::OperationNotFound);
        }
        y.acks.entry(operation_id).or_default().insert(acker);
        Ok(y)
    }

    /// Returns the member set visible to `viewer`.
    ///
    /// In the owner-centric model, all members see the same authoritative
    /// member set (since operations from the owner are immediately effective
    /// for everyone).  The viewer must be a current member.
    fn members_view(y: &Self::State, _viewer: &PeerId) -> Result<HashSet<PeerId>, Self::Error> {
        Ok(y.members.clone())
    }

    /// Returns `true` if the given operation ID corresponds to an add
    /// operation.
    fn is_add(y: &Self::State, operation_id: OpId) -> bool {
        y.add_ops.contains(&operation_id)
    }

    /// Returns `true` if the given operation ID corresponds to a remove
    /// operation.
    fn is_remove(y: &Self::State, operation_id: OpId) -> bool {
        y.remove_ops.contains(&operation_id)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group_encryption::types::PeerId;

    /// Helper: generate a PeerId for testing.
    fn make_peer() -> PeerId {
        let sk = iroh::SecretKey::generate();
        PeerId::from(sk.public())
    }

    /// Helper: generate a unique OpId for testing.
    fn make_op_id(seed: u64) -> OpId {
        let hash = blake3::hash(&seed.to_le_bytes());
        OpId::from(iroh_blobs::Hash::from_bytes(*hash.as_bytes()))
    }

    // ── add member tests ──────────────────────────────────────────────────

    #[test]
    fn test_add_member_appears_in_member_view() {
        let owner = make_peer();
        let new_member = make_peer();
        let op_id = make_op_id(1);

        let state = Membership::create(owner, &[]).expect("create");
        let state = Membership::add(state, owner, new_member, op_id).expect("add");

        let view = Membership::members_view(&state, &owner).expect("members_view");
        assert!(
            view.contains(&new_member),
            "added member should appear in view"
        );
        assert!(view.contains(&owner), "owner should be in view");
        assert_eq!(view.len(), 2, "expected 2 members");
    }

    #[test]
    fn test_add_existing_member_is_idempotent() {
        let owner = make_peer();
        let op_id = make_op_id(1);

        let state = Membership::create(owner, &[]).expect("create");
        let result = Membership::add(state, owner, owner, op_id);
        assert!(
            result.is_ok(),
            "adding an already-present member should be idempotent (DCGKA welcome flow)"
        );
        let state = result.unwrap();
        let view = Membership::members_view(&state, &owner).expect("members_view");
        assert_eq!(view.len(), 1, "member should not be duplicated");
    }

    #[test]
    fn test_add_with_initial_members() {
        let owner = make_peer();
        let alice = make_peer();
        let bob = make_peer();

        let state = Membership::create(owner, &[alice, bob]).expect("create");
        let view = Membership::members_view(&state, &owner).expect("members_view");

        assert!(view.contains(&owner), "owner should be in view");
        assert!(view.contains(&alice), "alice should be in view");
        assert!(view.contains(&bob), "bob should be in view");
        assert_eq!(view.len(), 3, "expected 3 members");
    }

    // ── remove member tests ───────────────────────────────────────────────

    #[test]
    fn test_remove_member_excluded_from_view() {
        let owner = make_peer();
        let alice = make_peer();
        let remove_op = make_op_id(2);

        let state = Membership::create(owner, &[alice]).expect("create");
        let state = Membership::remove(state, owner, &alice, remove_op).expect("remove");

        let view = Membership::members_view(&state, &owner).expect("members_view");
        assert!(
            !view.contains(&alice),
            "removed member should be excluded from view"
        );
        assert!(view.contains(&owner), "owner should still be in view");
        assert_eq!(view.len(), 1, "expected only owner");
    }

    #[test]
    fn test_remove_non_member_returns_error() {
        let owner = make_peer();
        let stranger = make_peer();
        let op_id = make_op_id(1);

        let state = Membership::create(owner, &[]).expect("create");
        let result = Membership::remove(state, owner, &stranger, op_id);
        assert!(result.is_err(), "removing a non-member should fail");
        assert_eq!(result.unwrap_err(), MembershipError::NotMember);
    }

    // ── is_add / is_remove tests ──────────────────────────────────────────

    #[test]
    fn test_is_add_returns_true_for_added_ops() {
        let owner = make_peer();
        let alice = make_peer();
        let op_id = make_op_id(1);

        let state = Membership::create(owner, &[]).expect("create");
        let state = Membership::add(state, owner, alice, op_id).expect("add");

        assert!(Membership::is_add(&state, op_id), "add op should be is_add");
        assert!(
            !Membership::is_remove(&state, op_id),
            "add op should not be is_remove"
        );
    }

    #[test]
    fn test_is_remove_returns_true_for_removed_ops() {
        let owner = make_peer();
        let alice = make_peer();
        let add_op = make_op_id(1);
        let remove_op = make_op_id(2);

        let state = Membership::create(owner, &[]).expect("create");
        let state = Membership::add(state, owner, alice, add_op).expect("add");
        let state = Membership::remove(state, owner, &alice, remove_op).expect("remove");

        assert!(
            Membership::is_remove(&state, remove_op),
            "remove op should be is_remove"
        );
        assert!(
            !Membership::is_add(&state, remove_op),
            "remove op should not be is_add"
        );
        // The add op should still be recognised.
        assert!(
            Membership::is_add(&state, add_op),
            "add op should still be is_add"
        );
    }

    // ── concurrent add/remove tests ───────────────────────────────────────

    #[test]
    fn test_concurrent_add_remove_handled_correctly() {
        let owner = make_peer();
        let alice = make_peer();
        let bob = make_peer();

        // Simulate two concurrent operations:
        //   1. Owner adds Alice  (op_id 1)
        //   2. Owner adds Bob    (op_id 2)
        // Both should succeed and both members should be visible.
        let state = Membership::create(owner, &[]).expect("create");
        let state = Membership::add(state, owner, alice, make_op_id(1)).expect("add alice");
        let state = Membership::add(state, owner, bob, make_op_id(2)).expect("add bob");

        let view = Membership::members_view(&state, &owner).expect("members_view");
        assert!(view.contains(&alice), "alice should be visible");
        assert!(view.contains(&bob), "bob should be visible");
        assert!(view.contains(&owner), "owner should be visible");
        assert_eq!(view.len(), 3, "expected 3 members");

        // Now simulate concurrent:
        //   1. Remove Alice (op_id 3)
        //   2. Remove Bob   (op_id 4)
        // Both should succeed and both should be gone.
        let state = Membership::remove(state, owner, &alice, make_op_id(3)).expect("remove alice");
        let state = Membership::remove(state, owner, &bob, make_op_id(4)).expect("remove bob");

        let view = Membership::members_view(&state, &owner).expect("members_view");
        assert!(!view.contains(&alice), "alice should be removed");
        assert!(!view.contains(&bob), "bob should be removed");
        assert!(view.contains(&owner), "owner should remain");
        assert_eq!(view.len(), 1, "expected only owner");
    }

    #[test]
    fn test_add_then_remove_same_member() {
        let owner = make_peer();
        let alice = make_peer();

        let state = Membership::create(owner, &[]).expect("create");
        let state = Membership::add(state, owner, alice, make_op_id(1)).expect("add");
        let state = Membership::remove(state, owner, &alice, make_op_id(2)).expect("remove");
        let state = Membership::add(state, owner, alice, make_op_id(3)).expect("add again");

        let view = Membership::members_view(&state, &owner).expect("members_view");
        assert!(view.contains(&alice), "alice should be back after re-add");
        assert_eq!(view.len(), 2, "expected 2 members");
    }

    // ── from_welcome tests ────────────────────────────────────────────────

    #[test]
    fn test_from_welcome_merges_members() {
        let owner = make_peer();
        let alice = make_peer();

        // Simulate Alice's local state (owner created group, Alice hasn't seen anything yet).
        let alice_local = Membership::create(alice, &[]).expect("alice create");

        // Simulate the welcome state from the owner (owner + Alice are members).
        let owner_state = Membership::create(owner, &[alice]).expect("owner create");

        // Alice processes the welcome from the owner.
        let merged = Membership::from_welcome(alice_local, owner_state).expect("from_welcome");

        let view = Membership::members_view(&merged, &alice).expect("members_view");
        assert!(view.contains(&owner), "owner should be visible");
        assert!(view.contains(&alice), "alice should be visible");
        assert_eq!(view.len(), 2, "expected 2 members");
    }

    // ── ack tests ─────────────────────────────────────────────────────────

    #[test]
    fn test_ack_records_acknowledgement() {
        let owner = make_peer();
        let alice = make_peer();
        let op_id = make_op_id(1);

        let state = Membership::create(owner, &[]).expect("create");
        let state = Membership::add(state, owner, alice, op_id).expect("add");

        // Alice acknowledges the add operation.
        let state = Membership::ack(state, alice, op_id).expect("ack");

        let ackers = state.acks_for(&op_id);
        assert!(ackers.is_some(), "acks should exist for the op");
        assert!(
            ackers.unwrap().contains(&alice),
            "alice should be in ackers"
        );
        assert_eq!(ackers.unwrap().len(), 1, "expected 1 acker");
    }

    #[test]
    fn test_ack_unknown_operation_returns_error() {
        let owner = make_peer();
        let alice = make_peer();
        let missing_op = make_op_id(99);

        let state = Membership::create(owner, &[]).expect("create");
        let result = Membership::ack(state, alice, missing_op);
        assert!(result.is_err(), "acking unknown op should fail");
        assert_eq!(result.unwrap_err(), MembershipError::OperationNotFound);
    }

    // ── serialisation tests ───────────────────────────────────────────────

    #[test]
    fn test_state_serialize_roundtrip() {
        let owner = make_peer();
        let alice = make_peer();
        let bob = make_peer();

        let state = Membership::create(owner, &[alice]).expect("create");
        let state = Membership::add(state, owner, bob, make_op_id(1)).expect("add bob");

        let bytes = postcard::to_allocvec(&state).expect("serialize");
        let deserialized: MembershipState = postcard::from_bytes(&bytes).expect("deserialize");

        assert_eq!(state, deserialized, "round-trip equality");
    }

    // ── MembershipState accessors ─────────────────────────────────────────

    #[test]
    fn test_membership_state_accessors() {
        let owner = make_peer();
        let alice = make_peer();

        let state = Membership::create(owner, &[alice]).expect("create");

        assert_eq!(state.owner(), owner, "owner accessor");
        assert!(state.is_member(&owner), "owner is a member");
        assert!(state.is_member(&alice), "alice is a member");
        assert!(!state.is_member(&make_peer()), "stranger is not a member");
    }
}
