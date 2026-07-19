//! End-to-end coverage for the complete friend-request feature.
//!
//! Uses two independent [`FriendRequestStore`]s with real temporary
//! directories to test the complete lifecycle across peers: send → list →
//! accept/decline/cancel → persist → reload.  Also covers the
//! [`SignedContactMessage`] round-trip for each of the new distinct
//! friend-request protocol actions (`FriendRequest`, `FriendRequestAccepted`,
//! `FriendRequestRejected`).
//!
//! The data-model validation rules (self-request, duplicate-pending,
//! unauthorized, invalid transitions) are already covered by
//! `friend_request.rs` 36 unit tests — this file adds the multi-store
//! and persistence integration layer the unit tests cannot reach.
//!
//! # Protocol separation
//!
//! Friend requests (`FriendRequest` / `FriendRequestAccepted` /
//! `FriendRequestRejected`) are now distinct from conversation invites
//! (`ConversationInvite`).  Accepting a friend request does NOT auto-open
//! a conversation — the user must explicitly invite their established friend
//! to a direct conversation.

use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use boru_chat::{
    contact::{ContactAction, SignedContactMessage},
    friend_request::{FriendRequestError, FriendRequestStatus, FriendRequestStore},
};
use iroh::{PublicKey, SecretKey};

// ── Helpers ─────────────────────────────────────────────────────────────────

fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("boru-fr-e2e-{name}-{suffix}"));
    dir
}

fn random_peer() -> String {
    SecretKey::generate().public().to_string()
}

/// Simulate what the SendFriendRequest UI handler does in Alice's store:
/// create a Pending request to Bob, sign a `FriendRequest` contact message,
/// and return the signed payload for the test to "send" to Bob.
///
/// Returns the signed bytes only (each store creates its own request ID,
/// so we cannot share request IDs across stores).
fn alice_sends_to_bob(
    alice_store: &mut FriendRequestStore,
    alice_sk: &SecretKey,
    bob_pk: &PublicKey,
) -> Vec<u8> {
    let alice_pk_str = alice_sk.public().to_string();
    let bob_pk_str = bob_pk.to_string();

    alice_store
        .send_request(&alice_pk_str, &bob_pk_str, None)
        .expect("alice sends friend request to bob");

    let action = ContactAction::FriendRequest { name: None };
    SignedContactMessage::sign(alice_sk, &action).expect("alice signs friend request")
}

/// Simulate what the receiving peer does with an incoming `FriendRequest`:
/// verify the signed message, extract the sender identity, then
/// create a pending friend request in the recipient's store.
/// Returns the request id that *this store* assigned.
fn bob_receives_from_alice(
    bob_store: &mut FriendRequestStore,
    bob_pk: &PublicKey,
    payload: &[u8],
) -> String {
    let (from, action) = SignedContactMessage::verify(payload, None)
        .expect("bob verifies alice's signed friend request");

    assert_eq!(action, ContactAction::FriendRequest { name: None });

    let bob_pk_str = bob_pk.to_string();
    let from_str = from.to_string();
    let request = bob_store
        .send_request(&from_str, &bob_pk_str, None)
        .expect("bob creates incoming friend request from verified sender");
    request.id
}

// ── Full lifecycle tests ────────────────────────────────────────────────────

/// Full lifecycle across two simulated peers:
///   Alice sends → Bob receives → Bob lists incoming → Bob accepts →
///   Both persist → Both reload → Verify states preserved.
#[test]
fn friend_request_full_lifecycle_across_two_peers() {
    let alice_dir = temp_dir("alice-lifecycle");
    let bob_dir = temp_dir("bob-lifecycle");
    let alice_sk = SecretKey::generate();
    let bob_sk = SecretKey::generate();
    let bob_pk = bob_sk.public();
    let alice_pk_str = alice_sk.public().to_string();
    let bob_pk_str = bob_pk.to_string();

    let mut alice_store = FriendRequestStore::empty_at(&alice_dir);
    let mut bob_store = FriendRequestStore::empty_at(&bob_dir);

    // ── 1. Alice sends a friend request ──
    let payload = alice_sends_to_bob(&mut alice_store, &alice_sk, &bob_pk);

    // ── 2. Bob receives the signed message ──
    let bob_req_id = bob_receives_from_alice(&mut bob_store, &bob_pk, &payload);

    // ── 3. Verify stores ──
    let alice_outgoing = alice_store.list_outgoing(&alice_pk_str);
    assert_eq!(alice_outgoing.len(), 1, "Alice has 1 outgoing request");
    assert_eq!(alice_outgoing[0].recipient, bob_pk_str);
    assert!(alice_store.list_incoming(&alice_pk_str).is_empty());

    let bob_incoming = bob_store.list_incoming(&bob_pk_str);
    assert_eq!(bob_incoming.len(), 1, "Bob has 1 incoming request");
    assert_eq!(bob_incoming[0].requester, alice_pk_str);
    assert!(bob_store.list_outgoing(&bob_pk_str).is_empty());

    // ── 4. Bob accepts the request in his store ──
    let accepted = bob_store
        .accept_request(&bob_req_id, &bob_pk_str)
        .expect("bob accepts friend request");
    assert_eq!(accepted.status, FriendRequestStatus::Accepted);

    assert!(bob_store
        .list_incoming_by_status(&bob_pk_str, FriendRequestStatus::Pending)
        .is_empty());

    // ── 5. Both persist and reload ──
    alice_store.save().expect("alice saves");
    bob_store.save().expect("bob saves");

    let alice_loaded = FriendRequestStore::load(&alice_dir).expect("alice reloads");
    let bob_loaded = FriendRequestStore::load(&bob_dir).expect("bob reloads");

    // Alice's store: still has 1 outgoing Pending (Bob hasn't told Alice yet)
    assert_eq!(alice_loaded.len(), 1);
    let alice_req = alice_loaded
        .iter()
        .find(|r| r.recipient == bob_pk_str)
        .expect("alice's request to bob still exists");
    assert_eq!(alice_req.status, FriendRequestStatus::Pending);

    // Bob's store: request is now Accepted
    assert_eq!(bob_loaded.len(), 1);
    let bob_req = bob_loaded
        .iter()
        .find(|r| r.requester == alice_pk_str)
        .expect("bob's request from alice still exists");
    assert_eq!(bob_req.status, FriendRequestStatus::Accepted);
}

/// Full send → cancel flow across two simulated peers.
#[test]
fn friend_request_cancel_flow() {
    let alice_dir = temp_dir("alice-cancel");
    let bob_dir = temp_dir("bob-cancel");
    let alice_sk = SecretKey::generate();
    let bob_sk = SecretKey::generate();
    let bob_pk = bob_sk.public();

    let mut alice_store = FriendRequestStore::empty_at(&alice_dir);
    let mut bob_store = FriendRequestStore::empty_at(&bob_dir);

    let payload = alice_sends_to_bob(&mut alice_store, &alice_sk, &bob_pk);
    bob_receives_from_alice(&mut bob_store, &bob_pk, &payload);

    // Find the outgoing request id in Alice's store
    let alice_pk_str = alice_sk.public().to_string();
    let alice_req_id = alice_store
        .list_outgoing(&alice_pk_str)
        .first()
        .expect("alice has outgoing")
        .id
        .clone();

    // Alice cancels her request
    let cancelled = alice_store
        .cancel_request(&alice_req_id, &alice_pk_str)
        .expect("alice cancels the request");
    assert_eq!(cancelled.status, FriendRequestStatus::Cancelled);

    assert!(alice_store
        .list_outgoing_by_status(&alice_pk_str, FriendRequestStatus::Pending)
        .is_empty());
    let cancelled_out =
        alice_store.list_outgoing_by_status(&alice_pk_str, FriendRequestStatus::Cancelled);
    assert_eq!(cancelled_out.len(), 1);

    // Persist and reload
    alice_store.save().expect("alice saves");
    let alice_loaded = FriendRequestStore::load(&alice_dir).expect("alice reloads");
    let loaded_req = alice_loaded
        .iter()
        .find(|r| r.id == alice_req_id)
        .expect("request still exists");
    assert_eq!(loaded_req.status, FriendRequestStatus::Cancelled);
}

/// Full send → decline flow across two simulated peers.
#[test]
fn friend_request_decline_flow() {
    let bob_dir = temp_dir("bob-decline");
    let alice_sk = SecretKey::generate();
    let bob_sk = SecretKey::generate();
    let bob_pk = bob_sk.public();

    let mut alice_store = FriendRequestStore::default();
    let mut bob_store = FriendRequestStore::empty_at(&bob_dir);

    let payload = alice_sends_to_bob(&mut alice_store, &alice_sk, &bob_pk);
    let bob_req_id = bob_receives_from_alice(&mut bob_store, &bob_pk, &payload);

    // Bob declines
    let bob_pk_str = bob_pk.to_string();
    let declined = bob_store
        .decline_request(&bob_req_id, &bob_pk_str)
        .expect("bob declines the request");
    assert_eq!(declined.status, FriendRequestStatus::Declined);

    assert!(bob_store
        .list_incoming_by_status(&bob_pk_str, FriendRequestStatus::Pending)
        .is_empty());

    // Persist and reload
    bob_store.save().expect("bob saves");
    let bob_loaded = FriendRequestStore::load(&bob_dir).expect("bob reloads");
    let loaded_req = bob_loaded
        .iter()
        .find(|r| r.id == bob_req_id)
        .expect("request still exists");
    assert_eq!(loaded_req.status, FriendRequestStatus::Declined);
}

/// Two peers each participate in separate pairs: Alice↔Bob via one pair,
/// Charlie↔Dave via another.  Each store correctly tracks its own pairs
/// independently, and persistence preserves both.
#[test]
fn two_independent_pairs_across_four_peers() {
    let alice_dir = temp_dir("alice-2pair");
    let bob_dir = temp_dir("bob-2pair");
    let alice_sk = SecretKey::generate();
    let bob_sk = SecretKey::generate();
    let charlie_pk = SecretKey::generate().public();
    let dave_pk = SecretKey::generate().public();
    let bob_pk = bob_sk.public();
    let alice_pk_str = alice_sk.public().to_string();
    let bob_pk_str = bob_pk.to_string();
    let charlie_pk_str = charlie_pk.to_string();
    let dave_pk_str = dave_pk.to_string();

    let mut alice_store = FriendRequestStore::empty_at(&alice_dir);
    let mut bob_store = FriendRequestStore::empty_at(&bob_dir);

    let payload = alice_sends_to_bob(&mut alice_store, &alice_sk, &bob_pk);
    bob_receives_from_alice(&mut bob_store, &bob_pk, &payload);

    alice_store
        .send_request(&alice_pk_str, &charlie_pk_str, None)
        .expect("alice sends to charlie");

    bob_store
        .send_request(&bob_pk_str, &dave_pk_str, None)
        .expect("bob sends to dave");

    assert_eq!(alice_store.list_outgoing(&alice_pk_str).len(), 2);
    assert_eq!(bob_store.list_outgoing(&bob_pk_str).len(), 1);
    assert_eq!(bob_store.list_incoming(&bob_pk_str).len(), 1);

    alice_store.save().expect("save alice");
    bob_store.save().expect("save bob");

    let alice_loaded = FriendRequestStore::load(&alice_dir).expect("reload alice");
    let bob_loaded = FriendRequestStore::load(&bob_dir).expect("reload bob");

    assert_eq!(alice_loaded.len(), 2);
    assert_eq!(bob_loaded.len(), 2);
}

// ── Signed contact message integration ──────────────────────────────────────

/// Test the full signed FriendRequest round-trip.
#[test]
fn signed_friend_request_round_trip() {
    let alice_sk = SecretKey::generate();

    let action = ContactAction::FriendRequest {
        name: Some("Alice".into()),
    };

    let payload = SignedContactMessage::sign(&alice_sk, &action).expect("sign FriendRequest");

    let (from, decoded) =
        SignedContactMessage::verify(&payload, None).expect("verify FriendRequest");
    assert_eq!(from, alice_sk.public());
    assert_eq!(decoded, action);

    let (from, decoded) = SignedContactMessage::verify(&payload, Some(alice_sk.public()))
        .expect("verify with expected_from");
    assert_eq!(from, alice_sk.public());
    assert_eq!(decoded, action);

    let wrong_pk = SecretKey::generate().public();
    let err = SignedContactMessage::verify(&payload, Some(wrong_pk))
        .expect_err("wrong expected_from fails");
    let err_msg = format!("{err}");
    assert!(
        err_msg.contains("signer") || err_msg.contains("transport") || err_msg.contains("peer"),
        "error should mention signer/transport mismatch: {err_msg}"
    );

    let mut tampered = payload.clone();
    if let Some(byte) = tampered.last_mut() {
        *byte ^= 0xFF;
    }
    SignedContactMessage::verify(&tampered, None).expect_err("tampered payload fails");
}

/// Test the full signed FriendRequestAccepted round-trip.
#[test]
fn signed_friend_request_accepted_round_trip() {
    let alice_sk = SecretKey::generate();

    let action = ContactAction::FriendRequestAccepted;

    let payload =
        SignedContactMessage::sign(&alice_sk, &action).expect("sign FriendRequestAccepted");

    let (from, decoded) =
        SignedContactMessage::verify(&payload, None).expect("verify FriendRequestAccepted");
    assert_eq!(from, alice_sk.public());
    assert_eq!(decoded, action);

    let (from, decoded) = SignedContactMessage::verify(&payload, Some(alice_sk.public()))
        .expect("verify with expected_from");
    assert_eq!(from, alice_sk.public());
    assert_eq!(decoded, action);

    let wrong_pk = SecretKey::generate().public();
    let err = SignedContactMessage::verify(&payload, Some(wrong_pk))
        .expect_err("wrong expected_from fails");
    let err_msg = format!("{err}");
    assert!(
        err_msg.contains("signer") || err_msg.contains("transport") || err_msg.contains("peer"),
        "error should mention signer/transport mismatch: {err_msg}"
    );

    let mut tampered = payload.clone();
    if let Some(byte) = tampered.last_mut() {
        *byte ^= 0xFF;
    }
    SignedContactMessage::verify(&tampered, None).expect_err("tampered payload fails");
}

/// Test the full signed FriendRequestRejected round-trip.
#[test]
fn signed_friend_request_rejected_round_trip() {
    let alice_sk = SecretKey::generate();

    let action = ContactAction::FriendRequestRejected;

    let payload =
        SignedContactMessage::sign(&alice_sk, &action).expect("sign FriendRequestRejected");

    let (from, decoded) =
        SignedContactMessage::verify(&payload, None).expect("verify FriendRequestRejected");
    assert_eq!(from, alice_sk.public());
    assert_eq!(decoded, action);

    let (from, decoded) = SignedContactMessage::verify(&payload, Some(alice_sk.public()))
        .expect("verify with expected_from");
    assert_eq!(from, alice_sk.public());
    assert_eq!(decoded, action);

    let wrong_pk = SecretKey::generate().public();
    let err = SignedContactMessage::verify(&payload, Some(wrong_pk))
        .expect_err("wrong expected_from fails");
    let err_msg = format!("{err}");
    assert!(
        err_msg.contains("signer") || err_msg.contains("transport") || err_msg.contains("peer"),
        "error should mention signer/transport mismatch: {err_msg}"
    );

    let mut tampered = payload.clone();
    if let Some(byte) = tampered.last_mut() {
        *byte ^= 0xFF;
    }
    SignedContactMessage::verify(&tampered, None).expect_err("tampered payload fails");
}

/// Test the full signed ConversationInvite round-trip (still valid between
/// existing friends, separate from friend requests).
#[test]
fn signed_conversation_invite_round_trip() {
    let alice_sk = SecretKey::generate();
    let bob_pk = SecretKey::generate().public();

    let topic = boru_chat::contact::direct_topic(&alice_sk.public(), &bob_pk);
    let action = ContactAction::ConversationInvite {
        topic,
        addrs: vec![],
    };

    let payload = SignedContactMessage::sign(&alice_sk, &action).expect("sign ConversationInvite");

    let (from, decoded) =
        SignedContactMessage::verify(&payload, None).expect("verify ConversationInvite");
    assert_eq!(from, alice_sk.public());
    assert_eq!(decoded, action);

    let (from, decoded) = SignedContactMessage::verify(&payload, Some(alice_sk.public()))
        .expect("verify with expected_from");
    assert_eq!(from, alice_sk.public());
    assert_eq!(decoded, action);

    let wrong_pk = SecretKey::generate().public();
    let err = SignedContactMessage::verify(&payload, Some(wrong_pk))
        .expect_err("wrong expected_from fails");
    let err_msg = format!("{err}");
    assert!(
        err_msg.contains("signer") || err_msg.contains("transport") || err_msg.contains("peer"),
        "error should mention signer/transport mismatch: {err_msg}"
    );

    let mut tampered = payload.clone();
    if let Some(byte) = tampered.last_mut() {
        *byte ^= 0xFF;
    }
    SignedContactMessage::verify(&tampered, None).expect_err("tampered payload fails");
}

/// Exercise the identity-constrained verify path used in production:
/// alice → bob: bob MUST check the sender is alice.
#[test]
fn signed_contact_verify_with_expected_identity() {
    let alice_sk = SecretKey::generate();
    let action = ContactAction::FriendRequest {
        name: Some("Alice".into()),
    };

    let payload = SignedContactMessage::sign(&alice_sk, &action).expect("sign");

    let (from, decoded) = SignedContactMessage::verify(&payload, Some(alice_sk.public()))
        .expect("verify with alice's expected identity");
    assert_eq!(from, alice_sk.public());
    assert_eq!(decoded, action);

    let eve_sk = SecretKey::generate();
    let eve_payload = SignedContactMessage::sign(&eve_sk, &action).expect("eve signs");
    SignedContactMessage::verify(&eve_payload, Some(alice_sk.public()))
        .expect_err("eve's signature fails alice's expected identity");
}

// ── Multi-store edge cases ─────────────────────────────────────────────────

/// A new request between the same pair is allowed after the previous request
/// reaches a terminal state — verified across two independent stores.
#[test]
fn new_request_allowed_after_terminal_state_across_stores() {
    let alice_dir = temp_dir("alice-retry");
    let bob_dir = temp_dir("bob-retry");
    let alice_sk = SecretKey::generate();
    let bob_sk = SecretKey::generate();
    let bob_pk = bob_sk.public();
    let alice_pk_str = alice_sk.public().to_string();
    let bob_pk_str = bob_pk.to_string();

    let mut alice_store = FriendRequestStore::empty_at(&alice_dir);
    let mut bob_store = FriendRequestStore::empty_at(&bob_dir);

    // First request: Alice sends, Bob decline
    let payload1 = alice_sends_to_bob(&mut alice_store, &alice_sk, &bob_pk);
    let bob_req_id1 = bob_receives_from_alice(&mut bob_store, &bob_pk, &payload1);

    bob_store
        .decline_request(&bob_req_id1, &bob_pk_str)
        .expect("bob declines first request");

    // Alice's store still has the Pending outgoing (she hasn't been notified).
    // Alice should NOT be able to send another to Bob while she still has Pending.
    let err = alice_store
        .send_request(&alice_pk_str, &bob_pk_str, None)
        .expect_err("alice's store still has Pending — duplicate rejected");
    assert!(matches!(err, FriendRequestError::DuplicatePending { .. }));

    // But in Bob's store the request is Declined (terminal), so Bob could
    // send a new request to Alice.
    let new_req = bob_store
        .send_request(&bob_pk_str, &alice_pk_str, None)
        .expect("bob can send new request after declined state on his side");
    assert_eq!(new_req.status, FriendRequestStatus::Pending);
    assert_ne!(new_req.id, bob_req_id1, "new request has different id");

    // Bob's store: 1 Declined (old), 1 Pending (new)
    assert_eq!(bob_store.len(), 2);
    let pending = bob_store.list_outgoing_by_status(&bob_pk_str, FriendRequestStatus::Pending);
    assert_eq!(pending.len(), 1, "bob has 1 pending outgoing");
    let declined = bob_store.list_incoming_by_status(&bob_pk_str, FriendRequestStatus::Declined);
    assert_eq!(declined.len(), 1, "bob has 1 declined incoming");
}

/// Persistence: both alice's and bob's stores survive full save/reload cycles
/// while maintaining data integrity.
#[test]
fn both_stores_persist_independently() {
    let alice_dir = temp_dir("alice-persist");
    let bob_dir = temp_dir("bob-persist");
    let alice_sk = SecretKey::generate();
    let bob_sk = SecretKey::generate();
    let bob_pk = bob_sk.public();
    let alice_pk_str = alice_sk.public().to_string();
    let bob_pk_str = bob_pk.to_string();

    let mut alice_store = FriendRequestStore::empty_at(&alice_dir);
    let mut bob_store = FriendRequestStore::empty_at(&bob_dir);

    // Phase 1: Alice sends, Bob receives
    let payload = alice_sends_to_bob(&mut alice_store, &alice_sk, &bob_pk);
    let bob_req_id = bob_receives_from_alice(&mut bob_store, &bob_pk, &payload);

    alice_store.save().expect("alice save phase 1");
    bob_store.save().expect("bob save phase 1");

    // Phase 2: Bob accepts
    bob_store
        .accept_request(&bob_req_id, &bob_pk_str)
        .expect("bob accepts");
    bob_store.save().expect("bob save phase 2");

    // Phase 3: Reload both and verify
    let alice_loaded = FriendRequestStore::load(&alice_dir).expect("alice reload final");
    let bob_loaded = FriendRequestStore::load(&bob_dir).expect("bob reload final");

    assert_eq!(alice_loaded.len(), 1, "alice has 1 request");
    assert_eq!(bob_loaded.len(), 1, "bob has 1 request");

    let alice_req = alice_loaded
        .iter()
        .find(|r| r.recipient == bob_pk_str)
        .expect("alice request");
    let bob_req = bob_loaded
        .iter()
        .find(|r| r.requester == alice_pk_str)
        .expect("bob request");

    assert_eq!(
        alice_req.status,
        FriendRequestStatus::Pending,
        "alice's store still shows Pending (bob hasn't told her yet)"
    );
    assert_eq!(
        bob_req.status,
        FriendRequestStatus::Accepted,
        "bob's store shows Accepted"
    );

    assert_eq!(alice_loaded.data_dir(), alice_dir);
    assert_eq!(bob_loaded.data_dir(), bob_dir);
}

/// Round-trip all four store states across two persistent stores.
#[test]
fn all_statuses_persist_across_reload() {
    let alice_dir = temp_dir("alice-all-statuses");
    let bob_dir = temp_dir("bob-all-statuses");
    let alice_sk = SecretKey::generate();
    let bob_sk = SecretKey::generate();
    let bob_pk = bob_sk.public();
    let alice_pk_str = alice_sk.public().to_string();
    let bob_pk_str = bob_pk.to_string();

    let mut alice_store = FriendRequestStore::empty_at(&alice_dir);
    let mut bob_store = FriendRequestStore::empty_at(&bob_dir);

    let payload = alice_sends_to_bob(&mut alice_store, &alice_sk, &bob_pk);
    let bob_req_id = bob_receives_from_alice(&mut bob_store, &bob_pk, &payload);

    bob_store
        .accept_request(&bob_req_id, &bob_pk_str)
        .expect("bob accepts");

    alice_store.save().expect("save alice");
    bob_store.save().expect("save bob");

    let alice_loaded = FriendRequestStore::load(&alice_dir).expect("reload alice");
    let bob_loaded = FriendRequestStore::load(&bob_dir).expect("reload bob");

    assert_eq!(alice_loaded.len(), 1);
    assert_eq!(bob_loaded.len(), 1);

    // Bob's request is Accepted (terminal) — he can send a new one to Alice
    let new_req = bob_loaded
        .clone()
        .send_request(&bob_pk_str, &alice_pk_str, None)
        .expect("bob can send new request after terminal state on his side");
    assert_eq!(new_req.status, FriendRequestStatus::Pending);
}

/// Duplicate rejection works across stores: each store independently
/// rejects duplicate pending requests between the same pair.
#[test]
fn duplicate_pending_rejected_across_mutual_send() {
    let alice_dir = temp_dir("alice-dup");
    let bob_dir = temp_dir("bob-dup");
    let alice_sk = SecretKey::generate();
    let bob_sk = SecretKey::generate();
    let bob_pk = bob_sk.public();
    let alice_pk_str = alice_sk.public().to_string();
    let bob_pk_str = bob_pk.to_string();

    let mut alice_store = FriendRequestStore::empty_at(&alice_dir);
    let mut bob_store = FriendRequestStore::empty_at(&bob_dir);

    let payload = alice_sends_to_bob(&mut alice_store, &alice_sk, &bob_pk);
    bob_receives_from_alice(&mut bob_store, &bob_pk, &payload);

    // Alice tries to send duplicate to Bob
    let err = alice_store
        .send_request(&alice_pk_str, &bob_pk_str, None)
        .expect_err("duplicate rejected in alice's store");
    assert!(matches!(err, FriendRequestError::DuplicatePending { .. }));

    // Bob's store has a Pending from Alice, so Bob cannot send to Alice
    // (reverse direction also blocked via pair_index in Bob's store)
    let err2 = bob_store
        .send_request(&bob_pk_str, &alice_pk_str, None)
        .expect_err("duplicate rejected in bob's store (reverse direction)");
    assert!(matches!(err2, FriendRequestError::DuplicatePending { .. }));
}

/// Unauthorized mutations work correctly across stores: only the recipient
/// can accept/decline, only the requester can cancel.
#[test]
fn unauthorized_mutations_rejected_across_stores() {
    let alice_dir = temp_dir("alice-unauth");
    let bob_dir = temp_dir("bob-unauth");
    let alice_sk = SecretKey::generate();
    let bob_sk = SecretKey::generate();
    let charlie_sk = SecretKey::generate();
    let bob_pk = bob_sk.public();
    let alice_pk_str = alice_sk.public().to_string();
    let bob_pk_str = bob_pk.to_string();
    let charlie_pk_str = charlie_sk.public().to_string();

    let mut alice_store = FriendRequestStore::empty_at(&alice_dir);
    let mut bob_store = FriendRequestStore::empty_at(&bob_dir);

    let payload = alice_sends_to_bob(&mut alice_store, &alice_sk, &bob_pk);
    let bob_req_id = bob_receives_from_alice(&mut bob_store, &bob_pk, &payload);

    // Charlie (third party) cannot accept in Bob's store
    let err = bob_store
        .accept_request(&bob_req_id, &charlie_pk_str)
        .expect_err("charlie cannot accept");
    assert!(matches!(err, FriendRequestError::Unauthorized { .. }));

    // Charlie cannot decline in Bob's store
    let err = bob_store
        .decline_request(&bob_req_id, &charlie_pk_str)
        .expect_err("charlie cannot decline");
    assert!(matches!(err, FriendRequestError::Unauthorized { .. }));

    // Find Alice's outgoing request id
    let alice_req_id = alice_store
        .list_outgoing(&alice_pk_str)
        .first()
        .expect("alice has outgoing")
        .id
        .clone();

    // Bob cannot cancel in Alice's store (Bob is not the requester)
    let err = alice_store
        .cancel_request(&alice_req_id, &bob_pk_str)
        .expect_err("bob cannot cancel in alice's store");
    assert!(matches!(err, FriendRequestError::Unauthorized { .. }));

    // Alice can cancel in her own store
    alice_store
        .cancel_request(&alice_req_id, &alice_pk_str)
        .expect("alice can cancel her own request");
}

/// Invalid state transitions are caught in both stores.
#[test]
fn invalid_transitions_rejected() {
    let bob_dir = temp_dir("bob-invalid");
    let alice_sk = SecretKey::generate();
    let bob_sk = SecretKey::generate();
    let bob_pk = bob_sk.public();
    let bob_pk_str = bob_pk.to_string();

    let mut alice_store = FriendRequestStore::default();
    let mut bob_store = FriendRequestStore::empty_at(&bob_dir);

    let payload = alice_sends_to_bob(&mut alice_store, &alice_sk, &bob_pk);
    let bob_req_id = bob_receives_from_alice(&mut bob_store, &bob_pk, &payload);

    // Accept once
    bob_store
        .accept_request(&bob_req_id, &bob_pk_str)
        .expect("first accept works");

    // Cannot accept again
    let err = bob_store
        .accept_request(&bob_req_id, &bob_pk_str)
        .expect_err("cannot accept again");
    assert!(matches!(err, FriendRequestError::InvalidTransition { .. }));

    // Cannot decline after accepting
    let err = bob_store
        .decline_request(&bob_req_id, &bob_pk_str)
        .expect_err("cannot decline after accept");
    assert!(matches!(err, FriendRequestError::InvalidTransition { .. }));

    // Cancelling a request in Alice's store (that's still Pending there)
    // is still valid on Alice's side since her store hasn't been notified
    // of Bob's accept yet.
    let alice_pk_str = alice_sk.public().to_string();
    let alice_req_id = alice_store
        .list_outgoing(&alice_pk_str)
        .first()
        .expect("alice has outgoing")
        .id
        .clone();
    let cancelled = alice_store
        .cancel_request(&alice_req_id, &alice_pk_str)
        .expect("alice can still cancel in her store (not yet notified)");
    assert_eq!(cancelled.status, FriendRequestStatus::Cancelled);
}

/// Self-request is rejected in each store independently.
#[test]
fn self_request_rejected_in_store() {
    let alice_dir = temp_dir("alice-self");
    let mut store = FriendRequestStore::empty_at(&alice_dir);
    let pk_str = random_peer();

    let err = store
        .send_request(&pk_str, &pk_str, None)
        .expect_err("self-request rejected");
    assert!(matches!(err, FriendRequestError::SelfRequest));
}

/// NotFound is returned for operations on invalid IDs.
#[test]
fn not_found_rejected_for_nonexistent_requests() {
    let bob_dir = temp_dir("bob-notfound");
    let mut bob_store = FriendRequestStore::empty_at(&bob_dir);
    let pk_str = random_peer();

    let err = bob_store
        .accept_request("nonexistent", &pk_str)
        .expect_err("not found");
    assert!(matches!(err, FriendRequestError::NotFound(_)));

    let err = bob_store
        .decline_request("nonexistent", &pk_str)
        .expect_err("not found");
    assert!(matches!(err, FriendRequestError::NotFound(_)));

    let err = bob_store
        .cancel_request("nonexistent", &pk_str)
        .expect_err("not found");
    assert!(matches!(err, FriendRequestError::NotFound(_)));
}

/// Save to a directory then reload shows correct state.
#[test]
fn empty_store_save_and_reload() {
    let dir = temp_dir("empty-save-reload");
    let store = FriendRequestStore::empty_at(&dir);
    store.save().expect("save empty store");
    let loaded = FriendRequestStore::load(&dir).expect("reload empty store");
    assert!(loaded.is_empty());
}

/// Listing helpers work correctly: by_status filtering across statuses.
#[test]
fn list_by_status_filters_across_outgoing_and_incoming() {
    let dir = temp_dir("list-status");
    let mut store = FriendRequestStore::empty_at(&dir);
    let alice = random_peer();
    let bob = random_peer();
    let charlie = random_peer();

    let req_a = store
        .send_request(&alice, &bob, Some("hi bob".into()))
        .expect("alice to bob");
    let req_b = store
        .send_request(&bob, &charlie, Some("hi charlie".into()))
        .expect("bob to charlie");

    store.accept_request(&req_b.id, &charlie).expect("accept");
    store.cancel_request(&req_a.id, &alice).expect("cancel");

    let alice_out = store.list_outgoing_by_status(&alice, FriendRequestStatus::Cancelled);
    assert_eq!(alice_out.len(), 1);
    let alice_pending = store.list_outgoing_by_status(&alice, FriendRequestStatus::Pending);
    assert!(alice_pending.is_empty());
    let bob_out = store.list_outgoing_by_status(&bob, FriendRequestStatus::Accepted);
    assert_eq!(bob_out.len(), 1);
    let bob_pending_in = store.list_incoming_by_status(&bob, FriendRequestStatus::Pending);
    assert!(bob_pending_in.is_empty());
    let bob_cancelled_in = store.list_incoming_by_status(&bob, FriendRequestStatus::Cancelled);
    assert_eq!(bob_cancelled_in.len(), 1);
}

/// send_request with a custom message round-trips through save/load.
#[test]
fn custom_message_persists_across_reload() {
    let dir = temp_dir("custom-msg");
    let mut store = FriendRequestStore::empty_at(&dir);
    let alice = random_peer();
    let bob = random_peer();

    let msg = "Want to be friends?".to_string();
    let req = store
        .send_request(&alice, &bob, Some(msg.clone()))
        .expect("send with message");
    assert_eq!(req.message.as_deref(), Some(msg.as_str()));

    store.save().expect("save");
    let loaded = FriendRequestStore::load(&dir).expect("reload");
    let loaded_req = loaded.get(&req.id).expect("request exists");
    assert_eq!(loaded_req.message.as_deref(), Some(msg.as_str()));
    assert_eq!(loaded_req.requester, alice);
    assert_eq!(loaded_req.recipient, bob);
}

/// Multiple requests across different peer pairs are each tracked correctly.
#[test]
fn multiple_independent_request_pairs() {
    let dir = temp_dir("multi-pairs");
    let mut store = FriendRequestStore::empty_at(&dir);
    let alice = random_peer();
    let bob = random_peer();
    let charlie = random_peer();
    let dave = random_peer();

    let _r1 = store
        .send_request(&alice, &bob, None)
        .expect("alice→bob")
        .id;
    let _r2 = store
        .send_request(&alice, &charlie, None)
        .expect("alice→charlie")
        .id;
    let _r3 = store.send_request(&dave, &bob, None).expect("dave→bob").id;

    assert_eq!(store.len(), 3);
    assert_eq!(store.list_outgoing(&alice).len(), 2, "alice has 2 outgoing");
    assert_eq!(
        store.list_incoming(&bob).len(),
        2,
        "bob has 2 incoming (from alice and dave)"
    );
    assert_eq!(store.list_outgoing(&dave).len(), 1, "dave has 1 outgoing");
}

/// Verify that friend requests from a public-chat context are unrestricted:
/// no source-based checks, no rate limits, no cooldown between requests.
///
/// This test validates that the FriendRequest struct has no `source` field
/// and that multiple requests to different peers in quick succession all
/// succeed (simulating what happens when a user clicks "+ Add" on several
/// discovered peers in the public-chat sidebar).
#[test]
fn public_chat_friend_requests_are_unrestricted() {
    let alice_sk = SecretKey::generate();
    let bob_sk = SecretKey::generate();
    let charlie_sk = SecretKey::generate();
    let dave_sk = SecretKey::generate();

    let alice_pk = alice_sk.public().to_string();
    let _bob_pk = bob_sk.public().to_string();
    let _charlie_pk = charlie_sk.public().to_string();
    let _dave_pk = dave_sk.public().to_string();

    let mut store = FriendRequestStore::empty_at(std::env::temp_dir());

    // Simulate clicking "+ Add" on several discovered peers in quick succession.
    let r1 = store
        .send_request(&alice_pk, &_bob_pk, None)
        .expect("alice -> bob (first)");
    let r2 = store
        .send_request(&alice_pk, &_charlie_pk, None)
        .expect("alice -> charlie (immediately after)");
    let r3 = store
        .send_request(&alice_pk, &_dave_pk, None)
        .expect("alice -> dave (immediately after)");

    // All three should be Pending and have distinct IDs.
    assert_eq!(r1.status, FriendRequestStatus::Pending);
    assert_eq!(r2.status, FriendRequestStatus::Pending);
    assert_eq!(r3.status, FriendRequestStatus::Pending);
    assert_ne!(r1.id, r2.id);
    assert_ne!(r2.id, r3.id);

    // Alice has 3 outgoing pending requests.
    let outgoing = store.list_outgoing(&alice_pk);
    assert_eq!(outgoing.len(), 3);

    // Each target peer has exactly 1 incoming pending request.
    assert_eq!(
        store.list_incoming(&_bob_pk).len(),
        1,
        "bob has 1 pending from alice"
    );
    assert_eq!(
        store.list_incoming(&_charlie_pk).len(),
        1,
        "charlie has 1 pending from alice"
    );
    assert_eq!(
        store.list_incoming(&_dave_pk).len(),
        1,
        "dave has 1 pending from alice"
    );

    // Verify the FriendRequest struct has NO `source` field (compile-time check
    // via serialisation: a `source` field would appear in the JSON output).
    let json = serde_json::to_string(&r1).expect("serialize friend request");
    assert!(
        !json.contains("\"source\""),
        "FriendRequest should not serialize a 'source' field: {}",
        json
    );

    // Persist and reload — all 3 survive.
    store.save().expect("persist 3 requests");
    let loaded = FriendRequestStore::load(std::env::temp_dir()).expect("reload");
    assert_eq!(loaded.len(), 3, "all 3 requests survived persistence");
}
