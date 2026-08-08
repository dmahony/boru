//! Per-group encryption state and high-level API for managing encrypted groups.
//!
//! [`EncryptionState`] holds per-group [`GroupState`] instances keyed by
//! [`GroupId`] and provides high-level methods for creating encrypted groups,
//! sending encrypted messages, and processing incoming encrypted messages.
//!
//! This module wires the p2panda-encryption [`MessageGroup`] API into boru's
//! application layer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use p2panda_encryption::crypto::Rng;
use p2panda_encryption::key_bundle::Lifetime;
use p2panda_encryption::message_scheme::group::{
    GroupConfig, GroupEvent, GroupState, MessageGroup,
};
use p2panda_encryption::traits::{AckedGroupMembership, PreKeyManager};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::group_id::GroupId;

use super::manager::{KmgState, Manager, ManagerError};
use super::membership::{MemberRole, Membership};
use super::message::EncryptedGroupEnvelope;
use super::ordering::{LamportOrderer, OrderingState};
use super::persistence;
use super::registry::{Registry, RegistryState};
use super::types::{OpId, PeerId};

/// Type alias for the fully-parameterised per-group encryption state.
pub type GroupEncryptionState =
    GroupState<PeerId, OpId, Registry, Membership, Manager, LamportOrderer>;

/// High-level encryption state holding per-group [`GroupEncryptionState`]s.
///
/// Each encrypted room has its own `GroupEncryptionState` keyed by
/// [`GroupId`].  The state is modified in-place by all operations.
///
/// # Role enforcement (Kith-style)
///
/// [`MemberRole`]s (Admin/Writer/Reader) are enforced **per message** here:
///
/// - [`Self::send_message`] refuses to encrypt for a local peer whose role is
///   [`MemberRole::Reader`] or who is not a member — even if the caller holds
///   a leaked copy of the group state.
/// - [`Self::receive_message`] refuses to surface plaintext authored by a
///   sender that is not a member or is a Reader (defense against a removed /
///   unauthorized sender with valid keys).
///
/// The mirror table (`group_roles`) is the application-layer policy used for
/// enforcement.  The authoritative member set always comes from the p2panda
/// DGM via [`MessageGroup::members`]; roles that ride inside the DGM state
/// (see [`MembershipState`](super::membership::MembershipState)) are the
/// durable/wire carrier, while this mirror is what the high-level API
/// consults.  p2panda-encryption 0.7 has **no role field in the wire message
/// scheme**, so a malicious sender with a valid ratchet can still emit
/// ciphertext; honest receivers drop it.
#[derive(Debug)]
pub struct EncryptionState {
    /// Per-group encryption states.
    pub groups: HashMap<GroupId, GroupEncryptionState>,
    /// Local peer's x25519 key material for the encryption layer.
    pub kmg_state: KmgState,
    /// Shared registry (SQLite-backed PKI store).
    pub registry: RegistryState,
    /// CSPRNG for key generation and cryptographic operations.
    pub rng: Rng,
    /// Optional SQLite connection for auto-persisting group state
    /// after every mutation. When `None`, state is only kept in memory.
    pub db: Option<Arc<Mutex<Connection>>>,
    /// Per-group role mirror used for per-message enforcement.
    ///
    /// Keyed by group id → (peer id → role).  Kept in sync with the DGM role
    /// state at create / init / add / remove / set-role call sites and
    /// persisted through [`persistence`].  Defaults: owner is `Admin`,
    /// everyone else `Writer`.
    pub group_roles: HashMap<GroupId, HashMap<PeerId, MemberRole>>,
    /// Local peer id per group (set at create/init, used for send-side checks).
    pub self_ids: HashMap<GroupId, PeerId>,
}

// ── Serialization workaround ──────────────────────────────────────────────
//
// RegistryState cannot be serialized (it holds a SQLite connection handle).
// We replace it with a dummy during serialization and expect the caller to
// re-attach a live connection after deserialization.

impl Serialize for EncryptionState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("EncryptionState", 5)?;
        s.serialize_field("groups", &self.groups)?;
        s.serialize_field("kmg_state", &self.kmg_state)?;
        // RegistryState serializes as unit (no-op).
        s.serialize_field("registry", &self.registry)?;
        s.serialize_field("group_roles", &self.group_roles)?;
        s.serialize_field("self_ids", &self.self_ids)?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for EncryptionState {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct EncryptionStateHelper {
            groups: HashMap<GroupId, GroupEncryptionState>,
            kmg_state: KmgState,
            registry: RegistryState,
            #[serde(default)]
            group_roles: HashMap<GroupId, HashMap<PeerId, MemberRole>>,
            #[serde(default)]
            self_ids: HashMap<GroupId, PeerId>,
        }
        let helper = EncryptionStateHelper::deserialize(deserializer)?;
        Ok(Self {
            groups: helper.groups,
            kmg_state: helper.kmg_state,
            registry: helper.registry,
            rng: Rng::default(),
            db: None,
            group_roles: helper.group_roles,
            self_ids: helper.self_ids,
        })
    }
}

impl EncryptionState {
    /// Create a new encryption state with a freshly generated x25519 identity
    /// and an initial long-term pre-key.
    ///
    /// The registry is initialised with an in-memory SQLite database (caller
    /// should persist to disk for production use).
    pub fn new_with_rng(rng: Rng) -> Result<Self, ManagerError> {
        let mut kmg_state = Manager::init_with_rng(&rng)?;
        kmg_state =
            <Manager as PreKeyManager>::rotate_prekey(kmg_state, Lifetime::default(), &rng)?;
        let conn = rusqlite::Connection::open_in_memory()
            .map_err(|e| ManagerError::Internal(e.to_string()))?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS identity_registry (
                peer_id BLOB PRIMARY KEY,
                key_bundle BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prekey_registry (
                peer_id BLOB NOT NULL,
                pre_key BLOB NOT NULL,
                used INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .map_err(|e| ManagerError::Internal(e.to_string()))?;
        let registry = RegistryState::new(std::sync::Arc::new(std::sync::Mutex::new(conn)));
        Ok(Self {
            groups: HashMap::new(),
            kmg_state,
            registry,
            rng,
            db: None,
            group_roles: HashMap::new(),
            self_ids: HashMap::new(),
        })
    }

    /// Attach a SQLite connection for auto-persisting group state.
    pub fn with_db(mut self, conn: Arc<Mutex<Connection>>) -> Self {
        self.db = Some(conn);
        self
    }

    /// Internal helper: persist the state for a single group to SQLite.
    fn save_current_group_state(&self, group_id: &GroupId) {
        let Some(ref conn) = self.db else { return };
        let Some(state) = self.groups.get(group_id) else {
            return;
        };
        let conn = conn.lock().unwrap();
        if let Err(e) = persistence::save_group_state(&conn, group_id, state) {
            tracing::warn!("failed to save group encryption state for {group_id}: {e}");
        }
        // Persist the role mirror + local identity alongside the state.
        let roles = self.group_roles.get(group_id).cloned().unwrap_or_default();
        let self_id = self.self_ids.get(group_id).copied();
        if let Err(e) = persistence::save_group_roles(&conn, group_id, &roles, self_id) {
            tracing::warn!("failed to save group role mirror for {group_id}: {e}");
        }
    }

    /// Load a previously-persisted `GroupEncryptionState` from SQLite.
    ///
    /// Returns `true` if a state was loaded and inserted into `self.groups`,
    /// `false` if no persisted state existed.
    pub fn load_group_state_from_db(
        &mut self,
        group_id: &GroupId,
    ) -> Result<bool, EncryptionError> {
        let Some(ref conn) = self.db else {
            return Ok(false);
        };
        let conn = conn
            .lock()
            .map_err(|e| EncryptionError::Internal(format!("db lock: {e}")))?;
        match persistence::load_group_state(&conn, group_id) {
            Ok(Some(state)) => {
                self.groups.insert(*group_id, state);
                // Restore the role mirror + local identity if present.
                if let Ok(Some((roles, self_id))) = persistence::load_group_roles(&conn, group_id) {
                    if !roles.is_empty() {
                        self.group_roles.insert(*group_id, roles);
                    }
                    if let Some(me) = self_id {
                        self.self_ids.insert(*group_id, me);
                    }
                }
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(e) => Err(EncryptionError::Internal(format!("db error: {e}"))),
        }
    }

    /// Remove a group's persisted encryption state from SQLite.
    pub fn delete_group_state_from_db(&self, group_id: &GroupId) {
        let Some(ref conn) = self.db else { return };
        let conn = conn.lock().unwrap();
        if let Err(e) = persistence::delete_group_state(&conn, group_id) {
            tracing::warn!("failed to delete group encryption state for {group_id}: {e}");
        }
        if let Err(e) = persistence::delete_group_roles(&conn, group_id) {
            tracing::warn!("failed to delete group role mirror for {group_id}: {e}");
        }
    }

    /// Initialise an empty group state for a group we expect to join.
    ///
    /// Creates a [`GroupState`] without establishing the group (no ratchet,
    /// no membership).  This state is ready to receive a control or welcome
    /// message via [`receive_message`](Self::receive_message).
    ///
    /// `my_id` should be this peer's identity (registered in the registry).
    pub fn init_group(&mut self, group_id: GroupId, my_id: PeerId) -> Result<(), EncryptionError> {
        let orderer_state = OrderingState::new(my_id);
        let dgm = <Membership as AckedGroupMembership<PeerId, OpId>>::create(my_id, &[])
            .map_err(|e| EncryptionError::Membership(Box::new(e)))?;
        let pki = self.registry.clone();

        let state = MessageGroup::init(
            my_id,
            self.kmg_state.clone(),
            pki,
            dgm,
            orderer_state,
            GroupConfig::default(),
        );
        self.groups.insert(group_id, state);
        // Track the local peer and its default (Writer) role.  The owner /
        // admin may promote the peer later via [`Self::set_member_role`].
        self.self_ids.insert(group_id, my_id);
        self.group_roles
            .entry(group_id)
            .or_default()
            .entry(my_id)
            .or_insert(MemberRole::Writer);
        Ok(())
    }

    /// Create a new encrypted group with `my_id` as the creator/owner and
    /// `initial_members` as the founding members.
    ///
    /// Returns the control message that should be broadcast on the gossip
    /// topic as `Message::EncryptedGroupMessage`.
    pub fn create_group(
        &mut self,
        group_id: GroupId,
        my_id: PeerId,
        initial_members: Vec<PeerId>,
    ) -> Result<EncryptedGroupEnvelope, EncryptionError> {
        // Build the initial ordering state for this new group.
        let orderer_state = OrderingState::new(my_id);
        let dgm =
            <Membership as AckedGroupMembership<PeerId, OpId>>::create(my_id, &initial_members)
                .map_err(|e| EncryptionError::Membership(Box::new(e)))?;
        let pki = self.registry.clone();

        let state = MessageGroup::init(
            my_id,
            self.kmg_state.clone(),
            pki,
            dgm,
            orderer_state,
            GroupConfig::default(),
        );

        // Track the local peer (owner => Admin) and seed the role mirror
        // before the members vector is moved into MessageGroup::create.
        self.self_ids.insert(group_id, my_id);
        let mut roles = HashMap::with_capacity(initial_members.len() + 1);
        roles.insert(my_id, MemberRole::Admin);
        for member in &initial_members {
            roles.entry(*member).or_insert(MemberRole::Writer);
        }
        self.group_roles.insert(group_id, roles);

        let (state, message) = MessageGroup::create(state, initial_members, &self.rng)
            .map_err(|e| EncryptionError::Group(Box::new(e)))?;

        // Save the state and extract the message for broadcast.
        self.groups.insert(group_id, state);

        // Persist the newly-created group state.
        self.save_current_group_state(&group_id);

        // The message from create() is an EncryptedGroupEnvelope (via our
        // ForwardSecureOrdering impl).
        Ok(message)
    }

    /// Set (or change) the role of a group member.
    ///
    /// Only an admin (or the group owner, who is always `Admin`) may change
    /// roles.  The change is applied to the local role mirror used for
    /// per-message enforcement and persisted with the group state.
    ///
    /// # Errors
    ///
    /// - [`EncryptionError::NotAuthorized`] if `actor` is not an admin.
    /// - [`EncryptionError::NotMember`] if `member` is not in the group.
    /// - [`EncryptionError::GroupNotFound`] if the group is unknown.
    pub fn set_member_role(
        &mut self,
        group_id: &GroupId,
        actor: PeerId,
        member: PeerId,
        role: MemberRole,
    ) -> Result<(), EncryptionError> {
        let actor_role = self
            .group_roles
            .get(group_id)
            .and_then(|roles| roles.get(&actor))
            .copied()
            .ok_or(EncryptionError::NotMember(actor))?;
        if !actor_role.can_manage() {
            return Err(EncryptionError::NotAuthorized(actor));
        }
        let roles = self
            .group_roles
            .get_mut(group_id)
            .ok_or(EncryptionError::GroupNotFound(*group_id))?;
        if !roles.contains_key(&member) {
            return Err(EncryptionError::NotMember(member));
        }
        roles.insert(member, role);
        self.save_current_group_state(group_id);
        Ok(())
    }

    /// Look up the local role mirror entry for a peer in a group.
    ///
    /// Returns `None` if the group is unknown or the peer has no entry.
    pub fn member_role(&self, group_id: &GroupId, peer: &PeerId) -> Option<MemberRole> {
        self.group_roles
            .get(group_id)
            .and_then(|roles| roles.get(peer))
            .copied()
    }

    /// Whether a peer may send (write) messages in a group.
    ///
    /// This consults the role mirror; the authoritative membership check is
    /// performed inside [`Self::send_message`] via the p2panda DGM.
    pub fn can_write(&self, group_id: &GroupId, peer: &PeerId) -> bool {
        self.member_role(group_id, peer)
            .is_some_and(MemberRole::can_write)
    }

    /// Encrypt and send an application message to an existing encrypted group.
    ///
    /// Returns the encrypted envelope that should be broadcast on the gossip
    /// topic.
    ///
    /// # Role enforcement
    ///
    /// The local peer must be a current member with a writing role
    /// ([`MemberRole::Admin`] or [`MemberRole::Writer`]).  A [`MemberRole::Reader`]
    /// is refused, and a non-member is refused even if they hold a leaked copy
    /// of the group state (the p2panda DGM is the authoritative member set).
    pub fn send_message(
        &mut self,
        group_id: &GroupId,
        plaintext: &[u8],
    ) -> Result<EncryptedGroupEnvelope, EncryptionError> {
        // ── Role enforcement (Kith-style) ─────────────────────────────
        // 1. The caller must know who they are in this group.
        let my_id = self.self_ids.get(group_id).copied().ok_or_else(|| {
            EncryptionError::Internal(format!("no local identity recorded for {group_id:?}"))
        })?;
        // 2. The p2panda DGM is the authoritative member set: a non-member is
        //    refused even with a leaked key.
        let state = self
            .groups
            .get(group_id)
            .ok_or(EncryptionError::GroupNotFound(*group_id))?;
        let members =
            MessageGroup::members(state).map_err(|e| EncryptionError::Group(Box::new(e)))?;
        if !members.contains(&my_id) {
            return Err(EncryptionError::NotMember(my_id));
        }
        // 3. The role mirror decides write permission.
        let role = self
            .member_role(group_id, &my_id)
            .unwrap_or(MemberRole::Writer);
        if !role.can_write() {
            return Err(EncryptionError::ForbiddenRole { peer: my_id, role });
        }

        let state = self
            .groups
            .remove(group_id)
            .ok_or(EncryptionError::GroupNotFound(*group_id))?;

        let (state, message) = MessageGroup::send(state, plaintext)
            .map_err(|e| EncryptionError::Group(Box::new(e)))?;

        self.groups.insert(*group_id, state);

        // Persist updated state after sending.
        self.save_current_group_state(group_id);

        Ok(message)
    }

    /// Process an incoming encrypted group message.
    ///
    /// Returns the output events from the group protocol, which may include
    /// decrypted application messages, control messages to rebroadcast, or
    /// a `RemovedOurselves` signal.
    ///
    /// # Role enforcement
    ///
    /// Application plaintext is only surfaced when the sender is a current
    /// member with a writing role.  A message authored by a non-member (e.g.
    /// a removed device with a leaked key) or by a [`MemberRole::Reader`] is
    /// dropped: the caller never receives the plaintext.
    pub fn receive_message(
        &mut self,
        group_id: &GroupId,
        envelope: &EncryptedGroupEnvelope,
    ) -> Result<Option<GroupEvent<PeerId, OpId, Membership, LamportOrderer>>, EncryptionError> {
        // First time seeing this group? Initialize state to queue messages.
        if !self.groups.contains_key(group_id) {
            // We haven't joined this group yet; the envelope is queued
            // by the MessageGroup::receive call which handles "not yet
            // established" gracefully.
            // For now, return None — the caller should store the message
            // for later processing when we're added.
            return Ok(None);
        }

        let state = self
            .groups
            .remove(group_id)
            .ok_or(EncryptionError::GroupNotFound(*group_id))?;

        let (state, output) = MessageGroup::receive(state, envelope, &self.rng)
            .map_err(|e| EncryptionError::Group(Box::new(e)))?;

        self.groups.insert(*group_id, state);

        // Persist updated state after receiving.
        self.save_current_group_state(group_id);

        // GroupOutput wraps events in a Vec. If this is our first welcome
        // or a normal message, unwrap the single GroupOutput.
        match output {
            Some(group_output) => {
                // Take the first event (GroupOutput is a batch wrapper).
                let event = group_output.events.into_iter().next();

                // ── Sender-side role enforcement (Kith-style) ─────────
                // Drop application plaintext authored by a non-member or a
                // Reader.  The sender identity comes from the envelope (the
                // p2panda ratchet only proves key possession, not current
                // membership).
                if matches!(event, Some(GroupEvent::Application { .. })) {
                    let sender = envelope.sender;
                    let members = MessageGroup::members(
                        self.groups
                            .get(group_id)
                            .ok_or(EncryptionError::GroupNotFound(*group_id))?,
                    )
                    .map_err(|e| EncryptionError::Group(Box::new(e)))?;
                    if !members.contains(&sender) {
                        tracing::warn!(
                            "dropping encrypted group message from non-member {sender:?} (leaked-key defense)"
                        );
                        return Ok(None);
                    }
                    let role = self
                        .member_role(group_id, &sender)
                        .unwrap_or(MemberRole::Writer);
                    if !role.can_write() {
                        tracing::warn!(
                            "dropping encrypted group message from {sender:?} with non-writing role {role:?}"
                        );
                        return Ok(None);
                    }
                }

                Ok(event)
            }
            None => Ok(None),
        }
    }

    /// Add a member to an existing encrypted group.
    ///
    /// Only the group owner / an admin should call this.  Returns the control
    /// message to broadcast.  The new member's mirror role defaults to
    /// [`MemberRole::Writer`].
    pub fn add_member(
        &mut self,
        group_id: &GroupId,
        member: PeerId,
    ) -> Result<EncryptedGroupEnvelope, EncryptionError> {
        // ── Role enforcement ─────────────────────────────────────────
        // The actor is the local peer; only an admin may add members.  The
        // p2panda DGM also enforces this inside `Membership::add`, but we
        // fail fast with a clear error here.
        let my_id = self.self_ids.get(group_id).copied().ok_or_else(|| {
            EncryptionError::Internal(format!("no local identity recorded for {group_id:?}"))
        })?;
        let actor_role = self
            .member_role(group_id, &my_id)
            .unwrap_or(MemberRole::Writer);
        if !actor_role.can_manage() {
            return Err(EncryptionError::NotAuthorized(my_id));
        }

        let state = self
            .groups
            .remove(group_id)
            .ok_or(EncryptionError::GroupNotFound(*group_id))?;

        let (state, message) = MessageGroup::add(state, member, &self.rng)
            .map_err(|e| EncryptionError::Group(Box::new(e)))?;

        self.groups.insert(*group_id, state);

        // Keep the mirror in sync: the new member defaults to Writer.
        self.group_roles
            .entry(*group_id)
            .or_default()
            .entry(member)
            .or_insert(MemberRole::Writer);

        // Persist updated state after adding member.
        self.save_current_group_state(group_id);

        Ok(message)
    }

    /// Remove a member from an existing encrypted group.
    ///
    /// Only the group owner / an admin should call this.  Returns the control
    /// message to broadcast.  The removed member's role is dropped from the
    /// mirror so a removed device cannot write even with a leaked key.
    pub fn remove_member(
        &mut self,
        group_id: &GroupId,
        member: PeerId,
    ) -> Result<EncryptedGroupEnvelope, EncryptionError> {
        // ── Role enforcement ─────────────────────────────────────────
        let my_id = self.self_ids.get(group_id).copied().ok_or_else(|| {
            EncryptionError::Internal(format!("no local identity recorded for {group_id:?}"))
        })?;
        let actor_role = self
            .member_role(group_id, &my_id)
            .unwrap_or(MemberRole::Writer);
        if !actor_role.can_manage() {
            return Err(EncryptionError::NotAuthorized(my_id));
        }

        let state = self
            .groups
            .remove(group_id)
            .ok_or(EncryptionError::GroupNotFound(*group_id))?;

        let (state, message) = MessageGroup::remove(state, member, &self.rng)
            .map_err(|e| EncryptionError::Group(Box::new(e)))?;

        self.groups.insert(*group_id, state);

        // Drop the removed member from the role mirror.
        if let Some(roles) = self.group_roles.get_mut(group_id) {
            roles.remove(&member);
        }

        // Persist updated state after removing member.
        self.save_current_group_state(group_id);

        Ok(message)
    }
}

// ── Error type ───────────────────────────────────────────────────────────

/// Errors that can occur during encryption operations.
#[derive(Debug)]
pub enum EncryptionError {
    /// Group not found in encryption state.
    GroupNotFound(GroupId),
    /// Group encryption protocol error.
    Group(Box<dyn std::error::Error + Send>),
    /// Membership operation error.
    Membership(Box<dyn std::error::Error + Send>),
    /// The acting peer is not a member of the group (or has no recorded role).
    NotMember(PeerId),
    /// The acting peer lacks the role required for this operation.
    NotAuthorized(PeerId),
    /// The peer's role does not permit writing messages.
    ForbiddenRole {
        /// The peer whose write was rejected.
        peer: PeerId,
        /// The role that does not permit writing.
        role: MemberRole,
    },
    /// ICE (internal consistency error).
    Internal(String),
}

impl std::fmt::Display for EncryptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncryptionError::GroupNotFound(id) => {
                write!(f, "encryption group not found: {id:?}")
            }
            EncryptionError::Group(e) => write!(f, "group encryption error: {e}"),
            EncryptionError::Membership(e) => write!(f, "membership error: {e}"),
            EncryptionError::NotMember(peer) => write!(f, "peer {peer:?} is not a member"),
            EncryptionError::NotAuthorized(peer) => {
                write!(f, "peer {peer:?} is not authorized for this operation")
            }
            EncryptionError::ForbiddenRole { peer, role } => {
                write!(f, "peer {peer:?} has role {role:?} which cannot write")
            }
            EncryptionError::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for EncryptionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EncryptionError::Group(e) => Some(&**e),
            EncryptionError::Membership(e) => Some(&**e),
            _ => None,
        }
    }
}
