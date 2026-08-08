//! Wire-level signalling tests. Media is intentionally not involved.

use std::time::Duration;

use boru_core::call::manager::{CallBuilder, CallEvent, CALL_ALPN};
use iroh::{endpoint::presets, protocol::Router, Endpoint};

async fn next_event(label: &str, events: &mut tokio::sync::mpsc::Receiver<CallEvent>) -> CallEvent {
    tokio::time::timeout(Duration::from_secs(3), events.recv())
        .await
        .unwrap_or_else(|_| panic!("call event timed out: {label}"))
        .expect("call actor stopped")
}

async fn connect_probe(client: &Endpoint, server: &Endpoint) {
    let connection = client
        .connect(server.addr(), CALL_ALPN)
        .await
        .expect("probe connection should establish");
    connection.close(0u32.into(), b"probe");
}

#[tokio::test]
async fn two_endpoints_complete_call_and_reject_busy_second_call() {
    let server = Endpoint::bind(presets::Minimal).await.unwrap();
    let server_builder = CallBuilder::new(server.clone(), server.secret_key().clone());
    let server_handler = server_builder.protocol_handler();
    let (server_handle, mut server_events) = server_builder.spawn();
    let server_router = Router::builder(server.clone())
        .accept(CALL_ALPN, server_handler)
        .spawn();

    let client = Endpoint::bind(presets::Minimal).await.unwrap();
    let client_builder = CallBuilder::new(client.clone(), client.secret_key().clone());
    let client_handler = client_builder.protocol_handler();
    let (client_handle, mut client_events) = client_builder.spawn();
    let client_router = Router::builder(client.clone())
        .accept(CALL_ALPN, client_handler)
        .spawn();

    // Authorization is deny-by-default: each side must authorize the other
    // before outbound start (CallHandle) or inbound accept (CallProtocol).
    server_handle.set_peer_authorized(client.id(), true);
    client_handle.set_peer_authorized(server.id(), true);

    // Populate the endpoint's address cache; production discovery does this
    // through mDNS/relay discovery before the user presses Call.
    connect_probe(&client, &server).await;

    let call_id = client_handle.start_voice_call(server.id()).await.unwrap();
    assert!(
        matches!(next_event("client ringing", &mut client_events).await, CallEvent::OutgoingRinging { call_id: id, .. } if id == call_id)
    );
    let incoming_id = match next_event("server incoming", &mut server_events).await {
        CallEvent::Incoming { call_id, .. } => call_id,
        event => panic!("expected incoming call, got {event:?}"),
    };
    assert_eq!(incoming_id, call_id);

    server_handle.accept(incoming_id).await.unwrap();
    assert!(
        matches!(next_event("server active", &mut server_events).await, CallEvent::Active { call_id: id, .. } if id == call_id)
    );
    assert!(
        matches!(next_event("client active", &mut client_events).await, CallEvent::Active { call_id: id, .. } if id == call_id)
    );

    // A third endpoint attempts a call while the first one is active. The
    // server sends Busy on the wire without creating a second local call.
    let busy_client = Endpoint::bind(presets::Minimal).await.unwrap();
    let busy_builder = CallBuilder::new(busy_client.clone(), busy_client.secret_key().clone());
    let busy_handler = busy_builder.protocol_handler();
    let (busy_handle, mut busy_events) = busy_builder.spawn();
    let busy_router = Router::builder(busy_client.clone())
        .accept(CALL_ALPN, busy_handler)
        .spawn();
    busy_handle.set_peer_authorized(server.id(), true);
    server_handle.set_peer_authorized(busy_client.id(), true);
    connect_probe(&busy_client, &server).await;
    let busy_id = busy_handle.start_voice_call(server.id()).await.unwrap();
    assert!(
        matches!(next_event("busy ringing", &mut busy_events).await, CallEvent::OutgoingRinging { call_id: id, .. } if id == busy_id)
    );
    assert!(
        matches!(next_event("busy rejected", &mut busy_events).await, CallEvent::Failed { call_id: Some(id), .. } if id == busy_id)
    );

    client_handle.hangup(call_id).await.unwrap();
    assert!(
        matches!(next_event("client ended", &mut client_events).await, CallEvent::Ended { call_id: id, .. } if id == call_id)
    );
    loop {
        let server_event = next_event("server teardown", &mut server_events).await;
        if matches!(server_event, CallEvent::Ended { call_id: id, .. } if id == call_id) {
            break;
        }
    }

    server_router.shutdown().await.unwrap();
    client_router.shutdown().await.unwrap();
    busy_router.shutdown().await.unwrap();
}

/// BORU-CALL-5.3: repeated start/end must not leak connections or call state.
///
/// The same two endpoints run a full start -> ringing -> accept -> active ->
/// hangup -> ended lifecycle 30 times.  After every hangup the next call must
/// be accepted immediately (the user can call again right away), exactly one
/// Ended event per side per iteration must be observed, and no event may be
/// left queued in either channel (a leaked actor state would emit strays).
#[tokio::test]
async fn repeated_start_end_loop_leaves_no_state_or_connection_growth() {
    const ITERATIONS: usize = 30;

    let server = Endpoint::bind(presets::Minimal).await.unwrap();
    let server_builder = CallBuilder::new(server.clone(), server.secret_key().clone());
    let server_handler = server_builder.protocol_handler();
    let (server_handle, mut server_events) = server_builder.spawn();
    let server_router = Router::builder(server.clone())
        .accept(CALL_ALPN, server_handler)
        .spawn();

    let client = Endpoint::bind(presets::Minimal).await.unwrap();
    let client_builder = CallBuilder::new(client.clone(), client.secret_key().clone());
    let client_handler = client_builder.protocol_handler();
    let (client_handle, mut client_events) = client_builder.spawn();
    let client_router = Router::builder(client.clone())
        .accept(CALL_ALPN, client_handler)
        .spawn();

    server_handle.set_peer_authorized(client.id(), true);
    client_handle.set_peer_authorized(server.id(), true);
    connect_probe(&client, &server).await;

    for iteration in 0..ITERATIONS {
        let call_id = client_handle.start_voice_call(server.id()).await.unwrap();
        assert!(
            matches!(next_event("client ringing", &mut client_events).await, CallEvent::OutgoingRinging { call_id: id, .. } if id == call_id),
            "iteration {iteration}: expected OutgoingRinging"
        );
        let incoming_id = match next_event("server incoming", &mut server_events).await {
            CallEvent::Incoming { call_id, .. } => call_id,
            event => panic!("iteration {iteration}: expected Incoming, got {event:?}"),
        };
        assert_eq!(incoming_id, call_id, "iteration {iteration}: call id mismatch");

        server_handle.accept(incoming_id).await.unwrap();
        assert!(
            matches!(next_event("server active", &mut server_events).await, CallEvent::Active { call_id: id, .. } if id == call_id),
            "iteration {iteration}: expected server Active"
        );
        assert!(
            matches!(next_event("client active", &mut client_events).await, CallEvent::Active { call_id: id, .. } if id == call_id),
            "iteration {iteration}: expected client Active"
        );

        client_handle.hangup(call_id).await.unwrap();
        assert!(
            matches!(next_event("client ended", &mut client_events).await, CallEvent::Ended { call_id: id, .. } if id == call_id),
            "iteration {iteration}: expected client Ended"
        );
        loop {
            let server_event = next_event("server teardown", &mut server_events).await;
            if matches!(server_event, CallEvent::Ended { call_id: id, .. } if id == call_id) {
                break;
            }
        }

        // No residual events: a leaked call state or connection would leave
        // strays behind after the terminal Ended.
        assert!(
            client_events.try_recv().is_err(),
            "iteration {iteration}: client channel has leftover events"
        );
        assert!(
            server_events.try_recv().is_err(),
            "iteration {iteration}: server channel has leftover events"
        );
    }

    server_router.shutdown().await.unwrap();
    client_router.shutdown().await.unwrap();
}
