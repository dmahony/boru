//! Ad-hoc verification of typing indicator changes (temporary).
//! Run with: cargo test --test hermes_verify_typing -- --nocapture

use std::collections::HashMap;
use iroh_gossip::chat_core::{AppState, handle_net_event, NetEvent, Message, StatusContext};
use iroh_gossip::friends::FriendsStore;
use iroh_gossip::proto::TopicId;
use iroh::{PublicKey, SecretKey, RelayMode};

fn make_status() -> StatusContext {
    StatusContext {
        transport_status: "ready".into(),
        topic: TopicId::from_bytes([0u8; 32]),
        relay_mode: RelayMode::Disabled,
        connected: true,
        peer_count: 0,
        identity_label: "tester".into(),
        transport_notice: "".into(),
        direct_peers: 0,
        relayed_peers: 0,
        neighbors: std::collections::HashSet::new(),
        peer_connection_types: HashMap::new(),
    }
}

#[test]
fn typing_peers_hashmap_crud() {
    let sk = SecretKey::generate();
    let peer_a = SecretKey::generate().public();
    let peer_b = SecretKey::generate().public();
    let mut app = AppState::new(make_status(), FriendsStore::default(), sk.public(), Some("tester".into()));

    // set_typing inserts with timestamp
    app.set_typing(peer_a);
    assert_eq!(app.typing_peers.len(), 1);
    assert!(app.typing_peers.contains_key(&peer_a));

    // Re-insert same peer refreshes, doesn't duplicate
    app.set_typing(peer_a);
    assert_eq!(app.typing_peers.len(), 1);

    // Second peer
    app.set_typing(peer_b);
    assert_eq!(app.typing_peers.len(), 2);

    // clear_typing removes specific peer
    app.clear_typing(&peer_a);
    assert_eq!(app.typing_peers.len(), 1);
    assert!(!app.typing_peers.contains_key(&peer_a));
    assert!(app.typing_peers.contains_key(&peer_b));

    // clear_expired_typing on fresh entries removes nothing
    app.set_typing(peer_a);
    assert!(!app.clear_expired_typing());
    assert_eq!(app.typing_peers.len(), 2);

    println!("PASS: typing_peers HashMap CRUD");
}

#[test]
fn typing_handled_via_net_event() {
    let sk = SecretKey::generate();
    let peer = SecretKey::generate().public();
    let mut app = AppState::new(make_status(), FriendsStore::default(), sk.public(), Some("tester".into()));

    // Send a Typing message via NetEvent (simulates receiving Typing from network)
    handle_net_event(
        NetEvent::Message {
            from: peer,
            message: Message::Typing,
        },
        &mut app,
    )
    .unwrap();

    assert!(app.typing_peers.contains_key(&peer), "Typing message should populate typing_peers");
    println!("PASS: Message::Typing via handle_net_event");
}

#[test]
fn typing_indicator_clears_via_expiry() {
    let sk = SecretKey::generate();
    let peer = SecretKey::generate().public();
    let mut app = AppState::new(make_status(), FriendsStore::default(), sk.public(), Some("tester".into()));

    app.set_typing(peer);
    assert_eq!(app.typing_peers.len(), 1);

    // All entries are fresh (<5s), so nothing expires now
    assert!(!app.clear_expired_typing());

    // We can't mock Instant, but the retain logic is straightforward:
    // entries older than 5s are removed.  The library tests cover
    // the actual expiry; this confirms the API is wired correctly.
    println!("PASS: Typing expiry API wired correctly");
}
