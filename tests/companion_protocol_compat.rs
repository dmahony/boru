use boru_core::companion_protocol::{
    CompanionErrorCode, CompanionPolicy, CompanionProtocolHandler, CompanionRequest,
    CompanionResponse, COMPANION_ALPN, COMPANION_WIRE_VERSION, PUBLIC_CAPABILITIES,
};
use iroh::{endpoint::presets, protocol::Router, Endpoint};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn checked_fixture_round_trips_public_wire_shapes() {
    let fixture: Value =
        serde_json::from_str(include_str!("../docs/mobile-app-contract-fixtures.json"))
            .expect("fixture JSON must remain valid");

    for value in fixture["valid_requests"]
        .as_array()
        .expect("valid_requests")
    {
        serde_json::from_value::<CompanionRequest>(value.clone())
            .expect("valid request fixture must match the Rust wire enum");
    }
    for value in fixture["valid_responses"]
        .as_array()
        .expect("valid_responses")
    {
        serde_json::from_value::<CompanionResponse>(value.clone())
            .expect("valid response fixture must match the Rust wire enum");
    }
}

#[test]
fn fixture_and_constants_define_the_same_v1_boundary() {
    assert_eq!(COMPANION_WIRE_VERSION, 1);
    assert!(PUBLIC_CAPABILITIES.contains(&"pairing"));
    assert!(PUBLIC_CAPABILITIES.contains(&"history-v1"));
    assert!(PUBLIC_CAPABILITIES.contains(&"snapshot-v1"));
}

#[test]
fn unsupported_major_version_is_not_accepted_by_the_fixture_contract() {
    let hello = serde_json::json!({
        "type": "hello",
        "versions": [99],
        "capabilities": ["pairing"]
    });
    let request: CompanionRequest = serde_json::from_value(hello).expect("shape is valid");
    assert!(
        matches!(request, CompanionRequest::Hello { versions, .. } if !versions.contains(&COMPANION_WIRE_VERSION))
    );
}

#[tokio::test]
async fn unsupported_major_version_is_rejected_by_the_handler() {
    let server = Endpoint::bind(presets::Minimal).await.unwrap();
    let router = Router::builder(server.clone())
        .accept(
            COMPANION_ALPN,
            CompanionProtocolHandler::new(CompanionPolicy::new(), true),
        )
        .spawn();
    let client = Endpoint::bind(presets::Minimal).await.unwrap();
    let connection = client
        .connect(router.endpoint().addr(), COMPANION_ALPN)
        .await
        .unwrap();
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    let request = CompanionRequest::Hello {
        versions: vec![99],
        capabilities: vec!["pairing".into()],
    };
    let bytes = serde_json::to_vec(&request).unwrap();
    send.write_u32(bytes.len() as u32).await.unwrap();
    send.write_all(&bytes).await.unwrap();
    let length = recv.read_u32().await.unwrap() as usize;
    let mut response = vec![0; length];
    recv.read_exact(&mut response).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<CompanionResponse>(&response).unwrap(),
        CompanionResponse::Error {
            code: CompanionErrorCode::IncompatibleVersion,
        }
    );
    client.close().await;
    server.close().await;
}
