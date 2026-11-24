use boru_core::{
    chat_core::{Message, SignedMessage},
    ui_events::UiEvent,
};

fn room_hex(room_id: &[u8; 32]) -> String {
    room_id.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn drain_events(events: &mut tokio::sync::broadcast::Receiver<UiEvent>) -> Vec<UiEvent> {
    let mut drained = Vec::new();
    while let Ok(event) = events.try_recv() {
        drained.push(event);
    }
    drained
}

fn event_pair(events: &[UiEvent]) -> (&UiEvent, &UiEvent) {
    let inbound: Vec<_> = events
        .iter()
        .filter(|event| matches!(event, UiEvent::InboundMessage { .. }))
        .collect();
    let unread: Vec<_> = events
        .iter()
        .filter(|event| matches!(event, UiEvent::UnreadChanged { .. }))
        .collect();
    assert_eq!(
        inbound.len(),
        1,
        "exactly one inbound_message event: {events:?}"
    );
    assert_eq!(
        unread.len(),
        1,
        "exactly one unread_changed event: {events:?}"
    );
    (inbound[0], unread[0])
}

#[tokio::test]
async fn inbound_and_unread_events_match_persisted_room_state() {
    let fixture = crate::test_fixture::test_app_state().await;
    let runtime_guard = fixture.runtime();
    let runtime = runtime_guard.as_ref().expect("runtime available");
    let room_id = [0xD1; 32];
    let store = runtime.message_store();
    let session = runtime.room_sender_session();
    let mut events = runtime.subscribe_ui_events();
    let initial_unread = store
        .get_unread_count(&room_id)
        .expect("read initial persisted unread")
        .unwrap_or(0);

    let remote = iroh::SecretKey::from_bytes(&[0xC4; 32]);
    let sender = *remote.public().as_bytes();
    let payload = "first inbound body";
    let frame = SignedMessage::sign_and_encode(
        &remote,
        &Message::Message {
            text: payload.to_owned(),
        },
    )
    .expect("encode first inbound message");
    assert!(session
        .ingest_room_frame_for_test(room_id, &frame)
        .expect("ingest first frame"));
    let hash = blake3::hash(&frame);
    let first_row = store
        .find_message_by_hash(hash.as_bytes())
        .expect("query persisted message")
        .expect("persisted message row");
    let message_id = first_row.id;
    let persisted_timestamp = first_row.timestamp_ms;
    let first_events = drain_events(&mut events);
    let (inbound, unread) = event_pair(&first_events);
    assert!(
        matches!(inbound, UiEvent::InboundMessage { room_id: r, message_id: id, sender: s, timestamp, payload: p }
        if r == &room_hex(&room_id)
            && *id == message_id
            && *s == sender
            && *timestamp == u64::try_from(persisted_timestamp).unwrap()
            && p == payload)
    );
    let persisted_after_first = store
        .get_unread_count(&room_id)
        .expect("read persisted unread after first message")
        .unwrap_or(0);
    assert_eq!(persisted_after_first, initial_unread + 1);
    assert!(
        matches!(unread, UiEvent::UnreadChanged { room_id: r, unread_count }
        if r == &room_id && *unread_count == persisted_after_first)
    );

    runtime.mark_read(&room_id).expect("mark room read");
    let persisted_after_read = store
        .get_unread_count(&room_id)
        .expect("read persisted unread after mark read");
    assert_eq!(persisted_after_read, Some(0));
    let read_events = drain_events(&mut events);
    assert_eq!(
        read_events
            .iter()
            .filter(|e| matches!(e, UiEvent::UnreadChanged { .. }))
            .count(),
        1
    );
    assert!(
        matches!(read_events.as_slice(), [UiEvent::UnreadChanged { room_id: r, unread_count: 0 }] if r == &room_id)
    );

    let second_payload = "second inbound body";
    let second_frame = SignedMessage::sign_and_encode(
        &remote,
        &Message::Message {
            text: second_payload.to_owned(),
        },
    )
    .expect("encode second inbound message");
    assert!(session
        .ingest_room_frame_for_test(room_id, &second_frame)
        .expect("ingest second frame"));
    let second_events = drain_events(&mut events);
    let (second_inbound, second_unread) = event_pair(&second_events);
    let second_hash = blake3::hash(&second_frame);
    let second_row = store
        .find_message_by_hash(second_hash.as_bytes())
        .expect("query persisted second message")
        .expect("second persisted message row");
    assert!(
        matches!(second_inbound, UiEvent::InboundMessage { room_id: r, message_id: id, sender: s, timestamp, payload: p }
        if r == &room_hex(&room_id)
            && *id == second_row.id
            && *s == sender
            && *timestamp == u64::try_from(second_row.timestamp_ms).unwrap()
            && p == second_payload)
    );
    let persisted_after_second = store
        .get_unread_count(&room_id)
        .expect("read persisted unread after second message")
        .expect("persisted unread row exists");
    assert_eq!(
        persisted_after_second, 1,
        "count increments from the persisted zero"
    );
    assert!(
        matches!(second_unread, UiEvent::UnreadChanged { room_id: r, unread_count }
        if r == &room_id && *unread_count == persisted_after_second)
    );
}
