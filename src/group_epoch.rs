//! Secure epoch rotation for private group conversations.
//!
//! Epoch credentials are deliberately not carried in the signed control event:
//! the event is safe to gossip, while the new topic and discovery secret are
//! delivered through individually encrypted mailbox envelopes to survivors.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};

use iroh::{PublicKey, SecretKey, Signature};
use serde::{Deserialize, Serialize};
use serde_byte_array::ByteArray;
use zeroize::Zeroize;

use crate::{
    discovery_secret::DiscoverySecret,
    group_id::GroupId,
    mailbox::{self, MailboxEnvelope, MailboxIdentity, MailboxPublicKey},
    TopicId,
};

const SIGNATURE_LEN: usize = Signature::LENGTH;

/// Canonical protocol tag for signed group-epoch removal events (BORU-AUDIT-27).
const GROUP_EPOCH_REMOVED_PROTOCOL: &str = "boru/group-epoch-removed";
/// Canonical protocol tag for signed group-epoch change events (BORU-AUDIT-27).
const GROUP_EPOCH_CHANGED_PROTOCOL: &str = "boru/group-epoch-changed";
/// Version of the signed group-epoch event layout (BORU-AUDIT-27).
const GROUP_EPOCH_VERSION: u16 = 1;

/// Credentials used to subscribe to one group epoch.
///
/// The bundle contains the epoch's [`DiscoverySecret`], so it is treated as
/// key material: `Copy` is deliberately NOT implemented (every duplication
/// must be an explicit `clone()`), the secret field zeroizes itself on drop,
/// and the serialized credential payload is scrubbed after use.
///
/// # Not `Copy` (regression)
///
/// ```compile_fail
/// // BORU-AUDIT-17: secret-bearing credentials must not be Copy. If `Copy`
/// // is re-added, this doctest compiles successfully and the suite fails.
/// use boru_core::group_epoch::EpochCredentials;
/// use boru_core::group_id::GroupId;
///
/// fn require_copy<T: Copy>(t: T) -> T { t }
///
/// let creds = EpochCredentials::generate(GroupId::generate(), 0);
/// require_copy(creds);
/// require_copy(creds); // would only compile if EpochCredentials: Copy
/// ```
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct EpochCredentials {
    group_id: GroupId,
    epoch: u64,
    topic: TopicId,
    discovery_secret: DiscoverySecret,
}

impl fmt::Debug for EpochCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EpochCredentials")
            .field("group_id", &self.group_id)
            .field("epoch", &self.epoch)
            .field("topic", &self.topic)
            .field("discovery_secret", &"[redacted]")
            .finish()
    }
}

impl EpochCredentials {
    /// Generate fresh random topic and discovery credentials.
    pub fn generate(group_id: GroupId, epoch: u64) -> Self {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("OS entropy source failed");
        Self::from_parts(
            group_id,
            epoch,
            TopicId::from_bytes(bytes),
            DiscoverySecret::generate(),
        )
    }

    /// Construct credentials, primarily for persistence and deterministic tests.
    pub fn from_parts(
        group_id: GroupId,
        epoch: u64,
        topic: TopicId,
        discovery_secret: DiscoverySecret,
    ) -> Self {
        Self {
            group_id,
            epoch,
            topic,
            discovery_secret,
        }
    }
    /// Stable group identity.
    pub fn group_id(&self) -> GroupId {
        self.group_id
    }
    /// Epoch number.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    /// Gossip topic for this epoch.
    pub fn topic(&self) -> &TopicId {
        &self.topic
    }
    /// Discovery secret for this epoch.
    pub fn secret(&self) -> &DiscoverySecret {
        &self.discovery_secret
    }
}

/// Signed public control event recording member removal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemberRemovedEvent {
    /// Group identity.
    pub group_id: GroupId,
    /// Epoch in which the removal was authorized.
    pub epoch: u64,
    /// Removed identity.
    pub member: PublicKey,
    /// Owner identity.
    pub actor: PublicKey,
    /// Event timestamp.
    pub timestamp: u64,
    /// Owner signature over all other fields.
    pub signature: ByteArray<SIGNATURE_LEN>,
}

/// Signed public control event recording the new epoch topic.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EpochChangedEvent {
    /// Group identity.
    pub group_id: GroupId,
    /// Previous epoch.
    pub old_epoch: u64,
    /// New epoch.
    pub new_epoch: u64,
    /// Owner identity.
    pub actor: PublicKey,
    /// Event timestamp.
    pub timestamp: u64,
    /// Owner signature over all other fields.
    pub signature: ByteArray<SIGNATURE_LEN>,
}

/// A secure, recipient-specific delivery of new epoch credentials.
#[derive(Clone, Debug)]
pub struct CredentialDelivery {
    recipient: PublicKey,
    envelope: MailboxEnvelope,
}
impl CredentialDelivery {
    /// Recipient identity.
    pub fn recipient(&self) -> PublicKey {
        self.recipient
    }
    /// Open after checking the recipient's mailbox identity.
    pub fn open(&self, identity: &MailboxIdentity) -> Result<EpochCredentials, EpochRotationError> {
        // The decrypted bytes are the serialized credential bundle, which
        // contains the epoch's discovery secret.  Scrub the buffer as soon
        // as the credentials have been parsed out of it.
        let mut bytes = identity
            .open(&self.envelope)
            .map_err(|_| EpochRotationError::DecryptionFailed)?;
        let credentials =
            postcard::from_bytes(&bytes).map_err(|e| EpochRotationError::Decode(e.to_string()))?;
        bytes.zeroize();
        Ok(credentials)
    }
}

/// Result of an atomic owner removal and epoch rotation.
#[derive(Clone, Debug)]
pub struct EpochRotation {
    credentials: EpochCredentials,
    member_removed: MemberRemovedEvent,
    epoch_changed: EpochChangedEvent,
    deliveries: Vec<CredentialDelivery>,
}
impl EpochRotation {
    /// New credentials for local owner state.
    pub fn credentials(&self) -> &EpochCredentials {
        &self.credentials
    }
    /// Signed removal event.
    pub fn member_removed_event(&self) -> &MemberRemovedEvent {
        &self.member_removed
    }
    /// Signed epoch event.
    pub fn epoch_changed_event(&self) -> &EpochChangedEvent {
        &self.epoch_changed
    }
    /// Encrypted deliveries, one for each remaining member with a mailbox key.
    pub fn deliveries(&self) -> &[CredentialDelivery] {
        &self.deliveries
    }
    /// Open the delivery addressed to `identity`.
    pub fn open_for(
        &self,
        identity: &MailboxIdentity,
    ) -> Result<EpochCredentials, EpochRotationError> {
        self.deliveries
            .iter()
            .find(|d| d.recipient == identity.public_key().identity)
            .ok_or(EpochRotationError::NotRecipient)
            .and_then(|d| d.open(identity))
    }
}

impl MemberRemovedEvent {
    /// Canonical bytes covered by the owner signature (BORU-AUDIT-27).
    fn signing_bytes(&self) -> Vec<u8> {
        crate::protocol_signing::canonical_signed_bytes(
            GROUP_EPOCH_REMOVED_PROTOCOL,
            GROUP_EPOCH_VERSION,
            &(
                self.group_id,
                self.epoch,
                self.member,
                self.actor,
                self.timestamp,
            ),
        )
        .expect("postcard member-removed signing bytes cannot fail")
    }

    /// Legacy pre-AUDIT-27 signing bytes: bare postcard tuple.
    fn legacy_signing_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(&(
            self.group_id,
            self.epoch,
            self.member,
            self.actor,
            self.timestamp,
        ))
        .expect("postcard member-removed signing bytes cannot fail")
    }

    /// Verify the owner signature without trusting the event contents.
    pub fn verify(&self, owner: PublicKey) -> bool {
        if self.actor != owner {
            return false;
        }
        crate::protocol_signing::verify_canonical_or_legacy(
            &owner,
            self.signature.as_ref(),
            &self.signing_bytes(),
            &self.legacy_signing_bytes(),
        )
    }
}

impl EpochChangedEvent {
    /// Canonical bytes covered by the owner signature (BORU-AUDIT-27).
    fn signing_bytes(&self) -> Vec<u8> {
        crate::protocol_signing::canonical_signed_bytes(
            GROUP_EPOCH_CHANGED_PROTOCOL,
            GROUP_EPOCH_VERSION,
            &(
                self.group_id,
                self.old_epoch,
                self.new_epoch,
                self.actor,
                self.timestamp,
            ),
        )
        .expect("postcard epoch-changed signing bytes cannot fail")
    }

    /// Legacy pre-AUDIT-27 signing bytes: bare postcard tuple.
    fn legacy_signing_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(&(
            self.group_id,
            self.old_epoch,
            self.new_epoch,
            self.actor,
            self.timestamp,
        ))
        .expect("postcard epoch-changed signing bytes cannot fail")
    }

    /// Verify the owner signature without trusting the event contents.
    pub fn verify(&self, owner: PublicKey) -> bool {
        if self.actor != owner {
            return false;
        }
        crate::protocol_signing::verify_canonical_or_legacy(
            &owner,
            self.signature.as_ref(),
            &self.signing_bytes(),
            &self.legacy_signing_bytes(),
        )
    }
}

/// Errors that prevent rotation. State is unchanged on every error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EpochRotationError {
    /// Caller is not the owner.
    NotOwner,
    /// Target was not an active member.
    NotMember,
    /// A survivor has no authenticated encryption key.
    MissingRecipient(PublicKey),
    /// Event signing failed or payload could not be encoded.
    Encode(String),
    /// Mailbox sealing failed.
    EncryptionFailed,
    /// Recipient could not decrypt the envelope.
    DecryptionFailed,
    /// Credential payload was malformed.
    Decode(String),
    /// No envelope was addressed to this identity.
    NotRecipient,
}

/// Authoritative local state for one group's current epoch and membership.
#[derive(Clone, Debug)]
pub struct EpochRotationState {
    current: EpochCredentials,
    owner: PublicKey,
    members: HashSet<PublicKey>,
}
impl EpochRotationState {
    /// Create state with the owner as the sole member.
    pub fn new(current: EpochCredentials, owner: PublicKey) -> Self {
        let mut members = HashSet::new();
        members.insert(owner);
        Self {
            current,
            owner,
            members,
        }
    }
    /// Add a member after the authenticated membership protocol accepts it.
    pub fn add_member(&mut self, member: PublicKey) {
        self.members.insert(member);
    }
    /// Current credentials.
    pub fn current(&self) -> &EpochCredentials {
        &self.current
    }
    /// Active members, including owner.
    pub fn members(&self) -> &HashSet<PublicKey> {
        &self.members
    }

    /// Remove one member and atomically create and distribute the next epoch.
    ///
    /// Recipient keys are looked up before any state mutation. The removed
    /// identity is explicitly excluded even if a stale key is supplied.
    pub fn rotate_after_removal(
        &mut self,
        owner_key: &SecretKey,
        removed: PublicKey,
        recipient_keys: &HashMap<PublicKey, MailboxPublicKey>,
    ) -> Result<EpochRotation, EpochRotationError> {
        if owner_key.public() != self.owner {
            return Err(EpochRotationError::NotOwner);
        }
        if !self.members.contains(&removed) || removed == self.owner {
            return Err(EpochRotationError::NotMember);
        }
        let survivors: Vec<_> = self
            .members
            .iter()
            .copied()
            .filter(|p| *p != removed && *p != self.owner)
            .collect();
        for member in &survivors {
            if !recipient_keys.contains_key(member) {
                return Err(EpochRotationError::MissingRecipient(*member));
            }
        }
        // Borrow the current epoch's public fields instead of copying the
        // whole credentials bundle (which contains the secret).  The secret
        // itself stays in `self.current` until it is replaced by `next`.
        let old_group_id = self.current.group_id();
        let old_epoch = self.current.epoch();
        let next = EpochCredentials::generate(
            old_group_id,
            old_epoch
                .checked_add(1)
                .ok_or_else(|| EpochRotationError::Encode("epoch overflow".into()))?,
        );
        let timestamp = now_secs();
        let member_removed = sign_removed(owner_key, old_group_id, old_epoch, removed, timestamp)?;
        let epoch_changed = sign_epoch(owner_key, old_group_id, old_epoch, next.epoch, timestamp)?;
        // Serialized credentials contain the epoch's discovery secret.
        // Scrub the buffer once every delivery has been sealed.
        let mut payload =
            postcard::to_stdvec(&next).map_err(|e| EpochRotationError::Encode(e.to_string()))?;
        let mut deliveries = Vec::with_capacity(survivors.len());
        for member in survivors {
            let envelope = mailbox::seal_for(owner_key, recipient_keys[&member], &payload)
                .map_err(|_| EpochRotationError::EncryptionFailed)?;
            deliveries.push(CredentialDelivery {
                recipient: member,
                envelope,
            });
        }
        payload.zeroize();
        self.members.remove(&removed);
        // Both the state and the returned rotation own the new credentials;
        // EpochCredentials is not Copy, so the state takes a deliberate clone
        // and the rotation carries the original.
        self.current = next.clone();
        Ok(EpochRotation {
            credentials: next,
            member_removed,
            epoch_changed,
            deliveries,
        })
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn sign_removed(
    key: &SecretKey,
    group_id: GroupId,
    epoch: u64,
    member: PublicKey,
    timestamp: u64,
) -> Result<MemberRemovedEvent, EpochRotationError> {
    let actor = key.public();
    let event = MemberRemovedEvent {
        group_id,
        epoch,
        member,
        actor,
        timestamp,
        signature: ByteArray::new([0u8; SIGNATURE_LEN]),
    };
    let bytes = event.signing_bytes();
    Ok(MemberRemovedEvent {
        signature: ByteArray::new(key.sign(&bytes).to_bytes()),
        ..event
    })
}
fn sign_epoch(
    key: &SecretKey,
    group_id: GroupId,
    old_epoch: u64,
    new_epoch: u64,
    timestamp: u64,
) -> Result<EpochChangedEvent, EpochRotationError> {
    let actor = key.public();
    let event = EpochChangedEvent {
        group_id,
        old_epoch,
        new_epoch,
        actor,
        timestamp,
        signature: ByteArray::new([0u8; SIGNATURE_LEN]),
    };
    let bytes = event.signing_bytes();
    Ok(EpochChangedEvent {
        signature: ByteArray::new(key.sign(&bytes).to_bytes()),
        ..event
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression (BORU-AUDIT-17): `Debug` must never print the discovery
    /// secret bytes — only the redaction marker.
    #[test]
    fn debug_redacts_discovery_secret() {
        let secret = [0xABu8; 32];
        let creds = EpochCredentials::from_parts(
            GroupId::from_bytes([1; 32]),
            7,
            TopicId::from_bytes([2; 32]),
            DiscoverySecret::from_bytes(secret),
        );
        let debug = format!("{creds:?}");
        assert!(
            !debug.contains("abababab"),
            "Debug leaked the full discovery secret: {debug}"
        );
        assert!(
            debug.contains("[redacted]"),
            "expected redaction marker: {debug}"
        );
        assert!(
            debug.contains("epoch: 7"),
            "epoch should be visible: {debug}"
        );
    }

    /// Regression (BORU-AUDIT-17): the credential delivery open path still
    /// parses after the decrypted buffer is zeroized.
    #[test]
    fn credential_delivery_open_roundtrip() {
        let owner = SecretKey::generate();
        let survivor = SecretKey::generate();
        let identity = MailboxIdentity::from_secret(&survivor);
        let creds = EpochCredentials::from_parts(
            GroupId::from_bytes([3; 32]),
            1,
            TopicId::from_bytes([4; 32]),
            DiscoverySecret::from_bytes([5; 32]),
        );
        let payload = postcard::to_stdvec(&creds).unwrap();
        let envelope = mailbox::seal_for(&owner, identity.public_key(), &payload).unwrap();
        let delivery = CredentialDelivery {
            recipient: survivor.public(),
            envelope,
        };
        let opened = delivery.open(&identity).unwrap();
        assert_eq!(opened, creds);
    }

    // ── BORU-AUDIT-27: canonical group-epoch signature framing ─────────────

    /// The canonical bytes a member-removed event signs must be stable:
    /// `boru/group-epoch-removed` domain tag + version + every
    /// security-relevant field.
    #[test]
    fn member_removed_event_canonical_bytes_golden_vector() {
        let owner = SecretKey::generate();
        let removed = SecretKey::generate().public();
        let group = GroupId::from_bytes([7; 32]);
        let event = sign_removed(&owner, group, 3, removed, 1_700_000_000).unwrap();
        let canonical = event.signing_bytes();
        assert_eq!(canonical[0] as usize, GROUP_EPOCH_REMOVED_PROTOCOL.len());
        assert_eq!(
            &canonical[1..1 + GROUP_EPOCH_REMOVED_PROTOCOL.len()],
            GROUP_EPOCH_REMOVED_PROTOCOL.as_bytes()
        );
        assert_eq!(canonical[1 + GROUP_EPOCH_REMOVED_PROTOCOL.len()], 0x01);
        let decoded: (String, u16, GroupId, u64, PublicKey, PublicKey, u64) =
            postcard::from_bytes(&canonical).expect("decode canonical removed bytes");
        assert_eq!(decoded.0, GROUP_EPOCH_REMOVED_PROTOCOL);
        assert_eq!(decoded.1, GROUP_EPOCH_VERSION);
        assert_eq!(decoded.2, event.group_id);
        assert_eq!(decoded.3, event.epoch);
        assert_eq!(decoded.4, event.member);
        assert_eq!(decoded.5, event.actor);
        assert_eq!(decoded.6, event.timestamp);
    }

    /// The canonical bytes an epoch-changed event signs must be stable:
    /// `boru/group-epoch-changed` domain tag + version + every
    /// security-relevant field.
    #[test]
    fn epoch_changed_event_canonical_bytes_golden_vector() {
        let owner = SecretKey::generate();
        let group = GroupId::from_bytes([8; 32]);
        let event = sign_epoch(&owner, group, 3, 4, 1_700_000_000).unwrap();
        let canonical = event.signing_bytes();
        assert_eq!(canonical[0] as usize, GROUP_EPOCH_CHANGED_PROTOCOL.len());
        assert_eq!(
            &canonical[1..1 + GROUP_EPOCH_CHANGED_PROTOCOL.len()],
            GROUP_EPOCH_CHANGED_PROTOCOL.as_bytes()
        );
        assert_eq!(canonical[1 + GROUP_EPOCH_CHANGED_PROTOCOL.len()], 0x01);
        let decoded: (String, u16, GroupId, u64, u64, PublicKey, u64) =
            postcard::from_bytes(&canonical).expect("decode canonical epoch-changed bytes");
        assert_eq!(decoded.0, GROUP_EPOCH_CHANGED_PROTOCOL);
        assert_eq!(decoded.1, GROUP_EPOCH_VERSION);
        assert_eq!(decoded.2, event.group_id);
        assert_eq!(decoded.3, event.old_epoch);
        assert_eq!(decoded.4, event.new_epoch);
        assert_eq!(decoded.5, event.actor);
        assert_eq!(decoded.6, event.timestamp);
    }

    /// Cross-version: pre-AUDIT-27 events signed over the bare tuple (no
    /// domain tag) still verify during the migration window.
    #[test]
    fn group_epoch_legacy_framing_still_verifies() {
        let owner = SecretKey::generate();
        let removed = SecretKey::generate().public();
        let group = GroupId::from_bytes([9; 32]);
        let mut removed_event = sign_removed(&owner, group, 1, removed, 1_700_000_000).unwrap();
        let legacy = postcard::to_stdvec(&(
            removed_event.group_id,
            removed_event.epoch,
            removed_event.member,
            removed_event.actor,
            removed_event.timestamp,
        ))
        .unwrap();
        removed_event.signature = ByteArray::new(owner.sign(&legacy).to_bytes());
        assert!(
            removed_event.verify(owner.public()),
            "legacy-framed member-removed event must verify (BORU-AUDIT-27)"
        );

        let mut epoch_event = sign_epoch(&owner, group, 1, 2, 1_700_000_000).unwrap();
        let legacy = postcard::to_stdvec(&(
            epoch_event.group_id,
            epoch_event.old_epoch,
            epoch_event.new_epoch,
            epoch_event.actor,
            epoch_event.timestamp,
        ))
        .unwrap();
        epoch_event.signature = ByteArray::new(owner.sign(&legacy).to_bytes());
        assert!(
            epoch_event.verify(owner.public()),
            "legacy-framed epoch-changed event must verify (BORU-AUDIT-27)"
        );
    }
}
