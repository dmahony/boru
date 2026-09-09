//! Action-time eligibility and real subscription lifecycle regressions.
use super::*;
use crate::app::tests::build_prewarm_test_app;

fn advertise(app: &mut IcedChat, topic: TopicId) {
    app.directory_store.lock().unwrap().upsert(
        RoomAdvertisement {
            room_name: "Action room".into(),
            description: "Keep this metadata on failure".into(),
            topic,
            ticket: Ticket {
                topic,
                peers: Vec::new(),
                discovery_secret: None,
            }
            .to_string(),
            member_count: 0,
            last_activity: 0,
            expires_after_secs: ADVERT_TTL_SECS,
        },
        app.endpoint.id(),
    );
}

fn completion(runtime: &tokio::runtime::Runtime, task: iced::Task<AppMessage>) -> AppMessage {
    runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(task).expect("subscription task");
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(action) = iced::futures::StreamExt::next(&mut stream).await {
                if let iced_runtime::Action::Output(message) = action {
                    return message;
                }
                // OpenRoom also emits a real scroll-to-bottom widget operation.
            }
            panic!("expected subscription result");
        })
        .await
        .expect("completion within timeout")
    })
}

#[test]
fn discover_action_rechecks_missing_and_hidden_legacy_rooms() {
    let (_runtime, mut app) = build_prewarm_test_app();
    app.screen = Screen::Discover;
    app.storage = Some(boru_core::storage::Storage::memory().unwrap());
    let topic = TopicId::from_bytes([0x59; 32]);
    assert!(app.directory_join_target(*topic.as_bytes()).is_err());
    advertise(&mut app, topic);
    assert!(app.directory_join_target(*topic.as_bytes()).is_ok());
    let generation = app.room_generation;
    drop(app.update(AppMessage::DirectoryRoomHideById(*topic.as_bytes())));
    assert!(app
        .storage
        .as_ref()
        .unwrap()
        .room_hidden_ids()
        .unwrap()
        .contains(topic.as_bytes()));
    assert!(app.discover_dependency().rooms.is_empty());
    drop(app.update(AppMessage::DirectoryRoomJoinById(*topic.as_bytes())));
    assert!(!app.room_loading);
    assert_eq!(app.room_generation, generation);
    assert!(app.directory_advert_for_topic(&topic).is_some());
    drop(app.update(AppMessage::DirectoryRoomUnhideById(*topic.as_bytes())));
    assert!(app.directory_join_target(*topic.as_bytes()).is_ok());
    assert_eq!(app.discover_dependency().rooms.len(), 1);
}

#[test]
fn discover_action_real_join_is_synchronous_pending_and_idempotent() {
    let (runtime, mut app) = build_prewarm_test_app();
    app.screen = Screen::Discover;
    app.private_dht_disabled = true;
    let topic = TopicId::from_bytes([0x5a; 32]);
    advertise(&mut app, topic);
    let task = app.update(AppMessage::DirectoryRoomJoinById(*topic.as_bytes()));
    assert!(app.room_loading);
    assert_eq!(app.pending_topic, Some(topic));
    let generation = app.room_generation;
    assert_eq!(
        app.update(AppMessage::DirectoryRoomJoinById(*topic.as_bytes()))
            .units(),
        0
    );
    assert_eq!(app.room_generation, generation);
    assert!(app.discover_dependency().rooms[0].joining);
    drop(app.update(AppMessage::DiscoverSearchChanged("no match".into())));
    assert!(app.discover_dependency().rooms.is_empty());
    drop(app.update(AppMessage::DiscoverClearFilters));
    assert!(app.discover_dependency().rooms[0].joining);
    let message = completion(&runtime, task);
    assert!(matches!(message, AppMessage::RoomOpened { .. }));
    runtime.block_on(async {
        drop(app.update(message));
    });
    assert!(!app.room_loading);
    assert_eq!(app.pending_topic, None);
    assert!(app.conversation_store.find(&topic).is_some());
    assert_eq!(
        app.discover_dependency().rooms[0].offered_action,
        boru_core::room_directory::RoomAction::Open
    );
    let count = app.conversations.len();
    runtime.block_on(async {
        drop(app.update(AppMessage::OpenRoom(topic)));
    });
    assert_eq!(app.conversations.len(), count);
    assert_eq!(app.room_generation, generation);
    assert_eq!(app.screen, Screen::Chat { topic });
}

#[test]
fn discover_action_real_failure_preserves_directory_and_ticket() {
    let (runtime, mut app) = build_prewarm_test_app();
    app.screen = Screen::Discover;
    app.private_dht_disabled = true;
    app.discover_ticket_input = "keep ticket draft".into();
    app.discover_search_query = "Action".into();
    let topic = TopicId::from_bytes([0x5b; 32]);
    advertise(&mut app, topic);
    let task = app.update(AppMessage::DirectoryRoomJoinById(*topic.as_bytes()));
    runtime.block_on(app._router.clone().shutdown()).unwrap();
    let message = completion(&runtime, task);
    assert!(matches!(message, AppMessage::RoomJoinFailed { .. }));
    drop(app.update(message));
    assert!(!app.room_loading);
    assert_eq!(app.pending_topic, None);
    assert_eq!(app.screen, Screen::Discover);
    assert_eq!(app.discover_ticket_input, "keep ticket draft");
    assert_eq!(app.discover_search_query, "Action");
    assert!(app.directory_advert_for_topic(&topic).is_some());
    assert!(app.conversation_store.find(&topic).is_none());
    assert!(!app.discover_dependency().rooms[0].joining);
    assert!(!app.discover_dependency().room_error.is_empty());
    let retry = app.update(AppMessage::DirectoryRoomJoinById(*topic.as_bytes()));
    assert!(app.room_loading);
    assert!(app.discover_dependency().room_error.is_empty());
    let failed_again = completion(&runtime, retry);
    drop(app.update(failed_again));
    assert!(!app.room_loading);
    assert_eq!(app.pending_topic, None);
}

#[test]
fn discover_action_expired_cache_entry_cannot_start_a_join() {
    let (_runtime, mut app) = build_prewarm_test_app();
    let topic = TopicId::from_bytes([0x5c; 32]);
    let owner = app.endpoint.id();
    let mut dir = boru_core::room_directory::RoomDirectory::new();
    let mut advert = boru_core::control_plane::advertisement::PublicRoomAdvertisement::minimal(
        topic,
        "Expired".into(),
        *owner.as_bytes(),
    );
    advert.expires_after_secs = 1;
    dir.apply_advertisement_at(
        advert,
        owner,
        boru_core::control_plane::advertisement::AdvertisementAuth::Verified { publisher: owner },
        1,
        1000,
        Instant::now() - Duration::from_secs(10),
    );
    assert!(dir.get(&topic).is_some(), "not swept yet");
    app.room_directory = Some(Arc::new(StdMutex::new(dir)));
    assert!(app.directory_join_target(*topic.as_bytes()).is_err());
    let generation = app.room_generation;
    drop(app.update(AppMessage::DirectoryRoomJoinById(*topic.as_bytes())));
    assert_eq!(app.room_generation, generation);
    assert!(!app.room_loading);
    assert!(app.conversation_store.find(&topic).is_none());
}

#[test]
fn discover_action_cached_open_supersedes_pending_and_ignores_late_completion() {
    let (runtime, mut app) = build_prewarm_test_app();
    app.private_dht_disabled = true;
    let first = TopicId::from_bytes([0x5d; 32]);
    let second = TopicId::from_bytes([0x5e; 32]);
    advertise(&mut app, first);
    advertise(&mut app, second);
    let task = app.update(AppMessage::DirectoryRoomJoinById(*first.as_bytes()));
    let message = completion(&runtime, task);
    runtime.block_on(async {
        drop(app.update(message));
    });
    app.screen = Screen::Discover;
    let task = app.update(AppMessage::DirectoryRoomJoinById(*second.as_bytes()));
    assert_eq!(app.pending_topic, Some(second));
    runtime.block_on(async {
        drop(app.update(AppMessage::OpenRoom(first)));
    });
    assert!(!app.room_loading);
    assert_eq!(app.pending_topic, None);
    let late = completion(&runtime, task);
    runtime.block_on(async {
        drop(app.update(late));
    });
    assert_eq!(app.screen, Screen::Chat { topic: first });
    assert!(app.conversation_store.find(&second).is_none());
}

#[test]
fn discover_action_hide_pending_is_local_and_does_not_cancel_explicit_join() {
    let (runtime, mut app) = build_prewarm_test_app();
    app.private_dht_disabled = true;
    app.screen = Screen::Discover;
    app.storage = Some(boru_core::storage::Storage::memory().unwrap());
    let topic = TopicId::from_bytes([0x5f; 32]);
    advertise(&mut app, topic);
    let task = app.update(AppMessage::DirectoryRoomJoinById(*topic.as_bytes()));
    let generation = app.room_generation;
    drop(app.update(AppMessage::DirectoryRoomHideById(*topic.as_bytes())));
    assert!(app.discover_dependency().rooms.is_empty());
    assert_eq!(app.room_generation, generation);
    let message = completion(&runtime, task);
    runtime.block_on(async {
        drop(app.update(message));
    });
    assert!(!app.room_loading);
    assert_eq!(app.pending_topic, None);
    assert!(app.conversation_store.find(&topic).is_some());
    assert!(app.sender.is_some());
    assert!(app.discover_dependency().rooms.is_empty());
}

#[test]
fn discover_action_hide_without_storage_reports_failure_without_removing_room() {
    let (_runtime, mut app) = build_prewarm_test_app();
    app.storage = None;
    let topic = TopicId::from_bytes([0x60; 32]);
    advertise(&mut app, topic);
    drop(app.update(AppMessage::DirectoryRoomHideById(*topic.as_bytes())));
    assert_eq!(app.discover_dependency().rooms.len(), 1);
    assert!(!app.discover_dependency().room_error.is_empty());
    assert!(app.directory_advert_for_topic(&topic).is_some());
}
