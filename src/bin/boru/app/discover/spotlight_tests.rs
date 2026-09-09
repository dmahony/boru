//! Spotlight selection is local, stable, eligible, and rendered exactly once.
use super::*;
use boru_core::room_directory::{LocalJoinState, RoomAction, RoomCompatibility};

#[test]
fn discover_spotlight_real_snapshot_honors_persisted_hide_and_search() {
    let (_runtime, mut app) = crate::app::tests::build_prewarm_test_app();
    app.storage = Some(boru_core::storage::Storage::memory().unwrap());
    let topic = TopicId::from_bytes([0x58; 32]);
    app.directory_store.lock().unwrap().upsert(
        RoomAdvertisement {
            room_name: "Local room".into(),
            description: String::new(),
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
    let generation = app.room_generation;
    let conversations = app.conversations.len();
    assert_eq!(
        app.discover_dependency().spotlight_room().unwrap().room_id,
        *topic.as_bytes()
    );
    app.storage
        .as_ref()
        .unwrap()
        .set_room_hidden(topic.as_bytes(), true)
        .unwrap();
    assert!(app.discover_dependency().spotlight_room().is_none());
    app.storage
        .as_ref()
        .unwrap()
        .set_room_hidden(topic.as_bytes(), false)
        .unwrap();
    assert!(app.discover_dependency().spotlight_room().is_some());
    drop(app.update(AppMessage::DiscoverSearchChanged("absent".into())));
    assert!(app.discover_dependency().spotlight_room().is_none());
    drop(app.update(AppMessage::DiscoverClearFilters));
    assert!(app.discover_dependency().spotlight_room().is_some());
    app.pending_topic = Some(topic);
    app.room_loading = true;
    assert!(app.discover_dependency().spotlight_room().is_none());
    assert_eq!(app.room_generation, generation);
    assert_eq!(app.conversations.len(), conversations);
}

fn rooms() -> DiscoverDependency {
    let mut dep = snapshot();
    dep.rooms = (1..=3)
        .rev()
        .map(|id| {
            let mut room = card_tests::room();
            room.room_id = [id; 32];
            room
        })
        .collect();
    dep.total_count = dep.rooms.len();
    dep
}

fn select(
    cache: &mut DiscoverSpotlightSelection,
    dep: &mut DiscoverDependency,
) -> Option<[u8; 32]> {
    dep.page.spotlight_room_id = cache.select(dep, |_| true);
    dep.page.spotlight_room_id
}

#[test]
fn discover_spotlight_eligibility_rejects_every_unavailable_state() {
    let good = card_tests::room();
    assert!(discover_spotlight_eligible(&good));
    for compatibility in [
        RoomCompatibility::Unknown,
        RoomCompatibility::UpgradeRequired,
        RoomCompatibility::Unsupported,
    ] {
        let mut room = good.clone();
        room.compatibility = compatibility;
        assert!(!discover_spotlight_eligible(&room));
    }
    for state in [
        LocalJoinState::Blocked,
        LocalJoinState::Incompatible,
        LocalJoinState::JoinPending,
    ] {
        let mut room = good.clone();
        room.local_join_state = state;
        assert!(!discover_spotlight_eligible(&room));
    }
    for action in [RoomAction::Hidden, RoomAction::Incompatible] {
        let mut room = good.clone();
        room.offered_action = action;
        assert!(!discover_spotlight_eligible(&room));
    }
    let mut room = good.clone();
    room.joining = true;
    assert!(!discover_spotlight_eligible(&room));
    room = good.clone();
    room.conflict = true;
    assert!(!discover_spotlight_eligible(&room));
    room = good;
    room.local_join_state = LocalJoinState::Joined;
    room.offered_action = RoomAction::Open;
    assert!(discover_spotlight_eligible(&room));
}

#[test]
fn discover_spotlight_ties_use_id_and_passive_updates_keep_identity() {
    let mut dep = rooms();
    let mut cache = DiscoverSpotlightSelection::default();
    assert_eq!(select(&mut cache, &mut dep), Some([1; 32]));
    dep.rooms[0].last_seen = Some(Instant::now());
    dep.rooms[0].member_count = Some(1_000_000);
    dep.rooms.reverse();
    assert_eq!(select(&mut cache, &mut dep), Some([1; 32]));
    dep.page.view_mode = DiscoverViewMode::Grid;
    dep.page.open_menu_room_id = Some([3; 32]);
    assert_eq!(select(&mut cache, &mut dep), Some([1; 32]));
    dep.rooms[0].conflict = true;
    assert_eq!(select(&mut cache, &mut dep), Some([3; 32]));
    // The live gate also invalidates an otherwise eligible cached selection.
    assert_eq!(cache.select(&dep, |r| r.room_id != [3; 32]), Some([2; 32]));
    dep.rooms.clear();
    assert_eq!(select(&mut cache, &mut dep), None);
}

#[test]
fn discover_spotlight_browse_context_changes_reselect() {
    let changes: Vec<Box<dyn Fn(&mut DiscoverDependency)>> = vec![
        Box::new(|d| d.search_query = "Example".into()),
        Box::new(|d| d.filter_compatible = true),
        Box::new(|d| d.filter_recently_seen = true),
        Box::new(|d| d.page.membership = DiscoverMembership::NotJoined),
        Box::new(|d| d.selected_tags = vec!["rust".into()]),
        Box::new(|d| d.sort = DiscoverSort::Name),
    ];
    for change in changes {
        let mut dep = rooms();
        let mut cache = DiscoverSpotlightSelection::default();
        assert_eq!(select(&mut cache, &mut dep), Some([1; 32]));
        dep.rooms[0].last_seen = Some(Instant::now());
        dep.rooms[0].room_name = "AAA Example".into();
        assert_eq!(select(&mut cache, &mut dep), Some([1; 32]));
        change(&mut dep);
        assert_eq!(select(&mut cache, &mut dep), Some([3; 32]));
    }
}

#[test]
fn discover_spotlight_cannot_escape_filtered_results() {
    for (query, filters, tags) in [
        ("absent", DiscoverFilterState::default(), vec![]),
        (
            "",
            DiscoverFilterState {
                membership: DiscoverMembership::Joined,
                ..Default::default()
            },
            vec![],
        ),
        ("", DiscoverFilterState::default(), vec!["absent".into()]),
    ] {
        let mut dep = rooms();
        let mut cache = DiscoverSpotlightSelection::default();
        assert!(select(&mut cache, &mut dep).is_some());
        dep.rooms = discover_filter_sort(
            dep.rooms.into_iter().map(|r| (r, None)).collect(),
            query,
            filters,
            &tags,
            DiscoverSort::Name,
            Instant::now(),
        );
        assert!(dep.rooms.is_empty());
        assert_eq!(select(&mut cache, &mut dep), None);
        assert!(dep.spotlight_room().is_none());
    }
}

#[test]
fn discover_spotlight_grid_list_dedup_and_disabled_restore() {
    for mode in [DiscoverViewMode::Grid, DiscoverViewMode::List] {
        let mut dep = rooms();
        dep.page.view_mode = mode;
        let mut cache = DiscoverSpotlightSelection::default();
        select(&mut cache, &mut dep);
        let ids: Vec<_> = dep.result_rooms().iter().map(|r| r.room_id).collect();
        assert_eq!(ids, vec![[3; 32], [2; 32]]);
        assert_eq!(dep.rooms.len(), 3);
        dep.layout.show_spotlight = false;
        assert!(dep.spotlight_room().is_none());
        assert_eq!(dep.result_rooms().len(), 3);
        assert_eq!(select(&mut cache, &mut dep), None);
        dep.layout.show_spotlight = true;
        dep.rooms.retain(|r| r.room_id == [1; 32]);
        dep.total_count = 1;
        select(&mut cache, &mut dep);
        assert!(dep.result_rooms().is_empty());
        // Native page has header, controls, ticket, spotlight; no blank results.
        let node = measure(IcedChat::view_discover_content(&dep), 720.0, 400.0);
        let sections = &node.children()[0].children()[0].children()[0].children()[0].children()[0];
        assert_eq!(sections.children().len(), 4);
        let ticket = &sections.children()[2];
        let spotlight = &sections.children()[3];
        assert!(spotlight.bounds().y >= ticket.bounds().y + ticket.size().height);
        assert_eq!(spotlight.size().width, ticket.size().width);
        dep.rooms[0].conflict = true;
        select(&mut cache, &mut dep);
        let node = measure(IcedChat::view_discover_content(&dep), 720.0, 400.0);
        let sections = &node.children()[0].children()[0].children()[0].children()[0].children()[0];
        assert_eq!(sections.children().len(), 4); // results replaces spotlight
        assert!(dep.spotlight_room().is_none());
    }
}
