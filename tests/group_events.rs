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
    let invite = GroupEvent::sign_with_nonce(
        &owner,
        group_id(),
        0,
        now_secs(),
        [1; 16],
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

    let join = GroupEvent::sign_with_nonce(
        &member,
        group_id(),
        0,
        now_secs(),
        [2; 16],
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

fn event_id(event: &GroupEvent) -> [u8; 16] {
    match event {
        GroupEvent::MemberInvited(e)
        | GroupEvent::MemberJoined(e)
        | GroupEvent::MemberLeft(e)
        | GroupEvent::MemberRemoved(e)
        | GroupEvent::MetadataChanged(e)
        | GroupEvent::EpochChanged(e) => *e.event_id.as_ref(),
    }
}

/// BORU-AUDIT-15: identical actions in the same wall-clock second must not
/// collide. This failed on the old derivation, where the event ID was a hash
/// of (actor, group, epoch, timestamp-seconds, payload) and two identical
/// events within one second produced the same ID (making a legitimate event
/// look like a replay).
#[test]
fn same_second_identical_payloads_get_distinct_event_ids() {
    let owner = SecretKey::generate();
    let member = SecretKey::generate().public();
    let timestamp = 1_700_000_000u64; // fixed second for both events
    let payload = GroupEventPayload::MemberInvited { member };

    let a = GroupEvent::sign_at(&owner, group_id(), 0, timestamp, payload.clone()).unwrap();
    let b = GroupEvent::sign_at(&owner, group_id(), 0, timestamp, payload).unwrap();

    assert_ne!(
        event_id(&a),
        event_id(&b),
        "two identical events in the same second must have distinct event IDs"
    );
}

/// BORU-AUDIT-15: the nonce is part of the signed canonical payload, so
/// mutating it invalidates the signature/event-ID relationship. The signature
/// check runs first and must fail; the recomputed event-ID check would also
/// fail if the checks were reordered.
#[test]
fn mutating_nonce_invalidates_signature_and_event_id_relationship() {
    let owner = SecretKey::generate();
    let state = GroupState::new(group_id(), owner.public());
    let event = GroupEvent::sign(
        &owner,
        group_id(),
        0,
        GroupEventPayload::MemberInvited {
            member: SecretKey::generate().public(),
        },
    )
    .unwrap();
    let tampered = match event {
        GroupEvent::MemberInvited(mut envelope) => {
            envelope.nonce[0] ^= 0x80;
            GroupEvent::MemberInvited(envelope)
        }
        _ => unreachable!(),
    };
    assert!(matches!(
        tampered.verify(&state),
        Err(GroupValidationError::InvalidSignature) | Err(GroupValidationError::EventIdMismatch)
    ));
}

/// BORU-AUDIT-15: an exact retransmission of the same serialized event keeps
/// the same event ID and is recognized as a replay, so deduplication still
/// works.
#[test]
fn exact_retransmission_keeps_same_event_id_and_is_replayed() {
    let owner = SecretKey::generate();
    let member = SecretKey::generate();
    let mut state = GroupState::new(group_id(), owner.public());
    let event = GroupEvent::sign(
        &owner,
        group_id(),
        0,
        GroupEventPayload::MemberInvited {
            member: member.public(),
        },
    )
    .unwrap();
    let encoded = event.encode().unwrap();

    let first = GroupEvent::decode(&encoded).unwrap();
    assert_eq!(event_id(&first), event_id(&event), "same serialized event");
    state.apply(first).unwrap();

    // The exact same bytes arrive again (retransmission/replay).
    let second = GroupEvent::decode(&encoded).unwrap();
    assert_eq!(event_id(&second), event_id(&event), "same serialized event");
    assert!(matches!(
        state.apply(second),
        Err(GroupValidationError::Replay)
    ));
}

// ── BORU-AUDIT-16: bound and persist replay tracking ────────────────────────

use std::sync::{Arc, Mutex};

use boru_core::group_replay::ReplayStore;
use rusqlite::Connection;

fn memory_store() -> ReplayStore {
    let conn = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
    ReplayStore::open(conn).unwrap()
}

/// Regression: replay protection must survive a process restart.
///
/// Old behaviour: `GroupState.seen` was a plain in-memory `HashSet`, so after
/// a restart (a fresh `GroupState`) an accepted event could be applied again.
/// New behaviour: with a durable [`ReplayStore`] attached, the marker is
/// persisted and a fresh state rejects the replay.
#[test]
fn replay_after_restart_is_rejected_with_persisted_store() {
    let owner = SecretKey::generate();
    let member = SecretKey::generate().public();
    let store = memory_store();
    let group = group_id();

    // First process lifetime: state with a durable store.
    let mut state = GroupState::with_replay_store(group, owner.public(), store.clone());
    let event = GroupEvent::sign(
        &owner,
        group,
        0,
        GroupEventPayload::MemberInvited { member },
    )
    .unwrap();
    state.apply(event.clone()).unwrap();
    assert_eq!(store.count(&group).unwrap(), 1);

    // Simulated restart: a fresh GroupState sharing the same durable store.
    // The in-memory cache is empty but the persisted marker must still reject
    // the replay.
    let mut restarted = GroupState::with_replay_store(group, owner.public(), store.clone());
    assert_eq!(restarted.replay_cache_len(), 0);
    assert!(matches!(
        restarted.apply(event),
        Err(GroupValidationError::Replay)
    ));
    assert_eq!(
        store.count(&group).unwrap(),
        1,
        "no duplicate marker recorded"
    );
}

/// The persisted store is the authority even when the in-memory cache has
/// evicted entries (memory bound): every accepted marker is durable, so a
/// replay of an evicted event is still rejected via the store.
#[test]
fn verify_consults_persisted_store_after_cache_eviction() {
    let owner = SecretKey::generate();
    let group = group_id();
    let store = memory_store();
    let mut state = GroupState::with_replay_store(group, owner.public(), store.clone());

    // Apply many events so the cache overflows and evicts the oldest entries.
    let volume = boru_core::group_events::REPLAY_MEMORY_CACHE_MAX + 10;
    let mut first_encoded = None;
    for i in 0..volume {
        let event = GroupEvent::sign(
            &owner,
            group,
            0,
            GroupEventPayload::MemberInvited {
                member: SecretKey::generate().public(),
            },
        )
        .unwrap();
        if i == 0 {
            first_encoded = Some(event.encode().unwrap());
        }
        state.apply(event).unwrap();
    }
    assert!(state.replay_cache_len() <= boru_core::group_events::REPLAY_MEMORY_CACHE_MAX);
    // The durable store keeps every marker (authority) even though the hot
    // cache was trimmed.
    assert_eq!(store.count(&group).unwrap(), volume);

    // Replay the very first event. The hot cache was trimmed to
    // REPLAY_MEMORY_CACHE_MAX, so at minimum some markers only exist in the
    // persisted store — and this replay must be rejected regardless of which
    // layer still holds its marker.
    let first = GroupEvent::decode(first_encoded.as_deref().unwrap()).unwrap();
    assert!(matches!(
        state.apply(first),
        Err(GroupValidationError::Replay)
    ));
}

/// Very old prunable epoch entries are removed without affecting active
/// epochs. On epoch rotation, markers older than the window
/// (current − REPLAY_WINDOW_PRIOR_EPOCHS) are pruned from both the cache and
/// the store; the active epoch's markers still reject replays.
#[test]
fn prune_removes_very_old_epochs_without_affecting_active() {
    let owner = SecretKey::generate();
    let group = group_id();
    let store = memory_store();
    let mut state = GroupState::with_replay_store(group, owner.public(), store.clone());

    // Epoch 0: one event.
    let e0 = GroupEvent::sign(
        &owner,
        group,
        0,
        GroupEventPayload::MemberInvited {
            member: SecretKey::generate().public(),
        },
    )
    .unwrap();
    state.apply(e0.clone()).unwrap();

    // Jump straight to epoch 3. The rotation event is authored in epoch 0 and
    // its marker is recorded at epoch 0; after the jump the window is
    // min_epoch = 3 − 2 = 1, so epoch-0 markers (e0 + rotation) are pruned.
    let rotate3 = GroupEvent::sign(
        &owner,
        group,
        0,
        GroupEventPayload::EpochChanged { epoch: 3 },
    )
    .unwrap();
    state.apply(rotate3).unwrap();
    assert_eq!(state.epoch(), 3);
    assert_eq!(
        store.count(&group).unwrap(),
        0,
        "epoch-0 markers pruned after rotation"
    );
    assert!(
        !store.contains(&group, &event_id(&e0)).unwrap(),
        "epoch-0 marker pruned"
    );

    // Active epoch 3: new events are accepted and their replays rejected.
    let e3 = GroupEvent::sign(
        &owner,
        group,
        3,
        GroupEventPayload::MemberInvited {
            member: SecretKey::generate().public(),
        },
    )
    .unwrap();
    state.apply(e3.clone()).unwrap();
    assert_eq!(store.count(&group).unwrap(), 1);
    assert!(matches!(
        state.apply(e3.clone()),
        Err(GroupValidationError::Replay)
    ));

    // Rotate again to epoch 10: min_epoch = 8, so the epoch-3 marker is
    // pruned, and active epoch-10 markers still work.
    let rotate10 = GroupEvent::sign(
        &owner,
        group,
        3,
        GroupEventPayload::EpochChanged { epoch: 10 },
    )
    .unwrap();
    state.apply(rotate10).unwrap();
    assert_eq!(store.count(&group).unwrap(), 0, "epoch-3 markers pruned");

    let e10 = GroupEvent::sign(
        &owner,
        group,
        10,
        GroupEventPayload::MemberInvited {
            member: SecretKey::generate().public(),
        },
    )
    .unwrap();
    state.apply(e10.clone()).unwrap();
    assert_eq!(store.count(&group).unwrap(), 1, "active marker retained");
    assert!(matches!(
        state.apply(e10),
        Err(GroupValidationError::Replay)
    ));
}

/// Large synthetic event volume keeps memory bounded: the in-memory replay
/// cache never exceeds REPLAY_MEMORY_CACHE_MAX even when far more events are
/// accepted, because the oldest entries are evicted (and remain durable).
#[test]
fn large_synthetic_volume_keeps_memory_bounded() {
    let owner = SecretKey::generate();
    let group = group_id();
    let store = memory_store();
    let mut state = GroupState::with_replay_store(group, owner.public(), store.clone());

    let volume = boru_core::group_events::REPLAY_MEMORY_CACHE_MAX * 3;
    for _ in 0..volume {
        let event = GroupEvent::sign(
            &owner,
            group,
            0,
            GroupEventPayload::MemberInvited {
                member: SecretKey::generate().public(),
            },
        )
        .unwrap();
        state.apply(event).unwrap();
    }
    assert!(
        state.replay_cache_len() <= boru_core::group_events::REPLAY_MEMORY_CACHE_MAX,
        "in-memory cache must stay bounded, got {}",
        state.replay_cache_len()
    );
    // The durable store keeps every marker (authority), so replay protection
    // is not lost — only the hot cache is trimmed.
    assert_eq!(store.count(&group).unwrap(), volume);
}

/// Concurrent duplicate arrivals result in one accepted mutation. With a
/// shared store the atomic INSERT OR IGNORE means exactly one caller records
/// the marker; the rest see AlreadySeen/Replay.
#[test]
fn concurrent_duplicate_arrivals_result_in_one_acceptance() {
    use std::thread;

    let owner = SecretKey::generate();
    let member = SecretKey::generate().public();
    let group = group_id();
    let store = memory_store();
    let state = Arc::new(Mutex::new(GroupState::with_replay_store(
        group,
        owner.public(),
        store.clone(),
    )));
    let event = GroupEvent::sign(
        &owner,
        group,
        0,
        GroupEventPayload::MemberInvited { member },
    )
    .unwrap();

    let mut handles = Vec::new();
    for _ in 0..8 {
        let state = state.clone();
        let event = event.clone();
        handles.push(thread::spawn(move || state.lock().unwrap().apply(event)));
    }
    let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    let accepted = outcomes.iter().filter(|r| r.is_ok()).count();
    assert_eq!(accepted, 1, "exactly one concurrent duplicate is accepted");
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| matches!(r, Err(GroupValidationError::Replay)))
            .count(),
        7,
        "the other seven see a replay"
    );
    assert_eq!(store.count(&group).unwrap(), 1, "one durable marker");
}
