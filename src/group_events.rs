//! Authenticated, versioned group membership control events.
//!
//! Group events are the authority for membership and permissions. A roster
//! snapshot is only a projection and is never consulted to grant access.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use iroh::{PublicKey, SecretKey, Signature};
use serde::{Deserialize, Serialize};
use serde_byte_array::ByteArray;

use crate::TopicId;

/// Current group-control protocol version.
pub const GROUP_EVENT_VERSION: u8 = 1;
/// Maximum encoded payload size accepted by validators.
pub const MAX_GROUP_EVENT_PAYLOAD: usize = 16 * 1024;
/// Maximum permitted clock skew for a received event.
pub const MAX_GROUP_EVENT_CLOCK_SKEW_SECS: u64 = 24 * 60 * 60;
const EVENT_ID_LEN: usize = 16;

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
    pub event_id: ByteArray<EVENT_ID_LEN>,
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
    /// Timestamp is outside the accepted clock window.
    TimestampOutOfRange,
    /// Encoded payload exceeds the protocol limit.
    PayloadTooLarge(usize),
    /// Event payload does not match its enum variant.
    PayloadMismatch,
}

impl GroupEvent {
    /// Create and sign a new event using the current wall-clock timestamp.
    pub fn sign(
        secret: &SecretKey,
        group_id: TopicId,
        epoch: u64,
        payload: GroupEventPayload,
    ) -> Result<Self, GroupValidationError> {
        let timestamp = now_secs();
        let event_id = make_event_id(secret.public(), group_id, epoch, timestamp, &payload);
        Self::sign_with_id(secret, group_id, event_id, epoch, timestamp, payload)
    }

    /// Create and sign an event with explicit timestamp and event ID.
    pub fn sign_with_id(
        secret: &SecretKey,
        group_id: TopicId,
        event_id: [u8; EVENT_ID_LEN],
        epoch: u64,
        timestamp: u64,
        payload: GroupEventPayload,
    ) -> Result<Self, GroupValidationError> {
        let actor = secret.public();
        let event_id_bytes = ByteArray::new(event_id);
        let unsigned = unsigned_bytes(
            GROUP_EVENT_VERSION,
            &group_id,
            &event_id_bytes,
            epoch,
            &actor,
            timestamp,
            &payload,
        )?;
        let signature = ByteArray::new(secret.sign(&unsigned).to_bytes());
        let envelope = GroupEventEnvelope {
            version: GROUP_EVENT_VERSION,
            group_id,
            event_id: ByteArray::new(event_id),
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
            envelope.epoch,
            &envelope.actor,
            envelope.timestamp,
            &envelope.payload,
        )?;
        envelope
            .actor
            .verify(&unsigned, &Signature::from_bytes(&envelope.signature))
            .map_err(|_| GroupValidationError::InvalidSignature)?;
        let event_id: &[u8; EVENT_ID_LEN] = envelope.event_id.as_ref();
        if state.seen.contains(event_id) {
            return Err(GroupValidationError::Replay);
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
        state.seen.insert(*envelope.event_id.as_ref());
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
    seen: HashSet<[u8; EVENT_ID_LEN]>,
}

impl GroupState {
    /// Create a state with an owner. The owner is implicitly a member.
    pub fn new(group_id: TopicId, owner: PublicKey) -> Self {
        let mut members = HashMap::new();
        members.insert(owner, Role::Owner);
        Self {
            group_id,
            owner,
            epoch: 0,
            members,
            invited: HashSet::new(),
            seen: HashSet::new(),
        }
    }
    /// Current member set (including owner).
    pub fn members(&self) -> &HashMap<PublicKey, Role> {
        &self.members
    }
    /// Validate and apply an authenticated event to this state.
    pub fn apply(&mut self, event: GroupEvent) -> Result<(), GroupValidationError> {
        event.apply_to(self)
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
    epoch: u64,
    actor: &PublicKey,
    timestamp: u64,
    payload: &GroupEventPayload,
) -> Result<Vec<u8>, GroupValidationError> {
    postcard::to_stdvec(&(
        version, group_id, event_id, epoch, actor, timestamp, payload,
    ))
    .map_err(|e| GroupValidationError::Decode(e.to_string()))
}
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn make_event_id(
    actor: PublicKey,
    group_id: TopicId,
    epoch: u64,
    timestamp: u64,
    payload: &GroupEventPayload,
) -> [u8; EVENT_ID_LEN] {
    let bytes =
        postcard::to_stdvec(&(actor, group_id, epoch, timestamp, payload)).unwrap_or_default();
    let hash = blake3::hash(&bytes);
    let mut id = [0u8; EVENT_ID_LEN];
    id.copy_from_slice(&hash.as_bytes()[..EVENT_ID_LEN]);
    id
}
