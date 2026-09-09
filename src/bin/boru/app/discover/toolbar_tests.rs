//! Real widget events and tree diffs, without a display or network.
use super::*;
use iced::advanced::{mouse, widget::Operation, Layout, Shell};
use iced::{Event, Rectangle};

struct FocusAt {
    target: usize,
    visited: usize,
    bounds: Option<Rectangle>,
}
impl Operation for FocusAt {
    fn traverse(&mut self, visit: &mut dyn FnMut(&mut dyn Operation)) {
        visit(self);
    }
    fn focusable(
        &mut self,
        _: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        if self.visited == self.target {
            state.focus();
            self.bounds = Some(bounds);
        } else {
            state.unfocus();
        }
        self.visited += 1;
    }
}

fn renderer() -> iced::Renderer {
    iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)))
}
fn key(named: iced::keyboard::key::Named) -> Event {
    use iced::keyboard::{key, Key, Location, Modifiers};
    Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: Key::Named(named),
        modified_key: Key::Named(named),
        physical_key: key::Physical::Code(key::Code::Enter),
        location: Location::Standard,
        modifiers: Modifiers::default(),
        text: None,
        repeat: false,
    })
}
fn activate(
    dep: &DiscoverDependency,
    target: usize,
    named: iced::keyboard::key::Named,
) -> Vec<AppMessage> {
    let mut element = IcedChat::discover_controls(dep);
    let mut tree = Tree::new(element.as_widget());
    let renderer = renderer();
    let node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, Size::new(900.0, 900.0)),
    );
    let mut focus = FocusAt {
        target,
        visited: 0,
        bounds: None,
    };
    element
        .as_widget_mut()
        .operate(&mut tree, Layout::new(&node), &renderer, &mut focus);
    assert!(
        focus.bounds.is_some(),
        "control {target} must participate in Tab traversal"
    );
    let mut messages = Vec::new();
    element.as_widget_mut().update(
        &mut tree,
        &key(named),
        Layout::new(&node),
        mouse::Cursor::Unavailable,
        &renderer,
        &mut iced::advanced::clipboard::Null,
        &mut Shell::new(&mut messages),
        &Rectangle::with_size(Size::new(900.0, 900.0)),
    );
    messages
}

#[test]
fn discover_toolbar_keyboard_reaches_every_action() {
    use iced::keyboard::key::Named;
    let mut dep = snapshot();
    dep.search_query = "query".into();
    // Input, clear, All, Not joined, Joined, Compatible, Recent, sort, Grid, List.
    assert!(
        matches!(activate(&dep, 1, Named::Enter).as_slice(), [AppMessage::DiscoverSearchChanged(q)] if q.is_empty())
    );
    for (index, expected) in [
        (2, DiscoverFilter::All),
        (3, DiscoverFilter::NotJoined),
        (4, DiscoverFilter::Joined),
        (5, DiscoverFilter::Compatible),
        (6, DiscoverFilter::RecentlySeen),
    ] {
        assert!(
            matches!(activate(&dep, index, Named::Space).as_slice(), [AppMessage::DiscoverFilterToggled(f)] if *f == expected)
        );
    }
    assert!(matches!(
        activate(&dep, 7, Named::ArrowDown).as_slice(),
        [AppMessage::DiscoverSortChanged(DiscoverSort::Compatibility)]
    ));
    assert!(matches!(
        activate(&dep, 7, Named::ArrowUp).as_slice(),
        [AppMessage::DiscoverSortChanged(DiscoverSort::Name)]
    ));
    assert!(matches!(
        activate(&dep, 8, Named::Enter).as_slice(),
        [AppMessage::DiscoverViewModeChanged(DiscoverViewMode::Grid)]
    ));
    assert!(matches!(
        activate(&dep, 9, Named::Space).as_slice(),
        [AppMessage::DiscoverViewModeChanged(DiscoverViewMode::List)]
    ));
    dep.available_tags = (0..14).map(|n| format!("tag-{n:02}")).collect();
    dep.selected_tags = vec![dep.available_tags[13].clone()];
    assert!(
        matches!(activate(&dep, 19, Named::Enter).as_slice(), [AppMessage::DiscoverTagToggled(t)] if t == "tag-13")
    );
    assert!(matches!(
        activate(&dep, 20, Named::Enter).as_slice(),
        [AppMessage::DiscoverFilterToggled(
            DiscoverFilter::TagsExpanded
        )]
    ));
}

#[test]
fn discover_sort_pointer_opens_real_overlay() {
    let dep = snapshot();
    let mut element = IcedChat::discover_controls(&dep);
    let mut tree = Tree::new(element.as_widget());
    let renderer = renderer();
    let node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, Size::new(900.0, 900.0)),
    );
    // Empty search disables clear, so sort is focus index 6.
    let mut focus = FocusAt {
        target: 6,
        visited: 0,
        bounds: None,
    };
    element
        .as_widget_mut()
        .operate(&mut tree, Layout::new(&node), &renderer, &mut focus);
    let cursor = mouse::Cursor::Available(focus.bounds.unwrap().center());
    let viewport = Rectangle::with_size(Size::new(900.0, 900.0));
    let mut messages = Vec::new();
    element.as_widget_mut().update(
        &mut tree,
        &Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
        Layout::new(&node),
        cursor,
        &renderer,
        &mut iced::advanced::clipboard::Null,
        &mut Shell::new(&mut messages),
        &viewport,
    );
    assert!(element
        .as_widget_mut()
        .overlay(
            &mut tree,
            Layout::new(&node),
            &renderer,
            &viewport,
            iced::Vector::ZERO
        )
        .is_some());
}

#[test]
fn discover_toolbar_wraps_and_selected_tags_survive_collapse() {
    let mut dep = snapshot();
    dep.available_tags = (0..18).map(|n| format!("category-{n:02}")).collect();
    dep.selected_tags = vec![dep.available_tags[17].clone()];
    let wide = measure(IcedChat::discover_controls(&dep), 900.0, f32::INFINITY);
    let narrow = measure(IcedChat::discover_controls(&dep), 240.0, f32::INFINITY);
    assert!(narrow.children()[1].size().height > wide.children()[1].size().height);
    assert!(narrow.children()[2].size().height > wide.children()[2].size().height);
    // Twelve initial tags, one selected late tag and expansion action.
    assert_eq!(narrow.children()[2].children().len(), 14);
    dep.page.tags_expanded = true;
    let expanded = measure(IcedChat::discover_controls(&dep), 240.0, f32::INFINITY);
    assert_eq!(expanded.children()[2].children().len(), 19);
}

#[test]
fn discover_search_caret_survives_passive_snapshot_diff() {
    type InputState = iced::widget::text_input::State<
        <iced::Renderer as iced::advanced::text::Renderer>::Paragraph,
    >;
    let mut dep = snapshot();
    dep.search_query = "unchanged local query".into();
    let element = IcedChat::discover_controls(&dep);
    let mut tree = Tree::new(element.as_widget());
    let state = tree.children[0].children[1]
        .state
        .downcast_mut::<InputState>();
    state.focus();
    state.move_cursor_to(4);
    let cursor = state.cursor();
    dep.total_count = 20;
    dep.available_tags = vec!["new passive tag".into()];
    dep.theme_revision += 1;
    let updated = IcedChat::discover_controls(&dep);
    tree.diff(updated.as_widget());
    let state = tree.children[0].children[1]
        .state
        .downcast_ref::<InputState>();
    assert!(state.is_focused());
    assert_eq!(state.cursor(), cursor);
}

#[test]
fn discover_page_scroll_survives_passive_lazy_diff() {
    use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, Scrollable};
    #[derive(Default)]
    struct ScrollProbe {
        set: bool,
        offset: f32,
        count: usize,
    }
    impl Operation for ScrollProbe {
        fn traverse(&mut self, visit: &mut dyn FnMut(&mut dyn Operation)) {
            visit(self);
        }
        fn scrollable(
            &mut self,
            id: Option<&iced::advanced::widget::Id>,
            _: Rectangle,
            _: Rectangle,
            translation: iced::Vector,
            state: &mut dyn Scrollable,
        ) {
            assert_eq!(id, Some(&iced::advanced::widget::Id::new("discover-page")));
            self.count += 1;
            self.offset = translation.y;
            if self.set {
                state.scroll_to(AbsoluteOffset {
                    x: None,
                    y: Some(80.0),
                });
            }
        }
    }
    let renderer = renderer();
    let mut dep = snapshot();
    dep.labels.subtitle = "A long local directory description. ".repeat(25);
    let mut element: iced::Element<'_, AppMessage> =
        iced::widget::lazy(dep.clone(), IcedChat::view_discover_content).into();
    let mut tree = Tree::new(element.as_widget());
    let limits = layout::Limits::new(Size::ZERO, Size::new(500.0, 200.0));
    let node = element
        .as_widget_mut()
        .layout(&mut tree, &renderer, &limits);
    element.as_widget_mut().operate(
        &mut tree,
        Layout::new(&node),
        &renderer,
        &mut ScrollProbe {
            set: true,
            ..Default::default()
        },
    );
    let mut before = ScrollProbe::default();
    element
        .as_widget_mut()
        .operate(&mut tree, Layout::new(&node), &renderer, &mut before);
    assert_eq!(before.count, 1);
    assert_eq!(before.offset, 80.0);
    dep.total_count = 2;
    let mut updated: iced::Element<'_, AppMessage> =
        iced::widget::lazy(dep, IcedChat::view_discover_content).into();
    tree.diff(updated.as_widget());
    let node = updated
        .as_widget_mut()
        .layout(&mut tree, &renderer, &limits);
    let mut after = ScrollProbe::default();
    updated
        .as_widget_mut()
        .operate(&mut tree, Layout::new(&node), &renderer, &mut after);
    assert_eq!(after.count, 1);
    assert_eq!(after.offset, before.offset);
}
