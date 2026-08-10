//! Authorization matrix tests (BORU-AUDIT-28, step 5).
//!
//! Matrix dimensions:
//! - stranger (never a member / never a contact)
//! - current member / current contact
//! - removed member
//! - wrong connected peer (peer is authenticated but not the one bound to the
//!   resource)
//! - stale capability (expired)
//! - replayed capability (same capability re-presented)

use std::sync::Arc;

use boru_core::backfill::BackfillAuthorizer;
use boru_core::chat_core::verify_advertisement;
use boru_core::file_access_protocol::{
    sign_download_descriptor, verify_download_descriptor, BlobFormat, DescriptorVerification,
};
use boru_core::group_events::{GroupEvent, GroupEventPayload, GroupState, GroupValidationError};
use boru_core::short_code::{
    ShortCodeAnnouncement, ShortCodeFreshnessPolicy, SignedShortCodeAnnouncement,
};
use boru_core::storage::{GroupEpochRow, GroupMemberRow, GroupRow, Storage};
use boru_core::tunnel::{TunnelCapability, TunnelId};
use boru_core::TopicId;
use iroh::SecretKey;
use std::time::{Duration, SystemTime};

fn group_id() -> TopicId {
    TopicId::from_bytes([7u8; 32])
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Backfill authorization matrix ─────────────────────────────────────────

/// Setup: a local node with one group that has one active member and one
/// removed member, plus a direct-chat counterpart.
fn setup_authorizer(
    local: &SecretKey,
    active_member: &SecretKey,
    removed_member: &SecretKey,
) -> (Arc<Storage>, BackfillAuthorizer) {
    let storage = Arc::new(Storage::memory().unwrap());
    let group: [u8; 32] = *group_id().as_bytes();

    storage
        .create_group(&GroupRow {
            group_id: group,
            name: "Test Group".into(),
            description: "auth matrix".into(),
            owner_public_key: local.public().as_bytes().to_vec(),
            current_epoch: 1,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
            archived: false,
        })
        .unwrap();

    storage
        .add_group_member(&GroupMemberRow {
            group_id: group,
            public_key: local.public().as_bytes().to_vec(),
            role: "Owner".into(),
            joined_at_ms: now_ms(),
            invited_by: None,
            epoch_joined: 1,
            state: "Active".into(),
        })
        .unwrap();
    storage
        .add_group_member(&GroupMemberRow {
            group_id: group,
            public_key: active_member.public().as_bytes().to_vec(),
            role: "Member".into(),
            joined_at_ms: now_ms(),
            invited_by: Some(local.public().as_bytes().to_vec()),
            epoch_joined: 1,
            state: "Active".into(),
        })
        .unwrap();
    storage
        .add_group_member(&GroupMemberRow {
            group_id: group,
            public_key: removed_member.public().as_bytes().to_vec(),
            role: "Member".into(),
            joined_at_ms: now_ms(),
            invited_by: Some(local.public().as_bytes().to_vec()),
            epoch_joined: 1,
            state: "Removed".into(),
        })
        .unwrap();

    // The authorizer resolves a group by its epoch topic
    // (`find_group_by_topic` queries `group_epochs`), so the topic→group
    // mapping must exist for the group-topic branch to be reached.
    storage
        .create_group_epoch(&GroupEpochRow {
            group_id: group,
            epoch: 1,
            topic_id: group_id(),
            discovery_secret: vec![0u8; 32],
            created_at_ms: now_ms(),
        })
        .unwrap();

    let authorizer = BackfillAuthorizer::new(storage.clone(), local.public());
    (storage, authorizer)
}

#[test]
fn backfill_authorization_matrix() {
    let local = SecretKey::generate();
    let member = SecretKey::generate();
    let removed = SecretKey::generate();
    let stranger = SecretKey::generate();
    let (_storage, authorizer) = setup_authorizer(&local, &member, &removed);

    let group_topic = group_id();

    // 1. Current member → allowed.
    assert!(
        authorizer.authorize(&member.public(), &group_topic),
        "current member must be authorized"
    );
    // 2. Owner (local) → allowed.
    assert!(
        authorizer.authorize(&local.public(), &group_topic),
        "owner must be authorized"
    );
    // 3. Removed member → denied.
    assert!(
        !authorizer.authorize(&removed.public(), &group_topic),
        "removed member must be denied"
    );
    // 4. Stranger → denied.
    assert!(
        !authorizer.authorize(&stranger.public(), &group_topic),
        "stranger must be denied"
    );
    // 5. Wrong connected peer → denied (peer not in this group).
    assert!(
        !authorizer.authorize(&SecretKey::generate().public(), &group_topic),
        "unrelated peer must be denied"
    );

    // 6. Unknown topic == forbidden topic (no information leak).
    let unknown_topic = TopicId::from_bytes([0xEE; 32]);
    assert!(
        !authorizer.authorize(&member.public(), &unknown_topic),
        "unknown topic must be denied"
    );
    // 7. Direct-chat topic: only the two participants are authorized.
    let direct = boru_core::contact::direct_topic(&member.public(), &local.public());
    assert!(
        authorizer.authorize(&member.public(), &direct),
        "direct-chat counterpart must be authorized"
    );
    assert!(
        !authorizer.authorize(&stranger.public(), &direct),
        "stranger must not access a direct topic"
    );
}

/// Removed member cannot serve group history to others (local node still
/// active) and a removed *local* node refuses to serve at all.
#[test]
fn removed_member_denied_both_directions() {
    let local = SecretKey::generate();
    let member = SecretKey::generate();
    let removed = SecretKey::generate();
    let (_storage, authorizer) = setup_authorizer(&local, &member, &removed);

    // A removed member requesting the group topic is denied.
    assert!(!authorizer.authorize(&removed.public(), &group_id()));

    // Even a valid member is denied when the LOCAL node was removed: mark
    // local as removed and re-check (same matrix, flipped local role).
    let storage = Arc::new(Storage::memory().unwrap());
    let group: [u8; 32] = *group_id().as_bytes();
    storage
        .create_group(&GroupRow {
            group_id: group,
            name: "G".into(),
            description: String::new(),
            owner_public_key: vec![0u8; 32],
            current_epoch: 1,
            created_at_ms: 0,
            updated_at_ms: 0,
            archived: false,
        })
        .unwrap();
    storage
        .add_group_member(&GroupMemberRow {
            group_id: group,
            public_key: member.public().as_bytes().to_vec(),
            role: "Member".into(),
            joined_at_ms: 0,
            invited_by: None,
            epoch_joined: 1,
            state: "Active".into(),
        })
        .unwrap();
    storage
        .add_group_member(&GroupMemberRow {
            group_id: group,
            public_key: local.public().as_bytes().to_vec(),
            role: "Member".into(),
            joined_at_ms: 0,
            invited_by: None,
            epoch_joined: 1,
            state: "Removed".into(),
        })
        .unwrap();
    let authorizer = BackfillAuthorizer::new(storage.clone(), local.public());
    assert!(
        !authorizer.authorize(&member.public(), &group_id()),
        "local node removed must refuse serving"
    );
}

// ── Download descriptor matrix ────────────────────────────────────────────

#[test]
fn download_descriptor_authorization_matrix() {
    let owner = SecretKey::generate();
    let requester = SecretKey::generate().public();
    let stranger = SecretKey::generate().public();
    let now = now_ms();
    let descriptor = sign_download_descriptor(
        &owner,
        requester,
        "file-1".into(),
        [0xAB; 32],
        1024,
        BlobFormat::Raw,
        now,
        now + 60_000,
    );

    // Valid owner + requester → valid.
    assert_eq!(
        verify_download_descriptor(&descriptor, &owner.public(), &requester, now + 1_000),
        DescriptorVerification::Valid
    );

    // Wrong connected peer (stranger expected as owner) → owner mismatch.
    assert_eq!(
        verify_download_descriptor(&descriptor, &stranger, &requester, now + 1_000),
        DescriptorVerification::OwnerMismatch
    );

    // Requester mismatch → rejected.
    assert_eq!(
        verify_download_descriptor(&descriptor, &owner.public(), &stranger, now + 1_000),
        DescriptorVerification::RequesterMismatch
    );

    // Stale capability: expired → rejected.
    assert_eq!(
        verify_download_descriptor(&descriptor, &owner.public(), &requester, now + 120_000),
        DescriptorVerification::Expired
    );

    // Replayed capability: re-presenting the same descriptor within its
    // validity window is still valid (expiry-bounded by design); after expiry
    // it is stale.
    assert_eq!(
        verify_download_descriptor(&descriptor, &owner.public(), &requester, now + 10_000),
        DescriptorVerification::Valid
    );

    // Not-yet-valid (issued in the future).
    let future = sign_download_descriptor(
        &owner,
        requester,
        "file-2".into(),
        [0xCD; 32],
        2048,
        BlobFormat::HashSeq,
        now + 30_000,
        now + 90_000,
    );
    assert_eq!(
        verify_download_descriptor(&future, &owner.public(), &requester, now),
        DescriptorVerification::NotYetValid
    );
}

// ── Group event authorization matrix ──────────────────────────────────────

#[test]
fn group_event_authorization_matrix() {
    let owner = SecretKey::generate();
    let member = SecretKey::generate();
    let stranger = SecretKey::generate();
    let group = group_id();
    let mut state = GroupState::new(group, owner.public());

    // Owner invites member.
    let invite = GroupEvent::sign(
        &owner,
        group,
        0,
        GroupEventPayload::MemberInvited {
            member: member.public(),
        },
    )
    .unwrap();
    invite.clone().apply(&mut state).unwrap();

    // Member joins.
    let join = GroupEvent::sign(
        &member,
        group,
        0,
        GroupEventPayload::MemberJoined {
            member: member.public(),
        },
    )
    .unwrap();
    join.apply(&mut state).unwrap();

    // Stranger attempts to change metadata → permission denied / not member.
    let stranger_meta = GroupEvent::sign(
        &stranger,
        group,
        0,
        GroupEventPayload::MetadataChanged {
            name: Some("hijacked".into()),
            description: None,
        },
    )
    .unwrap();
    assert!(
        matches!(
            stranger_meta.verify(&state),
            Err(GroupValidationError::NotMember | GroupValidationError::PermissionDenied)
        ),
        "stranger metadata change must be denied"
    );

    // Stranger attempts to remove the owner → denied.
    let stranger_remove = GroupEvent::sign(
        &stranger,
        group,
        0,
        GroupEventPayload::MemberRemoved {
            member: owner.public(),
        },
    )
    .unwrap();
    assert!(stranger_remove.verify(&state).is_err());

    // Replayed capability: re-presenting the *same* event object (same
    // nonce → same event id) after it was applied must be rejected as a
    // replay.  (Re-signing would mint a fresh nonce and a fresh event id —
    // that is a new event, not a replay.)
    assert!(
        matches!(
            invite.clone().apply(&mut state),
            Err(GroupValidationError::Replay)
        ),
        "replayed event must be rejected"
    );
    assert!(matches!(
        invite.verify(&state),
        Err(GroupValidationError::Replay)
    ));

    // Wrong group: event for another group id → rejected.
    let other_group = TopicId::from_bytes([0x99; 32]);
    let wrong_group = GroupEvent::sign(
        &owner,
        other_group,
        0,
        GroupEventPayload::MetadataChanged {
            name: Some("x".into()),
            description: None,
        },
    )
    .unwrap();
    assert!(matches!(
        wrong_group.verify(&state),
        Err(GroupValidationError::WrongGroup)
    ));
}

// ── Short-code announcement freshness matrix ──────────────────────────────

#[test]
fn short_code_freshness_matrix() {
    let sk = SecretKey::generate();
    let policy = ShortCodeFreshnessPolicy::new(Duration::from_secs(300), Duration::from_secs(60));
    let announcement = ShortCodeAnnouncement {
        code: "CODE1".into(),
        name: "f.pdf".into(),
        ticket: "blob:iroh:t".into(),
        size: 10,
        created_at_ms: 0,
    };
    let bytes = SignedShortCodeAnnouncement::sign(&sk, &announcement).unwrap();

    // Fresh (now) → accepted.
    assert!(
        SignedShortCodeAnnouncement::verify_at(SystemTime::now(), &policy, &bytes, "CODE1").is_ok()
    );

    // Stale capability: 10 minutes old → rejected.
    let stale_now = SystemTime::now() + Duration::from_secs(10 * 60);
    assert!(SignedShortCodeAnnouncement::verify_at(stale_now, &policy, &bytes, "CODE1").is_err());

    // Future beyond skew (claiming to be from the future) → rejected.
    let future_now = SystemTime::now() - Duration::from_secs(10 * 60);
    assert!(SignedShortCodeAnnouncement::verify_at(future_now, &policy, &bytes, "CODE1").is_err());

    // Wrong expected code → rejected.
    assert!(
        SignedShortCodeAnnouncement::verify_at(SystemTime::now(), &policy, &bytes, "WRONG")
            .is_err()
    );
}

// ── Tunnel capability matrix ──────────────────────────────────────────────

#[test]
fn tunnel_capability_matrix() {
    let owner = SecretKey::generate();
    let peer = SecretKey::generate().public();
    let stranger = SecretKey::generate().public();
    let tunnel = TunnelId([0x11; 32]);
    let other_tunnel = TunnelId([0x22; 32]);
    let now = now_ms();
    let cap = TunnelCapability::sign(&owner, peer, tunnel, now, now + 60_000);

    // Valid owner + peer + tunnel + active → ok.
    assert!(cap
        .verify_for(&owner.public(), &peer, tunnel, now + 1_000, true)
        .is_ok());

    // Wrong connected peer (stranger presents) → recipient mismatch.
    assert!(cap
        .verify_for(&owner.public(), &stranger, tunnel, now + 1_000, true)
        .is_err());

    // Stale capability: expired → err.
    assert!(cap
        .verify_for(&owner.public(), &peer, tunnel, now + 120_000, true)
        .is_err());

    // Replayed capability: same token for the wrong tunnel → err.
    assert!(cap
        .verify_for(&owner.public(), &peer, other_tunnel, now + 1_000, true)
        .is_err());

    // Wrong owner expected → owner mismatch.
    assert!(cap
        .verify_for(&stranger, &peer, tunnel, now + 1_000, true)
        .is_err());
}

// ── Room advertisement signature matrix ───────────────────────────────────

#[test]
fn room_advertisement_signature_matrix() {
    use boru_core::chat_core::{sign_advertisement, RoomAdvertisement};
    let author = SecretKey::generate();
    let other = SecretKey::generate();
    let ad = RoomAdvertisement {
        room_name: "Lobby".into(),
        description: "x".into(),
        topic: group_id(),
        ticket: "blob:iroh:t".into(),
        member_count: 3,
        last_activity: now_ms(),
    };
    let sig = sign_advertisement(&ad, &author);

    // Correct author → verifies.
    assert!(verify_advertisement(&ad, &sig, author.public()));

    // Wrong connected peer → fails.
    assert!(!verify_advertisement(&ad, &sig, other.public()));

    // Replayed capability with tampered signature → fails.
    let mut tampered = sig.clone();
    if let Some(b) = tampered.first_mut() {
        *b ^= 0x01;
    }
    assert!(!verify_advertisement(&ad, &tampered, author.public()));

    // Wrong-length signature → fails cleanly (no panic).
    assert!(!verify_advertisement(
        &ad,
        &sig[..sig.len() - 1],
        author.public()
    ));
}
