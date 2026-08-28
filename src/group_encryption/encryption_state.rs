//! Per-group encryption state and high-level API for managing encrypted groups.
//!
//! [`EncryptionState`](crate::group_encryption::encryption_state::EncryptionState) holds per-group [`GroupState`](crate::group_events::GroupState) instances keyed by
//! [`GroupId`](crate::group_id::GroupId) and provides high-level methods for creating encrypted groups,
//! sending encrypted messages, and processing incoming encrypted messages.
//!
//! This module wires the p2panda-encryption [`MessageGroup`](p2panda_encryption::message_scheme::group::MessageGroup) API into boru's
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
use super::persistence::{self, GroupAuthFault, GroupAuthTxError, GroupStateLoadError};
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
    /// Last committed optimistic-concurrency version per group (BORU-AUDIT-09).
    ///
    /// Mirrors the `version` column of `group_encryption_state`.  Every
    /// transactional membership/role/epoch mutation bumps it; concurrent
    /// mutations from the same base version are rejected by
    /// [`persistence::save_group_state_and_roles`].  This map is NOT part of
    /// the serialized state (it is derived from the DB and reloaded on
    /// [`Self::load_group_state_from_db`]).
    pub group_versions: HashMap<GroupId, u64>,
}

// ── Serialization workaround ──────────────────────────────────────────────
//
// RegistryState cannot be serialized (it holds a SQLite connection handle).
// We replace it with a dummy during serialization and expect the caller to
// re-attach a live connection after deserialization.

impl Serialize for EncryptionState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("EncryptionState", 6)?;
        s.serialize_field("groups", &self.groups)?;
        s.serialize_field("kmg_state", &self.kmg_state)?;
        // RegistryState serializes as unit (no-op).
        s.serialize_field("registry", &self.registry)?;
        s.serialize_field("group_roles", &self.group_roles)?;
        s.serialize_field("self_ids", &self.self_ids)?;
        s.serialize_field("group_versions", &self.group_versions)?;
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
            #[serde(default)]
            group_versions: HashMap<GroupId, u64>,
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
            group_versions: helper.group_versions,
        })
    }
}

/// Outcome of loading a group's encryption state from the database.
///
/// Callers must treat everything except [`Self::Missing`] as fail-closed:
/// fresh initialization is only permitted when no saved state exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupStateLoadOutcome {
    /// A valid state was loaded, validated, and installed.
    Loaded,
    /// No persisted state exists (a genuinely new group). The ONLY outcome
    /// that permits fresh initialization.
    Missing,
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
            group_versions: HashMap::new(),
        })
    }

    /// Attach a SQLite connection for auto-persisting group state.
    pub fn with_db(mut self, conn: Arc<Mutex<Connection>>) -> Self {
        self.db = Some(conn);
        self
    }

    /// Internal helper: persist the state + role mirror for a single group
    /// as ONE logical transaction (BORU-AUDIT-09).
    ///
    /// Runs the optimistic-concurrency version check, writes the role mirror
    /// and the encrypted group state, and commits.  On failure the
    /// transaction rolls back completely and the error propagates — the
    /// caller must not treat the mutation as applied.
    ///
    /// Returns the new committed version.
    fn persist_group_mutation(
        &self,
        group_id: &GroupId,
        state: &GroupEncryptionState,
        roles: &HashMap<PeerId, MemberRole>,
        expected_version: u64,
        fault: Option<GroupAuthFault>,
    ) -> Result<u64, EncryptionError> {
        let Some(ref db) = self.db else {
            // No persistence configured: nothing to commit; the version
            // stays at its in-memory value (0 for never-persisted groups).
            return Ok(expected_version);
        };
        let self_id = self.self_ids.get(group_id).copied();
        let mut conn = db
            .lock()
            .map_err(|_| EncryptionError::Internal("db lock poisoned".into()))?;
        let result = match fault {
            Some(f) => persistence::save_group_state_and_roles_with_fault(
                &mut conn,
                group_id,
                state,
                roles,
                self_id,
                expected_version,
                f,
            ),
            None => persistence::save_group_state_and_roles(
                &mut conn,
                group_id,
                state,
                roles,
                self_id,
                expected_version,
            ),
        };
        result.map_err(EncryptionError::GroupStateWrite)
    }

    /// Persist the current in-memory state + role mirror for a group
    /// atomically (message-plane path: send/receive ratchet advancement).
    ///
    /// This is best-effort durability for message traffic: the ratchet must
    /// advance in memory to continue the protocol, so a persistence failure
    /// is logged rather than failing the send.  The write itself is still
    /// transactional — a partial state/roles commit can never occur.
    fn persist_current_group_state(&mut self, group_id: &GroupId) -> Result<u64, EncryptionError> {
        let Some(state) = self.groups.get(group_id) else {
            return Ok(self.group_versions.get(group_id).copied().unwrap_or(0));
        };
        let roles = self.group_roles.get(group_id).cloned().unwrap_or_default();
        let expected = self.group_versions.get(group_id).copied().unwrap_or(0);
        let new_version = self.persist_group_mutation(group_id, state, &roles, expected, None)?;
        self.group_versions.insert(*group_id, new_version);
        Ok(new_version)
    }

    /// Roll back in-memory state for `group_id` to the last committed state.
    ///
    /// Used by the repository methods when a p2panda op or the persistence
    /// transaction fails: the authoritative state lives in the DB (the failed
    /// transaction rolled back, so the DB still holds the pre-mutation
    /// state).  If no DB is configured, restore the serialised pre-mutation
    /// backup.
    fn rollback_group_state(&mut self, group_id: &GroupId, backup: &[u8]) {
        if self.db.is_some() {
            self.groups.remove(group_id);
            self.group_roles.remove(group_id);
            self.group_versions.remove(group_id);
            let _ = self.load_group_state_from_db(group_id);
            return;
        }
        if let Ok(old) = postcard::from_bytes::<GroupEncryptionState>(backup) {
            self.groups.insert(*group_id, old);
        }
    }

    /// Load a previously-persisted `GroupEncryptionState` from SQLite.
    ///
    /// Returns the load outcome:
    ///
    /// - [`GroupStateLoadOutcome::Loaded`] — a valid state was loaded,
    ///   validated, and inserted into `self.groups`.
    /// - [`GroupStateLoadOutcome::Missing`] — no persisted state exists (a
    ///   genuinely new group). This is the **only** outcome that permits
    ///   fresh initialization.
    ///
    /// A saved state that exists but cannot be decoded or validated fails
    /// closed with [`EncryptionError::GroupStateLoad`] (corruption or an
    /// unsupported format version) — it is never reported as missing, and the
    /// raw record is left untouched for recovery.
    pub fn load_group_state_from_db(
        &mut self,
        group_id: &GroupId,
    ) -> Result<GroupStateLoadOutcome, EncryptionError> {
        let Some(ref conn) = self.db else {
            // No persistence configured: there is nothing saved to load.
            return Ok(GroupStateLoadOutcome::Missing);
        };
        let conn = conn
            .lock()
            .map_err(|e| EncryptionError::Internal(format!("db lock: {e}")))?;

        let state = match persistence::load_group_state(&conn, group_id) {
            Ok(state) => state,
            Err(GroupStateLoadError::Missing) => return Ok(GroupStateLoadOutcome::Missing),
            Err(e) => return Err(EncryptionError::GroupStateLoad(e)),
        };

        // Restore the role mirror + local identity if present. A role mirror
        // that exists but cannot be decoded is ALSO fail-closed corruption.
        let (roles, self_id) = match persistence::load_group_roles(&conn, group_id) {
            Ok(Some(pair)) => pair,
            Ok(None) => (HashMap::new(), None),
            Err(e) => return Err(EncryptionError::GroupStateLoad(e)),
        };

        // Validate decoded invariants BEFORE mutating in-memory state so a
        // bad record cannot leave a half-loaded group behind.
        validate_loaded_group_state(group_id, &state, &roles, self_id.as_ref())?;

        // Restore the optimistic-concurrency version alongside the state so
        // subsequent transactional mutations start from the committed base.
        let version = persistence::load_group_version(&conn, group_id).map_err(|e| {
            EncryptionError::GroupStateLoad(GroupStateLoadError::Io(match e {
                GroupAuthTxError::Io(e) => e,
                GroupAuthTxError::VersionConflict { .. } => {
                    unreachable!("load has no version check")
                }
            }))
        })?;

        self.groups.insert(*group_id, state);
        if !roles.is_empty() {
            self.group_roles.insert(*group_id, roles);
        }
        if let Some(me) = self_id {
            self.self_ids.insert(*group_id, me);
        }
        self.group_versions.insert(*group_id, version);
        Ok(GroupStateLoadOutcome::Loaded)
    }

    /// Remove a group's persisted encryption state from SQLite.
    pub fn delete_group_state_from_db(&mut self, group_id: &GroupId) {
        let Some(ref conn) = self.db else { return };
        let conn = conn.lock().unwrap();
        if let Err(e) = persistence::delete_group_state(&conn, group_id) {
            tracing::warn!("failed to delete group encryption state for {group_id}: {e}");
        }
        if let Err(e) = persistence::delete_group_roles(&conn, group_id) {
            tracing::warn!("failed to delete group role mirror for {group_id}: {e}");
        }
        self.group_versions.remove(group_id);
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

        let (state, message) = MessageGroup::create(state, initial_members, &self.rng)
            .map_err(|e| EncryptionError::Group(Box::new(e)))?;

        // Persist the newly-created group state + role mirror as ONE
        // transaction (BORU-AUDIT-09).  A new group commits from version 0.
        let new_version = self
            .persist_group_mutation(&group_id, &state, &roles, 0, None)
            .inspect_err(|_e| {
                self.group_roles.remove(&group_id);
                self.self_ids.remove(&group_id);
            })?;

        // Save the state and extract the message for broadcast.  In-memory
        // state is installed only after the persistence transaction
        // succeeded, so a failed create leaves no half-initialised group.
        self.groups.insert(group_id, state);
        self.group_roles.insert(group_id, roles);
        self.group_versions.insert(group_id, new_version);

        // The message from create() is an EncryptedGroupEnvelope (via our
        // ForwardSecureOrdering impl).
        Ok(message)
    }

    /// Apply a member-join as one atomic transaction (BORU-AUDIT-09).
    ///
    /// Validates actor authority, computes the new crypto state and role
    /// mirror, then persists BOTH in a single SQLite transaction with an
    /// optimistic-concurrency version check.  In-memory state is only
    /// installed after the transaction commits; on failure the group is
    /// rolled back to the last committed state.
    ///
    /// Returns the control message to broadcast and a domain event for the
    /// UI/cache.
    pub fn apply_member_join(
        &mut self,
        group_id: &GroupId,
        member: PeerId,
    ) -> Result<(EncryptedGroupEnvelope, GroupAuthEvent), EncryptionError> {
        self.apply_member_join_inner(group_id, member, None)
    }

    /// Test-only fault-injecting variant of [`Self::apply_member_join`].
    pub fn apply_member_join_with_fault(
        &mut self,
        group_id: &GroupId,
        member: PeerId,
        fault: GroupAuthFault,
    ) -> Result<(EncryptedGroupEnvelope, GroupAuthEvent), EncryptionError> {
        self.apply_member_join_inner(group_id, member, Some(fault))
    }

    fn apply_member_join_inner(
        &mut self,
        group_id: &GroupId,
        member: PeerId,
        fault: Option<GroupAuthFault>,
    ) -> Result<(EncryptedGroupEnvelope, GroupAuthEvent), EncryptionError> {
        // ── Role enforcement (actor = local peer must be admin) ────────
        let my_id = self.self_ids.get(group_id).copied().ok_or_else(|| {
            EncryptionError::Internal(format!("no local identity recorded for {group_id:?}"))
        })?;
        let actor_role = self
            .member_role(group_id, &my_id)
            .unwrap_or(MemberRole::Writer);
        if !actor_role.can_manage() {
            return Err(EncryptionError::NotAuthorized(my_id));
        }
        // Pre-validate so the consuming p2panda op cannot fail after we have
        // already taken the state out of memory.
        {
            let state = self
                .groups
                .get(group_id)
                .ok_or(EncryptionError::GroupNotFound(*group_id))?;
            let members =
                MessageGroup::members(state).map_err(|e| EncryptionError::Group(Box::new(e)))?;
            if members.contains(&member) {
                return Err(EncryptionError::Group(Box::new(std::io::Error::other(
                    "member is already in the group",
                ))));
            }
            if member == my_id {
                return Err(EncryptionError::Group(Box::new(std::io::Error::other(
                    "cannot add ourselves to the group",
                ))));
            }
        }

        // ── Compute the new crypto state WITHOUT committing to memory ──
        // The p2panda op consumes the old state; keep a serialised backup so
        // a failed transaction can restore it.
        let state = self
            .groups
            .remove(group_id)
            .ok_or(EncryptionError::GroupNotFound(*group_id))?;
        let backup = postcard::to_stdvec(&state)
            .map_err(|e| EncryptionError::Internal(format!("state backup failed: {e}")))?;
        let (new_state, message) = match MessageGroup::add(state, member, &self.rng) {
            Ok(v) => v,
            Err(e) => {
                // The op failed before any persistence: restore the old
                // state so memory does not lose the group.
                self.rollback_group_state(group_id, &backup);
                return Err(EncryptionError::Group(Box::new(e)));
            }
        };

        // New role mirror: the added member defaults to Writer.
        let mut new_roles = self.group_roles.get(group_id).cloned().unwrap_or_default();
        new_roles.insert(member, MemberRole::Writer);

        // ── Persist atomically (roles + crypto state, ONE transaction) ──
        let expected = self.group_versions.get(group_id).copied().unwrap_or(0);
        let new_version =
            match self.persist_group_mutation(group_id, &new_state, &new_roles, expected, fault) {
                Ok(v) => v,
                Err(e) => {
                    // Roll back: reload authoritative state; do NOT keep the new
                    // membership in memory.
                    self.rollback_group_state(group_id, &backup);
                    return Err(e);
                }
            };

        // ── Commit to memory ONLY after the transaction succeeded ──
        self.groups.insert(*group_id, new_state);
        self.group_roles.insert(*group_id, new_roles);
        self.group_versions.insert(*group_id, new_version);

        Ok((
            message,
            GroupAuthEvent::MemberJoined {
                group_id: *group_id,
                member,
                version: new_version,
            },
        ))
    }

    /// Apply a member-removal as one atomic transaction (BORU-AUDIT-09).
    ///
    /// Validates actor authority, computes the new crypto state (which
    /// rotates the epoch keys for surviving members) and drops the removed
    /// member from the role mirror, then persists BOTH in a single SQLite
    /// transaction with an optimistic-concurrency version check.  In-memory
    /// state is only installed after the transaction commits.
    pub fn apply_member_remove(
        &mut self,
        group_id: &GroupId,
        member: PeerId,
    ) -> Result<(EncryptedGroupEnvelope, GroupAuthEvent), EncryptionError> {
        self.apply_member_remove_inner(group_id, member, None)
    }

    /// Test-only fault-injecting variant of [`Self::apply_member_remove`].
    pub fn apply_member_remove_with_fault(
        &mut self,
        group_id: &GroupId,
        member: PeerId,
        fault: GroupAuthFault,
    ) -> Result<(EncryptedGroupEnvelope, GroupAuthEvent), EncryptionError> {
        self.apply_member_remove_inner(group_id, member, Some(fault))
    }

    fn apply_member_remove_inner(
        &mut self,
        group_id: &GroupId,
        member: PeerId,
        fault: Option<GroupAuthFault>,
    ) -> Result<(EncryptedGroupEnvelope, GroupAuthEvent), EncryptionError> {
        // ── Role enforcement (actor = local peer must be admin) ────────
        let my_id = self.self_ids.get(group_id).copied().ok_or_else(|| {
            EncryptionError::Internal(format!("no local identity recorded for {group_id:?}"))
        })?;
        let actor_role = self
            .member_role(group_id, &my_id)
            .unwrap_or(MemberRole::Writer);
        if !actor_role.can_manage() {
            return Err(EncryptionError::NotAuthorized(my_id));
        }
        // Pre-validate so the consuming p2panda op cannot fail after we have
        // already taken the state out of memory.
        {
            let state = self
                .groups
                .get(group_id)
                .ok_or(EncryptionError::GroupNotFound(*group_id))?;
            let members =
                MessageGroup::members(state).map_err(|e| EncryptionError::Group(Box::new(e)))?;
            if !members.contains(&member) {
                return Err(EncryptionError::Group(Box::new(std::io::Error::other(
                    "member is not in the group",
                ))));
            }
            if member == my_id {
                return Err(EncryptionError::Group(Box::new(std::io::Error::other(
                    "cannot remove ourselves with this API",
                ))));
            }
        }

        // ── Compute the new crypto state WITHOUT committing to memory ──
        let state = self
            .groups
            .remove(group_id)
            .ok_or(EncryptionError::GroupNotFound(*group_id))?;
        let backup = postcard::to_stdvec(&state)
            .map_err(|e| EncryptionError::Internal(format!("state backup failed: {e}")))?;
        let (new_state, message) = match MessageGroup::remove(state, member, &self.rng) {
            Ok(v) => v,
            Err(e) => {
                self.rollback_group_state(group_id, &backup);
                return Err(EncryptionError::Group(Box::new(e)));
            }
        };

        // New role mirror: drop the removed member.
        let mut new_roles = self.group_roles.get(group_id).cloned().unwrap_or_default();
        new_roles.remove(&member);

        // ── Persist atomically (roles + crypto state, ONE transaction) ──
        let expected = self.group_versions.get(group_id).copied().unwrap_or(0);
        let new_version =
            match self.persist_group_mutation(group_id, &new_state, &new_roles, expected, fault) {
                Ok(v) => v,
                Err(e) => {
                    self.rollback_group_state(group_id, &backup);
                    return Err(e);
                }
            };

        // ── Commit to memory ONLY after the transaction succeeded ──
        self.groups.insert(*group_id, new_state);
        self.group_roles.insert(*group_id, new_roles);
        self.group_versions.insert(*group_id, new_version);

        Ok((
            message,
            GroupAuthEvent::MemberRemoved {
                group_id: *group_id,
                member,
                version: new_version,
            },
        ))
    }

    /// Apply a role change as one atomic transaction (BORU-AUDIT-09).
    ///
    /// Only an admin may change roles.  The updated role mirror and the
    /// (unchanged) encrypted group state are persisted in a single SQLite
    /// transaction with an optimistic-concurrency version check; the mirror
    /// in memory is only updated after commit.
    pub fn apply_role_change(
        &mut self,
        group_id: &GroupId,
        actor: PeerId,
        member: PeerId,
        role: MemberRole,
    ) -> Result<GroupAuthEvent, EncryptionError> {
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
            .get(group_id)
            .cloned()
            .ok_or(EncryptionError::GroupNotFound(*group_id))?;
        if !roles.contains_key(&member) {
            return Err(EncryptionError::NotMember(member));
        }
        let mut new_roles = roles;
        new_roles.insert(member, role);

        // Persist the updated mirror + the (unchanged) crypto state as ONE
        // transaction.
        let state = self
            .groups
            .get(group_id)
            .ok_or(EncryptionError::GroupNotFound(*group_id))?;
        let expected = self.group_versions.get(group_id).copied().unwrap_or(0);
        let new_version =
            self.persist_group_mutation(group_id, state, &new_roles, expected, None)?;

        self.group_roles.insert(*group_id, new_roles);
        self.group_versions.insert(*group_id, new_version);

        Ok(GroupAuthEvent::RoleChanged {
            group_id: *group_id,
            member,
            role,
            version: new_version,
        })
    }

    /// Rotate the group epoch (fresh key material) as one atomic transaction
    /// (BORU-AUDIT-09).
    ///
    /// Rotates the group secret via the p2panda DCGKA (`MessageGroup::update`)
    /// and persists the new encrypted group state in a single SQLite
    /// transaction with an optimistic-concurrency version check.  Only an
    /// admin may rotate.  In-memory state is installed only after commit.
    ///
    /// Returns the control message to broadcast and a domain event for the
    /// UI/cache.
    pub fn rotate_epoch(
        &mut self,
        group_id: &GroupId,
        actor: PeerId,
    ) -> Result<(EncryptedGroupEnvelope, GroupAuthEvent), EncryptionError> {
        self.rotate_epoch_inner(group_id, actor, None)
    }

    /// Test-only fault-injecting variant of [`Self::rotate_epoch`].
    pub fn rotate_epoch_with_fault(
        &mut self,
        group_id: &GroupId,
        actor: PeerId,
        fault: GroupAuthFault,
    ) -> Result<(EncryptedGroupEnvelope, GroupAuthEvent), EncryptionError> {
        self.rotate_epoch_inner(group_id, actor, Some(fault))
    }

    fn rotate_epoch_inner(
        &mut self,
        group_id: &GroupId,
        actor: PeerId,
        fault: Option<GroupAuthFault>,
    ) -> Result<(EncryptedGroupEnvelope, GroupAuthEvent), EncryptionError> {
        let actor_role = self
            .member_role(group_id, &actor)
            .unwrap_or(MemberRole::Writer);
        if !actor_role.can_manage() {
            return Err(EncryptionError::NotAuthorized(actor));
        }

        // ── Compute the new crypto state WITHOUT committing to memory ──
        let state = self
            .groups
            .remove(group_id)
            .ok_or(EncryptionError::GroupNotFound(*group_id))?;
        let backup = postcard::to_stdvec(&state)
            .map_err(|e| EncryptionError::Internal(format!("state backup failed: {e}")))?;
        let (new_state, message) = match MessageGroup::update(state, &self.rng) {
            Ok(v) => v,
            Err(e) => {
                self.rollback_group_state(group_id, &backup);
                return Err(EncryptionError::Group(Box::new(e)));
            }
        };

        // Roles are unchanged by rotation.
        let roles = self.group_roles.get(group_id).cloned().unwrap_or_default();

        // ── Persist atomically (roles + crypto state, ONE transaction) ──
        let expected = self.group_versions.get(group_id).copied().unwrap_or(0);
        let new_version =
            match self.persist_group_mutation(group_id, &new_state, &roles, expected, fault) {
                Ok(v) => v,
                Err(e) => {
                    self.rollback_group_state(group_id, &backup);
                    return Err(e);
                }
            };

        // ── Commit to memory ONLY after the transaction succeeded ──
        self.groups.insert(*group_id, new_state);
        self.group_roles.insert(*group_id, roles);
        self.group_versions.insert(*group_id, new_version);

        Ok((
            message,
            GroupAuthEvent::EpochRotated {
                group_id: *group_id,
                old_version: expected,
                new_version,
            },
        ))
    }

    /// Set (or change) the role of a group member.
    ///
    /// Only an admin (or the group owner, who is always `Admin`) may change
    /// roles.  The change is applied to the local role mirror used for
    /// per-message enforcement and persisted atomically with the group state.
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
        self.apply_role_change(group_id, actor, member, role)
            .map(|_| ())
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

        // Persist updated state after sending.  The write is transactional
        // (state + role mirror in one commit), so a partial write can never
        // occur; a failure is logged but does not fail the send (the ratchet
        // must advance in memory regardless).
        if let Err(e) = self.persist_current_group_state(group_id) {
            tracing::warn!("failed to persist group state for {group_id} after send: {e}");
        }

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

        // Persist updated state after receiving (transactional, best-effort
        // like the send path).
        if let Err(e) = self.persist_current_group_state(group_id) {
            tracing::warn!("failed to persist group state for {group_id} after receive: {e}");
        }

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
    ///
    /// This delegates to [`Self::apply_member_join`], which owns the atomic
    /// transaction boundary: membership + crypto state commit together.
    pub fn add_member(
        &mut self,
        group_id: &GroupId,
        member: PeerId,
    ) -> Result<EncryptedGroupEnvelope, EncryptionError> {
        self.apply_member_join(group_id, member).map(|(env, _)| env)
    }

    /// Remove a member from an existing encrypted group.
    ///
    /// Only the group owner / an admin should call this.  Returns the control
    /// message to broadcast.  The removed member's role is dropped from the
    /// mirror so a removed device cannot write even with a leaked key.
    ///
    /// This delegates to [`Self::apply_member_remove`], which owns the atomic
    /// transaction boundary: membership + epoch-key rotation commit together.
    pub fn remove_member(
        &mut self,
        group_id: &GroupId,
        member: PeerId,
    ) -> Result<EncryptedGroupEnvelope, EncryptionError> {
        self.apply_member_remove(group_id, member)
            .map(|(env, _)| env)
    }
}

/// Domain event published AFTER a group-auth mutation commits
/// (BORU-AUDIT-09).
///
/// The repository methods ([`EncryptionState::apply_member_join`],
/// [`EncryptionState::apply_member_remove`],
/// [`EncryptionState::apply_role_change`], [`EncryptionState::rotate_epoch`])
/// persist membership/roles + crypto state as one transaction and only then
/// return this event.  UI/cache layers should treat the event as the single
/// "committed" signal — never mutate visible state before the persistence
/// transaction succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupAuthEvent {
    /// A member joined; `version` is the committed optimistic-concurrency
    /// version after the join.
    MemberJoined {
        /// The group that changed.
        group_id: GroupId,
        /// The peer that joined.
        member: PeerId,
        /// Committed state version after this mutation.
        version: u64,
    },
    /// A member was removed (epoch keys rotated for survivors).
    MemberRemoved {
        /// The group that changed.
        group_id: GroupId,
        /// The peer that was removed.
        member: PeerId,
        /// Committed state version after this mutation.
        version: u64,
    },
    /// A member's role changed.
    RoleChanged {
        /// The group that changed.
        group_id: GroupId,
        /// The peer whose role changed.
        member: PeerId,
        /// The new role.
        role: MemberRole,
        /// Committed state version after this mutation.
        version: u64,
    },
    /// The group epoch was rotated (fresh key material).
    EpochRotated {
        /// The group that changed.
        group_id: GroupId,
        /// The committed version before the rotation.
        old_version: u64,
        /// The committed version after the rotation.
        new_version: u64,
    },
}

// ── Error type ───────────────────────────────────────────────────────────

/// Validate that a decoded state and its companion role mirror are
/// self-consistent before they are installed into memory.
///
/// Checks:
///
/// - the p2panda DGM member view is readable and non-empty (a decoded but
///   internally inconsistent membership state is corruption);
/// - the stored local identity (if any) is a member of the group (local
///   identity binding);
/// - every peer in the role mirror is a current member (the mirror is kept in
///   sync with the DGM at every mutation, so a mirror that references a
///   non-member indicates tampering or corruption).
fn validate_loaded_group_state(
    group_id: &GroupId,
    state: &GroupEncryptionState,
    roles: &HashMap<PeerId, MemberRole>,
    self_id: Option<&PeerId>,
) -> Result<(), EncryptionError> {
    let members = MessageGroup::members(state).map_err(|e| {
        EncryptionError::GroupStateLoad(GroupStateLoadError::Corrupt {
            group_id: *group_id,
            reason: format!("decoded state failed membership validation: {e}"),
        })
    })?;
    if members.is_empty() {
        return Err(EncryptionError::GroupStateLoad(
            GroupStateLoadError::Corrupt {
                group_id: *group_id,
                reason: "decoded state has an empty member set".to_string(),
            },
        ));
    }
    if let Some(me) = self_id {
        if !members.contains(me) {
            return Err(EncryptionError::GroupStateLoad(
                GroupStateLoadError::Corrupt {
                    group_id: *group_id,
                    reason: format!(
                        "stored local identity {me:?} is not a member of the decoded group state"
                    ),
                },
            ));
        }
    }
    for peer in roles.keys() {
        if !members.contains(peer) {
            return Err(EncryptionError::GroupStateLoad(
                GroupStateLoadError::Corrupt {
                    group_id: *group_id,
                    reason: format!("role mirror references non-member {peer:?}"),
                },
            ));
        }
    }
    Ok(())
}

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
    /// A saved encryption state exists but cannot be loaded (corruption or an
    /// unsupported format version). The caller must NOT initialize fresh
    /// state in response — surface a recovery action instead.
    GroupStateLoad(GroupStateLoadError),
    /// A transactional membership/role/epoch write failed (BORU-AUDIT-09).
    ///
    /// The underlying transaction rolled back completely; the caller must
    /// treat the mutation as NOT applied and may retry after reloading
    /// authoritative state.
    GroupStateWrite(GroupAuthTxError),
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
            EncryptionError::GroupStateLoad(e) => {
                write!(f, "encrypted group state load error: {e}")
            }
            EncryptionError::GroupStateWrite(e) => {
                write!(f, "group state transaction failed: {e}")
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
            EncryptionError::GroupStateLoad(e) => Some(e),
            EncryptionError::GroupStateWrite(e) => Some(e),
            _ => None,
        }
    }
}
