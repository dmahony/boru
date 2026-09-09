//! Real pane layout, including resize of an existing widget tree.
use super::*;
use iced::advanced::{widget::Operation, Layout};
use iced::Rectangle;

fn directory() -> DiscoverDependency {
    let mut dep = snapshot();
    let config = crate::layout::LayoutConfig::default();
    let screen = &config.screens["discover"];
    dep.layout = screen.into();
    dep.max_content_width_bits = screen.max_content_width.to_bits();
    dep.layout.show_ticket = false;
    dep.layout.show_spotlight = false;
    dep.page.view_mode = DiscoverViewMode::Grid;
    // Deliberately stale estimates: only the actual pane limits may decide.
    dep.columns = 1;
    dep.available_width_bits = 100.0_f32.to_bits();
    dep.rooms = (1..=7)
        .map(|id| {
            let mut room = card_tests::room();
            room.room_id = [id; 32];
            room.room_name = format!("Room {id}");
            if id == 1 {
                room.short_description = "Long wrapped room description. ".repeat(20);
                room.tags = vec!["a-long-category-label".into(); 4];
            }
            room
        })
        .collect();
    dep.total_count = dep.rooms.len();
    dep
}

fn results(node: &layout::Node) -> &layout::Node {
    let canvas = &node.children()[0];
    let scroll = &canvas.children()[0];
    let outer = &scroll.children()[0];
    let capped = &outer.children()[0];
    let sections = &capped.children()[0];
    sections.children().last().unwrap()
}

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
fn discover_grid_resizes_at_default_boundaries_without_stale_window_estimates() {
    let dep = directory();
    let mut element = IcedChat::view_discover_content(&dep);
    let mut tree = Tree::new(element.as_widget());
    let renderer =
        iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)));
    // 32px padding + 14px reserved gutter; each extra card adds 280 + 8.
    for (width, columns) in [
        (180.0, 1usize),
        (613.0, 1),
        (614.0, 2),
        (735.0, 2),
        (736.0, 2),
        (737.0, 2),
        (901.0, 2),
        (902.0, 3),
        (1111.0, 3),
        (1112.0, 3),
        (1113.0, 3),
        (1600.0, 3),
        (614.0, 2),
        (180.0, 1),
    ] {
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(width, 240.0)),
        );
        assert_eq!(node.size(), Size::new(width, 240.0));
        let grid = results(&node);
        assert_eq!(
            grid.children().len(),
            dep.rooms.len().div_ceil(columns),
            "{width}"
        );
        let first = &grid.children()[0];
        let card_width = first.children()[0].size().width;
        if columns > 1 {
            assert!(card_width >= 280.0);
        }
        for row in grid.children() {
            assert_eq!(row.children().len(), columns);
            for slot in row.children() {
                assert!((slot.size().width - card_width).abs() < 0.1);
            }
        }
        let last = grid.children().last().unwrap();
        assert_eq!(
            last.children()[0].bounds().x,
            first.children()[0].bounds().x
        );
        for pair in grid.children().windows(2) {
            assert!(pair[0].bounds().y + pair[0].size().height <= pair[1].bounds().y);
        }
        assert_contained(grid);
    }
}

#[test]
fn discover_grid_honors_column_limit_and_list_uses_full_width_growing_cards() {
    let mut dep = directory();
    for configured in [1, 2, 3, 12] {
        dep.layout.max_columns = configured.clamp(1, 3);
        let node = measure(IcedChat::view_discover_content(&dep), 1600.0, 240.0);
        assert_eq!(
            results(&node).children()[0].children().len(),
            configured.min(3)
        );
    }
    dep.page.view_mode = DiscoverViewMode::List;
    for width in [180.0, 614.0, 902.0, 1600.0] {
        let node = measure(IcedChat::view_discover_content(&dep), width, 240.0);
        let list = results(&node);
        assert_eq!(list.children().len(), dep.rooms.len());
        for row in list.children() {
            assert_eq!(row.children().len(), 1);
            assert_eq!(row.children()[0].size().width, list.size().width);
        }
        assert!(list.children()[0].size().height > list.children()[1].size().height);
        assert_contained(list);
    }
}

#[test]
fn discover_custom_minimum_gap_and_partial_rows_keep_equal_slots() {
    let mut dep = directory();
    dep.layout.min_card_width_bits = 200.0_f32.to_bits();
    dep.layout.card_gap_bits = 20.0_f32.to_bits();
    dep.layout.padding_bits = 10.0_f32.to_bits();
    // Custom two/three-column thresholds include body padding and gutter.
    for (width, columns) in [(453.0, 1usize), (454.0, 2), (673.0, 2), (674.0, 3)] {
        let node = measure(IcedChat::view_discover_content(&dep), width, 240.0);
        assert_eq!(results(&node).children()[0].children().len(), columns);
    }
    for count in 1..=7 {
        let mut dep = dep.clone();
        dep.rooms.truncate(count);
        dep.total_count = count;
        let node = measure(IcedChat::view_discover_content(&dep), 674.0, 240.0);
        let grid = results(&node);
        assert_eq!(grid.children().len(), count.div_ceil(3));
        let mut rendered = 0;
        for row in grid.children() {
            assert_eq!(row.children().len(), 3);
            for slot in row.children() {
                assert!((slot.size().width - 200.0).abs() < 0.1);
                if slot.size().height > 0.0 {
                    rendered += 1;
                }
            }
        }
        assert_eq!(rendered, count);
    }
}

#[derive(Default)]
struct ScrollProbe {
    count: usize,
    offset: f32,
    set: bool,
    width: f32,
}
impl Operation for ScrollProbe {
    fn traverse(&mut self, visit: &mut dyn FnMut(&mut dyn Operation)) {
        visit(self);
    }
    fn scrollable(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        _: Rectangle,
        translation: iced::Vector,
        state: &mut dyn iced::advanced::widget::operation::scrollable::Scrollable,
    ) {
        assert_eq!(id, Some(&iced::advanced::widget::Id::new("discover-page")));
        assert_eq!(bounds.x, 0.0);
        assert_eq!(bounds.width, self.width);
        self.count += 1;
        self.offset = translation.y;
        if self.set {
            state.scroll_to(
                iced::advanced::widget::operation::scrollable::AbsoluteOffset {
                    x: None,
                    y: Some(80.0),
                },
            );
        }
    }
}

#[test]
fn discover_mode_switch_and_resize_keep_one_pane_edge_scroll_and_offset() {
    let mut dep = directory();
    dep.search_query = "retained query".into();
    dep.ticket_input = "retained ticket".into();
    dep.filter_compatible = true;
    dep.layout.show_ticket = true;
    let renderer =
        iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)));
    let mut element: iced::Element<'_, AppMessage> =
        iced::widget::lazy(dep.clone(), IcedChat::view_discover_content).into();
    let mut tree = Tree::new(element.as_widget());
    let limits = layout::Limits::new(Size::ZERO, Size::new(902.0, 240.0));
    let node = element
        .as_widget_mut()
        .layout(&mut tree, &renderer, &limits);
    let mut set = ScrollProbe {
        set: true,
        width: 902.0,
        ..Default::default()
    };
    element
        .as_widget_mut()
        .operate(&mut tree, Layout::new(&node), &renderer, &mut set);
    assert_eq!(set.count, 1);
    for (mode, width) in [
        (DiscoverViewMode::List, 902.0),
        (DiscoverViewMode::Grid, 614.0),
        (DiscoverViewMode::Grid, 180.0),
    ] {
        dep.page.view_mode = mode;
        let mut updated: iced::Element<'_, AppMessage> =
            iced::widget::lazy(dep.clone(), IcedChat::view_discover_content).into();
        tree.diff(updated.as_widget());
        let node = updated.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(width, 240.0)),
        );
        let mut probe = ScrollProbe {
            width,
            ..Default::default()
        };
        updated
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut probe);
        assert_eq!(probe.count, 1);
        assert_eq!(probe.offset, 80.0);
    }
}
