//! Signalling timeout integration coverage.

use std::time::Duration;

use boru_core::call::manager::{CallBuilder, CallEndReason, CallEvent, CALL_ALPN};
use iroh::{endpoint::presets, protocol::Router, Endpoint};

#[tokio::test]
async fn unanswered_offer_emits_negotiation_timeout() {
    let server = Endpoint::bind(presets::Minimal).await.unwrap();
    let server_builder = CallBuilder::new(server.clone(), server.secret_key().clone());
    let server_handler = server_builder.protocol_handler();
    let (_server_handle, mut server_events) = server_builder.spawn();
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

    let probe = client.connect(server.addr(), CALL_ALPN).await.unwrap();
    probe.close(0u32.into(), b"probe");
    let call_id = client_handle.start_voice_call(server.id()).await.unwrap();
    let _ = client_events.recv().await;
    let _ = server_events.recv().await;

    let terminal = tokio::time::timeout(Duration::from_secs(7), async {
        loop {
            if let Some(event) = client_events.recv().await {
                if event.is_terminal() {
                    break event;
                }
            }
        }
    })
    .await
    .expect("negotiation timeout did not fire");
    assert!(
        matches!(terminal, CallEvent::Ended { call_id: id, reason: CallEndReason::NegotiationTimeout } if id == call_id)
    );

    server_router.shutdown().await.unwrap();
    client_router.shutdown().await.unwrap();
}
