//! Integration tests for stable identities and contact visibility.
//!
//! These scenarios use the shared two-peer fixture fabric to verify:
//!
//! **Identity stability** — each peer retains its expected cryptographic
//! identity across restarts and repeated catalogue observations.
//!
//! **Contact visibility** — contacts only appear when visibility rules
//! permit them.  Changing the visibility relationship updates the
//! observable catalogue without stale entries.
//!
//! All scenarios exercise both peers symmetrically and reset fixture
//! state between cases (each test creates a fresh fixture).

use std::time::Duration;

// Include the shared two-peer fixture (reuses the same deterministic
// identity setup, local relay, and gossip infrastructure).
#[path = "test_fixture.rs"]
mod fixture;

use fixture::{PeerId, TwoPeerFixture};

use boru_core::{
    catalogue_client::{fetch_remote_catalogue, RemoteCatalogueFetchError},
    catalogue_model::SignedFileCatalogue,
    friends::{FriendId, FriendRecord, FriendRelationship},
};
use n0_future::time::sleep;

// ── Helpers ────────────────────────────────────────────────────────────────

/// Fetch a remote peer's catalogue through the requester's endpoint.
async fn fetch_catalogue(
    fx: &TwoPeerFixture,
    requester: PeerId,
    server: PeerId,
) -> Result<SignedFileCatalogue, RemoteCatalogueFetchError> {
    let ep = fx
        .peer(requester)
        .endpoint
        .as_ref()
        .expect("requester endpoint must be available");
    let server_pk = fx.peer(server).public_key;
    fetch_remote_catalogue(ep, server_pk, None).await
}

/// Assert that a catalogue fetch succeeds and returns the expected number of
/// files.  Returns the catalogue for further inspection.
async fn assert_catalogue_file_count(
    fx: &TwoPeerFixture,
    requester: PeerId,
    server: PeerId,
    expected_count: usize,
) -> SignedFileCatalogue {
    let cat = fetch_catalogue(fx, requester, server)
        .await
        .unwrap_or_else(|e| {
            panic!(
                "{:?} should be able to fetch {:?}'s catalogue: {e}",
                requester, server
            )
        });
    assert_eq!(
        cat.files.len(),
        expected_count,
        "{:?} sees {} files in {:?}'s catalogue, expected {expected_count}",
        requester,
        cat.files.len(),
        server,
    );
    // The owner_id in the signed catalogue must match the server's identity.
    assert_eq!(
        cat.owner_id,
        fx.peer(server).public_key,
        "catalogue owner_id must match {:?}'s public key",
        server,
    );
    cat
}

/// Assert that a catalogue fetch returns PermissionDenied.
async fn assert_permission_denied(fx: &TwoPeerFixture, requester: PeerId, server: PeerId) {
    let result = fetch_catalogue(fx, requester, server).await;
    match result {
        Err(RemoteCatalogueFetchError::PermissionDenied) => {
            // Expected — blocked peer
        }
        other => {
            panic!(
                "{:?} fetching {:?}'s catalogue should receive PermissionDenied, got: {other:?}",
                requester, server
            );
        }
    }
}

/// Assert that a catalogue fetch succeeds with zero files (empty catalogue).
async fn assert_catalogue_empty(fx: &TwoPeerFixture, requester: PeerId, server: PeerId) {
    let cat = fetch_catalogue(fx, requester, server)
        .await
        .unwrap_or_else(|e| {
            panic!(
                "{:?} fetching {:?}'s catalogue should succeed with 0 files: {e}",
                requester, server
            )
        });
    assert_eq!(
        cat.files.len(),
        0,
        "{:?} should see an empty catalogue from {:?}",
        requester,
        server,
    );
}

/// Set the relationship from `owner` toward `other` to a specific state
/// and restart the owner so the change takes effect in the running handler.
async fn set_relationship(
    fx: &mut TwoPeerFixture,
    owner: PeerId,
    other: PeerId,
    relationship: FriendRelationship,
) {
    let other_pk = fx.peer(other).public_key;
    let p = fx.peer_mut(owner);
    let record = FriendRecord {
        relationship,
        ..FriendRecord::default()
    };
    p.friends
        .upsert(FriendId::from_public_key(other_pk), record);
    if p.is_running() {
        fx.restart_peer(owner)
            .await
            .expect("restart owner after relationship change");
    }
}

/// Remove a peer from the friends store entirely (no record at all).
async fn remove_contact(fx: &mut TwoPeerFixture, owner: PeerId, other: PeerId) {
    let other_pk = fx.peer(other).public_key;
    let p = fx.peer_mut(owner);
    p.friends.remove(&FriendId::from_public_key(other_pk));
    if p.is_running() {
        fx.restart_peer(owner)
            .await
            .expect("restart owner after contact removal");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Identity Stability
// ═══════════════════════════════════════════════════════════════════════════

/// Each peer retains its expected identity after a full restart.
///
/// Bob fetches Alice's catalogue before and after Alice restarts.
/// The owner_id in the signed catalogue must match Alice's public key
/// both times, proving the identity survived the restart.
#[tokio::test]
async fn identity_preserved_across_peer_restart() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Make Bob a friend of Alice so Bob can see files.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;

    // Seed a file so Alice's catalogue is non-empty.
    fx.add_file(PeerId::Alice, "id-restart-hash", "restart-test.txt")
        .unwrap();

    // Bob fetches Alice's catalogue before restart.
    let cat_before = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    let alice_pk_before = cat_before.owner_id;

    // Restart Alice.
    fx.restart_peer(PeerId::Alice).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    // Bob fetches Alice's catalogue after restart.
    let cat_after = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    let alice_pk_after = cat_after.owner_id;

    assert_eq!(
        alice_pk_before, alice_pk_after,
        "Alice's public key must be identical across restart"
    );
    assert!(
        cat_after.verify().is_ok(),
        "catalogue signature must verify after restart"
    );

    // Symmetric: Alice fetches Bob's catalogue across restart.
    set_relationship(
        &mut fx,
        PeerId::Bob,
        PeerId::Alice,
        FriendRelationship::Friends,
    )
    .await;

    fx.add_file(PeerId::Bob, "id-restart-hash-bob", "bob-restart.txt")
        .unwrap();
    let bob_cat_before = assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;
    let bob_pk_before = bob_cat_before.owner_id;

    fx.restart_peer(PeerId::Bob).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    let bob_cat_after = assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;
    let bob_pk_after = bob_cat_after.owner_id;

    assert_eq!(
        bob_pk_before, bob_pk_after,
        "Bob's public key must be identical across restart"
    );

    fx.shutdown().await;
}

/// Repeated catalogue observations return the same identity.
///
/// Bob fetches Alice's catalogue multiple times and confirms the
/// owner_id never changes, even as the revision may advance.
#[tokio::test]
async fn identity_stable_across_repeated_observations() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Make Bob a friend of Alice so Bob can see files.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;

    fx.add_file(PeerId::Alice, "id-stable-hash", "stable.txt")
        .unwrap();

    let first = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    let alice_pk = first.owner_id;

    // Fetch again — same identity.
    let second = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    assert_eq!(
        second.owner_id, alice_pk,
        "identity stable across 2nd fetch"
    );

    // Add another file (bumps revision) and fetch again.
    fx.add_file(PeerId::Alice, "id-stable-hash-2", "stable2.txt")
        .unwrap();
    let third = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 2).await;
    assert_eq!(
        third.owner_id, alice_pk,
        "identity stable even as revision changes"
    );

    // Also verify the signature is valid for each fetch.
    assert!(first.verify().is_ok(), "first catalogue signature valid");
    assert!(second.verify().is_ok(), "second catalogue signature valid");
    assert!(third.verify().is_ok(), "third catalogue signature valid");

    // Symmetric: Alice fetches Bob's catalogue repeatedly.
    set_relationship(
        &mut fx,
        PeerId::Bob,
        PeerId::Alice,
        FriendRelationship::Friends,
    )
    .await;

    fx.add_file(PeerId::Bob, "bob-stable-hash", "bob-stable.txt")
        .unwrap();
    let bob_first = assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;
    let bob_pk = bob_first.owner_id;

    let bob_second = assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;
    assert_eq!(bob_second.owner_id, bob_pk, "Bob identity stable");

    fx.shutdown().await;
}

// ═══════════════════════════════════════════════════════════════════════════
// Contact Visibility
// ═══════════════════════════════════════════════════════════════════════════

/// A non-friend peer sees an empty catalogue (no files visible) when the
/// owner has shared files.  After becoming friends, the files appear.
#[tokio::test]
async fn non_friend_sees_empty_catalogue_after_friendship_sees_files() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice adds a file to her catalogue.
    fx.add_file(PeerId::Alice, "vis-hash-1", "visible.txt")
        .unwrap();

    // Initially no relationship — NotFriend is the default.
    // Bob should see an empty catalogue because Alice has no relationship
    // record for Bob.
    assert_catalogue_empty(&fx, PeerId::Bob, PeerId::Alice).await;

    // Make Bob a friend of Alice.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;

    // Now Bob sees Alice's file.
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;

    // Symmetric: Bob adds a file; Alice sees nothing until friendship is mutual.
    fx.add_file(PeerId::Bob, "vis-hash-bob", "bob-visible.txt")
        .unwrap();

    // Alice is NOT a friend of Bob yet (only Bob is a friend of Alice).
    assert_catalogue_empty(&fx, PeerId::Alice, PeerId::Bob).await;

    // Make Alice a friend of Bob.
    set_relationship(
        &mut fx,
        PeerId::Bob,
        PeerId::Alice,
        FriendRelationship::Friends,
    )
    .await;

    // Now Alice sees Bob's file.
    assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;

    fx.shutdown().await;
}

/// Changing visibility from Friends → NotFriend hides previously visible
/// entries.  No stale entries remain.
#[tokio::test]
async fn removing_friendship_hides_entries() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice adds a file and makes Bob a friend.
    fx.add_file(PeerId::Alice, "vis-hash-2", "friend-only.txt")
        .unwrap();
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;

    // Bob sees the file.
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;

    // Downgrade Bob to NotFriend.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::NotFriend,
    )
    .await;

    // Bob now sees an empty catalogue — no stale entries.
    assert_catalogue_empty(&fx, PeerId::Bob, PeerId::Alice).await;

    fx.shutdown().await;
}

/// A blocked peer receives PermissionDenied and cannot see the catalogue at
/// all (the handler refuses to serve any response).  Changing blocked back
/// to friend restores access.
#[tokio::test]
async fn blocked_peer_gets_permission_denied() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice adds a file and makes Bob a friend.
    fx.add_file(PeerId::Alice, "block-hash", "blocked-test.txt")
        .unwrap();
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;

    // Bob sees the file.
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;

    // Block Bob.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Blocked,
    )
    .await;
    // Allow the restarted endpoint to settle before Bob connects.
    sleep(Duration::from_millis(300)).await;

    // Bob gets PermissionDenied.
    assert_permission_denied(&fx, PeerId::Bob, PeerId::Alice).await;

    // Restore Bob to Friends — access returns.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;
    sleep(Duration::from_millis(300)).await;

    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;

    fx.shutdown().await;
}

/// Completely removing a contact from the friends store (no record at all)
/// results in an empty catalogue, because no friendship record means
/// `is_friend` is false and no files pass the visibility filter.
#[tokio::test]
async fn removed_contact_has_empty_catalogue() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice adds a file and makes Bob a friend.
    fx.add_file(PeerId::Alice, "remove-hash", "remove-test.txt")
        .unwrap();
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;

    // Bob sees the file.
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;

    // Remove Bob as a contact entirely (no record).
    remove_contact(&mut fx, PeerId::Alice, PeerId::Bob).await;

    // Bob sees an empty catalogue — no stale entries.
    assert_catalogue_empty(&fx, PeerId::Bob, PeerId::Alice).await;

    // Symmetric: also verify Alice is removed from Bob's side
    fx.add_file(PeerId::Bob, "bob-remove-hash", "bob-remove.txt")
        .unwrap();
    set_relationship(
        &mut fx,
        PeerId::Bob,
        PeerId::Alice,
        FriendRelationship::Friends,
    )
    .await;
    assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;

    remove_contact(&mut fx, PeerId::Bob, PeerId::Alice).await;
    assert_catalogue_empty(&fx, PeerId::Alice, PeerId::Bob).await;

    fx.shutdown().await;
}

/// Multiple files remain visible/invisible consistently under friendship
/// changes.  Verify a hidden or removed contact does not leave stale
/// entries in the observable catalogue.
#[tokio::test]
async fn multiple_files_visibility_consistent_across_changes() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice shares three files.
    fx.add_file(PeerId::Alice, "mf-hash-1", "file1.txt")
        .unwrap();
    fx.add_file(PeerId::Alice, "mf-hash-2", "file2.txt")
        .unwrap();
    fx.add_file(PeerId::Alice, "mf-hash-3", "file3.txt")
        .unwrap();

    // Bob is a friend — sees all 3.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 3).await;

    // NotFriend — sees 0 (no stale entries).
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::NotFriend,
    )
    .await;
    assert_catalogue_empty(&fx, PeerId::Bob, PeerId::Alice).await;

    // Friends again — all 3 return.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 3).await;

    // Blocked — PermissionDenied.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Blocked,
    )
    .await;
    assert_permission_denied(&fx, PeerId::Bob, PeerId::Alice).await;

    // Friends again — all 3 return.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 3).await;

    fx.shutdown().await;
}

/// Both peers add files and verify each other's visibility rules
/// simultaneously.  This exercises that each peer's friend store is
/// isolated and the visibility filter respects per-peer relationships.
#[tokio::test]
async fn both_peers_verify_symmetric_visibility() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Both peers add files.
    fx.add_file(PeerId::Alice, "sym-hash-a1", "alice-file.txt")
        .unwrap();
    fx.add_file(PeerId::Bob, "sym-hash-b1", "bob-file.txt")
        .unwrap();

    // No relationships exist — both see empty catalogues from each other.
    assert_catalogue_empty(&fx, PeerId::Bob, PeerId::Alice).await;
    assert_catalogue_empty(&fx, PeerId::Alice, PeerId::Bob).await;

    // Alice makes Bob a friend.  Bob can now see Alice's files, but Alice
    // still cannot see Bob's (Bob hasn't reciprocated).
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    assert_catalogue_empty(&fx, PeerId::Alice, PeerId::Bob).await;

    // Bob reciprocates.  Now both see each other's files.
    set_relationship(
        &mut fx,
        PeerId::Bob,
        PeerId::Alice,
        FriendRelationship::Friends,
    )
    .await;
    assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;

    // Alice removes Bob's friendship.  Bob's view of Alice empties, but
    // Alice still sees Bob's files (Bob's relationship is still Friends).
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::NotFriend,
    )
    .await;
    assert_catalogue_empty(&fx, PeerId::Bob, PeerId::Alice).await;
    assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;

    // Bob also removes Alice.
    set_relationship(
        &mut fx,
        PeerId::Bob,
        PeerId::Alice,
        FriendRelationship::NotFriend,
    )
    .await;
    assert_catalogue_empty(&fx, PeerId::Alice, PeerId::Bob).await;

    fx.shutdown().await;
}

// ═══════════════════════════════════════════════════════════════════════════
// Positive Permission-Filter Visibility Tests
// ═══════════════════════════════════════════════════════════════════════════

/// A non-contact who is explicitly granted "read" on a specific file sees
/// that file, even though they are not a friend and cannot see other files.
#[tokio::test]
async fn selected_peer_sees_explicitly_granted_file() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice shares two files.
    fx.add_file(PeerId::Alice, "granted-hash", "granted.txt")
        .unwrap();
    fx.add_file(PeerId::Alice, "other-hash", "other.txt")
        .unwrap();

    // Bob is NOT a friend — should see no files by default.
    assert_catalogue_empty(&fx, PeerId::Bob, PeerId::Alice).await;

    // Alice explicitly grants Bob "read" on one file only.
    fx.grant_permission(PeerId::Alice, PeerId::Bob, "granted-hash", "read")
        .unwrap();

    // Bob now sees exactly the granted file (not the other file).
    let cat = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    assert_eq!(
        cat.files[0].content_hash, "granted-hash",
        "selected peer sees the explicitly granted file"
    );

    fx.shutdown().await;
}

/// A friend who receives an explicit "read" grant on a specific file
/// sees that file.  This exercises the grant path within an existing
/// contact relationship.
#[tokio::test]
async fn explicit_grant_works_within_contact_relationship() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice and Bob are friends.
    fx.set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await
        .unwrap();

    // Alice shares two files.
    fx.add_file(PeerId::Alice, "granted-hash-2", "granted2.txt")
        .unwrap();
    fx.add_file(PeerId::Alice, "other-hash-2", "other2.txt")
        .unwrap();

    // Bob sees both files as a friend.
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 2).await;

    // Alice grants Bob explicit "read" on one file.
    fx.grant_permission(PeerId::Alice, PeerId::Bob, "granted-hash-2", "read")
        .unwrap();

    // Bob still sees both files (the explicit grant does not reduce visibility
    // for a friend who already had access).
    let cat = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 2).await;
    assert!(
        cat.files.iter().any(|f| f.content_hash == "granted-hash-2"),
        "explicitly granted file is visible to contact"
    );
    assert!(
        cat.files.iter().any(|f| f.content_hash == "other-hash-2"),
        "other file remains visible to friend"
    );

    fx.shutdown().await;
}

/// A non-contact sees only the explicitly granted file, not other shared
/// files.  Verifies the permission filter correctly scopes visibility
/// to only the file with the active grant.
#[tokio::test]
async fn non_contact_sees_only_explicitly_granted_file() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice shares three files.
    fx.add_file(PeerId::Alice, "file-a", "file-a.txt").unwrap();
    fx.add_file(PeerId::Alice, "file-b", "file-b.txt").unwrap();
    fx.add_file(PeerId::Alice, "file-c", "file-c.txt").unwrap();

    // Bob is not a contact — sees nothing initially.
    assert_catalogue_empty(&fx, PeerId::Bob, PeerId::Alice).await;

    // Alice grants Bob "read" on only file-b.
    fx.grant_permission(PeerId::Alice, PeerId::Bob, "file-b", "read")
        .unwrap();

    // Bob sees exactly file-b.
    let cat = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    assert_eq!(
        cat.files[0].content_hash, "file-b",
        "non-contact sees only the explicitly granted file"
    );

    // Symmetric: Bob shares files and grants Alice access to one.
    fx.add_file(PeerId::Bob, "bob-file-x", "bob-file-x.txt")
        .unwrap();
    fx.add_file(PeerId::Bob, "bob-file-y", "bob-file-y.txt")
        .unwrap();
    let _ = fx
        .set_relationship(PeerId::Bob, PeerId::Alice, FriendRelationship::NotFriend)
        .await;
    assert_catalogue_empty(&fx, PeerId::Alice, PeerId::Bob).await;

    fx.grant_permission(PeerId::Bob, PeerId::Alice, "bob-file-y", "read")
        .unwrap();
    let bob_cat = assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;
    assert_eq!(
        bob_cat.files[0].content_hash, "bob-file-y",
        "symmetric: Alice sees only the file Bob granted"
    );

    fx.shutdown().await;
}

// ═══════════════════════════════════════════════════════════════════════════
// Negative Permission-Filter Visibility Tests
// ═══════════════════════════════════════════════════════════════════════════
//
// These scenarios exercise visibility that must be *denied* or *suppressed*
// through the end-to-end permission filter.  Each test verifies the precise
// expected behaviour — exclusion, denial, or the documented missing-file
// response — while touching only the state needed for its rule.

/// A non-contact (no relationship record at all) sees an empty catalogue
/// from a peer who has shared files, even when the peer has not set any
/// explicit relationship.
#[tokio::test]
async fn non_contact_sees_empty_catalogue() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice adds two files to her catalogue.
    fx.add_file(PeerId::Alice, "nc-hash-1", "alice-file-1.txt")
        .unwrap();
    fx.add_file(PeerId::Alice, "nc-hash-2", "alice-file-2.txt")
        .unwrap();

    // Bob is NOT a contact — no relationship record exists.  The
    // permission filter should show Bob an empty catalogue.
    assert_catalogue_empty(&fx, PeerId::Bob, PeerId::Alice).await;

    // Symmetric: Bob adds files, Alice is not a contact.
    fx.add_file(PeerId::Bob, "nc-hash-bob", "bob-file.txt")
        .unwrap();
    assert_catalogue_empty(&fx, PeerId::Alice, PeerId::Bob).await;

    fx.shutdown().await;
}

/// An explicit `deny` permission on a specific file hides that file from
/// a friend who would otherwise see it, while other files remain visible.
#[tokio::test]
async fn explicit_deny_hides_file_from_friend() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice adds two files and makes Bob a friend.
    fx.add_file(PeerId::Alice, "deny-hash-1", "denied.txt")
        .unwrap();
    fx.add_file(PeerId::Alice, "deny-hash-2", "allowed.txt")
        .unwrap();
    fx.set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await
        .unwrap();

    // Bob sees both files as a friend.
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 2).await;

    // Alice denies Bob access to one file specifically.
    fx.grant_permission(PeerId::Alice, PeerId::Bob, "deny-hash-1", "deny")
        .unwrap();

    // Bob now sees only "allowed.txt" — "denied.txt" is hidden.
    let cat = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    assert_eq!(
        cat.files[0].content_hash, "deny-hash-2",
        "denied file must be hidden from friend; only the non-denied file remains"
    );

    // Symmetric: Bob grants deny to Alice.
    fx.add_file(PeerId::Bob, "bob-deny-hash", "bob-deny.txt")
        .unwrap();
    fx.set_relationship(PeerId::Bob, PeerId::Alice, FriendRelationship::Friends)
        .await
        .unwrap();
    assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;

    fx.grant_permission(PeerId::Bob, PeerId::Alice, "bob-deny-hash", "deny")
        .unwrap();
    assert_catalogue_empty(&fx, PeerId::Alice, PeerId::Bob).await;

    fx.shutdown().await;
}

/// A blocked peer receives PermissionDenied at the protocol boundary —
/// the handler refuses to serve any catalogue response.
#[tokio::test]
async fn blocked_peer_gets_permission_denied_at_boundary() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice adds a file and makes Bob a friend.
    fx.add_file(PeerId::Alice, "block-neg-hash", "block-neg.txt")
        .unwrap();
    fx.set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await
        .unwrap();

    // Bob sees the file.
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;

    // Block Bob.
    fx.set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Blocked)
        .await
        .unwrap();

    // Bob gets PermissionDenied (protocol-level denial, not empty catalogue).
    assert_permission_denied(&fx, PeerId::Bob, PeerId::Alice).await;

    // Symmetric: Bob blocks Alice.
    fx.set_relationship(PeerId::Bob, PeerId::Alice, FriendRelationship::Blocked)
        .await
        .unwrap();
    assert_permission_denied(&fx, PeerId::Alice, PeerId::Bob).await;

    fx.shutdown().await;
}

/// A file whose offer is disabled (`offered = false`) is hidden from the
/// catalogue even when the requester is a friend who would otherwise have
/// access.
#[tokio::test]
async fn disabled_offer_hidden_from_catalogue() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice adds two files and makes Bob a friend.
    fx.add_file(PeerId::Alice, "dso-hash-1", "enabled.txt")
        .unwrap();
    fx.add_file(PeerId::Alice, "dso-hash-2", "disabled.txt")
        .unwrap();
    fx.set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await
        .unwrap();

    // Bob sees both files.
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 2).await;

    // Disable the offer for one file by upserting with `offered = false`.
    let alice_profile = fx.peer(PeerId::Alice).profile_id();
    fx.peer(PeerId::Alice)
        .storage
        .upsert_shared_file(
            "dso-hash-2",
            &alice_profile,
            "dso-hash-2",
            "disabled.txt",
            None,
            false,
        )
        .unwrap();
    // Bump the manifest revision so the catalogue handler picks up the change.
    fx.peer(PeerId::Alice)
        .storage
        .bump_manifest_revision(&alice_profile, "disable offer")
        .unwrap();

    // Bob now sees only the enabled file — the disabled offer is hidden.
    let cat = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    assert_eq!(
        cat.files[0].content_hash, "dso-hash-1",
        "disabled offer must be hidden from catalogue"
    );

    // Symmetric: Bob disables an offer.
    fx.add_file(PeerId::Bob, "bob-dso-hash", "bob-enabled.txt")
        .unwrap();
    fx.set_relationship(PeerId::Bob, PeerId::Alice, FriendRelationship::Friends)
        .await
        .unwrap();
    assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;

    let bob_profile = fx.peer(PeerId::Bob).profile_id();
    fx.peer(PeerId::Bob)
        .storage
        .upsert_shared_file(
            "bob-dso-hash",
            &bob_profile,
            "bob-dso-hash",
            "bob-enabled.txt",
            None,
            false,
        )
        .unwrap();
    fx.peer(PeerId::Bob)
        .storage
        .bump_manifest_revision(&bob_profile, "disable offer")
        .unwrap();
    assert_catalogue_empty(&fx, PeerId::Alice, PeerId::Bob).await;

    fx.shutdown().await;
}

/// A shared file whose backing file object has been deleted (missing
/// referenced file) is hidden from the catalogue even when the requester
/// is a friend with access.
///
/// Note: SQLite FOREIGN KEY constraints normally prevent deleting a
/// file_object that is still referenced by shared_files.  This test
/// temporarily disables FK enforcement to exercise the
/// `file_object_exists` safety check in `catalogue_entries_for_peer`.
#[tokio::test]
async fn missing_file_object_hidden_from_catalogue() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice adds two files and makes Bob a friend.
    fx.add_file(PeerId::Alice, "mfo-hash-1", "kept.txt")
        .unwrap();
    fx.add_file(PeerId::Alice, "mfo-hash-2", "removed.txt")
        .unwrap();
    fx.set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await
        .unwrap();

    // Bob sees both files.
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 2).await;

    // Temporarily disable FK enforcement so we can delete the file_object
    // while the shared_file still references it, then re-enable.
    fx.peer(PeerId::Alice)
        .storage
        .with_conn(|conn| {
            conn.execute_batch(
                "PRAGMA foreign_keys = OFF;\
                 DELETE FROM file_objects WHERE content_hash = 'mfo-hash-2';\
                 PRAGMA foreign_keys = ON;",
            )
            .unwrap();
            Ok(())
        })
        .unwrap();

    // Bump the manifest revision so the catalogue handler picks up the change.
    let alice_profile = fx.peer(PeerId::Alice).profile_id();
    fx.peer(PeerId::Alice)
        .storage
        .bump_manifest_revision(&alice_profile, "remove file object")
        .unwrap();

    // Bob now sees only "kept.txt" — the reference to the missing file
    // object is suppressed.
    let cat = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    assert_eq!(
        cat.files[0].content_hash, "mfo-hash-1",
        "shared file with missing backing object must be hidden"
    );

    // Symmetric: Bob removes a file object.
    fx.add_file(PeerId::Bob, "bob-mfo-hash", "bob-kept.txt")
        .unwrap();
    fx.set_relationship(PeerId::Bob, PeerId::Alice, FriendRelationship::Friends)
        .await
        .unwrap();
    assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;

    fx.peer(PeerId::Bob)
        .storage
        .with_conn(|conn| {
            conn.execute_batch(
                "PRAGMA foreign_keys = OFF;\
                 DELETE FROM file_objects WHERE content_hash = 'bob-mfo-hash';\
                 PRAGMA foreign_keys = ON;",
            )
            .unwrap();
            Ok(())
        })
        .unwrap();

    let bob_profile = fx.peer(PeerId::Bob).profile_id();
    fx.peer(PeerId::Bob)
        .storage
        .bump_manifest_revision(&bob_profile, "remove file object")
        .unwrap();
    assert_catalogue_empty(&fx, PeerId::Alice, PeerId::Bob).await;

    fx.shutdown().await;
}
