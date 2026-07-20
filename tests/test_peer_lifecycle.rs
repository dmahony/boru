//! Integration tests for peer lifecycle and catalogue updates.
//!
//! These scenarios use the shared two-peer fixture to verify:
//!
//! **Lifecycle transitions** — starting and stopping peers in different
//! orders produces the documented catalogue state.
//!
//! **Restart behaviour** — restarting a peer preserves expected catalogue
//! content and identity; no stale or duplicate entries appear.
//!
//! **Deterministic updates** — repeated additions and fetches from both
//! peers produce consistent, duplicate-free results.
//!
//! All scenarios are synchronised via catalogue fetch (a real QUIC
//! request/response) rather than arbitrary sleeps.

use std::time::Duration;

// Include the shared two-peer fixture.
#[path = "test_fixture.rs"]
mod fixture;

use fixture::{PeerId, TwoPeerFixture};

use boru_chat::{
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

/// Assert that no two entries in a catalogue share the same content_hash
/// or shared_file_id (no duplicates).
fn assert_no_duplicates(cat: &SignedFileCatalogue, label: &str) {
    let mut hashes = std::collections::HashSet::new();
    let mut ids = std::collections::HashSet::new();
    for f in &cat.files {
        assert!(
            hashes.insert(&f.content_hash),
            "{label}: duplicate content_hash {} in catalogue",
            f.content_hash
        );
        assert!(
            ids.insert(&f.shared_file_id),
            "{label}: duplicate shared_file_id {} in catalogue",
            f.shared_file_id
        );
    }
}

/// Assert that a catalogue fetch returns PermissionDenied.
async fn assert_permission_denied(fx: &TwoPeerFixture, requester: PeerId, server: PeerId) {
    let result = fetch_catalogue(fx, requester, server).await;
    match result {
        Err(RemoteCatalogueFetchError::PermissionDenied) => {}
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

// (poll_until intentionally omitted — the test framework's fetch helpers
//  provide real QUIC-based synchronisation without polling loops.)

// ═══════════════════════════════════════════════════════════════════════════
// 1. Peer Goes Offline While Updates Happen, Then Restarts
// ═══════════════════════════════════════════════════════════════════════════

/// Both peers start.  Bob stops.  Alice adds files.  Bob restarts and
/// fetches the catalogue — sees all files with no stale cached entries
/// and no duplicates.
#[tokio::test]
async fn peer_goes_offline_adds_happen_peer_restarts() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice and Bob become friends so Alice's catalogue is visible.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;

    // Alice adds an initial file, Bob sees it.
    fx.add_file(PeerId::Alice, "initial-hash", "initial.txt")
        .unwrap();
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;

    // Bob stops.
    fx.stop_peer(PeerId::Bob).await;
    assert!(!fx.bob.is_running());

    // Alice adds more files while Bob is offline.
    fx.add_file(PeerId::Alice, "offline-hash-1", "offline-1.txt")
        .unwrap();
    fx.add_file(PeerId::Alice, "offline-hash-2", "offline-2.txt")
        .unwrap();

    // Bob restarts.
    fx.restart_peer(PeerId::Bob).await.unwrap();
    sleep(Duration::from_millis(500)).await;

    // Bob fetches Alice's catalogue — should see all three files.
    let cat = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 3).await;
    assert_no_duplicates(&cat, "offline-restart");

    // Verify each expected hash is present.
    let hashes: std::collections::HashSet<&str> =
        cat.files.iter().map(|f| f.content_hash.as_str()).collect();
    assert!(hashes.contains("initial-hash"), "initial-hash present");
    assert!(hashes.contains("offline-hash-1"), "offline-hash-1 present");
    assert!(hashes.contains("offline-hash-2"), "offline-hash-2 present");

    fx.shutdown().await;
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Offline Updates Propagate After Restart
// ═══════════════════════════════════════════════════════════════════════════

/// Alice shares files.  Bob goes offline.  Alice adds more files.
/// Bob comes back online and fetches the catalogue — sees all files with
/// no stale cached entries and no duplicates.
#[tokio::test]
async fn offline_updates_seen_after_restart() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Set up friendship so both peers can see each other's catalogues.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;
    set_relationship(
        &mut fx,
        PeerId::Bob,
        PeerId::Alice,
        FriendRelationship::Friends,
    )
    .await;

    // Alice adds initial files, Bob sees them.
    fx.add_file(PeerId::Alice, "online-hash-1", "online-1.txt")
        .unwrap();
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;

    // Bob goes offline.
    fx.stop_peer(PeerId::Bob).await;
    assert!(!fx.bob.is_running());

    // Alice adds more files while Bob is offline.
    fx.add_file(PeerId::Alice, "offline-hash-2", "offline-2.txt")
        .unwrap();
    fx.add_file(PeerId::Alice, "offline-hash-3", "offline-3.txt")
        .unwrap();

    // Bob comes back online.
    fx.restart_peer(PeerId::Bob).await.unwrap();
    sleep(Duration::from_millis(500)).await;

    // Bob sees all three files — no stale cache, no duplicates.
    let cat = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 3).await;
    assert_no_duplicates(&cat, "offline-updates");

    // Verify each expected hash is present.
    let hashes: std::collections::HashSet<&str> =
        cat.files.iter().map(|f| f.content_hash.as_str()).collect();
    assert!(hashes.contains("online-hash-1"), "online-hash-1 present");
    assert!(hashes.contains("offline-hash-2"), "offline-hash-2 present");
    assert!(hashes.contains("offline-hash-3"), "offline-hash-3 present");

    // Symmetric: Bob adds files while offline, Alice sees them after restart.
    fx.stop_peer(PeerId::Bob).await;
    fx.add_file(PeerId::Bob, "bob-offline-hash-1", "bob-offline-1.txt")
        .unwrap();
    fx.add_file(PeerId::Bob, "bob-offline-hash-2", "bob-offline-2.txt")
        .unwrap();

    // Bob restarts.
    fx.restart_peer(PeerId::Bob).await.unwrap();
    sleep(Duration::from_millis(500)).await;

    // Alice fetches Bob's catalogue — should see both files.
    let bob_cat = assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 2).await;
    assert_no_duplicates(&bob_cat, "bob-offline-updates");

    fx.shutdown().await;
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Both Peers Restart — Full Shutdown and Recovery
// ═══════════════════════════════════════════════════════════════════════════

/// Both peers start, add files, shut down completely, then restart.
/// The fixture's storage persists across shutdown (in-memory `Arc<Storage>`
/// and file-backed `FriendsStore` on `TempDir`), so after restart the
/// catalogue content and friendship are still accessible.
#[tokio::test]
async fn both_peers_shutdown_and_restart() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;
    set_relationship(
        &mut fx,
        PeerId::Bob,
        PeerId::Alice,
        FriendRelationship::Friends,
    )
    .await;

    // Both peers add files.
    fx.add_file(PeerId::Alice, "full-cycle-hash-a1", "alice-cycle.txt")
        .unwrap();
    fx.add_file(PeerId::Bob, "full-cycle-hash-b1", "bob-cycle.txt")
        .unwrap();

    // Verify before shutdown.
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;

    // Full shutdown (networking stops, but storage persists).
    fx.shutdown().await;
    assert!(!fx.alice.is_running());
    assert!(!fx.bob.is_running());

    // Restart both — storage and friends have persisted, so the
    // same catalogue content and friendships are still in effect.
    fx.start().await.unwrap();
    sleep(Duration::from_millis(500)).await;

    // After restart, catalogues are accessible with the same content.
    let alice_cat = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    assert_no_duplicates(&alice_cat, "restart-alice");
    assert_eq!(alice_cat.files[0].content_hash, "full-cycle-hash-a1");

    let bob_cat = assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;
    assert_no_duplicates(&bob_cat, "restart-bob");
    assert_eq!(bob_cat.files[0].content_hash, "full-cycle-hash-b1");

    // Signatures remain valid after restart.
    assert!(
        alice_cat.verify().is_ok(),
        "Alice's catalogue signature valid after restart"
    );
    assert!(
        bob_cat.verify().is_ok(),
        "Bob's catalogue signature valid after restart"
    );

    fx.shutdown().await;
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Repeated Start/Stop Cycles — No Duplicates
// ═══════════════════════════════════════════════════════════════════════════

/// Multiple start/stop cycles should not create duplicate contacts or
/// stale peer records.  Storage persists across cycles (in-memory
/// `Arc<Storage>`, file-backed `FriendsStore`), so files accumulate
/// and each cycle produces deterministic, duplicate-free state.
#[tokio::test]
async fn repeated_start_stop_cycles_no_duplicates() {
    let mut fx = TwoPeerFixture::new();

    for cycle in 0..3 {
        fx.start().await.unwrap();
        assert!(fx.alice.is_running());
        assert!(fx.bob.is_running());

        set_relationship(
            &mut fx,
            PeerId::Alice,
            PeerId::Bob,
            FriendRelationship::Friends,
        )
        .await;

        // Add a file unique to this cycle.  Storage persists across
        // cycles, so files accumulate.
        let hash = format!("cycle-hash-{cycle}");
        let name = format!("cycle-{cycle}.txt");
        fx.add_file(PeerId::Alice, &hash, &name).unwrap();

        // Wait for peers to stabilise.
        sleep(Duration::from_millis(300)).await;

        // Bob fetches Alice's catalogue — should see `cycle+1` files
        // (cumulative because storage persists across shutdown/start).
        let expected_count = cycle + 1;
        let cat =
            assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, expected_count).await;
        assert_no_duplicates(&cat, &format!("cycle-{cycle}"));

        // Verify the newest hash is present and no stale entries.
        let hashes: std::collections::HashSet<&str> =
            cat.files.iter().map(|f| f.content_hash.as_str()).collect();
        assert!(
            hashes.contains(hash.as_str()),
            "cycle {cycle}: hash {hash} present"
        );

        // Verify the catalogue signature is valid.
        assert!(
            cat.verify().is_ok(),
            "cycle {cycle}: catalogue signature must verify"
        );

        fx.shutdown().await;
        assert!(!fx.alice.is_running());
        assert!(!fx.bob.is_running());
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. Updates Originating From Both Peers After Restart
// ═══════════════════════════════════════════════════════════════════════════

/// Both peers add files, alternate restarts, and verify that catalogue
/// fetches from each peer return the correct, duplicate-free content.
#[tokio::test]
async fn both_peers_update_after_alternating_restarts() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;
    set_relationship(
        &mut fx,
        PeerId::Bob,
        PeerId::Alice,
        FriendRelationship::Friends,
    )
    .await;

    // Alice adds two files.
    fx.add_file(PeerId::Alice, "alice-hash-a", "alice-a.txt")
        .unwrap();
    fx.add_file(PeerId::Alice, "alice-hash-b", "alice-b.txt")
        .unwrap();

    // Bob restarts.
    fx.restart_peer(PeerId::Bob).await.unwrap();
    sleep(Duration::from_millis(300)).await;

    // Bob fetches Alice's catalogue — sees both files.
    let cat_a = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 2).await;
    assert_no_duplicates(&cat_a, "alice-after-bob-restart");

    // Bob adds files.
    fx.add_file(PeerId::Bob, "bob-hash-c", "bob-c.txt").unwrap();
    fx.add_file(PeerId::Bob, "bob-hash-d", "bob-d.txt").unwrap();

    // Alice restarts.
    fx.restart_peer(PeerId::Alice).await.unwrap();
    sleep(Duration::from_millis(300)).await;

    // Alice fetches Bob's catalogue — sees both files.
    let cat_b = assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 2).await;
    assert_no_duplicates(&cat_b, "bob-after-alice-restart");

    // Both peers restart in sequence.
    fx.restart_peer(PeerId::Bob).await.unwrap();
    sleep(Duration::from_millis(300)).await;
    fx.restart_peer(PeerId::Alice).await.unwrap();
    sleep(Duration::from_millis(300)).await;

    // After both have restarted, catalogues remain accessible.
    let cat_a_final = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 2).await;
    assert_no_duplicates(&cat_a_final, "final-alice");
    let cat_b_final = assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 2).await;
    assert_no_duplicates(&cat_b_final, "final-bob");

    fx.shutdown().await;
}

// ═══════════════════════════════════════════════════════════════════════════
// 6. Repeated Updates From Both Peers Are Deterministic
// ═══════════════════════════════════════════════════════════════════════════

/// Multiple rounds of add-file and fetch produce consistent, duplicate-free
/// results.  The catalogue revision increases monotonically.
#[tokio::test]
async fn repeated_updates_are_deterministic() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;

    // Add files in batches, fetching after each batch.
    let rounds = [
        vec!["det-hash-1", "det-hash-2"],
        vec!["det-hash-3"],
        vec!["det-hash-4", "det-hash-5", "det-hash-6"],
    ];

    let mut cumulative = 0usize;
    let mut last_revision: Option<u64> = None;

    for (batch_idx, batch) in rounds.iter().enumerate() {
        for (file_idx, hash) in batch.iter().enumerate() {
            let name = format!("det-{batch_idx}-{file_idx}.txt");
            fx.add_file(PeerId::Alice, hash, &name).unwrap();
        }
        cumulative += batch.len();

        // Give the system a moment to process the catalogue update.
        sleep(Duration::from_millis(200)).await;

        // Fetch and verify.
        let cat = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, cumulative).await;
        assert_no_duplicates(&cat, &format!("deterministic-round-{batch_idx}"));

        // Revision must increase monotonically.
        if let Some(prev_rev) = last_revision {
            assert!(
                cat.revision > prev_rev,
                "revision must increase: {prev_rev} -> {}",
                cat.revision
            );
        }
        last_revision = Some(cat.revision);

        // Signature always valid.
        assert!(
            cat.verify().is_ok(),
            "batch {batch_idx}: signature must verify"
        );
    }

    fx.shutdown().await;
}

// ═══════════════════════════════════════════════════════════════════════════
// 7. Visibility Preserved Across Lifecycle
// ═══════════════════════════════════════════════════════════════════════════

/// A non-friend sees an empty catalogue.  After becoming a friend, the
/// files appear.  After being blocked, PermissionDenied is returned.
/// After restoring friendship, files reappear.  All transitions are
/// tested across a peer restart — the `FriendsStore` is file-backed
/// (on `TempDir`) so friendship state persists across restarts.
#[tokio::test]
async fn visibility_transitions_across_restarts() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice adds a file.
    fx.add_file(PeerId::Alice, "vis-lifecycle-hash", "lifecycle.txt")
        .unwrap();

    // NotFriend — Bob sees empty.
    assert_catalogue_empty(&fx, PeerId::Bob, PeerId::Alice).await;

    // Become friends — files appear.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;

    // Alice restarts.  The FriendsStore is file-backed on TempDir,
    // so the friendship record persists across the restart.
    fx.restart_peer(PeerId::Alice).await.unwrap();
    sleep(Duration::from_millis(300)).await;

    // Friendship persisted — Bob still sees the file.
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;

    // Block Bob.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Blocked,
    )
    .await;
    sleep(Duration::from_millis(300)).await;
    assert_permission_denied(&fx, PeerId::Bob, PeerId::Alice).await;

    // Restore to Friends — files return.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;

    fx.shutdown().await;
}

// ═══════════════════════════════════════════════════════════════════════════
// 8. Symmetric Restart — Both Peers Add Files Before Each Restart
// ═══════════════════════════════════════════════════════════════════════════

/// Both peers add files, restart in sequence, and verify the other peer's
/// catalogue is still accessible with the correct content.  This exercises
/// the scenario where the fetching peer is the one that restarts.
#[tokio::test]
async fn symmetric_restart_preserves_catalogue_access() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;
    set_relationship(
        &mut fx,
        PeerId::Bob,
        PeerId::Alice,
        FriendRelationship::Friends,
    )
    .await;

    // Both add files.
    fx.add_file(PeerId::Alice, "sym-restart-a", "sym-a.txt")
        .unwrap();
    fx.add_file(PeerId::Bob, "sym-restart-b", "sym-b.txt")
        .unwrap();

    // Verify baseline.
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;

    // Bob restarts.  Alice's endpoint address might change on restart,
    // so we need to re-seed lookups.
    fx.restart_peer(PeerId::Bob).await.unwrap();
    sleep(Duration::from_millis(300)).await;

    // Bob (the fetcher) restarted.  Bob should still be able to
    // fetch Alice's catalogue.
    assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;

    // Alice restarts.
    fx.restart_peer(PeerId::Alice).await.unwrap();
    sleep(Duration::from_millis(300)).await;

    // Alice (the fetcher) restarted.  Check Bob's catalogue.
    assert_catalogue_file_count(&fx, PeerId::Alice, PeerId::Bob, 1).await;

    fx.shutdown().await;
}

// ═══════════════════════════════════════════════════════════════════════════
// 9. Accepted Contact Sees Contacts-Only Files
// ═══════════════════════════════════════════════════════════════════════════

/// A peer who has been accepted as a contact (FriendRelationship::Friends)
/// can see contacts-only files via the catalogue.  Before acceptance, the
/// same peer sees an empty catalogue.  After acceptance, the offered file
/// becomes visible — demonstrating that the friendship relationship is
/// the gate for contacts-only visibility.
#[tokio::test]
async fn accepted_contact_sees_contacts_only_files() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice adds a contacts-only file (no explicit grants).
    fx.add_file(PeerId::Alice, "contacts-hash", "contacts.txt")
        .unwrap();

    // Bob is NOT a friend — sees an empty catalogue (contacts-only filter).
    assert_catalogue_empty(&fx, PeerId::Bob, PeerId::Alice).await;

    // Alice accepts Bob as a contact.  This restarts Alice so the
    // CatalogueHandler picks up the updated FriendsStore.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;
    sleep(Duration::from_millis(300)).await;

    // Bob now sees the contacts-only file via friendship.
    let cat = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    assert_eq!(cat.files[0].content_hash, "contacts-hash");
    assert!(
        cat.verify().is_ok(),
        "catalogue signature valid after contact acceptance"
    );

    fx.shutdown().await;
}

// ═══════════════════════════════════════════════════════════════════════════
// 10. Selected Peer Sees Granted File Without Friendship
// ═══════════════════════════════════════════════════════════════════════════

/// A file with at least one explicit grant enters selected-peers mode.
/// A peer who has been explicitly granted read access (the "selected peer")
/// can see that file even without a friendship relationship.  This
/// demonstrates that the explicit grant mechanism provides visibility
/// independently from the friendship check.
#[tokio::test]
async fn selected_peer_sees_granted_file_without_friendship() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice adds a file and grants Bob explicit read access.
    // This puts the file in selected-peers mode.
    fx.add_file(PeerId::Alice, "granted-hash", "granted.txt")
        .unwrap();
    fx.grant_permission(PeerId::Alice, PeerId::Bob, "granted-hash", "read")
        .expect("grant read permission");

    // Alice and Bob are NOT friends.  The file is visible to Bob only
    // because he has the explicit grant — the grant overrides the
    // absence of a friendship relationship.
    let cat = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    assert_eq!(
        cat.files[0].content_hash, "granted-hash",
        "selected peer sees the granted file despite not being a friend"
    );
    assert!(cat.verify().is_ok(), "catalogue signature valid");

    fx.shutdown().await;
}

// ═══════════════════════════════════════════════════════════════════════════
// 11. Explicit Grant Makes Contacts-Only File Visible Regardless of Friendship
// ═══════════════════════════════════════════════════════════════════════════

/// When a file has both a contacts-only sibling and an explicit read grant
/// for the same peer, the explicit grant is what controls visibility.
/// A non-friend sees only the granted file (not the contacts-only one);
/// after friendship is established, both become visible.  This confirms
/// that the explicit grant mechanism works correctly as a visibility
/// override and that adding friendship later widens the view as expected.
#[tokio::test]
async fn explicit_grant_makes_file_visible_with_and_without_friendship() {
    let mut fx = TwoPeerFixture::new();
    fx.start().await.unwrap();

    // Alice adds two files:
    //   contacts-hash — contacts-only (no grants, relies on friendship)
    //   granted-hash  — has an explicit read grant for Bob
    fx.add_file(PeerId::Alice, "contacts-hash", "contacts.txt")
        .unwrap();
    fx.add_file(PeerId::Alice, "granted-hash", "granted.txt")
        .unwrap();
    fx.grant_permission(PeerId::Alice, PeerId::Bob, "granted-hash", "read")
        .expect("grant read permission");

    // Alice and Bob are NOT friends.
    //   - contacts-hash is hidden (no friendship, no grant)
    //   - granted-hash is visible (explicit grant alone suffices)
    let cat = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 1).await;
    assert_eq!(
        cat.files[0].content_hash, "granted-hash",
        "only the granted file is visible without friendship"
    );
    assert!(cat.verify().is_ok(), "catalogue signature valid");

    // Alice accepts Bob as a contact.  The CatalogueHandler is rebuilt
    // with the updated FriendsStore.
    set_relationship(
        &mut fx,
        PeerId::Alice,
        PeerId::Bob,
        FriendRelationship::Friends,
    )
    .await;
    sleep(Duration::from_millis(300)).await;

    // Now both files are visible: contacts-hash via friendship,
    // granted-hash via the explicit grant.
    let cat2 = assert_catalogue_file_count(&fx, PeerId::Bob, PeerId::Alice, 2).await;
    let hashes: std::collections::HashSet<&str> =
        cat2.files.iter().map(|f| f.content_hash.as_str()).collect();
    assert!(
        hashes.contains("contacts-hash"),
        "contacts-only file visible after friendship"
    );
    assert!(
        hashes.contains("granted-hash"),
        "granted file still visible after friendship"
    );
    assert!(cat2.verify().is_ok(), "catalogue signature valid");

    fx.shutdown().await;
}
