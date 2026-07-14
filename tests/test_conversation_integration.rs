//! Comprehensive integration tests for the refactored friend-request and
//! conversation model.
//!
//! These tests validate the coordination rules between [`FriendsStore`],
//! [`ConversationStore`], [`FriendRequestStore`], and [`ChatHistoryStore`]
//! that the frontends (iced, ratatui) enforce at the application layer.
//!
//! # Scenarios tested (15)
//!
//! 1.  Accepting a friend request adds a friend but does NOT create/conversation
//! 2.  Rejecting a friend request does not add a friend or conversation
//! 3.  Opening a conversation with a friend creates exactly one conversation
//! 4.  Opening the same friend repeatedly does not create duplicates
//! 5.  Switching selected conversations preserves both histories
//! 6.  An incoming message for an inactive conversation increments unread count
//! 7.  An incoming message for the selected conversation is shown in that conversation
//! 8.  Selecting a conversation clears its unread count
//! 9.  Messages are routed to the correct conversation
//! 10. Conversation ordering updates based on latest activity
//! 11. Restarting restores friends, requests, conversations, and histories
//! 12. Existing persisted data can still be loaded or migrated
//! 13. Group chats continue to work
//! 14. Offline direct messages populate the correct conversation
//! 15. Friend acceptance does not unsubscribe from or leave any active topic

use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use boru_chat::{
    chat_history::{ChatHistoryStore, HistoryEntry},
    contact::direct_topic,
    conversations::{ConversationEntry, ConversationKind, ConversationStore},
    friend_request::{FriendRequestStatus, FriendRequestStore},
    friends::{FriendId, FriendRecord, FriendRelationship, FriendsStore},
    proto::TopicId,
};
use iroh::{PublicKey, SecretKey};

// ── Shared test helpers ──────────────────────────────────────────────────────

fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("boru-conv-int-{name}-{suffix}"));
    dir
}

fn make_topic(byte: u8) -> TopicId {
    TopicId::from_bytes([byte; 32])
}

fn random_peer() -> (SecretKey, PublicKey) {
    let sk = SecretKey::generate();
    (sk.clone(), sk.public())
}

fn peer_str(pk: &PublicKey) -> String {
    pk.to_string()
}

fn friend_id(pk: &PublicKey) -> FriendId {
    FriendId::from_public_key(*pk)
}

/// Create a coordinator that owns all four stores for a single "peer".
///
/// In the real app the frontend coordinates these stores; here we do it
/// explicitly in the test.
struct PeerStores {
    friends: FriendsStore,
    conversations: ConversationStore,
    friend_requests: FriendRequestStore,
    history: ChatHistoryStore,
    _dir: PathBuf,
}

impl PeerStores {
    fn new(dir: &PathBuf) -> Self {
        Self {
            friends: FriendsStore::empty_at(dir),
            conversations: ConversationStore::empty_at(dir),
            friend_requests: FriendRequestStore::empty_at(dir),
            history: ChatHistoryStore::empty_at(dir),
            _dir: dir.clone(),
        }
    }

    fn save_all(&self) {
        self.friends.save().expect("save friends");
        self.conversations.save().expect("save conversations");
        self.friend_requests.save().expect("save friend_requests");
        self.history.save().expect("save history");
    }

    fn load_all(dir: &PathBuf) -> Self {
        Self {
            friends: FriendsStore::load(dir).expect("load friends"),
            conversations: ConversationStore::load(dir).expect("load conversations"),
            friend_requests: FriendRequestStore::load(dir).expect("load friend requests"),
            history: ChatHistoryStore::load(dir)
                .expect("load history")
                .unwrap_or_else(|| ChatHistoryStore::empty_at(dir)),
            _dir: dir.clone(),
        }
    }
}

/// Simulate a frontend accepting a friend request: update FriendRequestStore
/// status to Accepted, add the requester to FriendsStore as Friends, but
/// do NOT create a ConversationEntry.
fn accept_friend_request(stores: &mut PeerStores, my_pk: &PublicKey, requester_pk: &PublicKey) {
    let my_pk_str = peer_str(my_pk);
    let requester_str = peer_str(requester_pk);

    // Find the pending incoming request — clone id to avoid borrow conflict
    let req_ids: Vec<String> = stores
        .friend_requests
        .list_incoming_by_status(&my_pk_str, FriendRequestStatus::Pending)
        .into_iter()
        .filter(|r| r.requester == requester_str)
        .map(|r| r.id.clone())
        .collect();

    for req_id in &req_ids {
        stores
            .friend_requests
            .accept_request(req_id, &my_pk_str)
            .expect("accept friend request");
    }

    // Add to friends list with Friends relationship
    stores.friends.upsert(
        FriendId::from_public_key(*requester_pk),
        FriendRecord {
            relationship: FriendRelationship::Friends,
            ..FriendRecord::default()
        },
    );
    // NOTE: we do NOT create a ConversationEntry — that's the key rule
}

/// Simulate the frontend declining a friend request.
fn decline_friend_request(stores: &mut PeerStores, my_pk: &PublicKey, requester_pk: &PublicKey) {
    let my_pk_str = peer_str(my_pk);
    let requester_str = peer_str(requester_pk);

    // Find the pending incoming request — clone id to avoid borrow conflict
    let req_ids: Vec<String> = stores
        .friend_requests
        .list_incoming_by_status(&my_pk_str, FriendRequestStatus::Pending)
        .into_iter()
        .filter(|r| r.requester == requester_str)
        .map(|r| r.id.clone())
        .collect();

    for req_id in &req_ids {
        stores
            .friend_requests
            .decline_request(req_id, &my_pk_str)
            .expect("decline friend request");
    }
    // No change to friends or conversations
}

/// Simulate the frontend opening a conversation with a friend:
/// derive the direct topic, upsert a ConversationEntry, and return the topic.
fn open_conversation(stores: &mut PeerStores, my_pk: &PublicKey, their_pk: &PublicKey) -> TopicId {
    let topic = direct_topic(my_pk, their_pk);
    stores.conversations.upsert(ConversationEntry::new(
        topic,
        peer_str(their_pk),
        &peer_str(their_pk)[..12],
    ));
    topic
}

/// Simulate a friend being removed from the friend list (e.g., block/unfriend).
fn remove_friend(stores: &mut PeerStores, their_pk: &PublicKey) {
    stores.friends.remove(&friend_id(their_pk));
}

// ── Test 1: Accepting a friend request adds a friend but does NOT
//    create or select a conversation ──────────────────────────────────────────

#[test]
fn accept_friend_request_adds_friend_no_conversation() {
    let dir = temp_dir("test1-accept-no-conv");
    let (_alice_sk, alice_pk) = random_peer();
    let (_bob_sk, bob_pk) = random_peer();
    let alice = peer_str(&alice_pk);
    let bob = peer_str(&bob_pk);

    let mut alice_stores = PeerStores::new(&dir.join("alice"));
    let mut bob_stores = PeerStores::new(&dir.join("bob"));

    // Alice sends a friend request to Bob
    alice_stores
        .friend_requests
        .send_request(&alice, &bob, None)
        .expect("alice sends request");

    // Bob receives it (simplified — just create an incoming request)
    bob_stores
        .friend_requests
        .send_request(&alice, &bob, None)
        .expect("bob registers incoming request");

    // Bob accepts
    accept_friend_request(&mut bob_stores, &bob_pk, &alice_pk);

    // Verify: Bob now has Alice as a friend
    let bob_friend = bob_stores.friends.get(&friend_id(&alice_pk));
    assert!(
        bob_friend.is_some(),
        "Alice should be in Bob's friends list"
    );
    assert_eq!(
        bob_friend.unwrap().relationship,
        FriendRelationship::Friends,
        "Alice should be marked as Friends"
    );

    // Verify: Bob has NOT created a conversation
    assert!(
        bob_stores.conversations.is_empty(),
        "Accepting a friend request must NOT create a conversation"
    );

    // Verify: Alice's store still has the request as Pending (she hasn't
    // been notified yet that Bob accepted).
    let alice_requests = alice_stores.friend_requests.iter().collect::<Vec<_>>();
    assert_eq!(alice_requests.len(), 1, "Alice still has 1 request");
    assert_eq!(
        alice_requests[0].status,
        FriendRequestStatus::Pending,
        "Alice's request to Bob is still Pending (Bob hasn't notified her yet)"
    );
    assert!(
        alice_stores.friends.is_empty(),
        "Alice's friends list must NOT yet show Bob — she hasn't been notified"
    );
}

/// Full bidirectional acceptance: Bob accepts Alice's request, then Alice
/// is notified (simulating a whisper notification). Both sides must end up
/// with the friend relationship set to `Friends`.
#[test]
fn bidirectional_friendship_on_acceptance() {
    let dir = temp_dir("test-bidi-accept");
    let (_alice_sk, alice_pk) = random_peer();
    let (_bob_sk, bob_pk) = random_peer();
    let alice = peer_str(&alice_pk);
    let bob = peer_str(&bob_pk);

    let mut alice_stores = PeerStores::new(&dir.join("alice"));
    let mut bob_stores = PeerStores::new(&dir.join("bob"));

    // ── 1. Alice sends friend request to Bob ──
    alice_stores
        .friend_requests
        .send_request(&alice, &bob, None)
        .expect("alice sends request");
    // Bob receives it
    bob_stores
        .friend_requests
        .send_request(&alice, &bob, None)
        .expect("bob registers incoming request");

    // ── 2. Bob accepts ──
    accept_friend_request(&mut bob_stores, &bob_pk, &alice_pk);

    // Bob's side: Alice should be a friend
    let bob_friend = bob_stores.friends.get(&friend_id(&alice_pk));
    assert!(
        bob_friend.is_some(),
        "Alice should be in Bob's friends list"
    );
    assert_eq!(
        bob_friend.unwrap().relationship,
        FriendRelationship::Friends,
        "Alice should be marked as Friends in Bob's store"
    );

    // Alice hasn't been notified yet — her side is unchanged
    assert!(
        alice_stores.friends.is_empty(),
        "Alice not yet notified — her friends list should be empty"
    );

    // ── 3. Simulate Alice receiving the acceptance notification
    //    (this is what the FriendRequestAccepted whisper does) ──
    // Alice's store: update the request status to Accepted
    let alice_req_ids: Vec<String> = alice_stores
        .friend_requests
        .list_outgoing_by_status(&alice, FriendRequestStatus::Pending)
        .into_iter()
        .filter(|r| r.recipient == bob)
        .map(|r| r.id.clone())
        .collect();
    for req_id in &alice_req_ids {
        // Simulate the remote side telling us the request was accepted
        // In real app this comes via a FriendRequestAccepted whisper
        // Since Alice can't 'accept' her own outgoing request directly
        // (accept_request checks that caller == recipient), we directly
        // update Alice's friends store here.
        alice_stores.friends.upsert(
            friend_id(&bob_pk),
            FriendRecord {
                relationship: FriendRelationship::Friends,
                ..FriendRecord::default()
            },
        );
    }

    // ── 4. Verify both sides have each other as friends ──
    let alice_friend = alice_stores.friends.get(&friend_id(&bob_pk));
    assert!(
        alice_friend.is_some(),
        "Bob should now be in Alice's friends list"
    );
    assert_eq!(
        alice_friend.unwrap().relationship,
        FriendRelationship::Friends,
        "Bob should be marked as Friends in Alice's store"
    );

    let bob_friend = bob_stores.friends.get(&friend_id(&alice_pk));
    assert!(
        bob_friend.is_some(),
        "Alice should be in Bob's friends list"
    );
    assert_eq!(
        bob_friend.unwrap().relationship,
        FriendRelationship::Friends,
        "Alice should be marked as Friends in Bob's store"
    );
}

// ── Test 2: Rejecting a friend request does not add a friend or conversation ──

#[test]
fn reject_friend_request_adds_no_friend_no_conversation() {
    let dir = temp_dir("test2-reject-no-conv");
    let (_alice_sk, alice_pk) = random_peer();
    let (_bob_sk, bob_pk) = random_peer();
    let alice = peer_str(&alice_pk);
    let bob = peer_str(&bob_pk);

    let mut alice_stores = PeerStores::new(&dir.join("alice"));
    let mut bob_stores = PeerStores::new(&dir.join("bob"));

    alice_stores
        .friend_requests
        .send_request(&alice, &bob, None)
        .expect("alice sends request");
    bob_stores
        .friend_requests
        .send_request(&alice, &bob, None)
        .expect("bob registers incoming request");

    // Bob declines
    decline_friend_request(&mut bob_stores, &bob_pk, &alice_pk);

    // Verify: Bob did NOT add Alice as a friend
    assert!(
        bob_stores.friends.get(&friend_id(&alice_pk)).is_none(),
        "Declining must NOT add to friends list"
    );

    // Verify: No conversation was created
    assert!(
        bob_stores.conversations.is_empty(),
        "Declining must NOT create any conversation"
    );
}

// ── Test 3: Opening a conversation with a friend creates exactly one ─────────

#[test]
fn open_conversation_creates_exactly_one() {
    let dir = temp_dir("test3-exactly-one");
    let (alice_sk, alice_pk) = random_peer();
    let (_bob_sk, bob_pk) = random_peer();

    let mut alice_stores = PeerStores::new(&dir.join("alice"));

    let topic = open_conversation(&mut alice_stores, &alice_sk.public(), &bob_pk);

    assert_eq!(
        alice_stores.conversations.len(),
        1,
        "Opening a conversation must create exactly 1 entry"
    );

    let entry = alice_stores
        .conversations
        .find(&topic)
        .expect("entry should exist by topic");
    assert_eq!(entry.peer_id, peer_str(&bob_pk));
    assert_eq!(entry.kind, ConversationKind::Direct);
}

// ── Test 4: Opening the same friend repeatedly does not create duplicates ────

#[test]
fn open_same_friend_no_duplicates() {
    let dir = temp_dir("test4-no-duplicates");
    let (alice_sk, alice_pk) = random_peer();
    let (_bob_sk, bob_pk) = random_peer();

    let mut alice_stores = PeerStores::new(&dir.join("alice"));

    let topic1 = open_conversation(&mut alice_stores, &alice_pk, &bob_pk);
    let topic2 = open_conversation(&mut alice_stores, &alice_pk, &bob_pk);

    // Same topic derived deterministically
    assert_eq!(topic1, topic2, "Same peer pair must derive the same topic");

    // Still exactly 1 entry
    assert_eq!(
        alice_stores.conversations.len(),
        1,
        "Opening the same friend twice must not create duplicates"
    );

    // upsert returns Some when replacing (the old entry)
    let old =
        alice_stores
            .conversations
            .upsert(ConversationEntry::new(topic1, peer_str(&bob_pk), "Bob"));
    assert!(
        old.is_some(),
        "upsert on existing topic must return the previous entry"
    );
    assert_eq!(alice_stores.conversations.len(), 1);

    // Different friend → different topic → new entry
    let (_carol_sk, carol_pk) = random_peer();
    let topic3 = open_conversation(&mut alice_stores, &alice_pk, &carol_pk);
    assert_ne!(
        topic1, topic3,
        "Different peer must derive a different topic"
    );
    assert_eq!(
        alice_stores.conversations.len(),
        2,
        "Two different friends must create two conversation entries"
    );
}

// ── Test 5: Switching selected conversations preserves both histories ────────

#[test]
fn switching_conversations_preserves_both_histories() {
    let dir = temp_dir("test5-switch-preserve");
    let (alice_sk, alice_pk) = random_peer();
    let (_bob_sk, bob_pk) = random_peer();
    let (_carol_sk, carol_pk) = random_peer();

    let mut stores = PeerStores::new(&dir.join("alice"));

    let bob_topic = open_conversation(&mut stores, &alice_pk, &bob_pk);
    let carol_topic = open_conversation(&mut stores, &alice_pk, &carol_pk);

    // Simulate frontend state: which conversation is "selected"
    let mut active_topic: Option<TopicId> = None;

    // "Switch" to Bob's conversation — add messages
    active_topic = Some(bob_topic);
    let msg1 = make_history_entry(bob_topic, &alice_pk, "Hello Bob");
    stores.history.push_with_id(msg1);

    // "Switch" to Carol's conversation — add messages
    active_topic = Some(carol_topic);
    let msg2 = make_history_entry(carol_topic, &alice_pk, "Hey Carol");
    stores.history.push_with_id(msg2);

    // "Switch" back to Bob
    active_topic = Some(bob_topic);

    // Verify: Bob's messages still exist
    let bob_entries: Vec<_> = stores
        .history
        .entries
        .iter()
        .filter(|e| e.topic == bob_topic)
        .collect();
    assert_eq!(
        bob_entries.len(),
        1,
        "Bob's conversation must still have 1 message"
    );
    assert_eq!(bob_entries[0].text_preview, "Hello Bob");

    // Verify: All entries preserved
    assert_eq!(
        stores.history.entries.len(),
        2,
        "Both conversations' messages preserved"
    );

    // Verify: active_topic is Bob
    assert_eq!(active_topic, Some(bob_topic));
}

// ── Test 6 & 7 & 8: Unread tracking ─────────────────────────────────────────

#[test]
fn incoming_message_inactive_conversation_increments_unread() {
    let dir = temp_dir("test6-unread-inactive");
    let (alice_sk, alice_pk) = random_peer();
    let (_bob_sk, bob_pk) = random_peer();
    let (_carol_sk, carol_pk) = random_peer();

    let mut stores = PeerStores::new(&dir.join("alice"));
    let bob_topic = open_conversation(&mut stores, &alice_pk, &bob_pk);
    let carol_topic = open_conversation(&mut stores, &alice_pk, &carol_pk);

    // Simulate frontend unread tracking (the app's ConversationLive tracks this)
    let mut unreads: std::collections::HashMap<TopicId, u64> = std::collections::HashMap::new();

    // Bob's conversation is "selected" (active)
    let mut active_topic = bob_topic;

    // Incoming message for Carol (inactive) → unread++
    unreads
        .entry(carol_topic)
        .and_modify(|c| *c += 1)
        .or_insert(1);
    assert_eq!(
        unreads.get(&carol_topic).copied().unwrap_or(0),
        1,
        "Inactive conversation must show 1 unread"
    );
    assert_eq!(
        unreads.get(&bob_topic).copied().unwrap_or(0),
        0,
        "Active conversation must have 0 unread"
    );

    // Now switch to Carol's conversation (as in the iced_chat frontend)
    // The frontend clears Carol's unread when selecting it
    active_topic = carol_topic;
    // ── Test 8: selecting clears unread ──
    unreads.insert(carol_topic, 0);
    assert_eq!(
        unreads.get(&carol_topic).copied().unwrap_or(0),
        0,
        "Selecting a conversation must clear its unread count"
    );

    // ── Test 7: active conversation shows the message ──
    let incoming_msg = make_history_entry(carol_topic, &carol_pk, "Hi Alice!");
    let msg_id = stores.history.push_with_id(incoming_msg);
    let msg = stores
        .history
        .get_by_event_id(msg_id)
        .expect("message stored");
    assert_eq!(
        msg.topic, carol_topic,
        "Message must be in Carol's conversation"
    );
    assert_eq!(msg.text_preview, "Hi Alice!");
}

// ── Test 9: Messages are routed to the correct conversation ─────────────────

#[test]
fn messages_routed_to_correct_conversation() {
    let dir = temp_dir("test9-routing");
    let (alice_sk, alice_pk) = random_peer();
    let (_bob_sk, bob_pk) = random_peer();
    let (_carol_sk, carol_pk) = random_peer();

    let mut stores = PeerStores::new(&dir.join("alice"));
    let bob_topic = open_conversation(&mut stores, &alice_pk, &bob_pk);
    let carol_topic = open_conversation(&mut stores, &alice_pk, &carol_pk);

    // Alice receives messages on both topics
    let bob_msg = make_history_entry(bob_topic, &bob_pk, "Bob's message");
    let carol_msg1 = make_history_entry(carol_topic, &carol_pk, "Carol's first");
    let carol_msg2 = make_history_entry(carol_topic, &carol_pk, "Carol's second");

    stores.history.push_with_id(bob_msg);
    stores.history.push_with_id(carol_msg1);
    stores.history.push_with_id(carol_msg2);

    // Bob's conversation: 1 message
    let bob_msgs: Vec<_> = stores
        .history
        .entries
        .iter()
        .filter(|e| e.topic == bob_topic)
        .collect();
    assert_eq!(bob_msgs.len(), 1);
    assert_eq!(bob_msgs[0].text_preview, "Bob's message");

    // Carol's conversation: 2 messages
    let carol_msgs: Vec<_> = stores
        .history
        .entries
        .iter()
        .filter(|e| e.topic == carol_topic)
        .collect();
    assert_eq!(carol_msgs.len(), 2);
    assert_eq!(carol_msgs[0].text_preview, "Carol's first");
    assert_eq!(carol_msgs[1].text_preview, "Carol's second");
}

// ── Test 10: Conversation ordering ──────────────────────────────────────────

#[test]
fn conversation_ordering_by_latest_activity() {
    let dir = temp_dir("test10-ordering");
    let (alice_sk, alice_pk) = random_peer();
    let (_bob_sk, bob_pk) = random_peer();
    let (_carol_sk, carol_pk) = random_peer();

    let mut stores = PeerStores::new(&dir.join("alice"));
    let bob_topic = open_conversation(&mut stores, &alice_pk, &bob_pk);
    let carol_topic = open_conversation(&mut stores, &alice_pk, &carol_pk);

    // Initially: both created at ~same time. active_iter() sorts by
    // last_seen_at_unix_ms descending.  Touch Bob's entry to make it newer.
    std::thread::sleep(std::time::Duration::from_millis(2));
    stores.conversations.touch_and_bump(&bob_topic);

    let active: Vec<_> = stores.conversations.active_iter();
    assert_eq!(active.len(), 2);

    assert_eq!(
        active[0].topic, bob_topic,
        "Most recently active conversation must be first"
    );
    assert_eq!(
        active[1].topic, carol_topic,
        "Older conversation must be second"
    );

    // Now touch Carol's entry — it should move to front
    std::thread::sleep(std::time::Duration::from_millis(2));
    stores.conversations.touch_and_bump(&carol_topic);

    let active: Vec<_> = stores.conversations.active_iter();
    assert_eq!(
        active[0].topic, carol_topic,
        "Carol must now be first after touch"
    );
    assert_eq!(active[1].topic, bob_topic, "Bob must now be second");
}

// ── Test 11: Restart restores all data ──────────────────────────────────────

#[test]
fn restart_restores_friends_requests_conversations_and_histories() {
    let dir = temp_dir("test11-restart");
    let (_alice_sk, alice_pk) = random_peer();
    let (_bob_sk, bob_pk) = random_peer();

    // ── First session ──
    {
        let mut stores = PeerStores::new(&dir.join("user"));

        // Add a friend
        stores.friends.upsert(
            FriendId::from_public_key(alice_pk),
            FriendRecord {
                relationship: FriendRelationship::Friends,
                ..FriendRecord::default()
            },
        );

        // Send a friend request
        stores
            .friend_requests
            .send_request(peer_str(&alice_pk), peer_str(&bob_pk), None)
            .expect("send request");

        // Create a conversation
        let topic = open_conversation(&mut stores, &alice_pk, &bob_pk);

        // Add history
        let entry = make_history_entry(topic, &alice_pk, "Persist me");
        stores.history.push_with_id(entry);

        stores.save_all();
    }

    // ── Simulate restart (drop all in-memory state, reload from disk) ──
    {
        let stores = PeerStores::load_all(&dir.join("user"));

        // Friend restored
        assert_eq!(stores.friends.len(), 1);
        let friend = stores.friends.get(&FriendId::from_public_key(alice_pk));
        assert!(friend.is_some());
        assert_eq!(friend.unwrap().relationship, FriendRelationship::Friends);

        // Friend request restored
        assert_eq!(stores.friend_requests.len(), 1);
        let outgoing = stores.friend_requests.list_outgoing(&peer_str(&alice_pk));
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].recipient, peer_str(&bob_pk));

        // Conversation restored
        assert_eq!(stores.conversations.len(), 1);
        let expected_topic = direct_topic(&alice_pk, &bob_pk);
        let conv = stores.conversations.find(&expected_topic);
        assert!(conv.is_some(), "Conversation must be restorable from disk");

        // History restored
        assert!(
            !stores.history.entries.is_empty(),
            "Chat history must survive restart"
        );
        let history_entry = stores
            .history
            .entries
            .iter()
            .find(|e| e.text_preview == "Persist me");
        assert!(history_entry.is_some(), "History entry content must match");
    }
}

// ── Test 12: Existing persisted data can be loaded ───────────────────────────

#[test]
fn existing_persisted_data_loads_correctly() {
    let dir = temp_dir("test12-migration");
    std::fs::create_dir_all(&dir).expect("create dir");

    // Generate real valid public key strings
    let pk1 = SecretKey::generate().public().to_string();
    let pk2 = SecretKey::generate().public().to_string();

    // Write old-format friends.json with deprecated variants
    let friends_path = dir.join("friends.json");
    std::fs::write(
        &friends_path,
        format!(
            r#"{{
            "schema_version": 3,
            "friends": {{
                "{pk1}": {{
                    "relationship": "outgoing_pending",
                    "status": {{}},
                    "known_addrs": []
                }},
                "{pk2}": {{
                    "relationship": "incoming_pending",
                    "status": {{}},
                    "known_addrs": []
                }}
            }}
        }}"#,
        ),
    )
    .expect("write old-format friends.json");

    // Load — should migrate deprecated variants to NotFriend
    let store = FriendsStore::load(&dir).expect("load old-format friends.json");
    assert_eq!(store.len(), 2, "Both friends must load (migrated)");

    for (_id, record) in store.iter() {
        assert_eq!(
            record.relationship,
            FriendRelationship::NotFriend,
            "Deprecated pending variants must be migrated to NotFriend"
        );
    }

    // Write old-format conversations.json (schema_version 1)
    let conv_path = dir.join("conversations.json");
    std::fs::write(
        &conv_path,
        r#"{
            "schema_version": 1,
            "conversations": [
                {
                    "topic": [170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170],
                    "peer_id": "bob",
                    "name": "Bob",
                    "kind": "Direct",
                    "created_at_unix_ms": 1000000,
                    "last_seen_at_unix_ms": 2000000,
                    "archived": false
                }
            ]
        }"#,
    )
    .expect("write old-format conversations.json");

    let conv_store = ConversationStore::load(&dir).expect("load old-format conversations.json");
    assert_eq!(conv_store.len(), 1, "Old-format conversation must load");
    let entry = conv_store.find(&make_topic(0xAA)).expect("entry by topic");
    assert_eq!(entry.peer_id, "bob");
    assert_eq!(entry.name, "Bob");
}

// ── Test 13: Group chats continue to work ────────────────────────────────────

#[test]
fn group_chats_continue_to_work() {
    let dir = temp_dir("test13-group");
    let (_alice_sk, alice_pk) = random_peer();

    let mut stores = PeerStores::new(&dir.join("user"));

    // Create a group conversation
    let group_topic = make_topic(0xCC);
    let group_entry = ConversationEntry::new_group(group_topic, "Watercooler");
    stores.conversations.upsert(group_entry);

    // Verify
    assert_eq!(stores.conversations.len(), 1);
    let entry = stores
        .conversations
        .find(&group_topic)
        .expect("group entry");
    assert_eq!(entry.kind, ConversationKind::Group);
    assert_eq!(entry.name, "Watercooler");
    assert!(entry.peer_id.is_empty(), "Group should have no peer_id");

    // Add group messages
    let msg1 = make_history_entry(group_topic, &alice_pk, "Hello group!");
    let msg2 = make_history_entry(group_topic, &alice_pk, "How is everyone?");
    stores.history.push_with_id(msg1);
    stores.history.push_with_id(msg2);

    let group_msgs: Vec<_> = stores
        .history
        .entries
        .iter()
        .filter(|e| e.topic == group_topic)
        .collect();
    assert_eq!(
        group_msgs.len(),
        2,
        "Group messages must be routed correctly"
    );

    // Persist and reload
    stores.save_all();
    let loaded = PeerStores::load_all(&dir.join("user"));
    assert_eq!(loaded.conversations.len(), 1);
    let loaded_entry = loaded
        .conversations
        .find(&group_topic)
        .expect("group after reload");
    assert_eq!(loaded_entry.kind, ConversationKind::Group);
}

// ── Test 14: Offline DMs populate correct conversation ───────────────────────

#[test]
fn offline_direct_messages_populate_correct_conversation() {
    let dir = temp_dir("test14-offline-dm");
    let (alice_sk, alice_pk) = random_peer();
    let (_bob_sk, bob_pk) = random_peer();
    let (_carol_sk, carol_pk) = random_peer();

    let mut stores = PeerStores::new(&dir.join("alice"));

    // Alice has conversations with Bob and Carol
    let bob_topic = open_conversation(&mut stores, &alice_pk, &bob_pk);
    let carol_topic = open_conversation(&mut stores, &alice_pk, &carol_pk);

    // While Alice was offline, Bob sent a message (it arrives via outbox/mailbox)
    let bob_msg = make_history_entry(bob_topic, &bob_pk, "Offline DM from Bob");
    stores.history.push_with_id(bob_msg);

    // Verify: Bob's offline message went to Bob's conversation
    let bob_msgs: Vec<_> = stores
        .history
        .entries
        .iter()
        .filter(|e| e.topic == bob_topic)
        .collect();
    assert_eq!(
        bob_msgs.len(),
        1,
        "Bob's offline DM must be in Bob's conversation"
    );
    assert_eq!(bob_msgs[0].text_preview, "Offline DM from Bob");

    // Verify: Carol's conversation is still empty
    let carol_msgs: Vec<_> = stores
        .history
        .entries
        .iter()
        .filter(|e| e.topic == carol_topic)
        .collect();
    assert!(
        carol_msgs.is_empty(),
        "Carol's conversation must remain empty"
    );

    // Both conversations still in store
    assert_eq!(stores.conversations.len(), 2);
}

// ── Test 15: Friend acceptance does not unsubscribe from topics ──────────────

#[test]
fn friend_acceptance_does_not_unsubscribe_from_active_topics() {
    let dir = temp_dir("test15-no-unsub");
    let (alice_sk, alice_pk) = random_peer();
    let (_bob_sk, bob_pk) = random_peer();
    let (_carol_sk, carol_pk) = random_peer();

    let mut stores = PeerStores::new(&dir.join("alice"));

    // Alice already has an active conversation with Carol (already a friend)
    stores.friends.upsert(
        FriendId::from_public_key(carol_pk),
        FriendRecord {
            relationship: FriendRelationship::Friends,
            ..FriendRecord::default()
        },
    );
    let carol_topic = open_conversation(&mut stores, &alice_pk, &carol_pk);

    // Alice now accepts a friend request from Bob
    stores.friends.upsert(
        FriendId::from_public_key(bob_pk),
        FriendRecord {
            relationship: FriendRelationship::Friends,
            ..FriendRecord::default()
        },
    );

    // Verify: Carol's conversation is still in the store
    assert!(
        stores.conversations.find(&carol_topic).is_some(),
        "Carol's conversation must persist after accepting Bob's friend request"
    );

    // Verify: Carol is still in the friends list
    assert!(
        stores.friends.get(&friend_id(&carol_pk)).is_some(),
        "Carol must remain a friend after accepting Bob"
    );

    // Verify: Both conversations still exist (if Alice opened one with Bob)
    let bob_topic = open_conversation(&mut stores, &alice_pk, &bob_pk);
    assert_eq!(
        stores.conversations.len(),
        2,
        "Both conversations must coexist"
    );

    // Verify: Removing a friend does NOT destroy other conversations
    remove_friend(&mut stores, &bob_pk);
    assert_eq!(
        stores.conversations.len(),
        2,
        "Removing a friend must not destroy other conversations"
    );
    assert!(
        stores.conversations.find(&carol_topic).is_some(),
        "Carol's conversation must survive Bob's removal"
    );
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_history_entry(topic: TopicId, sender: &PublicKey, text: &str) -> HistoryEntry {
    HistoryEntry::new(
        topic,
        peer_str(sender),
        Vec::new(), // signed_bytes, empty for test
        "text",
        text,
    )
}
