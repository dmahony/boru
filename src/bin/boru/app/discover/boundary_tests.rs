//! Owned cache merge and passive-update boundaries.
use super::*;
use crate::app::tests::build_prewarm_test_app;
use boru_core::room_directory::{LocalJoinState, LocalRoomFacts, RoomDirectory};

#[test]
fn discover_idle_prewarm_excludes_live_dropdown_page() {
    assert!(!PREWARM_ORDER.contains(&Screen::Discover));
    let (_runtime, mut app) = build_prewarm_test_app();
    app.idle_timer.last_input = Instant::now() - Duration::from_secs(3);
    for _ in 0..=PREWARM_ORDER.len() {
        let _ = app.update(AppMessage::IdleTick);
    }
    assert!(!app.prewarm_cache.contains_key(&Screen::Discover));
    assert_eq!(app.prewarm_cache.len(), PREWARM_ORDER.len());
}

fn legacy(topic: TopicId, name: &str) -> RoomAdvertisement {
    RoomAdvertisement {
        topic,
        room_name: name.into(),
        description: "Legacy metadata".into(),
        ticket: Ticket {
            topic,
            peers: vec![],
            discovery_secret: None,
        }
        .to_string(),
        member_count: 0,
        last_activity: 0,
        expires_after_secs: ADVERT_TTL_SECS,
    }
}

fn directory(app: &IcedChat, topic: TopicId, age: Duration) -> RoomDirectory {
    let owner = app.endpoint.id();
    let mut ad = boru_core::control_plane::advertisement::PublicRoomAdvertisement::minimal(
        topic,
        "Canonical".into(),
        *owner.as_bytes(),
    );
    ad.expires_after_secs = 60;
    let mut dir = RoomDirectory::new();
    dir.apply_advertisement_at(
        ad,
        owner,
        boru_core::control_plane::advertisement::AdvertisementAuth::Verified { publisher: owner },
        1,
        1000,
        Instant::now() - age,
    );
    dir
}

#[test]
fn discover_boundary_legacy_dedup_is_stable_and_canonical_wins() {
    let (_runtime, mut app) = build_prewarm_test_app();
    let topic = TopicId::from_bytes([0x81; 32]);
    let other = SecretKey::generate().public();
    app.directory_store
        .lock()
        .unwrap()
        .upsert(legacy(topic, "First"), app.endpoint.id());
    app.directory_store
        .lock()
        .unwrap()
        .upsert(legacy(topic, "Second"), other);
    let first = app.discover_dependency();
    assert_eq!(first.total_count, 1);
    assert_eq!(first.rooms[0].last_seen, None);
    assert_eq!(first.rooms[0].room_protocol_version, 0);
    assert_eq!(first.rooms[0].owner_peer_id, [0; 32]);
    assert!(first.rooms[0].tags.is_empty());
    assert_eq!(first.rooms, app.discover_dependency().rooms);
    assert_eq!(
        first.rooms[0].room_name,
        app.directory_advert_for_topic(&topic).unwrap().room_name
    );
    app.room_directory = Some(Arc::new(StdMutex::new(directory(
        &app,
        topic,
        Duration::ZERO,
    ))));
    let dep = app.discover_dependency();
    assert_eq!(dep.total_count, 1);
    assert_eq!(dep.rooms[0].room_name, "Canonical");
}

#[test]
fn discover_sort_ties_are_stable_across_cache_insertion_orders() {
    use boru_core::control_plane::advertisement::{AdvertisementAuth, PublicRoomAdvertisement};
    let (_runtime, mut app) = build_prewarm_test_app();
    let owner = app.endpoint.id();
    let seen = Instant::now();
    for canonical in [false, true] {
        for order in [[3, 1, 2], [2, 3, 1], [1, 2, 3]] {
            let mut dir = RoomDirectory::new();
            for id in order {
                let topic = TopicId::from_bytes([id; 32]);
                if canonical {
                    dir.apply_advertisement_at(
                        PublicRoomAdvertisement::minimal(topic, "Same".into(), *owner.as_bytes()),
                        owner,
                        AdvertisementAuth::Verified { publisher: owner },
                        1,
                        1000,
                        seen,
                    );
                } else {
                    app.directory_store
                        .lock()
                        .unwrap()
                        .upsert(legacy(topic, "Same"), owner);
                }
            }
            app.room_directory = canonical.then(|| Arc::new(StdMutex::new(dir)));
            for sort in [
                DiscoverSort::Name,
                DiscoverSort::Compatibility,
                DiscoverSort::RecentlySeen,
            ] {
                app.discover_sort = sort;
                let dep = app.discover_dependency();
                assert_eq!(
                    dep.rooms
                        .iter()
                        .map(|room| room.room_id[0])
                        .collect::<Vec<_>>(),
                    vec![1, 2, 3]
                );
                assert_eq!(
                    dep.total_count, 3,
                    "canonical entries deduplicate legacy rows"
                );
            }
        }
    }
}

#[test]
fn discover_boundary_expired_and_blocked_cache_cannot_leak_via_legacy() {
    let (_runtime, mut app) = build_prewarm_test_app();
    let topic = TopicId::from_bytes([0x82; 32]);
    app.directory_store
        .lock()
        .unwrap()
        .upsert(legacy(topic, "Fallback"), app.endpoint.id());
    app.room_directory = Some(Arc::new(StdMutex::new(directory(
        &app,
        topic,
        Duration::from_secs(1000),
    ))));
    assert!(
        app.room_directory
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .get(&topic)
            .unwrap()
            .expires_at
            < Instant::now()
    );
    assert!(
        app.discover_dependency().rooms.is_empty(),
        "unswept expiry must match action gate"
    );
    let mut dir = directory(&app, topic, Duration::ZERO);
    dir.sync_local_states(LocalRoomFacts {
        hidden: [topic].into(),
        ..Default::default()
    });
    app.room_directory = Some(Arc::new(StdMutex::new(dir)));
    assert!(
        app.discover_dependency().rooms.is_empty(),
        "legacy cannot bypass blocked canonical verdict"
    );
}

#[test]
fn discover_boundary_membership_comes_from_current_local_store() {
    let (_runtime, mut app) = build_prewarm_test_app();
    let topic = TopicId::from_bytes([0x83; 32]);
    let mut dir = directory(&app, topic, Duration::ZERO);
    dir.sync_local_states(LocalRoomFacts {
        joined: [topic].into(),
        ..Default::default()
    });
    app.room_directory = Some(Arc::new(StdMutex::new(dir)));
    assert_eq!(
        app.discover_dependency().rooms[0].local_join_state,
        LocalJoinState::NotJoined
    );
    app.conversation_store
        .upsert(ConversationEntry::new(topic, "", "Joined"));
    assert_eq!(
        app.discover_dependency().rooms[0].local_join_state,
        LocalJoinState::Joined
    );
}

#[test]
fn discover_boundary_timestamp_only_refresh_does_not_invalidate_render_key() {
    let (_runtime, mut app) = build_prewarm_test_app();
    let topic = TopicId::from_bytes([0x84; 32]);
    app.room_directory = Some(Arc::new(StdMutex::new(directory(
        &app,
        topic,
        Duration::from_secs(10),
    ))));
    let before = app.discover_dependency();
    let mut after = before.clone();
    after.rooms[0].last_seen = after.rooms[0]
        .last_seen
        .map(|seen| seen + Duration::from_secs(1));
    assert_eq!(fxhash_of(&before), fxhash_of(&after));
    after.rooms[0].last_seen_minutes = Some(1);
    assert_ne!(fxhash_of(&before), fxhash_of(&after));
}

#[test]
fn discover_boundary_passive_advertisement_never_joins_or_creates_activity() {
    let (runtime, mut app) = build_prewarm_test_app();
    let _guard = runtime.enter();
    let topic = TopicId::from_bytes([0x85; 32]);
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    app.directory_room_rx = Arc::new(Mutex::new(rx));
    app.discover_search_query = "Passive".into();
    app.discover_ticket_input = "untouched draft".into();
    let activities = app.recent_activity_card_data().total;
    let generation = app.room_generation;
    tx.try_send(DirectoryRoomEvent::Advertisement(
        legacy(topic, "Passive"),
        app.endpoint.id(),
    ))
    .unwrap();
    drop(app.update(AppMessage::ConnMonitorTick));
    assert!(app.conversation_store.find(&topic).is_none());
    assert!(!app.conversations.contains_key(&topic));
    assert!(!app.rooms_state.auto_subscribed_rooms.contains(&topic));
    assert_eq!(app.recent_activity_card_data().total, activities);
    assert_eq!(app.room_generation, generation);
    assert_eq!(app.discover_search_query, "Passive");
    assert_eq!(app.discover_ticket_input, "untouched draft");
    assert_eq!(app.discover_dependency().rooms.len(), 1);
    drop(app.update(AppMessage::DiscoverRoomMenuChanged(Some(*topic.as_bytes()))));
    tx.try_send(DirectoryRoomEvent::Withdrawal(topic, app.endpoint.id()))
        .unwrap();
    drop(app.update(AppMessage::ConnMonitorTick));
    assert_eq!(app.discover_page.open_menu_room_id, None);
    tx.try_send(DirectoryRoomEvent::Advertisement(
        legacy(topic, "Passive"),
        app.endpoint.id(),
    ))
    .unwrap();
    drop(app.update(AppMessage::ConnMonitorTick));
    assert_eq!(app.discover_dependency().page.open_menu_room_id, None);
}

#[test]
fn discover_boundary_real_snapshot_update_preserves_caret_and_renders_without_locks() {
    use iced::advanced::{layout, widget::Tree};
    use iced::{Font, Pixels, Size};
    type InputState = iced::widget::text_input::State<
        <iced::Renderer as iced::advanced::text::Renderer>::Paragraph,
    >;
    let (_runtime, mut app) = build_prewarm_test_app();
    let topic = TopicId::from_bytes([0x86; 32]);
    app.discover_search_query = "Legacy".into();
    app.directory_store
        .lock()
        .unwrap()
        .upsert(legacy(topic, "Legacy"), app.endpoint.id());
    let before = app.discover_dependency();
    let controls = IcedChat::discover_controls(&before);
    let mut tree = Tree::new(controls.as_widget());
    let state = tree.children[0].children[1]
        .state
        .downcast_mut::<InputState>();
    state.focus();
    state.move_cursor_to(3);
    let cursor = state.cursor();
    app.directory_store
        .lock()
        .unwrap()
        .upsert(legacy(topic, "Legacy refreshed"), app.endpoint.id());
    let after = app.discover_dependency();
    assert_ne!(fxhash_of(&before), fxhash_of(&after));
    let controls = IcedChat::discover_controls(&after);
    tree.diff(controls.as_widget());
    let state = tree.children[0].children[1]
        .state
        .downcast_ref::<InputState>();
    assert!(state.is_focused());
    assert_eq!(state.cursor(), cursor);

    // Render the real owned snapshot while both source locks are unavailable.
    app.room_directory = Some(Arc::new(StdMutex::new(RoomDirectory::new())));
    let _directory_guard = app.room_directory.as_ref().unwrap().lock().unwrap();
    let _legacy_guard = app.directory_store.lock().unwrap();
    let renderer =
        iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)));
    let mut page = IcedChat::view_discover_content(&after);
    let mut tree = Tree::new(page.as_widget());
    let node = page.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, Size::new(900.0, 600.0)),
    );
    assert_eq!(node.size(), Size::new(900.0, 600.0));
}
