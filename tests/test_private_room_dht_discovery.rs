#![cfg(feature = "net")]

//! Offline coverage for private-room DHT discovery.
//!
//! Every discovery test uses the shared in-memory backend.  No socket, DHT,
//! relay, or DNS lookup is involved.

use std::str::FromStr;
use std::time::Duration;

use boru_chat::chat_core::Ticket;
use boru_chat::discovery_backend::{InMemoryDiscoveryBackend, TopicDiscoveryBackend};
use boru_chat::discovery_secret::DiscoverySecret;
use boru_chat::private_room_tracker::PrivateRoomTracker;
use boru_chat::proto::TopicId;
use boru_chat::room::RoomStore;
use iroh::{EndpointAddr, SecretKey};
use tempfile::tempdir;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn topic(byte: u8) -> TopicId {
    TopicId::from_bytes([byte; 32])
}

fn identity() -> (SecretKey, iroh::EndpointId) {
    let key = SecretKey::generate();
    let endpoint = key.public();
    (key, endpoint)
}

fn tracker(
    backend: &InMemoryDiscoveryBackend,
    room_topic: TopicId,
    secret: DiscoverySecret,
) -> (PrivateRoomTracker, iroh::EndpointId) {
    let (key, endpoint) = identity();
    (
        PrivateRoomTracker::new(Box::new(backend.clone()), room_topic, secret, endpoint, key),
        endpoint,
    )
}

#[test]
fn discovery_secret_generation_and_round_trip() {
    let secret = DiscoverySecret::generate();
    assert!(secret.as_bytes().iter().any(|byte| *byte != 0));
    let restored = DiscoverySecret::from_bytes(*secret.as_bytes());
    assert_eq!(restored, secret);
    assert_eq!(restored.as_namespace_id().as_bytes(), secret.as_bytes());
}

#[test]
fn ticket_serialization_preserves_secret_and_legacy_shape() -> TestResult {
    let room_topic = topic(0x11);
    let peer = EndpointAddr::new(identity().1);
    let secret = DiscoverySecret::from_bytes([0xAB; 32]);

    let with_secret = Ticket::with_discovery(room_topic, vec![peer.clone()], secret);
    assert_eq!(Ticket::from_bytes(&with_secret.to_bytes())?, with_secret);
    assert_eq!(Ticket::from_str(&with_secret.to_string())?, with_secret);

    let legacy = Ticket::new(room_topic, vec![peer]);
    let decoded = Ticket::from_bytes(&legacy.to_bytes())?;
    assert_eq!(decoded.discovery_secret, None);
    assert_eq!(Ticket::from_str(&legacy.to_string())?, legacy);
    Ok(())
}

#[test]
fn room_store_v2_migrates_without_secret() -> TestResult {
    let dir = tempdir()?;
    let raw = serde_json::json!({
        "schema_version": 2,
        "topic": vec![0x42u8; 32],
        "peers": [],
    });
    std::fs::write(dir.path().join("room.json"), raw.to_string())?;

    let loaded = RoomStore::load(dir.path())?.expect("v2 room should load");
    assert_eq!(loaded.schema_version, 3);
    assert_eq!(loaded.topic, topic(0x42));
    assert_eq!(loaded.discovery_secret, None);

    let migrated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("room.json"))?)?;
    assert_eq!(migrated["schema_version"], 3);
    assert!(migrated.get("discovery_secret").is_some());
    Ok(())
}

#[tokio::test]
async fn two_peers_publish_and_discover_without_network() -> TestResult {
    let backend = InMemoryDiscoveryBackend::new();
    let secret = DiscoverySecret::generate();
    let (alice, alice_id) = tracker(&backend, topic(1), secret);
    let (bob, _) = tracker(&backend, topic(1), secret);

    alice.publish_once().await?;
    assert_eq!(bob.discover_once().await?, vec![alice_id]);
    Ok(())
}

#[tokio::test]
async fn three_peers_form_a_chain() -> TestResult {
    let backend = InMemoryDiscoveryBackend::new();
    let secret = DiscoverySecret::generate();
    let (alice, alice_id) = tracker(&backend, topic(2), secret);
    let (bob, bob_id) = tracker(&backend, topic(2), secret);
    let (carol, carol_id) = tracker(&backend, topic(2), secret);

    alice.publish_once().await?;
    bob.publish_once().await?;
    carol.publish_once().await?;

    let bob_peers = bob.discover_once().await?;
    let carol_peers = carol.discover_once().await?;
    assert!(bob_peers.contains(&alice_id));
    assert!(bob_peers.contains(&carol_id));
    assert!(carol_peers.contains(&alice_id));
    assert!(carol_peers.contains(&bob_id));
    Ok(())
}

#[tokio::test]
async fn creator_offline_does_not_prevent_remaining_peers() -> TestResult {
    let backend = InMemoryDiscoveryBackend::new();
    let secret = DiscoverySecret::generate();
    let (creator, _) = tracker(&backend, topic(3), secret);
    creator.publish_once().await?;
    drop(creator);

    let (bob, bob_id) = tracker(&backend, topic(3), secret);
    let (carol, carol_id) = tracker(&backend, topic(3), secret);
    bob.publish_once().await?;
    carol.publish_once().await?;

    assert!(bob.discover_once().await?.contains(&carol_id));
    assert!(carol.discover_once().await?.contains(&bob_id));
    Ok(())
}

#[tokio::test]
async fn different_secrets_are_namespace_isolated() -> TestResult {
    let backend = InMemoryDiscoveryBackend::new();
    let (room_a, _) = tracker(&backend, topic(4), DiscoverySecret::from_bytes([1; 32]));
    let (room_b, _) = tracker(&backend, topic(4), DiscoverySecret::from_bytes([2; 32]));
    room_a.publish_once().await?;
    assert!(room_b.discover_once().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn self_and_duplicate_records_are_filtered() -> TestResult {
    let backend = InMemoryDiscoveryBackend::new();
    let secret = DiscoverySecret::generate();
    let (alice, alice_id) = tracker(&backend, topic(5), secret);
    let (bob, _) = tracker(&backend, topic(5), secret);
    alice.publish_once().await?;
    alice.publish_once().await?;
    assert_eq!(
        bob.discover_once()
            .await?
            .iter()
            .filter(|id| **id == alice_id)
            .count(),
        1
    );

    let (self_tracker, _) = tracker(&backend, topic(6), secret);
    self_tracker.publish_once().await?;
    assert!(self_tracker.discover_once().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn empty_discovery_leaves_ticket_bootstrap_fallback_available() -> TestResult {
    let backend = InMemoryDiscoveryBackend::new();
    let (tracker, _) = tracker(&backend, topic(7), DiscoverySecret::generate());
    assert!(tracker.discover_once().await?.is_empty());

    let peer = EndpointAddr::new(identity().1);
    let ticket = Ticket::new(topic(7), vec![peer.clone()]);
    assert_eq!(ticket.peers, vec![peer]);
    Ok(())
}

#[tokio::test]
async fn shutdown_is_idempotent_for_in_memory_backend_and_does_not_hang() -> TestResult {
    let backend = InMemoryDiscoveryBackend::new();
    let (tracker, _) = tracker(&backend, topic(8), DiscoverySecret::generate());
    tokio::time::timeout(Duration::from_secs(1), tracker.shutdown()).await?;
    let _ = tokio::time::timeout(Duration::from_secs(1), backend.shutdown()).await?;
    assert!(backend
        .lookup(&boru_chat::discovery_backend::NamespaceId::new([0; 32]))
        .await?
        .is_empty());
    Ok(())
}
