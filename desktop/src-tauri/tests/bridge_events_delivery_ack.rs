use boru_core::{
    chat_core::{Message, SignedMessage},
    desktop_runtime::{DesktopRuntime, DesktopRuntimeConfig},
    ui_events::UiEvent,
};
use std::time::Duration;
use tempfile::TempDir;

// A missing duplicate event should become observable quickly without making the
// integration test sensitive to scheduler noise on a loaded CI worker.
const DUPLICATE_EVENT_WINDOW: Duration = Duration::from_millis(150);

struct AckContext {
    _profile: TempDir,
    runtime: DesktopRuntime,
    events: tokio::sync::broadcast::Receiver<UiEvent>,
    conversation_id: [u8; 32],
    message_id: i64,
    message_hash: [u8; 32],
    receipt: Vec<u8>,
}

async fn context_with_pending_message() -> AckContext {
    let profile = tempfile::tempdir().expect("temporary isolated profile");
    let runtime = DesktopRuntime::start_from_config(DesktopRuntimeConfig {
        data_dir: profile.path().to_path_buf(),
        secret_key: None,
        relay_mode: iroh::RelayMode::Disabled,
        bind_port: 0,
        enable_mdns: false,
        enable_dht: false,
        publish_direct_addresses: false,
    })
    .await
    .expect("network-isolated desktop runtime starts")
    .0;

    let conversation_id = [0xA1; 32];
    let sender = iroh::SecretKey::from_bytes(&[0xC3; 32]);
    let outgoing = SignedMessage::sign_and_encode(
        &sender,
        &Message::Message {
            text: "delivery acknowledgement fixture".into(),
        },
    )
    .expect("encode outgoing fixture message");
    let message_hash = *blake3::hash(&outgoing).as_bytes();
    let local_user = *runtime.endpoint().id().as_bytes();
    let store = runtime.message_store();
    assert!(store
        .insert_chat_message(
            &message_hash,
            &conversation_id,
            sender.public().as_bytes(),
            1,
            "text",
            "delivery acknowledgement fixture",
            Some(&outgoing),
            None,
            &local_user,
        )
        .expect("persist pending outgoing row"));
    let message_id = store
        .find_message_by_hash(&message_hash)
        .expect("find pending outgoing row")
        .expect("pending outgoing row exists")
        .id;

    // Subscribe before injecting the peer receipt so the emitted event is
    // asserted from the observable receiver, not inferred from stored state.
    let events = runtime.subscribe_ui_events();
    let receipt = SignedMessage::sign_and_encode(
        &iroh::SecretKey::from_bytes(&[0xD4; 32]),
        &Message::ReadReceipt { message_hash },
    )
    .expect("encode peer delivery acknowledgement");

    AckContext {
        _profile: profile,
        runtime,
        events,
        conversation_id,
        message_id,
        message_hash,
        receipt: receipt.to_vec(),
    }
}

fn inject_receipt(context: &AckContext) {
    context
        .runtime
        .room_sender_session()
        .ingest_room_frame_for_test(context.conversation_id, &context.receipt)
        .expect("process peer receipt");
}

#[tokio::test]
async fn delivery_ack_emitted_with_matching_message_id() {
    let mut context = context_with_pending_message().await;
    inject_receipt(&context);

    let event = tokio::time::timeout(DUPLICATE_EVENT_WINDOW, context.events.recv())
        .await
        .expect("receipt should emit a delivery event within the bounded window")
        .expect("delivery event channel remains open");
    assert!(
        matches!(event, UiEvent::DeliveryStateChanged { message_id, state: boru_core::chat_history::DeliveryState::Seen } if message_id == context.message_id),
        "expected typed Seen event for local message id {}, got {event:?}",
        context.message_id,
    );
    assert!(
        matches!(
            context.events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ),
        "exactly one delivery event should be emitted for the receipt"
    );

    context.runtime.stop().await.expect("stop isolated runtime");
}

#[tokio::test]
async fn duplicate_delivery_ack_is_idempotent() {
    let mut context = context_with_pending_message().await;
    inject_receipt(&context);
    let first = tokio::time::timeout(DUPLICATE_EVENT_WINDOW, context.events.recv())
        .await
        .expect("first receipt event arrives within the bounded window")
        .expect("delivery event channel remains open");
    assert!(matches!(
        first,
        UiEvent::DeliveryStateChanged { message_id, state: boru_core::chat_history::DeliveryState::Seen }
            if message_id == context.message_id
    ));

    inject_receipt(&context);
    assert!(
        tokio::time::timeout(DUPLICATE_EVENT_WINDOW, context.events.recv())
            .await
            .is_err(),
        "duplicate receipt must not emit a second delivery event"
    );

    context.runtime.stop().await.expect("stop isolated runtime");
}

#[tokio::test]
async fn typed_event_assertions_used_throughout() {
    let mut context = context_with_pending_message().await;
    inject_receipt(&context);

    let event = tokio::time::timeout(DUPLICATE_EVENT_WINDOW, context.events.recv())
        .await
        .expect("receipt event arrives within the bounded window")
        .expect("delivery event channel remains open");
    match event {
        UiEvent::DeliveryStateChanged { message_id, state } => {
            assert_eq!(message_id, context.message_id);
            assert_eq!(state, boru_core::chat_history::DeliveryState::Seen);
        }
        other => panic!("expected typed delivery state event, got {other:?}"),
    }
    assert_eq!(
        context
            .runtime
            .message_store()
            .find_message_by_hash(&context.message_hash)
            .expect("read persisted delivery state")
            .expect("acknowledged message remains present")
            .delivery_state,
        "seen",
    );

    context.runtime.stop().await.expect("stop isolated runtime");
}
