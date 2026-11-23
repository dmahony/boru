pub(crate) mod common;

#[test]
fn send_message_bridge_json_uses_camel_case_contract() {
    let args: super::SendMessageArgs = serde_json::from_value(serde_json::json!({
        "request": {
            "idempotencyKey": "retry-1",
            "conversationId": vec![7; 32],
            "text": "hello",
            "threadTarget": null
        }
    })).expect("valid send_message contract request");
    assert_eq!(args.request.idempotency_key, "retry-1");
    assert_eq!(args.request.conversation_id, vec![7; 32]);
    let response = super::SendMessageResponse { event_id: 9, message_hash: "ab".into() };
    assert_eq!(serde_json::to_value(response).expect("serialize response"), serde_json::json!({"eventId": 9, "messageHash": "ab"}));
}

#[test]
fn mark_read_bridge_json_uses_camel_case_and_bare_numeric_response() {
    let request: super::MarkReadRequest = serde_json::from_value(serde_json::json!({
        "conversationId": vec![8; 32]
    })).expect("valid mark_read contract request");
    assert_eq!(request.conversation_id, vec![8; 32]);
    let response: u32 = 4;
    assert_eq!(serde_json::to_value(response).expect("serialize unread count"), serde_json::json!(4));
}

#[tokio::test]
async fn get_bootstrap_smoke_test() {
    let harness = common::Harness::new().await;
    let response = harness.invoke("get_bootstrap", serde_json::json!({}));
    assert!(response.is_ok(), "get_bootstrap IPC response: {response:?}");
}
