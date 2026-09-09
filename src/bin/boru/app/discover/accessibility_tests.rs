//! Whole-page native keyboard traversal and logical HiDPI layout checks.
use super::*;
use iced::advanced::{
    widget::{operation, Operation},
    Layout,
};
use iced::keyboard::key::Named;

#[test]
fn discover_accessibility_search_ime_commit_survives_passive_lazy_diff() {
    use iced::advanced::{mouse, Shell};
    let mut dep = snapshot();
    dep.search_query = "before after".into();
    let renderer =
        iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)));
    let limits = layout::Limits::new(Size::ZERO, Size::new(600.0, 800.0));
    let mut element: iced::Element<'static, AppMessage> =
        iced::widget::lazy(dep.clone(), IcedChat::view_discover_content).into();
    let mut tree = Tree::new(element.as_widget());
    let node = element
        .as_widget_mut()
        .layout(&mut tree, &renderer, &limits);
    run_operation(
        &mut element,
        &mut tree,
        &renderer,
        &node,
        Box::new(operation::focusable::focus("discover-search".into())),
    );
    run_operation(
        &mut element,
        &mut tree,
        &renderer,
        &node,
        Box::new(operation::text_input::move_cursor_to(
            "discover-search".into(),
            7,
        )),
    );
    let send = |element: &mut iced::Element<'static, AppMessage>,
                tree: &mut Tree,
                node: &layout::Node,
                event| {
        let mut messages = Vec::new();
        element.as_widget_mut().update(
            tree,
            &event,
            Layout::new(node),
            mouse::Cursor::Unavailable,
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut Shell::new(&mut messages),
            &iced::Rectangle::with_size(Size::new(600.0, 800.0)),
        );
        messages
    };
    assert!(send(
        &mut element,
        &mut tree,
        &node,
        iced::Event::InputMethod(iced::advanced::input_method::Event::Preedit(
            "日本".into(),
            None
        ))
    )
    .is_empty());
    dep.total_count = 4;
    dep.available_tags = vec!["passive".into()];
    let mut updated: iced::Element<'static, AppMessage> =
        iced::widget::lazy(dep, IcedChat::view_discover_content).into();
    tree.diff(updated.as_widget());
    let node = updated
        .as_widget_mut()
        .layout(&mut tree, &renderer, &limits);
    let messages = send(
        &mut updated,
        &mut tree,
        &node,
        iced::Event::InputMethod(iced::advanced::input_method::Event::Commit("日本".into())),
    );
    assert!(
        matches!(messages.as_slice(), [AppMessage::DiscoverSearchChanged(q)] if q == "before 日本after")
    );
}

fn run_operation(
    element: &mut iced::Element<'static, AppMessage>,
    tree: &mut Tree,
    renderer: &iced::Renderer,
    node: &layout::Node,
    mut op: Box<dyn Operation>,
) {
    loop {
        element
            .as_widget_mut()
            .operate(tree, Layout::new(node), renderer, op.as_mut());
        match op.finish() {
            operation::Outcome::Chain(next) => op = next,
            _ => break,
        }
    }
}

#[test]
fn discover_accessibility_full_page_action_order() {
    let mut dep = snapshot();
    dep.layout.show_ticket = true;
    dep.layout.show_controls = true;
    dep.layout.show_spotlight = false;
    dep.ticket_input = "draft".into();
    dep.rooms = vec![card_tests::room()];
    dep.total_count = 1;
    // Back, Refresh, search, five filters, sort, Grid, List, ticket, Join, room Join, More.
    for key in [Named::Enter, Named::Space] {
        let activate =
            |index| card_tests::activate(IcedChat::view_discover_content(&dep), index, key);
        assert!(matches!(
            activate(0).as_slice(),
            [AppMessage::CloseDiscover]
        ));
        assert!(matches!(
            activate(1).as_slice(),
            [AppMessage::RefreshRoomRegistry]
        ));
        assert!(matches!(
            activate(3).as_slice(),
            [AppMessage::DiscoverFilterToggled(DiscoverFilter::All)]
        ));
        assert!(matches!(
            activate(8).as_slice(),
            [AppMessage::DiscoverSortChanged(_)]
        ));
        assert!(matches!(
            activate(9).as_slice(),
            [AppMessage::DiscoverViewModeChanged(DiscoverViewMode::Grid)]
        ));
        assert!(matches!(
            activate(10).as_slice(),
            [AppMessage::DiscoverViewModeChanged(DiscoverViewMode::List)]
        ));
        assert!(matches!(
            activate(12).as_slice(),
            [AppMessage::DiscoverJoinFromTicket]
        ));
        assert!(matches!(
            activate(13).as_slice(),
            [AppMessage::DirectoryRoomJoinById(_)]
        ));
        assert!(matches!(
            activate(14).as_slice(),
            [AppMessage::DiscoverRoomMenuChanged(Some(_))]
        ));
    }
}

#[test]
fn discover_accessibility_tab_reveals_controls_at_all_scales() {
    let mut dep = snapshot();
    dep.layout.show_ticket = true;
    dep.ticket_input = "draft".into();
    dep.layout.show_spotlight = false;
    dep.rooms = vec![card_tests::room(); 8];
    let renderer =
        iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)));
    for scale in [1.0, 1.25, 1.5, 2.0] {
        // Iced lays out in logical pixels: fixed physical pane / device scale.
        let size = Size::new(800.0 / scale, 600.0 / scale);
        let mut element = IcedChat::view_discover_content(&dep);
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        for reverse in [false, true] {
            for _ in 0..32 {
                let op: Box<dyn Operation> = if reverse {
                    Box::new(operation::focusable::focus_previous())
                } else {
                    Box::new(operation::focusable::focus_next())
                };
                run_operation(&mut element, &mut tree, &renderer, &node, op);
                run_operation(
                    &mut element,
                    &mut tree,
                    &renderer,
                    &node,
                    Box::new(keyboard::RevealFocus::default()),
                );
                let mut probe = keyboard::RevealFocus::default();
                element.as_widget_mut().operate(
                    &mut tree,
                    Layout::new(&node),
                    &renderer,
                    &mut probe,
                );
                if let Some(focused) = probe.focused {
                    let viewport = probe.viewport.unwrap();
                    let y = focused.y - probe.translation.y;
                    assert!(y >= viewport.y - 0.1, "scale {scale} reverse {reverse}: {focused:?} viewport {viewport:?} offset {:?}", probe.translation);
                    assert!(y + focused.height <= viewport.y + viewport.height + 0.1);
                }
            }
        }
    }
}

#[test]
fn discover_accessibility_escape_closes_menu_before_leaving() {
    let (_runtime, mut app) = crate::app::tests::build_prewarm_test_app();
    app.screen = Screen::Discover;
    app.discover_page.open_menu_room_id = Some([7; 32]);
    app.discover_search_query = "preserve query".into();
    drop(app.update(AppMessage::Shortcut(Shortcut::Escape)));
    assert!(matches!(app.screen, Screen::Discover));
    assert!(app.discover_page.open_menu_room_id.is_none());
    assert_eq!(app.discover_search_query, "preserve query");
    drop(app.update(AppMessage::Shortcut(Shortcut::Escape)));
    assert!(matches!(app.screen, Screen::ChatList));
}

#[test]
fn discover_accessibility_focus_draws_black_white_inset_outlines() {
    use iced::advanced::{mouse, renderer};
    for theme in [iced::Theme::Light, iced::Theme::Dark] {
        let mut dep = snapshot();
        dep.palette = crate::theme::BoruTheme::for_theme(&theme).into();
        dep.palette.primary = iced::Color::from_rgb(0.95, 0.95, 0.8).into();
        let mut element = IcedChat::discover_header(&dep);
        let mut tree = Tree::new(element.as_widget());
        let mut renderer =
            iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)));
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(800.0, 600.0)),
        );
        run_operation(
            &mut element,
            &mut tree,
            &renderer,
            &node,
            Box::new(operation::focusable::focus_next()),
        );
        element.as_widget().draw(
            &tree,
            &mut renderer,
            &theme,
            &renderer::Style::default(),
            Layout::new(&node),
            mouse::Cursor::Unavailable,
            &iced::Rectangle::with_size(Size::new(800.0, 600.0)),
        );
        let iced::Renderer::Secondary(renderer) = &mut renderer else {
            unreachable!()
        };
        let rings: Vec<_> = renderer
            .layers()
            .iter()
            .flat_map(|l| &l.quads)
            .filter(|(q, bg)| {
                q.border.width == 2.0 && *bg == iced::Background::Color(iced::Color::TRANSPARENT)
            })
            .collect();
        assert_eq!(rings.len(), 2);
        assert_eq!(rings[0].0.border.color, iced::Color::BLACK);
        assert_eq!(rings[1].0.border.color, iced::Color::WHITE);
        assert_eq!(rings[1].0.bounds.x, rings[0].0.bounds.x + 2.0);
        assert_eq!(rings[1].0.bounds.width, rings[0].0.bounds.width - 4.0);
    }
}

#[test]
fn discover_accessibility_scaled_grid_list_contain_long_copy() {
    fn contained(node: &layout::Node) {
        for child in node.children() {
            assert!(child.bounds().x >= -0.1);
            assert!(
                child.bounds().x + child.size().width <= node.size().width + 0.1,
                "child {:?} exceeds {:?}",
                child.bounds(),
                node.size()
            );
            contained(child);
        }
    }
    let mut dep = snapshot();
    dep.layout.show_ticket = true;
    dep.labels.subtitle = "Long directory description with accents éèà. ".repeat(10);
    dep.ticket_error = "A long error message which must wrap. ".repeat(8);
    dep.rooms = vec![card_tests::room(); 4];
    dep.rooms[0].short_description = "Long room description. ".repeat(20);
    for scale in [1.0, 1.25, 1.5, 2.0] {
        for mode in [DiscoverViewMode::Grid, DiscoverViewMode::List] {
            dep.page.view_mode = mode;
            let node = measure(
                IcedChat::view_discover_content(&dep),
                800.0 / scale,
                600.0 / scale,
            );
            contained(&node);
        }
    }
}
