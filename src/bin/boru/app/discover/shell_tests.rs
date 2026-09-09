//! Native layout regression tests; no display, network, or fake production rooms.
use super::*;
use iced::advanced::{layout, widget::Tree};
use iced::{Font, Pixels, Size};

#[path = "toolbar_tests.rs"]
mod toolbar_tests;
#[path = "ticket_tests.rs"]
mod ticket_tests;
#[path = "card_tests.rs"]
mod card_tests;
#[path = "responsive_tests.rs"]
mod responsive_tests;

fn snapshot() -> DiscoverDependency {
    DiscoverDependency {
        dark_mode: false,
        theme_revision: 0,
        layout_revision: 0,
        responsive_mode: crate::layout::ViewportTier::Narrow,
        max_content_width_bits: 720.0_f32.to_bits(),
        available_width_bits: 720.0_f32.to_bits(),
        columns: 1,
        rooms: Vec::new(),
        page: DiscoverPageState::default(),
        layout: (&crate::layout::ScreenLayout::default()).into(),
        palette: crate::theme::BoruTheme::default().into(),
        labels: DiscoverLabels::default(),
        search_query: String::new(),
        filter_compatible: false,
        filter_recently_seen: false,
        selected_tags: Vec::new(),
        available_tags: Vec::new(),
        sort: DiscoverSort::RecentlySeen,
        total_count: 0,
        ticket_input: String::new(),
        ticket_error: String::new(),
        ticket_pending: false,
        ticket_blocked: false,
    }
}

fn measure(
    mut element: iced::Element<'static, AppMessage>,
    width: f32,
    height: f32,
) -> layout::Node {
    let mut tree = Tree::new(element.as_widget());
    let renderer =
        iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)));
    element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, Size::new(width, height)),
    )
}

#[test]
fn discover_shell_fills_canvas_and_caps_only_inner_content() {
    for width in [240.0, 320.0, 720.0, 1280.0, 1920.0] {
        let dep = snapshot();
        let node = measure(IcedChat::view_discover_content(&dep), width, 300.0);
        assert_eq!(node.size(), Size::new(width, 300.0));
        let canvas = &node.children()[0]; // bounded responsive wrapper
        let scroll = &canvas.children()[0];
        assert_eq!(scroll.size(), Size::new(width, 300.0));
        let outer = &scroll.children()[0];
        let capped = &outer.children()[0];
        assert!(capped.size().width <= 720.0);
        assert!(capped.size().width <= width);
        assert!(capped.size().width >= width.min(720.0) - 14.0);
        // Header and controls grow in scroll content, never a fixed-height strip.
        let sections = &capped.children()[0];
        assert!(sections.size().height > 300.0);
        let header = &sections.children()[0];
        let controls = &sections.children()[1];
        assert!(header.bounds().y + header.size().height <= controls.bounds().y);
    }
}

#[test]
fn discover_header_wraps_full_copy_without_elision() {
    let mut dep = snapshot();
    dep.labels.title = "Public Rooms — conversations from your local directory".into();
    dep.labels.subtitle =
        "Rooms appear here when they are discovered on the Boru network. ".repeat(4);
    let wide = measure(IcedChat::discover_header(&dep), 1000.0, f32::INFINITY);
    let narrow = measure(IcedChat::discover_header(&dep), 180.0, f32::INFINITY);
    assert!(narrow.size().height > wide.size().height);
    assert!(narrow.children()[1].size().height > wide.children()[1].size().height);
    assert!(narrow.children()[2].size().height > wide.children()[2].size().height);
    assert_eq!(narrow.children()[0].children().len(), 2); // Back and Refresh
    for child in narrow.children() {
        assert!(child.size().width <= 180.0);
    }
}

#[test]
fn discover_search_remains_one_full_width_row_without_optional_controls() {
    let mut dep = snapshot();
    dep.search_query = "local query".into();
    for visible in [true, false] {
        dep.layout.show_controls = visible;
        let node = measure(IcedChat::discover_controls(&dep), 280.0, f32::INFINITY);
        let search = &node.children()[0];
        assert_eq!(search.size().width, 280.0);
        assert_eq!(search.children().len(), 3); // Icon, input and stable clear slot
        if !visible {
            assert_eq!(node.children().len(), 1);
        }
    }
}
