//! Unit and integration tests for the history backfill protocol.
//!
//! These cover wire round-trips, authorization (BORU-AUDIT-01), rate
//! limiting, and end-to-end request/response exchanges over loopback QUIC
//! with `RelayMode::Disabled`, so they never touch the debsrv prod relay.

// Imported from the original `use super::*` in the flat module (these are
// the crate items the old top-level backfill.rs brought into test scope).
use super::client::do_backfill_request;
use super::rate_limit::BackfillRateLimit;
use super::server::serve_backfill;
use super::wire::{BackfillRequest, BackfillResponse};
use super::*;
use crate::chat_core::{Message, SignedMessage};
use crate::contact::direct_topic;
use crate::proto::TopicId;
use crate::public_room::{public_lobby_topic, PublicNetwork};
use crate::storage::{GroupEpochRow, GroupMemberRow, GroupRow, Storage};
use bytes::Bytes;
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr, PublicKey, SecretKey,
};
use n0_error::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

fn backfill_request_roundtrips() {
    let req = BackfillRequest {
        since_ms: 1000,
        max_messages: 50,
        topic: None,
    };
    let bytes = postcard::to_stdvec(&req).unwrap();
    let decoded: BackfillRequest = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(decoded.since_ms, 1000);
    assert_eq!(decoded.max_messages, 50);
}

#[test]
fn backfill_response_roundtrips() {
    let resp = BackfillResponse {
        messages: vec![Bytes::from(vec![1u8; 64]), Bytes::from(vec![2u8; 64])],
        skipped: 10,
        truncated_by_bytes: false,
    };
    let bytes = postcard::to_stdvec(&resp).unwrap();
    let decoded: BackfillResponse = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(decoded.messages.len(), 2);
    assert_eq!(decoded.skipped, 10);
    assert!(!decoded.truncated_by_bytes);
    assert_eq!(decoded.messages[0].as_ref(), &[1u8; 64]);
}

/// The internal DISCOVERY topic is networking infrastructure, never a
/// conversation store (BORU-DISC-13): backfill authorization must deny
/// it for every network — even to the node's own key or a peer that
/// could derive the topic. Discovery payloads must never be served as
/// chat history.
#[test]
fn authorize_denies_discovery_topic() {
    let local = SecretKey::generate();
    let storage = Arc::new(Storage::memory().unwrap());
    let authorizer = BackfillAuthorizer::new(storage, local.public());
    let peer = SecretKey::generate().public();
    for network in [
        crate::public_room::PublicNetwork::Mainnet,
        crate::public_room::PublicNetwork::Development,
        crate::public_room::PublicNetwork::Test,
    ] {
        let topic = crate::discovery_topic::discovery_topic(network);
        assert!(
            !authorizer.authorize(&local.public(), &topic),
            "discovery topic must be denied backfill on {network:?}"
        );
        assert!(
            !authorizer.authorize(&peer, &topic),
            "discovery topic must be denied backfill for any peer on {network:?}"
        );
    }
    // Positive control: a real direct-chat topic between the two
    // participants remains authorized, proving the exclusion did not
    // weaken conversation backfill.
    let direct = crate::contact::direct_topic(&local.public(), &peer);
    assert!(
        authorizer.authorize(&peer, &direct),
        "direct-chat topic must remain authorized"
    );
}

#[test]
fn backfill_rate_limit_accept_once() {
    let mut rl = BackfillRateLimit::default();
    let pk = SecretKey::generate().public();
    assert!(rl.try_accept(pk));
    assert!(!rl.try_accept(pk));
    rl.release(&pk);
    assert!(rl.try_accept(pk));
}

#[test]
fn backfill_rate_limit_multiple_peers() {
    let mut rl = BackfillRateLimit::default();
    let pk1 = SecretKey::generate().public();
    let pk2 = SecretKey::generate().public();
    assert!(rl.try_accept(pk1));
    assert!(rl.try_accept(pk2));
    assert!(!rl.try_accept(pk1));
    assert!(!rl.try_accept(pk2));
}

/// The GUI has no scroll-triggered pagination: history is loaded
/// wholesale on open, and backfill is network-driven, gated by
/// [`BACKFILL_TRIGGER_THRESHOLD`].  This pins the gate itself — when the
/// local history count meets the threshold no request is made (and no
/// network round trip is attempted), and an unknown peer below the
/// threshold degrades to `Ok(None)` rather than erroring.
#[tokio::test]
async fn try_backfill_skips_when_history_at_or_above_threshold() {
    let ep = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
        .secret_key(SecretKey::generate())
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .unwrap()
        .bind()
        .await
        .expect("bind endpoint");
    let handle = BackfillHandle::spawn(ep.clone());
    let peer = SecretKey::generate().public();
    let topic = TopicId::from_bytes([0u8; 32]);
    let (net_tx, _net_rx) = mpsc::channel(16);

    // At exactly the threshold: no backfill request.
    let at = handle
        .try_backfill_from_peer(
            &ep,
            peer,
            BACKFILL_TRIGGER_THRESHOLD,
            topic,
            net_tx.clone(),
            None,
        )
        .await
        .expect("threshold skip is not an error");
    assert_eq!(at, None, "at threshold → no backfill request");

    // Above the threshold: no backfill request.
    let above = handle
        .try_backfill_from_peer(
            &ep,
            peer,
            BACKFILL_TRIGGER_THRESHOLD + 10,
            topic,
            net_tx.clone(),
            None,
        )
        .await
        .expect("above-threshold skip is not an error");
    assert_eq!(above, None, "above threshold → no backfill request");

    // Below the threshold but no known route to the peer: Ok(None), not an
    // error — the caller simply gets no history this round.
    let below = handle
        .try_backfill_from_peer(&ep, peer, 0, topic, net_tx.clone(), None)
        .await
        .expect("unknown-peer below threshold degrades gracefully");
    assert_eq!(below, None, "no route → no backfill performed");
}

#[tokio::test]
async fn test_backfill_handle_spawn_and_drop() {
    let ep = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
        .secret_key(SecretKey::generate())
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .unwrap()
        .bind()
        .await
        .expect("bind endpoint");
    let handle = BackfillHandle::spawn(ep);
    // Just verify it doesn't panic and can be dropped
    drop(handle);
}

/// A ProtocolHandler that delays before serving backfill.
/// Used to test timeout behaviour.
#[derive(Debug, Clone)]
struct DelayedBackfillHandler {
    storage: Arc<Storage>,
    authorizer: BackfillAuthorizer,
    delay: Duration,
}

impl ProtocolHandler for DelayedBackfillHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let storage = self.storage.clone();
        let authorizer = self.authorizer.clone();
        let delay = self.delay;
        tokio::task::spawn(async move {
            // Add the configured delay before processing
            tokio::time::sleep(delay).await;
            let _result = serve_backfill(connection, &storage, &authorizer).await;
        });
        Ok(())
    }
}

/// Test that a slow peer triggers a timeout on the client side.
///
/// The responder delays for 7s (above the 5s timeout), so the
/// client's timeout fires before the server finishes sleeping.
#[tokio::test]
async fn test_backfill_slow_peer_times_out() {
    // Use tokio time manipulation so the 5s timeout is instant.
    tokio::time::pause();

    let sk_responder = SecretKey::generate();
    let ep_responder = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
        .secret_key(sk_responder.clone())
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .unwrap()
        .bind()
        .await
        .expect("bind responder endpoint");

    // Empty SQLite storage — we never get to query it anyway because
    // the delay fires first on the server side.
    let storage = Arc::new(Storage::memory().unwrap());
    let slow_handler = DelayedBackfillHandler {
        storage: storage.clone(),
        authorizer: BackfillAuthorizer::new(storage.clone(), sk_responder.public()),
        // Delay long enough that the client timeout fires first.
        // With paused tokio time, this is virtual time — instant in wall-clock.
        delay: Duration::from_secs(7),
    };

    let _router = iroh::protocol::Router::builder(ep_responder.clone())
        .accept(BACKFILL_ALPN, slow_handler)
        .spawn();

    let sk_requester = SecretKey::generate();
    let ep_requester = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
        .secret_key(sk_requester.clone())
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .unwrap()
        .bind()
        .await
        .expect("bind requester endpoint");

    let addr = EndpointAddr::from_parts(sk_responder.public(), ep_responder.addr().addrs.clone());

    // Authorized direct-chat topic between requester and responder, so the
    // only reason the request fails is the server-side delay.
    let topic = direct_topic(&sk_requester.public(), &sk_responder.public());

    let (net_tx, _) = tokio::sync::mpsc::channel(64);

    // Spawn the backfill request in a background task so we can
    // advance time while it blocks waiting for the slow responder.
    // Clone the endpoint so the spawned task owns its own reference.
    let ep_for_task = ep_requester.clone();
    let handle = tokio::spawn(async move {
        do_backfill_request(&ep_for_task, addr, 0, 10, topic, net_tx, None).await
    });

    // Advance time past the client's 5s timeout.  The server's 7s
    // delay hasn't expired yet, so the client's timeout fires first.
    tokio::time::advance(BACKFILL_REQUEST_TIMEOUT + Duration::from_secs(1)).await;

    let result = handle.await.expect("backfill task panicked");
    let err = result.expect_err("slow backfill should time out");
    let err_msg = err.to_string();
    assert!(
        err_msg.contains(BACKFILL_TIMEOUT_MSG),
        "expected timeout error, got: {err_msg}"
    );

    tokio::time::resume();
}

/// Test that a normal (fast) backfill succeeds against a handler with no delay.
#[tokio::test]
async fn test_backfill_normal_succeeds() {
    let sk_responder = SecretKey::generate();
    let ep_responder = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
        .secret_key(sk_responder.clone())
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .unwrap()
        .bind()
        .await
        .expect("bind responder endpoint");

    // Set up an empty SQLite storage.
    let storage = Arc::new(Storage::memory().unwrap());

    let handler = BackfillProtocolHandler::new(storage.clone(), sk_responder.public());

    let _router = iroh::protocol::Router::builder(ep_responder.clone())
        .accept(BACKFILL_ALPN, handler)
        .spawn();

    let sk_requester = SecretKey::generate();
    let ep_requester = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
        .secret_key(sk_requester.clone())
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .unwrap()
        .bind()
        .await
        .expect("bind requester endpoint");

    let addr = EndpointAddr::from_parts(sk_responder.public(), ep_responder.addr().addrs.clone());

    // The requester is the direct-chat counterpart of the responder, so
    // authorization passes; the store is empty so 0 messages return.
    let topic = direct_topic(&sk_requester.public(), &sk_responder.public());

    let (net_tx, _) = tokio::sync::mpsc::channel(64);

    let result = do_backfill_request(&ep_requester, addr, 0, 10, topic, net_tx, None).await;

    // Even with an empty store, the backfill should succeed (returning 0 messages).
    assert!(
        result.is_ok(),
        "normal backfill should succeed: {:?}",
        result.err()
    );
    let count = result.unwrap();
    assert_eq!(count, 0, "empty store should return 0 messages");
}

// ── Authorization regression tests (BORU-AUDIT-01) ─────────────────

/// Spawn a responder endpoint with the real backfill handler over the
/// given storage; returns the responder's address and the kept-alive
/// router.
async fn spawn_responder(
    storage: Arc<Storage>,
    sk: &SecretKey,
) -> (EndpointAddr, iroh::protocol::Router) {
    let ep = Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
        .secret_key(sk.clone())
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .unwrap()
        .bind()
        .await
        .expect("bind responder endpoint");
    let handler = BackfillProtocolHandler::new(storage.clone(), sk.public());
    let router = iroh::protocol::Router::builder(ep.clone())
        .accept(BACKFILL_ALPN, handler)
        .spawn();
    let addr = EndpointAddr::from_parts(sk.public(), ep.addr().addrs.clone());
    (addr, router)
}

/// Spawn a fresh requester endpoint and return it plus its public key.
async fn spawn_requester() -> (Endpoint, PublicKey) {
    let sk = SecretKey::generate();
    (spawn_requester_with(&sk).await, sk.public())
}

/// Spawn a requester endpoint keyed by a specific secret key.
async fn spawn_requester_with(sk: &SecretKey) -> Endpoint {
    Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
        .secret_key(sk.clone())
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .unwrap()
        .bind()
        .await
        .expect("bind requester endpoint")
}

/// Storage with one group (owner = responder, member = member_sk), one
/// epoch topic, and one signed message from the member in that topic.
fn make_group_storage(
    local_sk: &SecretKey,
    member_sk: &SecretKey,
) -> (Arc<Storage>, TopicId, [u8; 32]) {
    let storage = Arc::new(Storage::memory().unwrap());
    let group_id = [7u8; 32];
    let topic = TopicId::from_bytes([0xAB; 32]);
    storage
        .create_group(&GroupRow {
            group_id,
            name: "AuditGroup".into(),
            description: String::new(),
            owner_public_key: local_sk.public().as_bytes().to_vec(),
            current_epoch: 1,
            created_at_ms: 1,
            updated_at_ms: 1,
            archived: false,
        })
        .unwrap();
    storage
        .create_group_epoch(&GroupEpochRow {
            group_id,
            epoch: 1,
            topic_id: topic,
            discovery_secret: vec![1u8; 32],
            created_at_ms: 1,
        })
        .unwrap();
    let add_member = |pk: &PublicKey, role: &str, state: &str| {
        storage
            .add_group_member(&GroupMemberRow {
                group_id,
                public_key: pk.as_bytes().to_vec(),
                role: role.into(),
                joined_at_ms: 1,
                invited_by: None,
                epoch_joined: 1,
                state: state.into(),
            })
            .unwrap();
    };
    add_member(&local_sk.public(), "Owner", "Owner");
    add_member(&member_sk.public(), "Member", "Active");
    let signed = SignedMessage::sign_and_encode(
        member_sk,
        &Message::Message {
            text: "audit hello".into(),
        },
    )
    .unwrap();
    storage
        .insert_chat_message(
            &[1u8; 32],
            &topic,
            member_sk.public().as_bytes(),
            1000,
            &signed,
        )
        .unwrap();
    (storage, topic, group_id)
}

/// Raw length-prefixed backfill exchange — lets a test send a request
/// the normal client API can no longer produce (e.g. `topic: None`).
/// Returns the decoded response or the transport-level error observed.
async fn raw_backfill_request(
    ep: &Endpoint,
    addr: EndpointAddr,
    request: &BackfillRequest,
) -> Result<BackfillResponse, String> {
    let conn = ep
        .connect(addr, BACKFILL_ALPN)
        .await
        .map_err(|e| e.to_string())?;
    let (mut writer, mut reader) = conn.open_bi().await.map_err(|e| e.to_string())?;
    let req_bytes = postcard::to_stdvec(request).map_err(|e| e.to_string())?;
    writer
        .write_u32_le(req_bytes.len() as u32)
        .await
        .map_err(|e| e.to_string())?;
    writer
        .write_all(&req_bytes)
        .await
        .map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    let resp_len = reader
        .read_u32_le()
        .await
        .map_err(|e| format!("read response: {e}"))?;
    if resp_len > 10 * 1024 * 1024 {
        return Err("response too large".into());
    }
    let mut buf = vec![0u8; resp_len as usize];
    reader
        .read_exact(&mut buf)
        .await
        .map_err(|e| e.to_string())?;
    postcard::from_bytes(&buf).map_err(|e| e.to_string())
}

/// Regression: a remote request with `topic = None` is rejected before
/// any DB query.  The storage holds a message that an unscoped query
/// would return — the client must observe a transport error instead.
#[tokio::test]
async fn backfill_rejects_request_without_topic() {
    let sk_responder = SecretKey::generate();
    let storage = Arc::new(Storage::memory().unwrap());
    let other = SecretKey::generate();
    let signed = SignedMessage::sign_and_encode(
        &other,
        &Message::Message {
            text: "private".into(),
        },
    )
    .unwrap();
    storage
        .insert_chat_message(
            &[2u8; 32],
            &TopicId::from_bytes([0xCD; 32]),
            other.public().as_bytes(),
            1,
            &signed,
        )
        .unwrap();

    let (addr, _router) = spawn_responder(storage, &sk_responder).await;
    let (ep, _pk) = spawn_requester().await;

    let result = raw_backfill_request(
        &ep,
        addr,
        &BackfillRequest {
            since_ms: 0,
            max_messages: 10,
            topic: None,
        },
    )
    .await;
    assert!(
        result.is_err(),
        "topic=None must be rejected, got a response: {result:?}"
    );
}

/// Regression: only active group members may backfill a group topic.
/// Non-members and removed members are denied with zero message
/// metadata; an active member receives the seeded history.
#[tokio::test]
async fn backfill_authorizes_group_membership() {
    let sk_responder = SecretKey::generate();
    let sk_member = SecretKey::generate();
    let (storage, topic, group_id) = make_group_storage(&sk_responder, &sk_member);
    let (addr, _router) = spawn_responder(storage.clone(), &sk_responder).await;

    // Outsider (never a member) → denied.
    let (ep_outsider, _) = spawn_requester().await;
    let result = do_backfill_request(
        &ep_outsider,
        addr.clone(),
        0,
        50,
        topic,
        mpsc::channel(64).0,
        None,
    )
    .await;
    assert!(
        result.is_err(),
        "non-member must be denied, got: {result:?}"
    );

    // Active member → succeeds and receives the seeded message.  The
    // requester endpoint must be keyed by the member's own identity.
    let ep_member = spawn_requester_with(&sk_member).await;
    let (net_tx, mut net_rx) = mpsc::channel(64);
    let result = do_backfill_request(&ep_member, addr.clone(), 0, 50, topic, net_tx, None).await;
    assert!(
        result.is_ok(),
        "member backfill should succeed: {:?}",
        result.err()
    );
    assert_eq!(
        result.unwrap(),
        1,
        "member should receive the seeded message"
    );
    assert!(
        net_rx.recv().await.is_some(),
        "member should receive a decoded NetEvent"
    );

    // Former member after removal → denied immediately.
    storage
        .remove_group_member(&group_id, sk_member.public().as_bytes(), "Removed")
        .unwrap();
    let result2 =
        do_backfill_request(&ep_member, addr, 0, 50, topic, mpsc::channel(64).0, None).await;
    assert!(
        result2.is_err(),
        "removed member must be denied, got: {result2:?}"
    );
}

/// Regression: authorization is re-checked on every request.  A first
/// page does not grant a permanent capability — after membership
/// revocation the next (continued) page is denied.
#[tokio::test]
async fn backfill_rechecks_authorization_on_next_page() {
    let sk_responder = SecretKey::generate();
    let sk_member = SecretKey::generate();
    let (storage, topic, group_id) = make_group_storage(&sk_responder, &sk_member);
    let (addr, _router) = spawn_responder(storage.clone(), &sk_responder).await;
    let ep = spawn_requester_with(&sk_member).await;

    // Page 1 (since=0): authorized member succeeds.
    let page1 =
        do_backfill_request(&ep, addr.clone(), 0, 50, topic, mpsc::channel(64).0, None).await;
    assert!(
        page1.is_ok(),
        "first page should succeed: {:?}",
        page1.err()
    );

    // Revocation between pages.
    storage
        .remove_group_member(&group_id, sk_member.public().as_bytes(), "Removed")
        .unwrap();

    // Page 2 (continued since_ms): denied immediately.
    let page2 = do_backfill_request(&ep, addr, 1000, 50, topic, mpsc::channel(64).0, None).await;
    assert!(
        page2.is_err(),
        "next page after revocation must be denied: {page2:?}"
    );
}

/// Regression: unknown topics and forbidden topics are externally
/// indistinguishable — both are denied with no response body, so an
/// attacker cannot probe for topic existence or history size.
#[tokio::test]
async fn backfill_unknown_and_forbidden_topics_look_identical() {
    let sk_responder = SecretKey::generate();
    let sk_member = SecretKey::generate();
    let (storage, topic, _group_id) = make_group_storage(&sk_responder, &sk_member);
    let (addr, _router) = spawn_responder(storage, &sk_responder).await;
    let (ep, _) = spawn_requester().await;

    // Unknown topic: no local record at all.
    let unknown = TopicId::from_bytes([0xEE; 32]);
    // Forbidden topic: a real group the requester is not a member of.
    let forbidden = topic;

    let unknown_res = raw_backfill_request(
        &ep,
        addr.clone(),
        &BackfillRequest {
            since_ms: 0,
            max_messages: 10,
            topic: Some(unknown),
        },
    )
    .await;
    let forbidden_res = raw_backfill_request(
        &ep,
        addr,
        &BackfillRequest {
            since_ms: 0,
            max_messages: 10,
            topic: Some(forbidden),
        },
    )
    .await;

    assert!(
        unknown_res.is_err(),
        "unknown topic must be denied: {unknown_res:?}"
    );
    assert!(
        forbidden_res.is_err(),
        "forbidden topic must be denied: {forbidden_res:?}"
    );
    // Both fail at the same stage (reading the response that never
    // comes) — identical external error behavior.
    let unknown_msg = unknown_res.unwrap_err();
    let forbidden_msg = forbidden_res.unwrap_err();
    assert!(
        unknown_msg.starts_with("read response"),
        "unknown failure should be a response-read error: {unknown_msg}"
    );
    assert!(
        forbidden_msg.starts_with("read response"),
        "forbidden failure should be a response-read error: {forbidden_msg}"
    );
}

/// Regression: a direct-chat topic is only readable by its two
/// participants.  The requester that matches the deterministic topic is
/// authorized; a third party guessing the topic is denied.
#[tokio::test]
async fn backfill_direct_chat_only_authorizes_participants() {
    let sk_responder = SecretKey::generate();
    let storage = Arc::new(Storage::memory().unwrap());
    let (addr, _router) = spawn_responder(storage, &sk_responder).await;

    let sk_peer = SecretKey::generate();
    let topic = direct_topic(&sk_peer.public(), &sk_responder.public());

    // The peer IS a participant in this direct topic — build the
    // requester endpoint from the participant's key.
    let ep_peer = spawn_requester_with(&sk_peer).await;
    let res = do_backfill_request(
        &ep_peer,
        addr.clone(),
        0,
        10,
        topic,
        mpsc::channel(64).0,
        None,
    )
    .await;
    assert!(
        res.is_ok(),
        "direct participant should be authorized: {:?}",
        res.err()
    );

    // A third party requesting the same topic is denied.
    let (ep_outsider, _) = spawn_requester().await;
    let res_out =
        do_backfill_request(&ep_outsider, addr, 0, 10, topic, mpsc::channel(64).0, None).await;
    assert!(
        res_out.is_err(),
        "non-participant must be denied: {res_out:?}"
    );
}

/// Regression: the canonical public lobby is readable by any
/// authenticated peer (public-room policy).
#[tokio::test]
async fn backfill_public_lobby_is_open_to_any_peer() {
    let sk_responder = SecretKey::generate();
    let storage = Arc::new(Storage::memory().unwrap());
    let (addr, _router) = spawn_responder(storage, &sk_responder).await;
    let (ep, _) = spawn_requester().await;

    let lobby = public_lobby_topic(PublicNetwork::Mainnet);
    let res = do_backfill_request(&ep, addr, 0, 10, lobby, mpsc::channel(64).0, None).await;
    assert!(
        res.is_ok(),
        "public lobby must be readable by any peer: {:?}",
        res.err()
    );
    assert_eq!(res.unwrap(), 0, "empty lobby store returns no messages");
}
