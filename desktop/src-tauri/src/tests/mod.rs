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

const ROOM_ID: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";

#[tokio::test]
async fn list_rooms_projects_seeded_snapshot_room() {
    let harness = common::Harness::new().await;
    let rooms = harness.invoke("list_rooms", serde_json::json!({})).unwrap();
    let alpha = rooms.as_array().unwrap().iter().find(|room| room["id"] == ROOM_ID).unwrap();
    assert_eq!(alpha["name"], "Fixture Alpha");
    assert_eq!(alpha["lastMessageSummary"], serde_json::Value::Null);
    assert_eq!(alpha["unreadCount"], 0);
}

#[tokio::test]
async fn open_room_returns_the_shared_first_history_page() {
    let harness = common::Harness::new().await;
    let opened = harness.invoke("open_room", serde_json::json!({"conversationId": ROOM_ID})).unwrap();
    let page = harness.invoke("get_messages_page", serde_json::json!({"conversationId": ROOM_ID, "cursor": null, "limit": 50, "requestId": "test-page"})).unwrap();
    assert_eq!(opened["room"]["name"], "Fixture Alpha");
    assert_eq!(opened["room"]["unreadCount"], 0);
    let opened_bodies: Vec<_> = opened["messages"].as_array().unwrap().iter().map(|item| &item["body"]).collect();
    let page_bodies: Vec<_> = page["items"].as_array().unwrap().iter().map(|item| &item["body"]).collect();
    assert_eq!(opened_bodies, page_bodies);
    assert_eq!(opened["messages"].as_array().unwrap().len(), 50);
    assert!(opened["nextCursor"].is_string());
}

#[tokio::test]
async fn send_message_returns_receipt_and_replay_conflicts_without_second_send() {
    let harness = common::Harness::new().await;
    let request = serde_json::json!({"args":{"request":{"idempotencyKey":"integration-send-1","conversationId":vec![0xA1;32],"text":"command path test","threadTarget":null}}});
    let first = harness.invoke("send_message", request.clone());
    assert!(first.is_ok(), "first send should succeed: {first:?}");
    let receipt = first.unwrap();
    assert!(receipt["eventId"].is_number());
    assert_eq!(receipt["messageHash"].as_str().unwrap().len(), 64);
    let second = harness.invoke("send_message", request);
    assert!(second.is_err(), "replayed idempotency key must conflict");
    let error = second.unwrap_err();
    assert!(error.to_ascii_lowercase().contains("conflict"), "unexpected error: {error}");
    assert!(!error.contains("/home/") && !error.contains("secret_key"), "error leaked local details: {error}");
}

#[tokio::test]
async fn mark_read_returns_prior_count_and_room_reads_remain_clear() {
    let harness = common::Harness::new().await;
    let previous = harness.invoke("mark_read", serde_json::json!({"request":{"conversationId":vec![0xA1;32]}})).unwrap();
    assert!(previous.is_null() || previous.as_u64().is_some());
    let rooms = harness.invoke("list_rooms", serde_json::json!({})).unwrap();
    let alpha = rooms.as_array().unwrap().iter().find(|room| room["id"] == ROOM_ID).unwrap();
    assert_eq!(alpha["unreadCount"], 0);
    let opened = harness.invoke("open_room", serde_json::json!({"conversationId": ROOM_ID})).unwrap();
    assert_eq!(opened["room"]["unreadCount"], 0);
}
