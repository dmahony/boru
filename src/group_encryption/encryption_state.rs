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
use super::membership::Membership;
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
}

// ── Serialization workaround ──────────────────────────────────────────────
//
// RegistryState cannot be serialized (it holds a SQLite connection handle).
// We replace it with a dummy during serialization and expect the caller to
// re-attach a live connection after deserialization.

impl Serialize for EncryptionState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("EncryptionState", 3)?;
        s.serialize_field("groups", &self.groups)?;
        s.serialize_field("kmg_state", &self.kmg_state)?;
        // RegistryState serializes as unit (no-op).
        s.serialize_field("registry", &self.registry)?;
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
        }
        let helper = EncryptionStateHelper::deserialize(deserializer)?;
        Ok(Self {
            groups: helper.groups,
            kmg_state: helper.kmg_state,
            registry: helper.registry,
            rng: Rng::default(),
            db: None,
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
        let Some(state) = self.groups.get(group_id) else { return };
        let conn = conn.lock().unwrap();
        if let Err(e) = persistence::save_group_state(&conn, group_id, state) {
            tracing::warn!("failed to save group encryption state for {group_id}: {e}");
        }
    }

    /// Load a previously-persisted `GroupEncryptionState` from SQLite.
    ///
    /// Returns `true` if a state was loaded and inserted into `self.groups`,
    /// `false` if no persisted state existed.
    pub fn load_group_state_from_db(&mut self, group_id: &GroupId) -> Result<bool, EncryptionError> {
        let Some(ref conn) = self.db else { return Ok(false) };
        let conn = conn.lock().map_err(|e| {
            EncryptionError::Internal(format!("db lock: {e}"))
        })?;
        match persistence::load_group_state(&conn, group_id) {
            Ok(Some(state)) => {
                self.groups.insert(*group_id, state);
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

    /// Encrypt and send an application message to an existing encrypted group.
    ///
    /// Returns the encrypted envelope that should be broadcast on the gossip
    /// topic.
    pub fn send_message(
        &mut self,
        group_id: &GroupId,
        plaintext: &[u8],
    ) -> Result<EncryptedGroupEnvelope, EncryptionError> {
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
                Ok(event)
            }
            None => Ok(None),
        }
    }

    /// Add a member to an existing encrypted group.
    ///
    /// Only the group owner should call this.  Returns the control message
    /// to broadcast.
    pub fn add_member(
        &mut self,
        group_id: &GroupId,
        member: PeerId,
    ) -> Result<EncryptedGroupEnvelope, EncryptionError> {
        let state = self
            .groups
            .remove(group_id)
            .ok_or(EncryptionError::GroupNotFound(*group_id))?;

        let (state, message) = MessageGroup::add(state, member, &self.rng)
            .map_err(|e| EncryptionError::Group(Box::new(e)))?;

        self.groups.insert(*group_id, state);

        // Persist updated state after adding member.
        self.save_current_group_state(group_id);

        Ok(message)
    }

    /// Remove a member from an existing encrypted group.
    pub fn remove_member(
        &mut self,
        group_id: &GroupId,
        member: PeerId,
    ) -> Result<EncryptedGroupEnvelope, EncryptionError> {
        let state = self
            .groups
            .remove(group_id)
            .ok_or(EncryptionError::GroupNotFound(*group_id))?;

        let (state, message) = MessageGroup::remove(state, member, &self.rng)
            .map_err(|e| EncryptionError::Group(Box::new(e)))?;

        self.groups.insert(*group_id, state);

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
