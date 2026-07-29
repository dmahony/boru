use std::time::{SystemTime, UNIX_EPOCH};

use boru_core::group_events::{
    GroupEvent, GroupEventPayload, GroupState, GroupValidationError, Role,
};
use boru_core::TopicId;
use iroh::SecretKey;

fn group_id() -> TopicId {
    [7u8; 32].into()
}

#[test]
fn owner_invite_is_signed_and_applies_once() {
    let owner = SecretKey::generate();
    let member = SecretKey::generate().public();
    let mut state = GroupState::new(group_id(), owner.public());
    let event = GroupEvent::sign(
        &owner,
        group_id(),
        0,
        GroupEventPayload::MemberInvited { member },
    )
    .unwrap();

    assert_eq!(event.verify(&state).unwrap(), Role::Owner);
    state.apply(event.clone()).unwrap();
    assert_eq!(state.members().len(), 1);
    assert!(matches!(
        state.apply(event),
        Err(GroupValidationError::Replay)
    ));
}

#[test]
fn invited_peer_can_join_and_member_can_leave() {
    let owner = SecretKey::generate();
    let member = SecretKey::generate();
    let mut owner_state = GroupState::new(group_id(), owner.public());
    let invite = GroupEvent::sign(
        &owner,
        group_id(),
        0,
        GroupEventPayload::MemberInvited {
            member: member.public(),
        },
    )
    .unwrap();
    owner_state.apply(invite).unwrap();
    let join = GroupEvent::sign(
        &member,
        group_id(),
        0,
        GroupEventPayload::MemberJoined {
            member: member.public(),
        },
    )
    .unwrap();
    owner_state.apply(join).unwrap();
    let leave = GroupEvent::sign(
        &member,
        group_id(),
        0,
        GroupEventPayload::MemberLeft {
            member: member.public(),
        },
    )
    .unwrap();
    owner_state.apply(leave).unwrap();
    assert!(!owner_state.members().contains_key(&member.public()));
}

#[test]
fn forged_signature_and_unauthorised_metadata_are_rejected() {
    let owner = SecretKey::generate();
    let member_key = SecretKey::generate();
    let mut state = GroupState::new(group_id(), owner.public());
    state.add_member_for_test(member_key.public());
    let event = GroupEvent::sign(
        &member_key,
        group_id(),
        0,
        GroupEventPayload::MetadataChanged {
            name: Some("x".into()),
            description: None,
        },
    )
    .unwrap();
    assert!(matches!(
        event.verify(&state),
        Err(GroupValidationError::PermissionDenied)
    ));
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn normal_member_fixture() -> (SecretKey, SecretKey, GroupState) {
    let owner = SecretKey::generate();
    let member = SecretKey::generate();
    let mut state = GroupState::new(group_id(), owner.public());
    state.add_member_for_test(member.public());
    (owner, member, state)
}

#[test]
fn normal_member_cannot_grant_themselves_owner_or_invite_arbitrary_members() {
    let (_owner, member, state) = normal_member_fixture();
    let attacker = member.public();
    let arbitrary = SecretKey::generate().public();

    let self_join = GroupEvent::sign(
        &member,
        group_id(),
        0,
        GroupEventPayload::MemberJoined { member: attacker },
    )
    .unwrap();
    assert!(matches!(
        self_join.verify(&state),
        Err(GroupValidationError::PermissionDenied) | Err(GroupValidationError::NotMember)
    ));

    let invite = GroupEvent::sign(
        &member,
        group_id(),
        0,
        GroupEventPayload::MemberInvited { member: arbitrary },
    )
    .unwrap();
    assert!(matches!(
        invite.verify(&state),
        Err(GroupValidationError::PermissionDenied)
    ));
}

#[test]
fn normal_member_cannot_remove_owner_or_rename_group() {
    let (owner, member, state) = normal_member_fixture();
    let remove_owner = GroupEvent::sign(
        &member,
        group_id(),
        0,
        GroupEventPayload::MemberRemoved {
            member: owner.public(),
        },
    )
    .unwrap();
    assert!(matches!(
        remove_owner.verify(&state),
        Err(GroupValidationError::PermissionDenied)
    ));

    let rename = GroupEvent::sign(
        &member,
        group_id(),
        0,
        GroupEventPayload::MetadataChanged {
            name: Some("attacker-controlled".into()),
            description: None,
        },
    )
    .unwrap();
    assert!(matches!(
        rename.verify(&state),
        Err(GroupValidationError::PermissionDenied)
    ));
}

#[test]
fn forged_actor_event_is_rejected_even_when_payload_is_authorized() {
    let (owner, _member, state) = normal_member_fixture();
    let invited = SecretKey::generate().public();
    let event = GroupEvent::sign(
        &owner,
        group_id(),
        0,
        GroupEventPayload::MemberInvited { member: invited },
    )
    .unwrap();
    let forged = match event {
        GroupEvent::MemberInvited(mut envelope) => {
            envelope.actor = SecretKey::generate().public();
            GroupEvent::MemberInvited(envelope)
        }
        _ => unreachable!(),
    };
    assert!(matches!(
        forged.verify(&state),
        Err(GroupValidationError::InvalidSignature)
    ));
}

#[test]
fn replayed_invite_and_membership_events_are_rejected() {
    let owner = SecretKey::generate();
    let member = SecretKey::generate();
    let mut state = GroupState::new(group_id(), owner.public());
    let invite = GroupEvent::sign_with_id(
        &owner,
        group_id(),
        [1; 16],
        0,
        now_secs(),
        GroupEventPayload::MemberInvited {
            member: member.public(),
        },
    )
    .unwrap();
    state.apply(invite.clone()).unwrap();
    assert!(matches!(
        state.apply(invite),
        Err(GroupValidationError::Replay)
    ));

    let join = GroupEvent::sign_with_id(
        &member,
        group_id(),
        [2; 16],
        0,
        now_secs(),
        GroupEventPayload::MemberJoined {
            member: member.public(),
        },
    )
    .unwrap();
    state.apply(join.clone()).unwrap();
    assert!(matches!(
        state.apply(join),
        Err(GroupValidationError::Replay)
    ));
}

#[test]
fn epoch_cannot_be_downgraded_or_used_with_old_credentials() {
    let owner = SecretKey::generate();
    let state = GroupState::new(group_id(), owner.public());
    let downgrade = GroupEvent::sign(
        &owner,
        group_id(),
        0,
        GroupEventPayload::EpochChanged { epoch: 0 },
    )
    .unwrap();
    assert!(matches!(
        downgrade.verify(&state),
        Err(GroupValidationError::WrongEpoch { .. })
    ));

    let old_epoch_event = GroupEvent::sign(
        &owner,
        group_id(),
        0,
        GroupEventPayload::MemberInvited {
            member: SecretKey::generate().public(),
        },
    )
    .unwrap();
    let mut advanced = state.clone();
    let rotate = GroupEvent::sign(
        &owner,
        group_id(),
        0,
        GroupEventPayload::EpochChanged { epoch: 1 },
    )
    .unwrap();
    advanced.apply(rotate).unwrap();
    assert!(matches!(
        old_epoch_event.verify(&advanced),
        Err(GroupValidationError::WrongEpoch { .. })
    ));
}

#[test]
fn oversized_control_messages_are_rejected() {
    let owner = SecretKey::generate();
    let state = GroupState::new(group_id(), owner.public());
    let oversized = GroupEvent::sign(
        &owner,
        group_id(),
        0,
        GroupEventPayload::MetadataChanged {
            name: Some("x".repeat(20 * 1024)),
            description: None,
        },
    )
    .unwrap();
    assert!(matches!(
        oversized.verify(&state),
        Err(GroupValidationError::PayloadTooLarge(_))
    ));
}

#[test]
fn removed_member_cannot_authenticate_after_epoch_rotation() {
    let owner = SecretKey::generate();
    let member = SecretKey::generate();
    let mut state = GroupState::new(group_id(), owner.public());
    state.add_member_for_test(member.public());
    let remove = GroupEvent::sign(
        &owner,
        group_id(),
        0,
        GroupEventPayload::MemberRemoved {
            member: member.public(),
        },
    )
    .unwrap();
    state.apply(remove).unwrap();
    let rotate = GroupEvent::sign(
        &owner,
        group_id(),
        0,
        GroupEventPayload::EpochChanged { epoch: 1 },
    )
    .unwrap();
    state.apply(rotate).unwrap();
    let message_credential = GroupEvent::sign(
        &member,
        group_id(),
        1,
        GroupEventPayload::MemberLeft {
            member: member.public(),
        },
    )
    .unwrap();
    assert!(matches!(
        message_credential.verify(&state),
        Err(GroupValidationError::NotMember)
    ));
}

#[test]
fn event_rejects_wrong_group_epoch_and_oversized_payload() {
    let owner = SecretKey::generate();
    let state = GroupState::new(group_id(), owner.public());
    let wrong_group = GroupEvent::sign(
        &owner,
        [8u8; 32].into(),
        0,
        GroupEventPayload::MemberInvited {
            member: SecretKey::generate().public(),
        },
    )
    .unwrap();
    assert!(matches!(
        wrong_group.verify(&state),
        Err(GroupValidationError::WrongGroup)
    ));

    let wrong_epoch = GroupEvent::sign(
        &owner,
        group_id(),
        1,
        GroupEventPayload::MemberInvited {
            member: SecretKey::generate().public(),
        },
    )
    .unwrap();
    assert!(matches!(
        wrong_epoch.verify(&state),
        Err(GroupValidationError::WrongEpoch { .. })
    ));
}
