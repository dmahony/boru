//! Empty/error recovery and localization, using the real native widget tree.
use super::*;
use crate::app::tests::build_prewarm_test_app;

fn assert_contained(node: &layout::Node) {
    for child in node.children() {
        assert!(child.bounds().x >= -0.1, "{:?}", child.bounds());
        assert!(
            child.bounds().x + child.size().width <= node.size().width + 0.1,
            "child {:?} exceeds {:?}",
            child.bounds(),
            node.size()
        );
        assert_contained(child);
    }
}

#[test]
fn discover_state_zero_one_many_and_no_matches_are_distinct() {
    let mut dep = snapshot();
    assert_eq!(
        dep.empty_copy(),
        Some((dep.labels.empty.as_str(), dep.labels.empty_hint.as_str()))
    );
    for count in [1, 8] {
        dep.rooms = (0..count)
            .map(|id| {
                let mut room = card_tests::room();
                room.room_id = [id; 32];
                room
            })
            .collect();
        dep.total_count = dep.rooms.len();
        assert!(dep.empty_copy().is_none());
        for mode in [DiscoverViewMode::Grid, DiscoverViewMode::List] {
            dep.page.view_mode = mode;
            assert_contained(&measure(
                IcedChat::view_discover_content(&dep),
                320.0,
                240.0,
            ));
        }
    }
    dep.rooms.clear();
    dep.search_query = "No matching room".into();
    dep.selected_tags = vec!["retained".into()];
    dep.available_tags = dep.selected_tags.clone();
    assert_eq!(
        dep.empty_copy(),
        Some((
            dep.labels.no_matches.as_str(),
            dep.labels.no_matches_hint.as_str()
        ))
    );
    let before = dep.clone();
    assert_contained(&measure(
        IcedChat::view_discover_content(&dep),
        320.0,
        240.0,
    ));
    assert_eq!(dep, before);
}

#[test]
fn discover_state_clear_recovery_works_without_optional_toolbar() {
    let (_runtime, mut app) = build_prewarm_test_app();
    app.screen = Screen::Discover;
    app.discover_search_query = "keep until clear".into();
    app.discover_filter_compatible = true;
    app.discover_filter_recently_seen = true;
    app.discover_page.membership = DiscoverMembership::Joined;
    app.discover_selected_tags = vec!["tag".into()];
    app.discover_ticket_input = "independent ticket draft".into();
    let mut dep = app.discover_dependency();
    dep.layout.show_controls = false;
    for key in [
        iced::keyboard::key::Named::Enter,
        iced::keyboard::key::Named::Space,
    ] {
        let messages = card_tests::activate(IcedChat::discover_empty_state(&dep), 0, key);
        assert!(matches!(
            messages.as_slice(),
            [AppMessage::DiscoverClearFilters]
        ));
    }
    drop(app.update(AppMessage::DiscoverClearFilters));
    assert!(!app.discover_dependency().has_filters());
    assert!(app.discover_selected_tags.is_empty());
    assert_eq!(app.discover_ticket_input, "independent ticket draft");
    assert_eq!(app.screen, Screen::Discover);
}

#[test]
fn discover_state_unavailable_refresh_retains_cache_and_never_claims_loading() {
    let (_runtime, mut app) = build_prewarm_test_app();
    app.screen = Screen::Discover;
    app.dht = None;
    let topic = TopicId::from_bytes([0x71; 32]);
    app.directory_store.lock().unwrap().upsert(
        RoomAdvertisement {
            room_name: "Retained room".into(),
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
    app.discover_ticket_input = "draft".into();
    let before = app.discover_dependency();
    assert_eq!(
        before.availability_warning,
        crate::i18n::t("discover.registry_unavailable")
    );
    assert_eq!(app.update(AppMessage::RefreshRoomRegistry).units(), 0);
    let after = app.discover_dependency();
    assert_eq!(after.rooms, before.rooms);
    assert_eq!(after.ticket_input, "draft");
    assert!(!after.ticket_pending && !after.ticket_blocked);
    assert!(after.rooms.iter().all(|room| !room.joining));
    assert_contained(&measure(
        IcedChat::view_discover_content(&after),
        320.0,
        240.0,
    ));
}

#[test]
fn discover_state_long_localized_empty_failure_and_pending_copy_wraps() {
    let fr = crate::i18n::Translations::load(
        "fr",
        &[std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/bin/boru/locales")],
    );
    let mut dep = snapshot();
    dep.labels.empty = fr.t("discover.no_public_rooms_yet");
    dep.labels.empty_hint = fr.t("discover.empty_local_hint").repeat(3);
    dep.labels.no_matches = fr.t("discover.no_matches");
    dep.labels.no_matches_hint = fr.t("discover.no_matches_hint").repeat(3);
    dep.labels.clear_filters = fr.t("discover.clear_filters");
    dep.labels.ticket_title = fr.t("sidebar.join_ticket_title");
    dep.labels.ticket_hint = fr.t("discover.ticket_hint").repeat(3);
    dep.labels.ticket_joining = fr.t("discover.ticket_joining");
    dep.labels.ticket_join = fr.t("sidebar.join_ticket_button");
    dep.room_error = fr.t("discover.advert_expired").repeat(3);
    dep.ticket_error = fr.t("discover.invalid_ticket").repeat(3);
    dep.availability_warning = fr.t("discover.registry_unavailable").repeat(3);
    dep.search_query = "retained query".into();
    dep.ticket_input = "retained ticket".into();
    for count in [0, 2] {
        dep.total_count = count;
        for pending in [false, true] {
            dep.ticket_pending = pending;
            dep.ticket_blocked = pending;
            for width in [240.0, 320.0, 720.0] {
                let node = measure(IcedChat::view_discover_content(&dep), width, 240.0);
                assert_eq!(node.size(), Size::new(width, 240.0));
                assert_contained(&node);
            }
        }
    }
    let wide = measure(IcedChat::discover_empty_state(&dep), 720.0, f32::INFINITY);
    let narrow = measure(IcedChat::discover_empty_state(&dep), 240.0, f32::INFINITY);
    assert!(narrow.size().height > wide.size().height);
}

#[test]
fn discover_state_localization_keys_and_placeholders_are_complete() {
    let en: std::collections::HashMap<String, String> =
        serde_json::from_str(include_str!("../../locales/en.json")).unwrap();
    let fr: std::collections::HashMap<String, String> =
        serde_json::from_str(include_str!("../../locales/fr.json")).unwrap();
    let placeholders = |value: &str| -> std::collections::BTreeSet<String> {
        value
            .split('{')
            .skip(1)
            .filter_map(|part| part.split_once('}').map(|(name, _)| name.to_owned()))
            .collect()
    };
    for key in include_str!("../discover.rs")
        .split('"')
        .chain(include_str!("../../app.rs").split('"'))
        .filter(|key| key.starts_with("discover."))
    {
        let english = en
            .get(key)
            .unwrap_or_else(|| panic!("missing English {key}"));
        let french = fr
            .get(key)
            .unwrap_or_else(|| panic!("missing French {key}"));
        assert!(!english.is_empty() && !french.is_empty(), "{key}");
        assert_eq!(placeholders(english), placeholders(french), "{key}");
    }
    let french = crate::i18n::Translations::load(
        "fr",
        &[std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/bin/boru/locales")],
    );
    let message = french.t_args("discover.join_upgrade", &[("remote", "42"), ("local", "1")]);
    assert!(message.contains("v42") && message.contains("v1"));
    assert!(!message.contains('{'));
    assert_ne!(french.t("discover.no_matches"), en["discover.no_matches"]);
}
