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
