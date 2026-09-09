//! Shared card contract exercised with real native layout and keyboard events.
use super::*;
use iced::advanced::{mouse, widget::Operation, Layout, Shell};
use iced::{Event, Rectangle};

pub(super) fn room() -> DiscoverRoomRow {
    use boru_core::room_directory::*;
    DiscoverRoomRow {
        room_id: [7; 32],
        room_name: "Example room".into(),
        short_description: String::new(),
        tags: Vec::new(),
        room_protocol_version: 1,
        owner_peer_id: [0; 32],
        member_count: None,
        last_seen: None,
        last_seen_minutes: None,
        joining: false,
        compatibility: RoomCompatibility::Compatible,
        feature_compat: RoomFeatureCompatibility::None,
        local_join_state: LocalJoinState::NotJoined,
        offered_action: RoomAction::Join,
        conflict: false,
    }
}

struct FocusAt {
    target: usize,
    visited: usize,
}
impl Operation for FocusAt {
    fn traverse(&mut self, visit: &mut dyn FnMut(&mut dyn Operation)) {
        visit(self);
    }
    fn focusable(
        &mut self,
        _: Option<&iced::advanced::widget::Id>,
        _: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        if self.visited == self.target {
            state.focus();
        } else {
            state.unfocus();
        }
        self.visited += 1;
    }
}
fn activate(
    mut element: iced::Element<'static, AppMessage>,
    target: usize,
    named: iced::keyboard::key::Named,
) -> Vec<AppMessage> {
    use iced::keyboard::{key, Key, Location, Modifiers};
    let mut tree = Tree::new(element.as_widget());
    let renderer =
        iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)));
    let node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, Size::new(320.0, 2000.0)),
    );
    let mut focus = FocusAt { target, visited: 0 };
    element
        .as_widget_mut()
        .operate(&mut tree, Layout::new(&node), &renderer, &mut focus);
    assert!(focus.visited > target);
    let event = Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: Key::Named(named),
        modified_key: Key::Named(named),
        physical_key: key::Physical::Code(key::Code::Enter),
        location: Location::Standard,
        modifiers: Modifiers::default(),
        text: None,
        repeat: false,
    });
    let mut messages = Vec::new();
    element.as_widget_mut().update(
        &mut tree,
        &event,
        Layout::new(&node),
        mouse::Cursor::Unavailable,
        &renderer,
        &mut iced::advanced::clipboard::Null,
        &mut Shell::new(&mut messages),
        &Rectangle::with_size(Size::new(320.0, 2000.0)),
    );
    messages
}

#[test]
fn discover_card_helpers_preserve_truth_and_bounds() {
    assert_eq!(discover_member_count_text(None), None);
    assert_eq!(discover_member_count_text(Some(0)), None);
    assert_eq!(
        discover_member_count_text(Some(1)).as_deref(),
        Some("~1 member (approx.)")
    );
    assert_eq!(
        discover_member_count_text(Some(2)).as_deref(),
        Some("~2 members (approx.)")
    );
    assert_eq!(discover_last_seen_text(None), "Last seen unknown");
    assert_eq!(
        discover_last_seen_text(Some(0)),
        "Last seen less than a minute ago"
    );
    assert_eq!(discover_last_seen_text(Some(1)), "Last seen 1 minute ago");
    assert_eq!(discover_last_seen_text(Some(2)), "Last seen 2 minutes ago");
    assert_eq!(discover_room_name(" \n\u{202e} "), "Unnamed room");
    assert_eq!(discover_room_name("  Chat\u{202e}\n  "), "Chat");
    assert_eq!(
        discover_room_name(&"界".repeat(100)),
        format!("{}…", "界".repeat(64))
    );
}

#[test]
fn discover_card_keyboard_actions_match_all_presentations() {
    use boru_core::room_directory::RoomAction;
    use iced::keyboard::key::Named;
    for mode in [DiscoverViewMode::Grid, DiscoverViewMode::List] {
        for spotlight in [false, true] {
            let mut dep = snapshot();
            dep.page.view_mode = mode;
            dep.rooms.push(room());
            dep.page.spotlight_room_id = spotlight.then_some([7; 32]);
            let render = |dep: &DiscoverDependency| {
                if spotlight && dep.spotlight_room().is_some() {
                    IcedChat::discover_room_content(dep, dep.spotlight_room().unwrap())
                } else {
                    IcedChat::discover_rooms(dep)
                }
            };
            for key in [Named::Enter, Named::Space] {
                assert!(matches!(activate(render(&dep), 0, key).as_slice(),
                    [AppMessage::DirectoryRoomJoinById(id)] if *id == [7; 32]));
                assert!(matches!(activate(render(&dep), 1, key).as_slice(),
                    [AppMessage::DiscoverRoomMenuChanged(Some(id))] if *id == [7; 32]));
                dep.page.open_menu_room_id = Some([7; 32]);
                assert!(matches!(
                    activate(render(&dep), 1, key).as_slice(),
                    [AppMessage::DiscoverRoomMenuChanged(None)]
                ));
                assert!(matches!(activate(render(&dep), 2, key).as_slice(),
                    [AppMessage::DirectoryRoomHideById(id)] if *id == [7; 32]));
                dep.page.open_menu_room_id = None;
            }
            dep.rooms[0].offered_action = RoomAction::Open;
            assert!(matches!(activate(render(&dep), 0, Named::Enter).as_slice(),
                [AppMessage::OpenRoom(topic)] if topic.as_bytes() == &[7; 32]));
            dep.rooms[0].joining = true;
            // Pending rooms leave spotlight and remain once in normal results.
            assert!(dep.spotlight_room().is_none());
            // Disabled wrappers do not participate in traversal; More is first.
            assert!(matches!(
                activate(render(&dep), 0, Named::Enter).as_slice(),
                [AppMessage::DiscoverRoomMenuChanged(Some(_))]
            ));
        }
    }
}

fn contained(node: &layout::Node, width: f32) {
    assert!(
        node.size().width <= width + 0.1,
        "{} > {width}",
        node.size().width
    );
    for child in node.children() {
        contained(child, width);
    }
}

#[test]
fn discover_card_incompatible_keeps_hide_reachable_without_join() {
    use boru_core::room_directory::{RoomAction, RoomCompatibility};
    use iced::keyboard::key::Named;
    let mut dep = snapshot();
    let mut r = room();
    r.offered_action = RoomAction::Incompatible;
    for compatibility in [
        RoomCompatibility::UpgradeRequired,
        RoomCompatibility::Unsupported,
        RoomCompatibility::Unknown,
    ] {
        r.compatibility = compatibility;
        assert!(matches!(
            activate(IcedChat::discover_room_content(&dep, &r), 0, Named::Enter).as_slice(),
            [AppMessage::DiscoverRoomMenuChanged(Some(_))]
        ));
        dep.page.open_menu_room_id = Some(r.room_id);
        assert!(matches!(
            activate(IcedChat::discover_room_content(&dep, &r), 1, Named::Space).as_slice(),
            [AppMessage::DirectoryRoomHideById(_)]
        ));
        dep.page.open_menu_room_id = None;
    }
}

#[test]
fn discover_card_long_metadata_grows_without_horizontal_overflow() {
    let mut dep = snapshot();
    let mut r = room();
    r.room_name = "界".repeat(300);
    r.short_description = "W".repeat(500);
    r.tags = vec!["W".repeat(80); 8];
    r.member_count = Some(u32::MAX);
    r.conflict = true;
    r.feature_compat =
        boru_core::room_directory::RoomFeatureCompatibility::SomeMissing(vec!["W".repeat(500)]);
    dep.page.open_menu_room_id = Some(r.room_id);
    for width in [180.0, 240.0, 320.0, 720.0] {
        let node = measure(
            IcedChat::discover_room_content(&dep, &r),
            width,
            f32::INFINITY,
        );
        contained(&node, width);
        assert!(node.size().height.is_finite());
    }
    let small = measure(
        IcedChat::discover_room_content(&dep, &r),
        240.0,
        f32::INFINITY,
    );
    let wide = measure(
        IcedChat::discover_room_content(&dep, &r),
        720.0,
        f32::INFINITY,
    );
    assert!(small.size().height > wide.size().height);
    let minimal = measure(
        IcedChat::discover_room_content(&dep, &room()),
        240.0,
        f32::INFINITY,
    );
    assert!(minimal.size().height < small.size().height);
}
