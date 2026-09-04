//! Authoritative room roles, capabilities, and signed moderation events.
//!
//! This module is the room-authorization root of authority. UI state, rosters,
//! invites, and capability advertisements are projections only; a receiver
//! must verify and apply [`AuthorizationEvent`] before accepting an action.
//! Events have a monotonic room sequence, a signed canonical body, and an
//! event-id cache, which makes replay and reordering deterministic.

#![allow(missing_docs)]

use std::collections::{HashMap, HashSet};

use iroh::{PublicKey, SecretKey, Signature};
use serde::{Deserialize, Serialize};
use serde_byte_array::ByteArray;

use crate::TopicId;

/// Current persisted and wire format version.
pub const AUTHORIZATION_VERSION: u8 = 1;
const EVENT_ID_LEN: usize = 32;
const SIGNING_DOMAIN: &str = "boru/room-authorization";

/// Room role, ordered from least to most authority.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Role {
    /// A newly admitted, restricted participant.
    #[default]
    Guest,
    /// A normal participant.
    Member,
    /// A trusted moderator.
    Moderator,
    /// The sole room authority.
    Owner,
}

/// Actions that can be authorized independently of the UI.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum Permission {
    SendMessages,
    PinMessages,
    ScreenShare,
    ManageRoom,
    Invite,
    Kick,
    Ban,
    ManageRoles,
}

impl Role {
    /// The default capabilities for a role.
    pub fn default_permissions(self) -> Vec<Permission> {
        use Permission::*;
        match self {
            Role::Owner => vec![SendMessages, PinMessages, ScreenShare, ManageRoom, Invite, Kick, Ban, ManageRoles],
            Role::Moderator => vec![SendMessages, PinMessages, ScreenShare, Invite, Kick, Ban],
            Role::Member => vec![SendMessages, PinMessages, ScreenShare],
            Role::Guest => vec![SendMessages],
        }
    }

    /// Numeric authority used for strict hierarchy checks.
    pub const fn authority(self) -> u8 {
        match self {
            Role::Guest => 0,
            Role::Member => 1,
            Role::Moderator => 2,
            Role::Owner => 3,
        }
    }
}

/// A signed room authorization operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AuthorizationAction {
    Grant { permission: Permission },
    Revoke { permission: Permission },
    ChangeRole { role: Role },
    Ban,
    Unban,
}

/// Signed, replay-protected authorization operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthorizationEvent {
    pub version: u8,
    pub group_id: TopicId,
    pub event_id: ByteArray<EVENT_ID_LEN>,
    pub sequence: u64,
    pub actor: PublicKey,
    pub target: PublicKey,
    pub action: AuthorizationAction,
    pub signature: ByteArray<{ Signature::LENGTH }>,
}

/// Fail-closed authorization errors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthorizationError {
    UnsupportedVersion(u8),
    WrongGroup,
    InvalidSignature,
    EventIdMismatch,
    Replay,
    OutOfOrder { expected_after: u64, actual: u64 },
    UnknownActor,
    UnknownTarget,
    Banned,
    PermissionDenied,
    OwnerSafety,
    InvalidTransition,
    Decode(String),
}

impl std::fmt::Display for AuthorizationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for AuthorizationError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct StoredAuthorizationState {
    version: u8,
    state: AuthorizationState,
}

/// Authoritative, serializable room authorization state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthorizationState {
    group_id: TopicId,
    owner: PublicKey,
    members: HashMap<PublicKey, Role>,
    permissions: HashMap<PublicKey, HashSet<Permission>>,
    banned: HashSet<PublicKey>,
    last_sequence: u64,
    applied_events: HashSet<ByteArray<EVENT_ID_LEN>>,
}

impl AuthorizationState {
    /// Start a room with exactly one authority.
    pub fn new(group_id: TopicId, owner: PublicKey) -> Self {
        let mut state = Self {
            group_id,
            owner,
            members: HashMap::new(),
            permissions: HashMap::new(),
            banned: HashSet::new(),
            last_sequence: 0,
            applied_events: HashSet::new(),
        };
        state.members.insert(owner, Role::Owner);
        state.permissions.insert(owner, Role::Owner.default_permissions().into_iter().collect());
        state
    }

    pub fn owner(&self) -> PublicKey { self.owner }
    pub fn role_of(&self, peer: &PublicKey) -> Option<Role> { self.members.get(peer).copied() }
    pub fn is_banned(&self, peer: &PublicKey) -> bool { self.banned.contains(peer) }
    pub fn last_sequence(&self) -> u64 { self.last_sequence }
    pub fn members(&self) -> &HashMap<PublicKey, Role> { &self.members }

    /// Validate the invariants required before trusting this state.
    pub fn validate(&self) -> Result<(), AuthorizationError> {
        let owner_count = self
            .members
            .values()
            .filter(|role| **role == Role::Owner)
            .count();
        let keys_match = self.members.keys().all(|peer| self.permissions.contains_key(peer))
            && self.permissions.keys().all(|peer| self.members.contains_key(peer));
        if self.members.get(&self.owner) != Some(&Role::Owner)
            || owner_count != 1
            || self.banned.contains(&self.owner)
            || !keys_match
        {
            return Err(AuthorizationError::InvalidTransition);
        }
        Ok(())
    }

    /// Test/bootstrap boundary for admitting a member. Runtime changes must
    /// arrive as signed events; this does not grant access to banned peers.
    pub fn admit_member(&mut self, peer: PublicKey, role: Role) -> Result<(), AuthorizationError> {
        if self.banned.contains(&peer) || self.members.contains_key(&peer) || role == Role::Owner {
            return Err(AuthorizationError::InvalidTransition);
        }
        self.members.insert(peer, role);
        self.permissions.insert(peer, role.default_permissions().into_iter().collect());
        Ok(())
    }

    /// Check a capability at the enforcement boundary.
    pub fn allows(&self, peer: &PublicKey, permission: Permission) -> bool {
        self.members.contains_key(peer)
            && !self.banned.contains(peer)
            && self.permissions.get(peer).is_some_and(|p| p.contains(&permission))
    }

    /// Pure policy evaluation. This performs all authentication, ordering,
    /// hierarchy, ban, and owner-safety checks without mutating the state.
    pub fn evaluate(&self, event: &AuthorizationEvent) -> Result<(), AuthorizationError> {
        self.validate()?;
        event.verify(self)?;
        let actor_role = self.role_of(&event.actor).ok_or(AuthorizationError::UnknownActor)?;
        if self.banned.contains(&event.actor) {
            return Err(AuthorizationError::Banned);
        }
        let target_role = self.role_of(&event.target);
        if self.banned.contains(&event.target)
            && !matches!(event.action, AuthorizationAction::Unban)
        {
            return Err(AuthorizationError::Banned);
        }
        let manages_target = target_role.is_some_and(|role| {
            event.target != self.owner && actor_role.authority() > role.authority()
        });
        let allowed = match event.action {
            AuthorizationAction::Grant { permission }
            | AuthorizationAction::Revoke { permission } => {
                manages_target
                    && match actor_role {
                        Role::Owner => true,
                        Role::Moderator => matches!(permission, Permission::PinMessages | Permission::Invite | Permission::Kick | Permission::Ban),
                        Role::Guest | Role::Member => false,
                    }
            }
            AuthorizationAction::ChangeRole { role } => {
                actor_role == Role::Owner && role != Role::Owner && manages_target
            }
            AuthorizationAction::Ban | AuthorizationAction::Unban => {
                manages_target && matches!(actor_role, Role::Owner | Role::Moderator)
                    && (matches!(event.action, AuthorizationAction::Ban)
                        || self.banned.contains(&event.target))
            }
        };
        if !allowed {
            if event.target == self.owner
                || matches!(event.action, AuthorizationAction::ChangeRole { role: Role::Owner })
            {
                return Err(AuthorizationError::OwnerSafety);
            }
            return Err(if target_role.is_none() {
                AuthorizationError::UnknownTarget
            } else {
                AuthorizationError::PermissionDenied
            });
        }
        Ok(())
    }

    /// Apply a signed event atomically after pure policy evaluation.
    pub fn apply(&mut self, event: &AuthorizationEvent) -> Result<(), AuthorizationError> {
        self.evaluate(event)?;
        match event.action {
            AuthorizationAction::Grant { permission } => {
                self.permissions.entry(event.target).or_default().insert(permission);
            }
            AuthorizationAction::Revoke { permission } => {
                self.permissions.entry(event.target).or_default().remove(&permission);
            }
            AuthorizationAction::ChangeRole { role } => {
                self.members.insert(event.target, role);
                self.permissions.insert(event.target, role.default_permissions().into_iter().collect());
            }
            AuthorizationAction::Ban => { self.banned.insert(event.target); }
            AuthorizationAction::Unban => { self.banned.remove(&event.target); }
        }
        self.last_sequence = event.sequence;
        self.applied_events.insert(event.event_id.clone());
        self.validate()
    }

    /// Versioned persistence used for restart and late-join backfill.
    pub fn to_bytes(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_stdvec(&StoredAuthorizationState { version: AUTHORIZATION_VERSION, state: self.clone() })
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AuthorizationError> {
        let stored: StoredAuthorizationState = postcard::from_bytes(bytes).map_err(|e| AuthorizationError::Decode(e.to_string()))?;
        if stored.version != AUTHORIZATION_VERSION { return Err(AuthorizationError::UnsupportedVersion(stored.version)); }
        stored.state.validate()?;
        Ok(stored.state)
    }
}

impl AuthorizationEvent {
    /// Sign a new event. `sequence` must be strictly increasing in a room.
    pub fn sign(secret: &SecretKey, group_id: TopicId, sequence: u64, target: PublicKey, action: AuthorizationAction) -> Result<Self, AuthorizationError> {
        let actor = secret.public();
        let event_id = Self::derive_id(group_id, sequence, actor, target, &action)?;
        let unsigned = Self::signing_bytes(AUTHORIZATION_VERSION, group_id, &event_id, sequence, actor, target, &action)?;
        Ok(Self { version: AUTHORIZATION_VERSION, group_id, event_id: ByteArray::new(event_id), sequence, actor, target, action, signature: ByteArray::new(secret.sign(&unsigned).to_bytes()) })
    }

    pub fn verify(&self, state: &AuthorizationState) -> Result<(), AuthorizationError> {
        if self.version != AUTHORIZATION_VERSION { return Err(AuthorizationError::UnsupportedVersion(self.version)); }
        if self.group_id != state.group_id { return Err(AuthorizationError::WrongGroup); }
        if state.applied_events.contains(&self.event_id) { return Err(AuthorizationError::Replay); }
        if self.sequence != state.last_sequence.saturating_add(1) { return Err(AuthorizationError::OutOfOrder { expected_after: state.last_sequence, actual: self.sequence }); }
        let expected = Self::derive_id(self.group_id, self.sequence, self.actor, self.target, &self.action)?;
        if self.event_id.as_ref() != expected { return Err(AuthorizationError::EventIdMismatch); }
        let bytes = Self::signing_bytes(self.version, self.group_id, self.event_id.as_ref(), self.sequence, self.actor, self.target, &self.action)?;
        let signature_bytes = *self.signature.as_ref();
        let signature = Signature::from_bytes(&signature_bytes);
        if self.actor.verify(&bytes, &signature).is_err() { return Err(AuthorizationError::InvalidSignature); }
        Ok(())
    }

    fn derive_id(group_id: TopicId, sequence: u64, actor: PublicKey, target: PublicKey, action: &AuthorizationAction) -> Result<[u8; EVENT_ID_LEN], AuthorizationError> {
        let bytes = Self::signing_bytes(AUTHORIZATION_VERSION, group_id, &[0; EVENT_ID_LEN], sequence, actor, target, action)?;
        Ok(*blake3::hash(&bytes).as_bytes())
    }
    fn signing_bytes(version: u8, group_id: TopicId, event_id: &[u8], sequence: u64, actor: PublicKey, target: PublicKey, action: &AuthorizationAction) -> Result<Vec<u8>, AuthorizationError> {
        crate::protocol_signing::canonical_signed_bytes(SIGNING_DOMAIN, version as u16, &(group_id, event_id, sequence, actor, target, action)).map_err(|e| AuthorizationError::Decode(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn keys() -> (SecretKey, SecretKey, SecretKey) { (SecretKey::from_bytes(&[1; 32]), SecretKey::from_bytes(&[2; 32]), SecretKey::from_bytes(&[3; 32])) }
    fn state(owner: PublicKey, member: PublicKey) -> AuthorizationState { let mut s = AuthorizationState::new([7; 32].into(), owner); s.admit_member(member, Role::Member).unwrap(); s }
    #[test]
    fn forged_event_is_rejected() {
        let (owner, member, _) = keys();
        let mut s = state(owner.public(), member.public());
        let mut e = AuthorizationEvent::sign(&owner, [7; 32].into(), 1, member.public(), AuthorizationAction::Grant { permission: Permission::Ban }).unwrap();
        let mut signature: [u8; Signature::LENGTH] = *e.signature.as_ref();
        signature[0] ^= 1;
        e.signature = ByteArray::new(signature);
        assert_eq!(s.apply(&e), Err(AuthorizationError::InvalidSignature));
    }
    #[test]
    fn revoke_and_replay_are_rejected() { let (owner, member, _) = keys(); let mut s = state(owner.public(), member.public()); let e = AuthorizationEvent::sign(&owner, [7; 32].into(), 1, member.public(), AuthorizationAction::Revoke { permission: Permission::ScreenShare }).unwrap(); s.apply(&e).unwrap(); assert!(!s.allows(&member.public(), Permission::ScreenShare)); assert_eq!(s.apply(&e), Err(AuthorizationError::Replay)); }
    #[test]
    fn out_of_order_event_is_rejected() { let (owner, member, _) = keys(); let mut s = state(owner.public(), member.public()); let e = AuthorizationEvent::sign(&owner, [7; 32].into(), 2, member.public(), AuthorizationAction::Grant { permission: Permission::PinMessages }).unwrap(); assert!(matches!(s.apply(&e), Err(AuthorizationError::OutOfOrder { .. }))); }
    #[test]
    fn owner_cannot_be_banned_or_demoted() { let (owner, member, _) = keys(); let mut s = state(owner.public(), member.public()); let e = AuthorizationEvent::sign(&owner, [7; 32].into(), 1, owner.public(), AuthorizationAction::Ban).unwrap(); assert_eq!(s.apply(&e), Err(AuthorizationError::OwnerSafety)); }
    #[test]
    fn hierarchy_and_failed_evaluation_are_fail_closed() {
        let (owner, member, moderator) = keys();
        let mut s = AuthorizationState::new([7; 32].into(), owner.public());
        s.admit_member(member.public(), Role::Member).unwrap();
        s.admit_member(moderator.public(), Role::Moderator).unwrap();
        let before = s.clone();
        let escalation = AuthorizationEvent::sign(
            &moderator,
            [7; 32].into(),
            1,
            moderator.public(),
            AuthorizationAction::Grant {
                permission: Permission::ManageRoles,
            },
        )
        .unwrap();
        assert_eq!(s.evaluate(&escalation), Err(AuthorizationError::PermissionDenied));
        assert_eq!(s, before);

        let role_change = AuthorizationEvent::sign(
            &owner,
            [7; 32].into(),
            1,
            member.public(),
            AuthorizationAction::ChangeRole { role: Role::Owner },
        )
        .unwrap();
        assert_eq!(s.apply(&role_change), Err(AuthorizationError::OwnerSafety));
        assert_eq!(s, before);
    }

    #[test]
    fn validate_rejects_corrupt_persisted_state() {
        let (owner, member, _) = keys();
        let mut s = state(owner.public(), member.public());
        s.permissions.remove(&member.public());
        let bytes = postcard::to_stdvec(&StoredAuthorizationState {
            version: AUTHORIZATION_VERSION,
            state: s,
        })
        .unwrap();
        assert_eq!(
            AuthorizationState::from_bytes(&bytes),
            Err(AuthorizationError::InvalidTransition)
        );
    }

    #[test]
    fn state_roundtrips_for_restart_and_backfill() { let (owner, member, _) = keys(); let s = state(owner.public(), member.public()); let bytes = s.to_bytes().unwrap(); assert_eq!(AuthorizationState::from_bytes(&bytes).unwrap(), s); }
}
