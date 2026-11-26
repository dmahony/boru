use super::common;
use boru_core::friends::{FriendId, FriendRecord, FriendRelationship, FriendsStore};
use serde_json::json;

/// Persist one friend record so the tunnel IPC commands see a real trust record.
fn seed_friend(
    harness: &common::Harness,
    public_key: iroh::PublicKey,
    relationship: FriendRelationship,
    label: &str,
) {
    let runtime = harness.runtime();
    let runtime = runtime.as_ref().expect("fixture runtime available");
    let mut friends = FriendsStore::load_from_sqlite(runtime.storage(), runtime.data_dir());
    let record = FriendRecord {
        relationship,
        label: Some(label.to_string()),
        ..FriendRecord::default()
    };
    friends.upsert(FriendId::from_public_key(public_key), record);
    friends
        .save_to_sqlite(runtime.storage())
        .expect("persist friends store");
}

fn offer_args(peer_id: String, port: u16) -> serde_json::Value {
    json!({
        "peerId": peer_id,
        "port": port,
        "serviceName": "fixture-local-service",
        "isHttp": false,
        "expiresInSeconds": 600u64,
        "preferredLocalPort": null,
    })
}

/// A peer without an accepted friendship must be rejected before any tunnel exists.
#[tokio::test]
async fn submit_tunnel_offer_rejects_untrusted_peer() {
    let harness = common::Harness::new().await;
    let stranger = iroh::SecretKey::generate().public();

    let error = harness
        .invoke("submit_tunnel_offer", offer_args(stranger.to_string(), 1))
        .expect_err("an untrusted peer must be rejected");

    assert!(error.contains("untrusted_peer"), "expected untrusted_peer, got: {error}");
    let tunnels = harness
        .invoke("list_tunnels", json!({}))
        .expect("list tunnels after rejection");
    assert_eq!(
        tunnels.as_array().map(|entries| entries.len()).unwrap_or(0),
        0,
        "rejecting an untrusted peer must not create a tunnel"
    );
}

/// An accepted friend offering a port with no listener is an unknown local service.
#[tokio::test]
async fn submit_tunnel_offer_rejects_unknown_local_service() {
    let harness = common::Harness::new().await;
    let friend = iroh::SecretKey::generate().public();
    seed_friend(&harness, friend, FriendRelationship::Friends, "Ada");

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("reserve a free port");
    let closed_port = listener.local_addr().expect("reserved address").port();
    drop(listener);

    let error = harness
        .invoke("submit_tunnel_offer", offer_args(friend.to_string(), closed_port))
        .expect_err("a closed local port must be rejected");

    assert!(
        error.contains("unknown_local_service"),
        "expected unknown_local_service, got: {error}"
    );
}

/// Only accepted friends are offered as tunnel peers, with their label as alias.
#[tokio::test]
async fn list_trusted_peers_projects_only_accepted_friends() {
    let harness = common::Harness::new().await;
    let friend = iroh::SecretKey::generate().public();
    let pending = iroh::SecretKey::generate().public();
    seed_friend(&harness, friend, FriendRelationship::Friends, "Ada");
    seed_friend(&harness, pending, FriendRelationship::OutgoingPending, "Pending");

    let peers = harness
        .invoke("list_trusted_peers", json!({}))
        .expect("list trusted peers");
    let peers = peers.as_array().expect("array of trusted peers");

    assert_eq!(peers.len(), 1, "only accepted friends are trusted peers: {peers:?}");
    assert_eq!(peers[0]["peerId"], json!(friend.to_string()));
    assert_eq!(peers[0]["alias"], json!("Ada"));
}
