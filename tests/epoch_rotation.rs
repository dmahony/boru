use std::collections::HashMap;

use boru_core::{
    discovery_secret::DiscoverySecret,
    group_epoch::{EpochCredentials, EpochRotationError, EpochRotationState},
    group_id::GroupId,
    mailbox::MailboxIdentity,
    TopicId,
};
use iroh::SecretKey;

fn group() -> GroupId {
    GroupId::from_bytes([9; 32])
}

#[test]
fn owner_rotation_generates_new_credentials_and_excludes_removed_member() {
    let owner = SecretKey::generate();
    let removed = SecretKey::generate();
    let survivor = SecretKey::generate();
    let mut state = EpochRotationState::new(
        EpochCredentials::from_parts(
            group(),
            1,
            TopicId::from_bytes([1; 32]),
            DiscoverySecret::from_bytes([2; 32]),
        ),
        owner.public(),
    );
    state.add_member(removed.public());
    state.add_member(survivor.public());
    let survivor_identity = MailboxIdentity::from_secret(&survivor);
    let keys = HashMap::from([(survivor.public(), survivor_identity.public_key())]);

    let result = state
        .rotate_after_removal(&owner, removed.public(), &keys)
        .unwrap();
    assert_eq!(result.credentials().group_id(), group());
    assert_eq!(result.credentials().epoch(), 2);
    assert_ne!(result.credentials().topic(), &TopicId::from_bytes([1; 32]));
    assert_ne!(
        result.credentials().secret(),
        &DiscoverySecret::from_bytes([2; 32])
    );
    assert_eq!(result.deliveries().len(), 1);
    assert_eq!(result.deliveries()[0].recipient(), survivor.public());
    assert!(!result
        .deliveries()
        .iter()
        .any(|d| d.recipient() == removed.public()));
    assert!(!state.members().contains(&removed.public()));
    assert_eq!(state.current().epoch(), 2);
    assert!(result.member_removed_event().verify(owner.public()));
    assert!(result.epoch_changed_event().verify(owner.public()));
}

#[test]
fn removed_member_cannot_open_new_credentials_and_survivor_can() {
    let owner = SecretKey::generate();
    let removed = SecretKey::generate();
    let survivor = SecretKey::generate();
    let mut state = EpochRotationState::new(EpochCredentials::generate(group(), 1), owner.public());
    state.add_member(removed.public());
    state.add_member(survivor.public());
    let mut keys = HashMap::new();
    keys.insert(
        survivor.public(),
        MailboxIdentity::from_secret(&survivor).public_key(),
    );
    let result = state
        .rotate_after_removal(&owner, removed.public(), &keys)
        .unwrap();

    assert!(result
        .open_for(&MailboxIdentity::from_secret(&removed))
        .is_err());
    let opened = result
        .open_for(&MailboxIdentity::from_secret(&survivor))
        .unwrap();
    assert_eq!(opened, *result.credentials());
}

#[test]
fn rotation_requires_every_remaining_member_and_is_atomic() {
    let owner = SecretKey::generate();
    let survivor = SecretKey::generate();
    let another = SecretKey::generate();
    let mut state = EpochRotationState::new(EpochCredentials::generate(group(), 4), owner.public());
    state.add_member(survivor.public());
    state.add_member(another.public());
    let err = state
        .rotate_after_removal(&owner, survivor.public(), &HashMap::new())
        .unwrap_err();
    assert_eq!(err, EpochRotationError::MissingRecipient(another.public()));
    assert_eq!(state.current().epoch(), 4);
}
