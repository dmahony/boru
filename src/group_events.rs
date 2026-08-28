#![allow(clippy::too_many_arguments, clippy::if_same_then_else)]

//! Authenticated, versioned group membership control events.
//!
//! Group events are the authority for membership and permissions. A roster
//! snapshot is only a projection and is never consulted to grant access.
//!
//! # Event identity (BORU-AUDIT-15)
//!
//! Every event carries a fresh 128-bit cryptographic [`nonce`](crate::group_events::GroupEventEnvelope::nonce)
//! generated at signing time. The `event_id` is derived from a
//! domain-separated BLAKE3 hash of the complete canonical signed event
//! contents (version, actor, group, epoch, timestamp, nonce and payload), so
//! event uniqueness never depends on wall-clock seconds: repeated identical
//! actions in the same second still produce distinct events. The nonce is part
//! of the signed payload, and `verify` recomputes the event ID from the signed
//! contents and rejects mismatches, so the ID-to-contents relationship is
//! mandatory rather than advisory.
//!
//! All current event classes use the nonce-based constructor
//! ([`GroupEvent::sign`](crate::group_events::GroupEvent::sign)); there is no deterministic-ID event class in the
//! protocol today. If one is ever introduced it must be documented separately
//! and must not reuse the generic event-ID constructor.
//!
//! # Replay tracking (BORU-AUDIT-16)
//!
//! Accepted event IDs are recorded in a bounded, durable replay store
//! ([`crate::group_replay::ReplayStore`]) when one is attached. The in-memory
//! `seen` map is a capped hot cache layered over that persisted state, never
//! the sole authority: replay protection survives restart, memory stays
//! bounded by [`REPLAY_MEMORY_CACHE_MAX`](crate::group_events::REPLAY_MEMORY_CACHE_MAX), and markers older than
//! [`REPLAY_WINDOW_PRIOR_EPOCHS`](crate::group_events::REPLAY_WINDOW_PRIOR_EPOCHS) prior epochs are pruned on rotation and
//! startup.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use iroh::{PublicKey, SecretKey, Signature};
use serde::{Deserialize, Serialize};
use serde_byte_array::ByteArray;

use crate::group_replay::{RecordOutcome, ReplayStore};
use crate::TopicId;

/// Current group-control protocol version.
///
/// Bumped from 1 to 2 by BORU-AUDIT-15: the envelope gained a per-event
/// nonce field, and the event ID derivation switched to a domain-separated
/// hash over the complete canonical signed event.
pub const GROUP_EVENT_VERSION: u8 = 2;
/// Maximum encoded payload size accepted by validators.
pub const MAX_GROUP_EVENT_PAYLOAD: usize = 16 * 1024;
/// Maximum permitted clock skew for a received event.
pub const MAX_GROUP_EVENT_CLOCK_SKEW_SECS: u64 = 24 * 60 * 60;
/// Number of prior epochs whose replay markers are retained (BORU-AUDIT-16).
///
/// The replay window is *current epoch plus [`REPLAY_WINDOW_PRIOR_EPOCHS`]
/// prior epochs*. Markers older than the window are pruned from both the
/// in-memory cache and the persisted store on epoch rotation and startup.
/// `verify` already rejects any event whose epoch differs from the current
/// one, so pruning old epochs never re-opens acceptance for the current
/// epoch; the window exists so a node that missed the last rotation still
/// rejects replays of events from the immediately preceding epochs.
pub const REPLAY_WINDOW_PRIOR_EPOCHS: u64 = 2;
/// Hard cap on the in-memory replay cache per [`GroupState`] (BORU-AUDIT-16).
///
/// When the cache exceeds this many entries the oldest (smallest-epoch)
/// entries are evicted. Eviction is safe because the persisted
/// [`ReplayStore`] remains the authority; the cache is only a hot fast-path
/// over durable state.
pub const REPLAY_MEMORY_CACHE_MAX: usize = 4096;
const EVENT_ID_LEN: usize = 16;
/// Length of the per-event cryptographic nonce (128 bits).
const NONCE_LEN: usize = 16;
/// Domain-separation tag prefixed to every event-ID preimage so IDs from
/// this protocol can never collide with hashes from other protocol objects.
const EVENT_ID_DOMAIN: &[u8] = b"boru.group-event.id";

/// Canonical protocol tag for the signed group-event envelope (BORU-AUDIT-27).
const GROUP_EVENT_PROTOCOL: &str = "boru/group-event";

/// The role an authenticated actor has in a group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// The group owner.
    Owner,
    /// A regular member.
    Member,
}

/// Group-control operations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GroupEventPayload {
    /// Owner invites a member.
    MemberInvited {
        /// Invited member identity.
        member: PublicKey,
    },
    /// An invited peer joins the group.
    MemberJoined {
        /// Joining member identity.
        member: PublicKey,
    },
    /// A member leaves voluntarily.
    MemberLeft {
        /// Leaving member identity.
        member: PublicKey,
    },
    /// Owner removes a member.
    MemberRemoved {
        /// Removed member identity.
        member: PublicKey,
    },
    /// Owner changes room metadata.
    MetadataChanged {
        /// Optional new room name.
        name: Option<String>,
        /// Optional new room description.
        description: Option<String>,
    },
    /// Owner advances the group epoch.
    EpochChanged {
        /// New epoch, strictly greater than the current epoch.
        epoch: u64,
    },
}

/// The signed envelope shared by every [`GroupEvent`] variant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GroupEventEnvelope {
    /// Wire protocol version.
    pub version: u8,
    /// Identity of the group, normally its gossip topic.
    pub group_id: TopicId,
    /// Unique event identifier used for replay protection.
    ///
    /// Derived from a domain-separated BLAKE3 hash of the complete canonical
    /// signed event contents (including [`nonce`](Self::nonce)), so identical
    /// actions in the same wall-clock second still yield distinct IDs.
    pub event_id: ByteArray<EVENT_ID_LEN>,
    /// Fresh per-event cryptographic nonce (128 bits, random at signing time).
    ///
    /// Guarantees event uniqueness independent of wall-clock seconds and is
    /// covered by [`signature`](Self::signature), so mutating it invalidates
    /// the event.
    pub nonce: ByteArray<NONCE_LEN>,
    /// Group epoch in which the event was authored.
    pub epoch: u64,
    /// Authenticated actor.
    pub actor: PublicKey,
    /// UNIX timestamp in seconds.
    pub timestamp: u64,
    /// Operation payload.
    pub payload: GroupEventPayload,
    /// Signature over all fields except this signature.
    pub signature: ByteArray<{ Signature::LENGTH }>,
}

/// An authenticated group-control event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GroupEvent {
    /// Invite a member.
    MemberInvited(GroupEventEnvelope),
    /// Accept an invitation.
    MemberJoined(GroupEventEnvelope),
    /// Leave the group.
    MemberLeft(GroupEventEnvelope),
    /// Remove a member.
    MemberRemoved(GroupEventEnvelope),
    /// Change group metadata.
    MetadataChanged(GroupEventEnvelope),
    /// Advance the group epoch.
    EpochChanged(GroupEventEnvelope),
}

/// Validation failures for group events.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GroupValidationError {
    /// Envelope could not be decoded.
    Decode(String),
    /// Unsupported wire version.
    UnsupportedVersion(u8),
    /// Signature is invalid.
    InvalidSignature,
    /// Event targets another group.
    WrongGroup,
    /// Event is not in the current epoch.
    WrongEpoch {
        /// Current local epoch.
        expected: u64,
        /// Epoch carried by the event.
        actual: u64,
    },
    /// Actor is not a member or invited member.
    NotMember,
    /// Actor cannot perform this operation.
    PermissionDenied,
    /// Event ID was already applied.
    Replay,
    /// Event ID does not match the canonical hash of the event contents.
    EventIdMismatch,
    /// Cryptographic randomness for the per-event nonce was unavailable.
    NonceGeneration,
    /// The persisted replay store could not be consulted (fail closed).
    ReplayStore(String),
    /// Timestamp is outside the accepted clock window.
    TimestampOutOfRange,
    /// Encoded payload exceeds the protocol limit.
    PayloadTooLarge(usize),
    /// Event payload does not match its enum variant.
    PayloadMismatch,
}

impl GroupEvent {
    /// Create and sign a new event using the current wall-clock timestamp.
    ///
    /// A fresh 128-bit cryptographic nonce is generated per event, so repeated
    /// identical actions within the same second still produce distinct event
    /// IDs (BORU-AUDIT-15).
    pub fn sign(
        secret: &SecretKey,
        group_id: TopicId,
        epoch: u64,
        payload: GroupEventPayload,
    ) -> Result<Self, GroupValidationError> {
        let timestamp = now_secs();
        Self::sign_at(secret, group_id, epoch, timestamp, payload)
    }

    /// Create and sign an event with an explicit wall-clock timestamp.
    ///
    /// The nonce is still freshly random per event. Exists so callers and
    /// tests can pin the timestamp without pinning event identity.
    pub fn sign_at(
        secret: &SecretKey,
        group_id: TopicId,
        epoch: u64,
        timestamp: u64,
        payload: GroupEventPayload,
    ) -> Result<Self, GroupValidationError> {
        let nonce = random_nonce()?;
        Self::sign_with_nonce(secret, group_id, epoch, timestamp, nonce, payload)
    }

    /// Create and sign an event with an explicit nonce.
    ///
    /// The `event_id` is derived deterministically from the complete canonical
    /// event contents, including `nonce`. This is the deterministic
    /// constructor used by tests and golden vectors; production callers should
    /// use [`Self::sign`] so the nonce is freshly random.
    pub fn sign_with_nonce(
        secret: &SecretKey,
        group_id: TopicId,
        epoch: u64,
        timestamp: u64,
        nonce: [u8; NONCE_LEN],
        payload: GroupEventPayload,
    ) -> Result<Self, GroupValidationError> {
        let actor = secret.public();
        let nonce_bytes = ByteArray::new(nonce);
        let event_id = make_event_id(
            GROUP_EVENT_VERSION,
            actor,
            group_id,
            epoch,
            timestamp,
            &nonce_bytes,
            &payload,
        )?;
        let event_id_bytes = ByteArray::new(event_id);
        let unsigned = unsigned_bytes(
            GROUP_EVENT_VERSION,
            &group_id,
            &event_id_bytes,
            &nonce_bytes,
            epoch,
            &actor,
            timestamp,
            &payload,
        )?;
        let signature = ByteArray::new(secret.sign(&unsigned).to_bytes());
        let envelope = GroupEventEnvelope {
            version: GROUP_EVENT_VERSION,
            group_id,
            event_id: event_id_bytes,
            nonce: nonce_bytes,
            epoch,
            actor,
            timestamp,
            payload: payload.clone(),
            signature,
        };
        Ok(Self::from_payload(envelope, &payload))
    }

    /// Decode a postcard-encoded event.
    pub fn decode(bytes: &[u8]) -> Result<Self, GroupValidationError> {
        postcard::from_bytes(bytes).map_err(|e| GroupValidationError::Decode(e.to_string()))
    }

    /// Encode this event for transport.
    pub fn encode(&self) -> Result<Vec<u8>, GroupValidationError> {
        postcard::to_stdvec(self).map_err(|e| GroupValidationError::Decode(e.to_string()))
    }

    /// Validate authentication, authority, identity, epoch, replay, time, and size.
    pub fn verify(&self, state: &GroupState) -> Result<Role, GroupValidationError> {
        let envelope = self.envelope();
        if envelope.version != GROUP_EVENT_VERSION {
            return Err(GroupValidationError::UnsupportedVersion(envelope.version));
        }
        if envelope.group_id != state.group_id {
            return Err(GroupValidationError::WrongGroup);
        }
        if envelope.epoch != state.epoch {
            return Err(GroupValidationError::WrongEpoch {
                expected: state.epoch,
                actual: envelope.epoch,
            });
        }
        let variant_matches = matches!(
            (self, &envelope.payload),
            (
                Self::MemberInvited(_),
                GroupEventPayload::MemberInvited { .. }
            ) | (
                Self::MemberJoined(_),
                GroupEventPayload::MemberJoined { .. }
            ) | (Self::MemberLeft(_), GroupEventPayload::MemberLeft { .. })
                | (
                    Self::MemberRemoved(_),
                    GroupEventPayload::MemberRemoved { .. }
                )
                | (
                    Self::MetadataChanged(_),
                    GroupEventPayload::MetadataChanged { .. }
                )
                | (
                    Self::EpochChanged(_),
                    GroupEventPayload::EpochChanged { .. }
                )
        );
        if !variant_matches {
            return Err(GroupValidationError::PayloadMismatch);
        }
        if let GroupEventPayload::EpochChanged { epoch } = &envelope.payload {
            if *epoch <= state.epoch {
                return Err(GroupValidationError::WrongEpoch {
                    expected: state.epoch + 1,
                    actual: *epoch,
                });
            }
        }
        let encoded_payload = postcard::to_stdvec(&envelope.payload)
            .map_err(|e| GroupValidationError::Decode(e.to_string()))?;
        if encoded_payload.len() > MAX_GROUP_EVENT_PAYLOAD {
            return Err(GroupValidationError::PayloadTooLarge(encoded_payload.len()));
        }
        if now_secs().abs_diff(envelope.timestamp) > MAX_GROUP_EVENT_CLOCK_SKEW_SECS {
            return Err(GroupValidationError::TimestampOutOfRange);
        }
        let unsigned = unsigned_bytes(
            envelope.version,
            &envelope.group_id,
            &envelope.event_id,
            &envelope.nonce,
            envelope.epoch,
            &envelope.actor,
            envelope.timestamp,
            &envelope.payload,
        )?;
        let legacy = legacy_unsigned_bytes(
            envelope.version,
            &envelope.group_id,
            &envelope.event_id,
            &envelope.nonce,
            envelope.epoch,
            &envelope.actor,
            envelope.timestamp,
            &envelope.payload,
        )?;
        if !crate::protocol_signing::verify_canonical_or_legacy(
            &envelope.actor,
            envelope.signature.as_ref(),
            &unsigned,
            &legacy,
        ) {
            return Err(GroupValidationError::InvalidSignature);
        }
        // The event ID must be exactly the domain-separated hash of the
        // complete canonical event contents (including the nonce). Recomputing
        // it here makes the ID-to-contents relationship a mandatory validation
        // step: a nonce or payload mutation breaks either this check or the
        // signature above (BORU-AUDIT-15).
        let expected_id = make_event_id(
            envelope.version,
            envelope.actor,
            envelope.group_id,
            envelope.epoch,
            envelope.timestamp,
            &envelope.nonce,
            &envelope.payload,
        )?;
        let signed_id: &[u8; EVENT_ID_LEN] = envelope.event_id.as_ref();
        if expected_id != *signed_id {
            return Err(GroupValidationError::EventIdMismatch);
        }
        let event_id: &[u8; EVENT_ID_LEN] = envelope.event_id.as_ref();
        if state.seen.contains_key(event_id) {
            return Err(GroupValidationError::Replay);
        }
        // Persisted authority (BORU-AUDIT-16): if a store is attached it is
        // consulted in addition to the in-memory cache. Any store error is
        // fail-closed — the event is rejected because we cannot prove it is
        // fresh.
        if let Some(store) = &state.replay_store {
            let persisted = store
                .contains(&state.group_id, event_id)
                .map_err(|e| GroupValidationError::ReplayStore(e.to_string()))?;
            if persisted {
                return Err(GroupValidationError::Replay);
            }
        }
        let role = if envelope.actor == state.owner {
            Role::Owner
        } else if state.members.contains_key(&envelope.actor) {
            Role::Member
        } else if matches!(
            &envelope.payload,
            GroupEventPayload::MemberJoined { member } if *member == envelope.actor
        ) && state.invited.contains(&envelope.actor)
        {
            Role::Member
        } else {
            return Err(GroupValidationError::NotMember);
        };
        let allowed = match &envelope.payload {
            GroupEventPayload::MemberInvited { .. }
            | GroupEventPayload::MemberRemoved { .. }
            | GroupEventPayload::MetadataChanged { .. }
            | GroupEventPayload::EpochChanged { .. } => role == Role::Owner,
            GroupEventPayload::MemberLeft { member } => {
                role == Role::Member && *member == envelope.actor
            }
            GroupEventPayload::MemberJoined { member } => {
                *member == envelope.actor && state.invited.contains(member)
            }
        };
        if !allowed {
            return Err(GroupValidationError::PermissionDenied);
        }
        Ok(role)
    }

    /// Validate and apply the event atomically to the local authoritative state.
    pub fn apply_to(self, state: &mut GroupState) -> Result<(), GroupValidationError> {
        self.verify(state)?;
        let envelope = self.envelope().clone();
        // Persist the replay marker BEFORE mutating state (BORU-AUDIT-16).
        // `INSERT OR IGNORE` is atomic, so a concurrent duplicate arriving
        // between `verify` and here cannot both be accepted: one caller gets
        // `Recorded`, the other `AlreadySeen` and is rejected. Recording
        // first also means a store failure aborts the mutation (fail closed).
        if let Some(store) = &state.replay_store {
            match store
                .record(
                    &state.group_id,
                    envelope.epoch,
                    *envelope.event_id.as_ref(),
                    now_secs(),
                )
                .map_err(|e| GroupValidationError::ReplayStore(e.to_string()))?
            {
                RecordOutcome::Recorded => {}
                RecordOutcome::AlreadySeen => return Err(GroupValidationError::Replay),
            }
        }
        let advanced_epoch = matches!(envelope.payload, GroupEventPayload::EpochChanged { .. });
        match envelope.payload {
            GroupEventPayload::MemberInvited { member } => {
                state.invited.insert(member);
            }
            GroupEventPayload::MemberJoined { member } => {
                state.invited.remove(&member);
                state.members.insert(member, Role::Member);
            }
            GroupEventPayload::MemberLeft { member }
            | GroupEventPayload::MemberRemoved { member } => {
                state.members.remove(&member);
            }
            GroupEventPayload::MetadataChanged { .. } => {}
            GroupEventPayload::EpochChanged { epoch } => {
                state.epoch = epoch;
            }
        }
        state
            .seen
            .insert(*envelope.event_id.as_ref(), envelope.epoch);
        state.cap_seen_cache();
        if advanced_epoch {
            state.prune_replay();
        }
        Ok(())
    }

    /// Alias for [`Self::apply_to`].
    pub fn apply(self, state: &mut GroupState) -> Result<(), GroupValidationError> {
        self.apply_to(state)
    }

    fn envelope(&self) -> &GroupEventEnvelope {
        match self {
            Self::MemberInvited(e)
            | Self::MemberJoined(e)
            | Self::MemberLeft(e)
            | Self::MemberRemoved(e)
            | Self::MetadataChanged(e)
            | Self::EpochChanged(e) => e,
        }
    }
    fn from_payload(e: GroupEventEnvelope, p: &GroupEventPayload) -> Self {
        match p {
            GroupEventPayload::MemberInvited { .. } => Self::MemberInvited(e),
            GroupEventPayload::MemberJoined { .. } => Self::MemberJoined(e),
            GroupEventPayload::MemberLeft { .. } => Self::MemberLeft(e),
            GroupEventPayload::MemberRemoved { .. } => Self::MemberRemoved(e),
            GroupEventPayload::MetadataChanged { .. } => Self::MetadataChanged(e),
            GroupEventPayload::EpochChanged { .. } => Self::EpochChanged(e),
        }
    }
}

/// Authoritative membership state used to validate and apply events.
#[derive(Clone, Debug)]
pub struct GroupState {
    group_id: TopicId,
    owner: PublicKey,
    epoch: u64,
    members: HashMap<PublicKey, Role>,
    invited: HashSet<PublicKey>,
    /// Bounded in-memory replay cache: event ID → epoch of acceptance.
    ///
    /// This is an optimization over the persisted [`ReplayStore`] (when one
    /// is attached), never the sole authority. Capped at
    /// [`REPLAY_MEMORY_CACHE_MAX`] and pruned to the replay window on epoch
    /// rotation (BORU-AUDIT-16).
    seen: HashMap<[u8; EVENT_ID_LEN], u64>,
    /// Optional durable replay-marker store (BORU-AUDIT-16).
    ///
    /// When present, every accepted event is recorded here atomically and
    /// replay checks consult it, so replay protection survives restart. When
    /// absent, replay protection is in-memory only (bounded, ephemeral).
    replay_store: Option<ReplayStore>,
}

impl GroupState {
    /// Create a state with an owner. The owner is implicitly a member.
    ///
    /// Replay protection is in-memory only; use
    /// [`Self::with_replay_store`] to make it durable across restarts.
    pub fn new(group_id: TopicId, owner: PublicKey) -> Self {
        let mut members = HashMap::new();
        members.insert(owner, Role::Owner);
        Self {
            group_id,
            owner,
            epoch: 0,
            members,
            invited: HashSet::new(),
            seen: HashMap::new(),
            replay_store: None,
        }
    }

    /// Create a state with a durable replay store attached.
    ///
    /// Accepted event IDs are persisted in `store` and consulted on every
    /// replay check, so replay protection survives process restart
    /// (BORU-AUDIT-16). The store must be opened over a connection that
    /// outlives this state (typically an app-owned `Arc<Mutex<Connection>>`).
    pub fn with_replay_store(group_id: TopicId, owner: PublicKey, store: ReplayStore) -> Self {
        let mut state = Self::new(group_id, owner);
        state.replay_store = Some(store);
        state
    }

    /// Attach (or replace) the durable replay store.
    ///
    /// Existing in-memory markers are not copied into the store; the store is
    /// authoritative going forward. Safe to call at startup with a store that
    /// already contains markers from a previous process lifetime.
    pub fn attach_replay_store(&mut self, store: ReplayStore) {
        self.replay_store = Some(store);
    }

    /// Current member set (including owner).
    pub fn members(&self) -> &HashMap<PublicKey, Role> {
        &self.members
    }
    /// Validate and apply an authenticated event to this state.
    pub fn apply(&mut self, event: GroupEvent) -> Result<(), GroupValidationError> {
        event.apply_to(self)
    }

    /// Current epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Number of entries currently in the in-memory replay cache.
    ///
    /// Useful for observing the [`REPLAY_MEMORY_CACHE_MAX`] bound in tests.
    pub fn replay_cache_len(&self) -> usize {
        self.seen.len()
    }

    /// Prune replay markers older than the retention window.
    ///
    /// The window is *current epoch plus [`REPLAY_WINDOW_PRIOR_EPOCHS`] prior
    /// epochs*. Both the in-memory cache and the persisted store (when
    /// attached) are pruned; the store is pruned in bounded batches
    /// (BORU-AUDIT-16). Called automatically on epoch rotation, and available
    /// for startup cleanup after a state is rebuilt from a persisted log.
    pub fn prune_replay(&mut self) {
        let min_epoch = self.epoch.saturating_sub(REPLAY_WINDOW_PRIOR_EPOCHS);
        self.seen.retain(|_, epoch| *epoch >= min_epoch);
        if let Some(store) = &self.replay_store {
            if let Err(e) = store.prune_older_than(&self.group_id, min_epoch) {
                tracing::warn!(
                    "failed to prune group replay store for {}: {e}",
                    self.group_id
                );
            }
        }
    }

    /// Enforce the in-memory replay cache bound by evicting the oldest
    /// (smallest-epoch) entries first.
    fn cap_seen_cache(&mut self) {
        if self.seen.len() <= REPLAY_MEMORY_CACHE_MAX {
            return;
        }
        let mut by_epoch: Vec<_> = self.seen.iter().map(|(id, e)| (*e, *id)).collect();
        by_epoch.sort_unstable();
        let excess = self.seen.len() - REPLAY_MEMORY_CACHE_MAX;
        for (_, id) in by_epoch.into_iter().take(excess) {
            self.seen.remove(&id);
        }
    }

    /// Test-only fixture helper; production state must come from events.
    #[doc(hidden)]
    pub fn add_member_for_test(&mut self, member: PublicKey) {
        self.members.insert(member, Role::Member);
    }
}

fn unsigned_bytes(
    version: u8,
    group_id: &TopicId,
    event_id: &ByteArray<EVENT_ID_LEN>,
    nonce: &ByteArray<NONCE_LEN>,
    epoch: u64,
    actor: &PublicKey,
    timestamp: u64,
    payload: &GroupEventPayload,
) -> Result<Vec<u8>, GroupValidationError> {
    // BORU-AUDIT-27: canonical framing — domain tag + version + every
    // security-relevant field (identity, routing, epoch, nonce, freshness).
    crate::protocol_signing::canonical_signed_bytes(
        GROUP_EVENT_PROTOCOL,
        version as u16,
        &(group_id, event_id, nonce, epoch, actor, timestamp, payload),
    )
    .map_err(|e| GroupValidationError::Decode(e.to_string()))
}

/// Legacy pre-AUDIT-27 unsigned bytes: bare postcard tuple without a domain
/// separator.  Kept only for the migration window; new events are signed with
/// the canonical framing.
fn legacy_unsigned_bytes(
    version: u8,
    group_id: &TopicId,
    event_id: &ByteArray<EVENT_ID_LEN>,
    nonce: &ByteArray<NONCE_LEN>,
    epoch: u64,
    actor: &PublicKey,
    timestamp: u64,
    payload: &GroupEventPayload,
) -> Result<Vec<u8>, GroupValidationError> {
    postcard::to_stdvec(&(
        version, group_id, event_id, nonce, epoch, actor, timestamp, payload,
    ))
    .map_err(|e| GroupValidationError::Decode(e.to_string()))
}
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
/// Generate a fresh 128-bit cryptographic nonce for a new event.
fn random_nonce() -> Result<[u8; NONCE_LEN], GroupValidationError> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|_| GroupValidationError::NonceGeneration)?;
    Ok(nonce)
}
/// Derive the event ID from the complete canonical signed event contents.
///
/// The preimage is the [`EVENT_ID_DOMAIN`] tag followed by the postcard
/// encoding of `(version, actor, group_id, epoch, timestamp, nonce, payload)`
/// — every field except `event_id` itself and the signature. The timestamp is
/// deliberately included for ordering/freshness only; uniqueness comes from
/// the random nonce, so two identical actions in the same second still hash
/// to distinct IDs (BORU-AUDIT-15).
fn make_event_id(
    version: u8,
    actor: PublicKey,
    group_id: TopicId,
    epoch: u64,
    timestamp: u64,
    nonce: &ByteArray<NONCE_LEN>,
    payload: &GroupEventPayload,
) -> Result<[u8; EVENT_ID_LEN], GroupValidationError> {
    let body = postcard::to_stdvec(&(version, actor, group_id, epoch, timestamp, nonce, payload))
        .map_err(|e| GroupValidationError::Decode(e.to_string()))?;
    let mut preimage = Vec::with_capacity(EVENT_ID_DOMAIN.len() + body.len());
    preimage.extend_from_slice(EVENT_ID_DOMAIN);
    preimage.extend_from_slice(&body);
    let hash = blake3::hash(&preimage);
    let mut id = [0u8; EVENT_ID_LEN];
    id.copy_from_slice(&hash.as_bytes()[..EVENT_ID_LEN]);
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex::ToHex;

    fn golden_key(seed: u8) -> SecretKey {
        SecretKey::from_bytes(&[seed; 32])
    }

    /// BORU-AUDIT-15 golden vector: event ID derivation must be stable.
    ///
    /// Fixed inputs (owner key `0x42*32`, member key `0x22*32`, group
    /// `0x07*32`, epoch 0, timestamp 1_700_000_000, nonce `0xAB*16`,
    /// `MemberInvited`) must produce the exact canonical preimage and event
    /// ID below. The preimage is the domain tag `boru.group-event.id`
    /// followed by the postcard encoding of
    /// `(version, actor, group_id, epoch, timestamp, nonce, payload)`
    /// (tuples and fixed arrays are length-prefix-free in postcard; the
    /// nonce `ByteArray` uses `serialize_bytes`, so it carries a varint
    /// length). If this test fails, someone changed the derivation without
    /// bumping [`GROUP_EVENT_VERSION`].
    #[test]
    fn event_id_golden_vector() {
        let owner = golden_key(0x42);
        let member = golden_key(0x22).public();
        let group: TopicId = [7u8; 32].into();
        let nonce = [0xABu8; 16];
        let timestamp = 1_700_000_000u64;
        let payload = GroupEventPayload::MemberInvited { member };
        let event =
            GroupEvent::sign_with_nonce(&owner, group, 0, timestamp, nonce, payload.clone())
                .unwrap();
        let envelope = event.envelope();

        // Independently reconstruct the canonical preimage: domain tag plus
        // the postcard encoding of every signed content field.
        let body = postcard::to_stdvec(&(
            GROUP_EVENT_VERSION,
            envelope.actor,
            envelope.group_id,
            envelope.epoch,
            envelope.timestamp,
            &envelope.nonce,
            &payload,
        ))
        .unwrap();
        let mut preimage = Vec::new();
        preimage.extend_from_slice(EVENT_ID_DOMAIN);
        preimage.extend_from_slice(&body);

        // The event ID must be exactly BLAKE3(domain || body) truncated to
        // the first 16 bytes.
        let hash = blake3::hash(&preimage);
        let expected_id: &[u8; EVENT_ID_LEN] = envelope.event_id.as_ref();
        assert_eq!(
            expected_id,
            &hash.as_bytes()[..EVENT_ID_LEN],
            "event ID must be the domain-separated hash of the signed contents"
        );

        // Golden constants pin the exact derivation (verified independently
        // with Python blake3 over the reconstructed postcard preimage).
        let golden_preimage = "\
            62 6f 72 75 2e 67 72 6f 75 70 2d 65 76 65 6e 74 2e 69 64 \
            02 \
            21 52 f8 d1 9b 79 1d 24 45 32 42 e1 5f 2e ab 6c b7 cf fa 7b 6a 5e d3 00 97 96 0e 06 98 81 db 12 \
            07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 07 \
            00 \
            80 e2 cf aa 06 \
            10 ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab \
            00 \
            a0 9a a5 f4 7a 67 59 80 2f f9 55 f8 dc 2d 2a 14 a5 c9 9d 23 be 97 f8 64 12 7f f9 38 34 55 a4 f0";
        let golden_preimage_bytes: Vec<u8> = golden_preimage
            .split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).expect("hex byte"))
            .collect();
        assert_eq!(
            preimage, golden_preimage_bytes,
            "event ID preimage must be stable (BORU-AUDIT-15)"
        );
        assert_eq!(
            expected_id.encode_hex::<String>(),
            "0a20c18d01f858ffb8dfbfadce1720fe",
            "event ID derivation must be stable (BORU-AUDIT-15)"
        );
    }

    // ── BORU-AUDIT-27: canonical group-event signature framing ─────────────

    /// The canonical bytes a group event signs must be stable: the
    /// `boru/group-event` domain tag, the version, then every
    /// security-relevant field.
    #[test]
    fn group_event_canonical_bytes_golden_vector() {
        let owner = golden_key(0x42);
        let member = golden_key(0x22).public();
        let group: TopicId = [7u8; 32].into();
        let nonce = [0xABu8; 16];
        let payload = GroupEventPayload::MemberInvited { member };
        let event =
            GroupEvent::sign_with_nonce(&owner, group, 0, 1_700_000_000, nonce, payload).unwrap();
        let envelope = event.envelope();

        let unsigned = unsigned_bytes(
            envelope.version,
            &envelope.group_id,
            &envelope.event_id,
            &envelope.nonce,
            envelope.epoch,
            &envelope.actor,
            envelope.timestamp,
            &envelope.payload,
        )
        .unwrap();
        assert_eq!(unsigned[0] as usize, GROUP_EVENT_PROTOCOL.len());
        assert_eq!(
            &unsigned[1..1 + GROUP_EVENT_PROTOCOL.len()],
            GROUP_EVENT_PROTOCOL.as_bytes()
        );
        assert_eq!(unsigned[1 + GROUP_EVENT_PROTOCOL.len()], 0x02); // GROUP_EVENT_VERSION

        // The canonical bytes must round-trip to the same signed fields.
        let decoded: (
            String,
            u16,
            TopicId,
            ByteArray<EVENT_ID_LEN>,
            ByteArray<NONCE_LEN>,
            u64,
            PublicKey,
            u64,
            GroupEventPayload,
        ) = postcard::from_bytes(&unsigned).expect("decode canonical group event bytes");
        assert_eq!(decoded.0, GROUP_EVENT_PROTOCOL);
        assert_eq!(decoded.1, envelope.version as u16);
        assert_eq!(decoded.2, envelope.group_id);
        assert_eq!(decoded.3, envelope.event_id);
        assert_eq!(decoded.4, envelope.nonce);
        assert_eq!(decoded.5, envelope.epoch);
        assert_eq!(decoded.6, envelope.actor);
        assert_eq!(decoded.7, envelope.timestamp);
        assert_eq!(decoded.8, envelope.payload);
    }

    /// Cross-version: a pre-AUDIT-27 event signed over the bare tuple (no
    /// domain tag) still verifies during the migration window.
    #[test]
    fn group_event_legacy_framing_still_verifies() {
        let owner = golden_key(0x42);
        let member = golden_key(0x22).public();
        let group: TopicId = [7u8; 32].into();
        let nonce = [0xABu8; 16];
        let payload = GroupEventPayload::MemberInvited { member };
        let timestamp = now_secs();
        let event =
            GroupEvent::sign_with_nonce(&owner, group, 0, timestamp, nonce, payload.clone())
                .unwrap();
        let mut envelope = event.envelope().clone();
        // Re-sign with the legacy pre-AUDIT-27 bytes (bare tuple).
        let legacy = legacy_unsigned_bytes(
            envelope.version,
            &envelope.group_id,
            &envelope.event_id,
            &envelope.nonce,
            envelope.epoch,
            &envelope.actor,
            envelope.timestamp,
            &envelope.payload,
        )
        .unwrap();
        envelope.signature = ByteArray::new(owner.sign(&legacy).to_bytes());
        let state = GroupState::new(group, owner.public());
        let legacy_event = GroupEvent::from_payload(envelope, &payload);
        assert!(
            legacy_event.verify(&state).is_ok(),
            "legacy-framed group event must verify during migration (BORU-AUDIT-27)"
        );
    }

    /// Cross-version: an unknown version value is rejected even when the
    /// signature itself is valid, because verification never guesses at
    /// interpretation (BORU-AUDIT-27 step 1: unknown-version rejection).
    #[test]
    fn group_event_unknown_version_is_rejected() {
        let owner = golden_key(0x42);
        let member = golden_key(0x22).public();
        let group: TopicId = [7u8; 32].into();
        let nonce = [0xABu8; 16];
        let payload = GroupEventPayload::MemberInvited { member };
        let event =
            GroupEvent::sign_with_nonce(&owner, group, 0, now_secs(), nonce, payload.clone())
                .unwrap();
        let mut envelope = event.envelope().clone();
        // Bump the version and re-sign with the canonical bytes for that
        // version: the signature is valid, yet verification must still
        // reject the object because its interpretation is unknown.
        envelope.version = GROUP_EVENT_VERSION + 1;
        let bytes = unsigned_bytes(
            envelope.version,
            &envelope.group_id,
            &envelope.event_id,
            &envelope.nonce,
            envelope.epoch,
            &envelope.actor,
            envelope.timestamp,
            &envelope.payload,
        )
        .unwrap();
        envelope.signature = ByteArray::new(owner.sign(&bytes).to_bytes());
        let state = GroupState::new(group, owner.public());
        let future_event = GroupEvent::from_payload(envelope, &payload);
        assert_eq!(
            future_event.verify(&state),
            Err(GroupValidationError::UnsupportedVersion(
                GROUP_EVENT_VERSION + 1
            )),
            "unknown group-event version must be rejected (BORU-AUDIT-27)"
        );
    }
}
