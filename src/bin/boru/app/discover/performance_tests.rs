//! Controlled headless profile; run explicitly with --ignored --nocapture.
use super::*;
use iced::advanced::{renderer, Layout};
use iced::{mouse, Event, Rectangle};

#[test]
fn discover_cache_reuses_cards_and_invalidates_visible_changes() {
    let mut dep = snapshot();
    dep.rooms = vec![card_tests::room()];
    dep.total_count = 1;
    let renderer =
        iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)));
    let mut tree = Tree::empty();
    let mut render = |dep: &DiscoverDependency, width| {
        let before = DISCOVER_CARD_BUILDS.with(|count| count.get());
        let mut element: iced::Element<'static, AppMessage> =
            iced::widget::lazy(dep.clone(), IcedChat::view_discover_content).into();
        tree.diff(element.as_widget());
        element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(width, 720.0)),
        );
        DISCOVER_CARD_BUILDS.with(|count| count.get()) - before
    };
    assert_eq!(render(&dep, 900.0), 1);
    dep.ticket_input = "ticket draft".into();
    assert_eq!(render(&dep, 900.0), 0);
    dep.search_query = "Room".into();
    assert_eq!(render(&dep, 900.0), 0);
    dep.ticket_pending = true;
    assert_eq!(render(&dep, 900.0), 0);
    // New width must relayout, but card formatting is independent of width.
    assert_eq!(render(&dep, 600.0), 0);
    dep.rooms[0].member_count = Some(99);
    assert_eq!(render(&dep, 600.0), 1);
    dep.rooms[0].joining = true;
    assert_eq!(render(&dep, 600.0), 1);
    dep.page.open_menu_room_id = Some(dep.rooms[0].room_id);
    assert_eq!(render(&dep, 600.0), 1);
    dep.palette = crate::theme::BoruTheme::for_theme(&iced::Theme::Dark).into();
    assert_eq!(render(&dep, 600.0), 1);
    dep.theme_revision += 1;
    assert_eq!(render(&dep, 600.0), 1);
    dep.labels.title.push('!');
    assert_eq!(render(&dep, 600.0), 1);
    use boru_core::room_directory::{
        LocalJoinState, RoomAction, RoomCompatibility, RoomFeatureCompatibility,
    };
    let changes: &[fn(&mut DiscoverRoomRow)] = &[
        |r| r.room_name.push('!'),
        |r| r.short_description.push('!'),
        |r| r.tags.push("new tag".into()),
        |r| r.room_protocol_version = 255,
        |r| r.owner_peer_id = [99; 32],
        |r| r.local_join_state = LocalJoinState::Joined,
        |r| r.offered_action = RoomAction::Open,
        |r| r.compatibility = RoomCompatibility::Unsupported,
        |r| r.feature_compat = RoomFeatureCompatibility::SomeMissing(vec!["video".into()]),
        |r| r.conflict = !r.conflict,
        |r| r.last_seen_minutes = Some(99),
    ];
    for change in changes {
        change(&mut dep.rooms[0]);
        assert_eq!(render(&dep, 600.0), 1);
    }
    dep.rooms[0].last_seen = Some(Instant::now());
    render(&dep, 600.0);
    dep.rooms[0].last_seen = Some(Instant::now() + Duration::from_secs(1));
    assert_eq!(
        render(&dep, 600.0),
        0,
        "precise timestamps are not displayed"
    );
    let icon = visuals::ticket_icon_handle();
    for _ in 0..100 {
        assert_eq!(icon.id(), visuals::ticket_icon_handle().id());
    }
}

#[test]
#[ignore = "controlled snapshot/lock profile"]
fn discover_performance_snapshot_profile() {
    use crate::app::tests::build_prewarm_test_app;
    use boru_core::control_plane::advertisement::{AdvertisementAuth, PublicRoomAdvertisement};
    use boru_core::room_directory::RoomDirectory;
    let (_runtime, mut app) = build_prewarm_test_app();
    for count in [500usize, 1000] {
        let mut directory = RoomDirectory::new();
        let owner = app.endpoint.id();
        for i in 0..count {
            let mut bytes = [0u8; 32];
            bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
            let mut ad = PublicRoomAdvertisement::minimal(
                TopicId::from_bytes(bytes),
                format!("Room {i:04}"),
                *owner.as_bytes(),
            );
            ad.short_description = "Controlled description".into();
            directory.apply_advertisement_at(
                ad,
                owner,
                AdvertisementAuth::Verified { publisher: owner },
                1,
                1000,
                Instant::now(),
            );
        }
        app.room_directory = Some(Arc::new(StdMutex::new(directory)));
        assert_eq!(app.discover_dependency().total_count, count);
        for query in ["", "room", "missing"] {
            app.discover_search_query = query.into();
            let mut samples = Vec::new();
            for _ in 0..30 {
                let start = Instant::now();
                std::hint::black_box(fxhash_of(&app.discover_dependency()));
                samples.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            samples.sort_by(f64::total_cmp);
            println!(
                "DISCOVER_SNAPSHOT count={count} query={query:?} median_ms={:.3} max_ms={:.3}",
                samples[15], samples[29]
            );
        }
    }
}

#[test]
#[ignore = "controlled directory profile, not a wall-clock CI assertion"]
fn discover_performance_profile() {
    use std::time::Instant;
    for count in [500usize, 1000] {
        for mode in [DiscoverViewMode::Grid, DiscoverViewMode::List] {
            let mut dep = snapshot();
            dep.max_content_width_bits = 1200.0_f32.to_bits();
            dep.layout.max_columns = 3;
            dep.page.view_mode = mode;
            dep.rooms = (0..count)
                .map(|i| {
                    let mut room = card_tests::room();
                    room.room_id[..8].copy_from_slice(&(i as u64).to_le_bytes());
                    room.room_name = format!("Room {i:04}");
                    room.short_description =
                        "A controlled directory with searchable descriptions and bounded metadata."
                            .into();
                    room.tags = vec!["community".into(), "testing".into()];
                    room
                })
                .collect();
            dep.total_count = count;
            let mut renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
                Font::default(),
                Pixels(16.0),
            ));
            let mut element: iced::Element<'static, AppMessage> =
                iced::widget::lazy(dep.clone(), IcedChat::view_discover_content).into();
            let mut tree = Tree::new(element.as_widget());
            let mut width = 1200.0;
            let limits = |width| layout::Limits::new(Size::ZERO, Size::new(width, 720.0));
            let mut node = element
                .as_widget_mut()
                .layout(&mut tree, &renderer, &limits(width));
            for operation in [
                "ticket_typing",
                "search_typing",
                "scroll",
                "resize",
                "refresh",
            ] {
                let mut samples = Vec::new();
                for i in 0..12 {
                    let start = Instant::now();
                    match operation {
                        "ticket_typing" => dep.ticket_input.push('a'),
                        // All rooms still match: isolates needless unchanged-card work.
                        "search_typing" => {
                            dep.search_query = if i % 2 == 0 { "Room" } else { "room" }.into()
                        }
                        "resize" => {
                            width = if i % 2 == 0 { 1100.0 } else { 1200.0 };
                            dep.available_width_bits = width.to_bits();
                        }
                        "refresh" => dep.rooms[0].member_count = Some(i + 1),
                        _ => (),
                    }
                    if operation == "scroll" {
                        let mut messages = Vec::new();
                        let mut shell = iced::advanced::Shell::new(&mut messages);
                        element.as_widget_mut().update(
                            &mut tree,
                            &Event::Mouse(mouse::Event::WheelScrolled {
                                delta: mouse::ScrollDelta::Lines { x: 0.0, y: -3.0 },
                            }),
                            Layout::new(&node),
                            mouse::Cursor::Available(iced::Point::new(600.0, 600.0)),
                            &renderer,
                            &mut iced::advanced::clipboard::Null,
                            &mut shell,
                            &Rectangle::with_size(Size::new(width, 720.0)),
                        );
                    } else {
                        element =
                            iced::widget::lazy(dep.clone(), IcedChat::view_discover_content).into();
                        tree.diff(element.as_widget());
                        node = element
                            .as_widget_mut()
                            .layout(&mut tree, &renderer, &limits(width));
                    }
                    element.as_widget().draw(
                        &tree,
                        &mut renderer,
                        &iced::Theme::Light,
                        &renderer::Style::default(),
                        Layout::new(&node),
                        mouse::Cursor::Unavailable,
                        &Rectangle::with_size(Size::new(width, 720.0)),
                    );
                    iced::advanced::Renderer::reset(
                        &mut renderer,
                        Rectangle::with_size(Size::new(width, 720.0)),
                    );
                    samples.push(start.elapsed().as_secs_f64() * 1000.0);
                }
                samples.sort_by(f64::total_cmp);
                println!("DISCOVER_PROFILE count={count} mode={mode:?} op={operation} median_ms={:.3} max_ms={:.3}", samples[6], samples[11]);
            }
        }
    }
}
