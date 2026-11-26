pub(crate) mod common;
mod bridge_events_inbound_unread;

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
async fn harness_invokes_commands_after_runtime_restart() {
    let mut harness = common::Harness::new().await;
    harness.restart().await;

    let bootstrap = harness.invoke("get_bootstrap", serde_json::json!({}));
    assert!(bootstrap.is_ok(), "get_bootstrap after restart: {bootstrap:?}");
    let page = harness.invoke(
        "get_messages_page",
        serde_json::json!({"conversationId": ROOM_ID, "cursor": null, "limit": 50, "requestId": "after-restart"}),
    );
    assert!(page.is_ok(), "get_messages_page after restart: {page:?}");
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
    // Preview comes from MessageStore's conversation_meta (the newest seeded
    // message), not from the snapshot projection, which carries no preview.
    assert_eq!(alpha["lastMessageSummary"], "msg-1200");
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
    let captured_hash: [u8; 32] = boru_core::chat_core::message_hash(&decoded);
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
async fn sent_message_survives_restart_and_appears_in_room_history() {
    let mut harness = common::Harness::new().await;
    let body = "persisted across runtime restart";
    let request = serde_json::json!({"args":{"request":{"idempotencyKey":"restart-history-send","conversationId":vec![0xA1;32],"text":body,"threadTarget":null}}});
    let receipt = harness.invoke("send_message", request).unwrap();
    assert!(receipt["eventId"].is_number());

    harness.restart().await;

    let page = harness.invoke(
        "get_messages_page",
        serde_json::json!({"conversationId": ROOM_ID, "cursor": null, "limit": 50, "requestId": "restart-history"}),
    ).unwrap();
    assert!(page["items"].as_array().unwrap().iter().any(|item| item["body"] == body), "sent body absent from page: {page}");
    let opened = harness.invoke("open_room", serde_json::json!({"conversationId": ROOM_ID})).unwrap();
    assert!(opened["messages"].as_array().unwrap().iter().any(|item| item["body"] == body), "sent body absent from open_room: {opened}");
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

/// The room pump must turn each inbound frame into exactly one persisted row and
/// exactly one pair of UI events, and must never do so twice for the same bytes.
///
/// `RoomSenderSession::join` needs a live gossip receiver, so this drives the
/// pump's frame path directly through the test seam; the loop around that path is
/// the only untested part and it does nothing except call through.
#[tokio::test]
async fn pump_ingest_persists_once_and_publishes_exactly_once_events() {
    let harness = common::Harness::new().await;
    let topic = [0xA1u8; 32];
    let (session, store, mut events) = {
        let runtime = harness.runtime();
        let runtime = runtime.as_ref().expect("runtime available");
        (
            runtime.room_sender_session(),
            runtime.message_store(),
            runtime.subscribe_ui_events(),
        )
    };

    // The fixture seeds 1,200 local-sender rows on this topic, so every assertion
    // below is relative to the state the runtime started with.
    let rows_before = store.count_messages_for_topic(&topic).expect("count seeded rows");
    let unread_before = store.get_unread_count(&topic).expect("read seeded unread").unwrap_or(0);

    let remote = iroh::SecretKey::from_bytes(&[0xC4; 32]);
    let message = boru_core::chat_core::Message::Message {
        text: "inbound pump frame".to_string(),
    };
    let frame = boru_core::chat_core::SignedMessage::sign_and_encode(&remote, &message)
        .expect("sign inbound frame");

    assert!(
        session
            .ingest_room_frame_for_test(topic, &frame)
            .expect("first ingest is not an error"),
        "a fresh frame must report that it inserted a row"
    );

    // Durable row keyed by the envelope hash, with the persisted bytes intact.
    let envelope_hash = blake3::hash(&frame);
    let row = store
        .find_message_by_hash(envelope_hash.as_bytes())
        .expect("lookup by envelope hash")
        .expect("pump persisted the inbound frame");
    assert_eq!(row.body, "inbound pump frame");
    assert_eq!(row.signed_bytes.as_deref(), Some(frame.as_ref()));
    assert_eq!(store.count_messages_for_topic(&topic).expect("count rows"), rows_before + 1);
    assert_eq!(
        store.get_unread_count(&topic).expect("read unread"),
        Some(unread_before + 1),
        "a remote sender increments unread exactly once"
    );

    // Replaying identical bytes is a duplicate, not an error: no row, no unread
    // increment, and no further events.
    assert!(
        !session
            .ingest_room_frame_for_test(topic, &frame)
            .expect("duplicate ingest is not an error"),
        "the second identical frame must be rejected by the exactly-once gate"
    );
    assert_eq!(store.count_messages_for_topic(&topic).expect("count rows"), rows_before + 1);
    assert_eq!(
        store.get_unread_count(&topic).expect("read unread"),
        Some(unread_before + 1),
        "a duplicate must not increment unread"
    );

    // Frames the pump ignores: undecodable payloads and room-doc markers.
    for ignored in [
        b"not a signed message".as_slice(),
        &[0xFE, 0x00][..],
        &[0xFF, 0x00][..],
    ] {
        assert!(
            !session
                .ingest_room_frame_for_test(topic, ignored)
                .expect("ignored frames are not errors"),
            "frame {ignored:?} must not insert a row"
        );
    }
    assert_eq!(store.count_messages_for_topic(&topic).expect("count rows"), rows_before + 1);
    assert_eq!(
        store.get_unread_count(&topic).expect("read unread"),
        Some(unread_before + 1),
        "ignored frames must not touch unread"
    );

    let mut inserted = Vec::new();
    let mut changed = Vec::new();
    let mut unread = Vec::new();
    while let Ok(event) = events.try_recv() {
        match event {
            boru_core::ui_events::UiEvent::MessageInserted { message_id } => inserted.push(message_id),
            boru_core::ui_events::UiEvent::ConversationChanged { conversation_id } => {
                changed.push(conversation_id)
            }
            boru_core::ui_events::UiEvent::UnreadChanged { room_id, unread_count } => {
                unread.push((room_id, unread_count))
            }
            _ => {}
        }
    }
    assert_eq!(inserted, vec![row.id], "exactly one MessageInserted, carrying the new row id");
    assert_eq!(changed, vec![topic], "exactly one ConversationChanged for the room");
    let persisted_unread = store.get_unread_count(&topic).expect("read persisted unread").unwrap_or(0);
    assert_eq!(unread, vec![(topic, persisted_unread)]);

    let runtime_guard = harness.runtime();
    let runtime = runtime_guard.as_ref().expect("runtime available");
    runtime.mark_read(&topic).expect("mark room read");
    assert_eq!(store.get_unread_count(&topic).expect("verify persisted clear"), Some(0));
    assert!(matches!(events.try_recv(), Ok(boru_core::ui_events::UiEvent::UnreadChanged { room_id, unread_count: 0 }) if room_id == topic));
    runtime.mark_read(&topic).expect("repeat mark read");
    assert!(events.try_recv().is_err(), "unchanged persisted unread count must not emit");
}

/// A real SQLite constraint failure must not leak UI events before commit.
#[tokio::test]
async fn pump_ingest_persist_failure_publishes_no_events() {
    let harness = common::Harness::new().await;
    let topic = [0xA1u8; 32];
    let (session, store, mut events) = {
        let runtime = harness.runtime();
        let runtime = runtime.as_ref().expect("runtime available");
        (
            runtime.room_sender_session(),
            runtime.message_store(),
            runtime.subscribe_ui_events(),
        )
    };
    store
        .fail_message_inserts_for_test()
        .expect("install deterministic SQLite failure trigger");

    let remote = iroh::SecretKey::from_bytes(&[0xD4; 32]);
    let message = boru_core::chat_core::Message::Message {
        text: "must not publish".to_string(),
    };
    let frame = boru_core::chat_core::SignedMessage::sign_and_encode(&remote, &message)
        .expect("sign inbound frame");

    assert!(
        session.ingest_room_frame_for_test(topic, &frame).is_err(),
        "the forced SQLite error must be returned by the real ingest path"
    );
    assert!(
        events.try_recv().is_err(),
        "neither MessageInserted nor ConversationChanged may publish on persistence failure"
    );
}
