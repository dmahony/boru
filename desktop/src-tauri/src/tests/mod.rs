pub(crate) mod common;

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

    // Grab the seams before the send so the recorder can be cleared first and
    // the same handles are used for the post-send assertions.
    let (session, message_store) = {
        let runtime = harness.runtime();
        let runtime = runtime.as_ref().expect("runtime available");
        (runtime.room_sender_session(), runtime.message_store())
    };
    let topic = boru_core::proto::TopicId::from_bytes([0xA1; 32]);
    session.clear_recorded_broadcasts();

    let request = serde_json::json!({"args":{"request":{"idempotencyKey":"integration-send-1","conversationId":vec![0xA1;32],"text":"command path test","threadTarget":null}}});
    let first = harness.invoke("send_message", request.clone());
    assert!(first.is_ok(), "first send should succeed: {first:?}");
    let receipt = first.unwrap();
    assert!(receipt["eventId"].as_u64().is_some(), "eventId must be u64: {receipt}");
    let message_hash = receipt["messageHash"].as_str().expect("messageHash string");
    assert_eq!(message_hash.len(), 64, "messageHash must be 64 hex chars");
    assert!(message_hash.bytes().all(|byte| byte.is_ascii_hexdigit()), "messageHash must be hexadecimal");

    let captures = session.recorded_broadcasts();
    let captured = captures.lock().unwrap();
    let conversation_captures: Vec<_> = captured.iter().filter(|(captured_topic, _)| *captured_topic == topic).collect();
    assert_eq!(conversation_captures.len(), 1, "expected exactly one broadcast for conversation");
    let broadcast_bytes = conversation_captures[0].1.clone();
    drop(captured);

    // The receipt hash identifies the *inner* `Message` (blake3 of its postcard
    // encoding), not the signed wire envelope, so hash the message the captured
    // broadcast actually carries. This proves the receipt describes exactly the
    // message that went out on the conversation topic.
    let (_sender, decoded, _sent_at) = boru_core::chat_core::SignedMessage::verify_and_decode(&broadcast_bytes)
        .expect("captured broadcast decodes as a verified signed message");
    let captured_hash: [u8; 32] = boru_core::chat_core::message_hash(&decoded).into();
    let captured_hash: String = captured_hash.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(captured_hash, message_hash, "receipt hash must identify the broadcast message");

    // Durable rows in `messages` are keyed by the *envelope* hash — the blake3
    // of the signed bytes that went on the wire (`repair_pending_text_projections`
    // inserts with `envelope_hash`), not the inner-message hash the receipt
    // reports. So locate the row by the captured broadcast's own hash and prove
    // the persisted bytes are byte-identical to what was broadcast.
    let envelope_hash = blake3::hash(&broadcast_bytes);
    let row = message_store
        .find_message_by_hash(envelope_hash.as_bytes())
        .unwrap()
        .expect("durable message row keyed by envelope hash");
    assert_eq!(row.body, "command path test");
    assert_eq!(
        row.signed_bytes.as_deref(),
        Some(broadcast_bytes.as_slice()),
        "persisted signed bytes must equal the broadcast bytes"
    );

    let second = harness.invoke("send_message", request);
    assert!(second.is_err(), "replayed idempotency key must conflict");
    let error = second.unwrap_err();
    assert!(error.contains("CONFLICT"), "expected CONFLICT-coded error: {error}");
    assert_eq!(session.recorded_broadcasts().lock().unwrap().iter().filter(|(captured_topic, _)| *captured_topic == topic).count(), 1, "conflict must not broadcast again");
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
