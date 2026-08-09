//! Offscreen render capture for the redesigned connection status card
//! (test-only harness, never built in production).
//!
//! Renders the real `status_card::view_status_card` widget with the
//! tiny-skia headless renderer (no GPU, no display, no network) and saves
//! PNGs to `./captures/` so the card can be visually inspected at the
//! evidence widths and connection states:
//!
//! - wide desktop (1600 window → ~1215 content) — full three-region row
//! - minimum supported window (1024 → ~679 content) — medium row
//! - narrow (below supported width) — stacked layout
//! - Ready / Connecting / Offline variants
//!
//! Run with:
//! ```text
//! rb test --example boru --features gui,video-playback,terminal -- capture_status_card --nocapture
//! rsync -az debsrv:~/boru-build/work-<slot>/captures/ ./captures/
//! ```

use iced::advanced::layout;
use iced::advanced::mouse::Cursor;
use iced::advanced::renderer::Headless;
use iced::advanced::widget::{Tree, Widget};
use iced::{Font, Pixels, Rectangle, Size};
use std::borrow::Cow;

use crate::app::{AppMessage, HomeConnectionVariant};
use crate::status_card::{view_status_card, StatusCardDependency};

const CAPTURE_DIR: &str = "captures";

/// Register one bundled font with iced's global font system (required
/// before any text can be laid out headlessly).
fn load_font(bytes: &'static [u8]) {
    use iced::advanced::graphics::text::font_system;
    font_system()
        .write()
        .unwrap()
        .load_font(Cow::Borrowed(bytes));
}

/// Realistic dependency snapshot for the given variant and content width
/// (the same live selectors app.rs feeds the card).
fn dep(variant: HomeConnectionVariant, width: f32) -> StatusCardDependency {
    let headline = match variant {
        HomeConnectionVariant::Starting => "Starting Boru \u{280B}".to_string(),
        HomeConnectionVariant::Connecting => {
            "Connecting \u{2014} waiting for peers\u{2026}".to_string()
        }
        HomeConnectionVariant::Ready => "Boru is connected and ready.".to_string(),
        HomeConnectionVariant::Degraded => "Mesh degraded \u{2014} No peers in the mesh".to_string(),
        HomeConnectionVariant::Offline => {
            "Boru is offline \u{2014} relay unreachable".to_string()
        }
    };
    StatusCardDependency {
        variant,
        content_width: width,
        headline,
        show_retry: matches!(variant, HomeConnectionVariant::Offline),
        show_details: matches!(
            variant,
            HomeConnectionVariant::Offline | HomeConnectionVariant::Degraded
        ),
        pulse_frame: 2,
        animate_mesh: matches!(variant, HomeConnectionVariant::Ready),
        dimmed_mesh: !matches!(variant, HomeConnectionVariant::Ready),
        home_menu_opacity: 1.0,
    }
}

/// Lay the card out at the given canvas size, draw it with tiny-skia, and
/// save a PNG.
fn render_card(dep: &StatusCardDependency, w: f32, h: f32, name: &str) {
    let mut renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
        Font::default(),
        Pixels(16.0),
    ));
    let mut element: iced::Element<'_, AppMessage> = view_status_card(dep);
    let mut tree = Tree::new(element.as_widget());
    let limits = layout::Limits::new(Size::ZERO, Size::new(w, h));
    let node = element.as_widget_mut().layout(&mut tree, &renderer, &limits);
    // CONN-04: report the card's REAL laid-out size (padding + content,
    // unaffected by the drop shadow) so the 200-230px band and the
    // no-horizontal-overflow criterion are verifiable directly from the
    // layout tree (CONN-12 sweep).
    println!(
        "layout size for {name}: {:.1}x{:.1}px (canvas {w}x{h})",
        node.bounds().width,
        node.bounds().height
    );
    let theme = iced::Theme::Light;
    let viewport = Rectangle::with_size(Size::new(w, h));
    element.as_widget().draw(
        &tree,
        &mut renderer,
        &theme,
        &iced::advanced::renderer::Style::default(),
        iced::advanced::Layout::new(&node),
        Cursor::default(),
        &viewport,
    );
    let rgba = renderer.screenshot(
        Size::new(w as u32, h as u32),
        1.0,
        iced::Color::from_rgb(0.9686, 0.9765, 0.9725), // light canvas #F7F9F8
    );
    std::fs::create_dir_all(CAPTURE_DIR).unwrap();
    let path = format!("{CAPTURE_DIR}/{name}.png");
    image::save_buffer_with_format(
        &path,
        &rgba,
        w as u32,
        h as u32,
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .unwrap();
    println!("captured {path} ({w} x {h})");
}

/// Lay the card out at the given content width and return its REAL
/// laid-out height (padding + content). The drop shadow is rendered, not
/// laid out, so this is the authoritative measure for the CONN-04 band.
fn measure_card_height(dep: &StatusCardDependency, w: f32) -> f32 {
    let renderer =
        iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)));
    let mut element: iced::Element<'_, AppMessage> = view_status_card(dep);
    let mut tree = Tree::new(element.as_widget());
    let limits = layout::Limits::new(Size::ZERO, Size::new(w, 320.0));
    let node = element.as_widget_mut().layout(&mut tree, &renderer, &limits);
    node.bounds().height
}

/// Lay the `Secure • Decentralized • Private` pill out at the given
/// available width and return its REAL laid-out (width, height). Used by
/// the CONN-07 nowrap regression test: with `Wrapping::None` the pill must
/// hug its content at wide widths and must NEVER grow taller (i.e. wrap
/// into a vertical column) when the available width shrinks.
fn measure_pill(available_width: f32) -> (f32, f32) {
    let renderer =
        iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)));
    let mut element: iced::Element<'_, AppMessage> = crate::status_card::security_pill();
    let mut tree = Tree::new(element.as_widget());
    let limits = layout::Limits::new(Size::ZERO, Size::new(available_width, 100.0));
    let node = element.as_widget_mut().layout(&mut tree, &renderer, &limits);
    let b = node.bounds();
    (b.width, b.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conn11_icon_heading_row_and_text_block_align() {
        // CONN-11 acceptance (spec §15): in the horizontal modes the
        // check indicator belongs to the HEADING ROW — its vertical
        // centre aligns with the heading's vertical centre — and the
        // divider / description / pill share the HEADING's left edge.
        // The text block is the visual anchor; the icon sits off to its
        // left. This walks the real layout tree of the Ready card and
        // asserts those shared alignment lines from the layout engine
        // (ground truth, not pixel estimates).
        load_font(include_bytes!("fonts/ArchivoSemiCondensed-Bold.ttf"));
        load_font(include_bytes!("fonts/IBMPlexSans-Regular.ttf"));
        load_font(include_bytes!("fonts/IBMPlexSans-Medium.ttf"));
        load_font(include_bytes!("fonts/IBMPlexSans-SemiBold.ttf"));

        for (width, label) in [(1215.0, "Full"), (679.0, "Medium")] {
            let dep = dep(HomeConnectionVariant::Ready, width);
            let renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
                Font::default(),
                Pixels(16.0),
            ));
            let mut element: iced::Element<'_, AppMessage> = view_status_card(&dep);
            let mut tree = Tree::new(element.as_widget());
            let limits = layout::Limits::new(Size::ZERO, Size::new(width, 320.0));
            let node = element.as_widget_mut().layout(&mut tree, &renderer, &limits);

            // Card container -> row -> info column.
            let row = &node.children()[0];
            let info = &row.children()[0];
            // Info column children: [header_row, content_row].
            let header_row = &info.children()[0];
            let content_row = &info.children()[1];

            // Header row: [icon][gap][heading]. Icon and heading centres
            // must coincide vertically (the icon/heading row). The icon
            // and heading are siblings in the header row, so their bounds
            // share the same coordinate space — compare centres directly.
            let icon = &header_row.children()[0];
            let heading = &header_row.children()[2];
            let icon_c = icon.bounds().center_y();
            let heading_c = heading.bounds().center_y();
            assert!(
                (icon_c - heading_c).abs() < 1.0,
                "{label}: icon centre {icon_c:.1}px must align with heading centre {heading_c:.1}px \
                 (CONN-11 spec §15 icon/heading row)"
            );

            // Node bounds are parent-relative, so reconstruct the
            // heading's absolute left edge by summing the ancestor row
            // offsets, and do the same for the content elements.
            let header_x = header_row.bounds().x;
            let heading_left = header_x + heading.bounds().x;

            // Content row: [indent spacer][content column]; the content
            // column children are [divider, gap, description, gap,
            // footer/pill]. All must share the heading's left edge.
            let content_row_x = content_row.bounds().x;
            let content_col = &content_row.children()[1];
            let content_col_x = content_row_x + content_col.bounds().x;
            let divider = &content_col.children()[0];
            let description = &content_col.children()[2];
            let footer = &content_col.children()[4];
            for (name, el) in [
                ("divider", divider),
                ("description", description),
                ("footer", footer),
            ] {
                let x = content_col_x + el.bounds().x;
                assert!(
                    (x - heading_left).abs() < 1.0,
                    "{label}: {name} left edge {x:.1}px must share the heading's left edge \
                     {heading_left:.1}px (text block is the visual anchor)"
                );
            }
            // The icon must sit to the LEFT of the text block, not inside it.
            let icon_x = header_x + icon.bounds().x;
            assert!(
                icon_x < heading_left,
                "{label}: the icon must sit left of the heading"
            );
        }
    }

    #[test]
    fn capture_mesh_isolated_on_white() {
        let mut renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
            Font::default(),
            Pixels(16.0),
        ));
        let mut element: iced::Element<'_, AppMessage> =
            crate::status_card::network_mesh_for_debug(2, true, false);
        let mut tree = Tree::new(element.as_widget());
        let limits = layout::Limits::new(Size::ZERO, Size::new(200.0, 136.0));
        let node = element.as_widget_mut().layout(&mut tree, &renderer, &limits);
        let theme = iced::Theme::Light;
        let viewport = Rectangle::with_size(Size::new(200.0, 136.0));
        element.as_widget().draw(
            &tree,
            &mut renderer,
            &theme,
            &iced::advanced::renderer::Style::default(),
            iced::advanced::Layout::new(&node),
            Cursor::default(),
            &viewport,
        );
        let rgba = renderer.screenshot(
            Size::new(200, 136),
            1.0,
            iced::Color::WHITE,
        );
        std::fs::create_dir_all(CAPTURE_DIR).unwrap();
        image::save_buffer_with_format(
            &format!("{CAPTURE_DIR}/mesh_isolated_white.png"),
            &rgba,
            200,
            136,
            image::ExtendedColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .unwrap();
        println!("captured captures/mesh_isolated_white.png");
    }

    #[test]
    fn capture_status_card_states() {
        // Fonts used by the status card: Archivo SemiCondensed Bold
        // (DisplayHeading) + IBM Plex Sans Regular/Medium/SemiBold.
        load_font(include_bytes!("fonts/ArchivoSemiCondensed-Bold.ttf"));
        load_font(include_bytes!("fonts/IBMPlexSans-Regular.ttf"));
        load_font(include_bytes!("fonts/IBMPlexSans-Medium.ttf"));
        load_font(include_bytes!("fonts/IBMPlexSans-SemiBold.ttf"));

        // CONN-12 width sweep (spec §18 "Test widths manually"): capture
        // the Ready card at every test width, width-tagged so the user
        // can review each PNG against the acceptance checklist. Tier map
        // (from status_card.rs constants): MODE A >= 760 (1215/900/800),
        // MODE B 560-759 (700/679/600), MODE C < 560 (550/500/450/400);
        // mesh hidden below 520 (500/450/400 have no mesh).
        for w in [1215.0, 900.0, 800.0, 700.0, 679.0, 600.0, 550.0, 500.0, 450.0, 400.0] {
            let (h, name) = match w {
                1215.0 => (360.0, "status_ready_w1215"),
                900.0 => (360.0, "status_ready_w900"),
                800.0 => (360.0, "status_ready_w800"),
                700.0 => (360.0, "status_ready_w700"),
                679.0 => (360.0, "status_ready_w679"),
                600.0 => (360.0, "status_ready_w600"),
                550.0 => (440.0, "status_ready_w550"),
                500.0 => (440.0, "status_ready_w500"),
                450.0 => (480.0, "status_ready_w450"),
                400.0 => (480.0, "status_ready_w400"),
                _ => unreachable!(),
            };
            render_card(&dep(HomeConnectionVariant::Ready, w), w, h, name);
        }
        // State captures at one width (679, MODE B medium row).
        render_card(
            &dep(HomeConnectionVariant::Connecting, 679.0),
            679.0,
            320.0,
            "status_connecting_medium_679",
        );
        render_card(
            &dep(HomeConnectionVariant::Offline, 679.0),
            679.0,
            360.0,
            "status_offline_medium_679",
        );
    }

    #[test]
    fn ready_card_lands_in_compact_band() {
        // CONN-04 acceptance (spec §4): the normal desktop Ready card must
        // be ~200-230px tall — content-determined, not a fixed height.
        // The Full tier (single-line heading) is held strictly to the band;
        // the minimum supported window (Medium 679) allows some tolerance
        // above 230 because requirement 3 sanctions content-driven growth.
        // CONN-06: the heading is now 25px at Medium and no longer wraps
        // at 679px. CONN-11: the check indicator now shares the heading
        // row, so the card's height is driven by the 74px icon + heading
        // row (159.9px content + 48px padding ≈ 208px at 679) — still
        // inside the compact band, and the spec's "grow only when its
        // content requires it" permits heights below the 200 target when
        // the content genuinely fits on one line.
        load_font(include_bytes!("fonts/ArchivoSemiCondensed-Bold.ttf"));
        load_font(include_bytes!("fonts/IBMPlexSans-Regular.ttf"));
        load_font(include_bytes!("fonts/IBMPlexSans-Medium.ttf"));
        load_font(include_bytes!("fonts/IBMPlexSans-SemiBold.ttf"));

        // Wide desktop (1600 window → ~1215 content) — full three-region row.
        let full = measure_card_height(&dep(HomeConnectionVariant::Ready, 1215.0), 1215.0);
        assert!(
            (200.0..=230.0).contains(&full),
            "Ready Full card height {full:.1}px must land in the spec's 200-230px band"
        );
        // Minimum supported window (1024 → ~679 content) — medium row.
        let medium = measure_card_height(&dep(HomeConnectionVariant::Ready, 679.0), 679.0);
        assert!(
            (170.0..=240.0).contains(&medium),
            "Ready Medium card height {medium:.1}px must stay compact (single-line heading at CONN-06 scale; wrapped-growth allowed to 240)"
        );
    }

    #[test]
    fn security_pill_stays_one_compact_row_at_every_width() {
        // CONN-07 acceptance (spec §9): the pill is one compact inline row
        // (icon + text on a single line, `white-space: nowrap`,
        // `width: fit-content`). It must hug its content at wide widths
        // and MUST NEVER grow taller (wrap into a second line / a vertical
        // column) when the available width shrinks — the card switches to
        // the stacked layout (CONN-09) instead of squeezing the pill.
        load_font(include_bytes!("fonts/IBMPlexSans-Regular.ttf"));

        // Measure at an unconstrained width first — the natural, hugging
        // size of the pill (icon 14 + gap 8 + text + padding 8/12).
        let (natural_w, natural_h) = measure_pill(1000.0);
        assert!(
            natural_w > 0.0 && natural_w < 400.0,
            "pill natural width {natural_w:.1}px should be a compact one-row element"
        );
        // A single line of 13px supporting text + 8px vertical padding.
        assert!(
            natural_h < 45.0,
            "pill natural height {natural_h:.1}px must be a single compact row"
        );

        // Now squeeze: every available width from wide down to absurdly
        // narrow must keep the SAME height (one line, never a vertical
        // stack). The laid-out width must never exceed the available width
        // (the pill never forces the card wider than its container).
        for available in [
            400.0, 300.0, 280.0, 260.0, 240.0, 220.0, 200.0, 180.0, 160.0, 140.0, 120.0,
            100.0, 80.0, 60.0,
        ] {
            let (w, h) = measure_pill(available);
            assert!(
                (h - natural_h).abs() < 0.5,
                "pill height {h:.1}px at {available:.0}px available must stay the \
                 single-row height {natural_h:.1}px (never a vertical column)"
            );
            assert!(
                w <= available + 0.5,
                "pill width {w:.1}px at {available:.0}px available must never exceed \
                 its container"
            );
        }
    }

    #[test]
    fn security_pill_uses_nowrap_and_fit_content() {
        // CONN-07: structural regression guard — the pill must stay a
        // single-row fit-content element. This renders the pill inside a
        // narrow fixed container and asserts the laid-out height is
        // identical to the natural single-row height (the wrapped pill
        // would grow vertically; the stacked pill would be a column).
        load_font(include_bytes!("fonts/IBMPlexSans-Regular.ttf"));

        let (_, natural_h) = measure_pill(1000.0);
        // The tightest horizontal tier the card supports before switching
        // to the stacked layout: the Medium text column floor (240px) and
        // the Full tier floor (260px) — plus the real 679/1215 widths.
        for available in [240.0, 260.0, 679.0, 1215.0] {
            let (w, h) = measure_pill(available);
            assert!(
                (h - natural_h).abs() < 0.5,
                "pill must stay a single row at {available:.0}px (got height {h:.1}px, \
                 natural {natural_h:.1}px)"
            );
            assert!(
                w <= available + 0.5,
                "pill must fit its container at {available:.0}px (got width {w:.1}px)"
            );
        }
    }

    // ── CONN-10 (spec §14): no parent layout stretching ────────────────
    //
    // The dashboard grid replicates app.rs's wide-mode structure exactly:
    // left column (hero card + mesh card + quick actions) in a
    // FillPortion(2) wrapper, a 24 px gutter, and the right rail in a
    // FillPortion(1) wrapper, all inside a Row with `align_y(Start)` —
    // then the outer Fill-height canvas chain and the gutter scrollable.
    // The hero card must keep its content-determined height whether the
    // rail is tall (open) or empty (closed): the wrappers are explicit
    // Shrink-height, so iced never stretches the card to match a taller
    // sibling.

    /// Build the dashboard grid with a rail of `rail_card_height` px cards
    /// (or an empty rail when `None`). Returns the full scrollable element.
    fn build_dashboard_grid(
        hero_dep: &StatusCardDependency,
        rail_card_height: Option<f32>,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{container, Column, Row, Space};
        use iced::{Alignment, Length};

        let card_gap = crate::design_tokens::SPACE_20;

        let hero_card = view_status_card(hero_dep);

        // Content-height stand-ins for the Mesh Health card and the quick
        // action grid (fixed heights, non-void, so they never stretch).
        let mesh_card = container(Space::new().height(Length::Fixed(140.0)))
            .width(Length::Fill)
            .height(Length::Shrink);
        let action_grid = container(Space::new().height(Length::Fixed(160.0)))
            .width(Length::Fill)
            .height(Length::Shrink);

        let left_col = Column::new()
            .push(hero_card)
            .push(Space::new().height(Length::Fixed(card_gap)))
            .push(mesh_card)
            .push(Space::new().height(Length::Fixed(card_gap)))
            .push(action_grid)
            .spacing(0)
            .width(Length::Fill);

        let right_col: iced::Element<'static, AppMessage> = if let Some(h) = rail_card_height {
            // Three tall rail cards (Online Peers / Recent Activity /
            // Tunnels), each taller than the left column, so the rail is
            // the tallest sibling in the row — exactly the scenario that
            // used to make the card "extremely tall" when the rail opened.
            // Rebuilt per push (iced elements are not Clone).
            let rail_card = || {
                container(Space::new().height(Length::Fixed(h)))
                    .width(Length::Fill)
                    .height(Length::Shrink)
            };
            Column::new()
                .push(rail_card())
                .push(Space::new().height(Length::Fixed(card_gap)))
                .push(rail_card())
                .push(Space::new().height(Length::Fixed(card_gap)))
                .push(rail_card())
                .spacing(0)
                .width(Length::Fill)
                .into()
        } else {
            // Rail closed: empty column — left column is the tallest.
            Column::new().spacing(0).width(Length::Fill).into()
        };

        // Wide mode: two-column dashboard grid, both columns aligned top
        // (mirrors app.rs; the wrappers carry the CONN-10 explicit
        // Shrink-height guard).
        let main_content: iced::Element<'static, AppMessage> = Row::new()
            .push(
                container(left_col)
                    .width(Length::FillPortion(2))
                    .height(Length::Shrink),
            )
            .push(Space::new().width(Length::Fixed(crate::design_tokens::SPACE_24)))
            .push(
                container(right_col)
                    .width(Length::FillPortion(1))
                    .height(Length::Shrink),
            )
            .spacing(0)
            .align_y(Alignment::Start)
            .width(Length::Fill)
            .into();

        // Outer canvas chain: header + grid + footer inside a Fill-height
        // container, centred and capped, inside the gutter scrollable.
        let header = container(Space::new().height(Length::Fixed(80.0)))
            .width(Length::Fill)
            .height(Length::Shrink);
        let footer = container(Space::new().height(Length::Fixed(40.0)))
            .width(Length::Fill)
            .height(Length::Shrink);
        let col = Column::new()
            .push(header)
            .push(Space::new().height(Length::Fixed(crate::design_tokens::SPACE_28)))
            .push(main_content)
            .push(Space::new().height(Length::Fixed(crate::design_tokens::SPACE_16)))
            .push(footer)
            .spacing(0)
            .width(Length::Fill);

        let canvas = container(
            container(col)
                .padding(iced::Padding::from([
                    crate::design_tokens::SPACE_28,
                    crate::design_tokens::SPACE_32,
                ]))
                .width(Length::Fill)
                .max_width(crate::design_tokens::DASHBOARD_MAX_WIDTH)
                .height(Length::Fill),
        )
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .height(Length::Fill);

        crate::ui_components::gutter_scrollable(canvas)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Lay the dashboard grid out at a maximized canvas and return the
    /// laid-out height of the hero card node (index path through the
    /// scrollable → canvas → col → main_content row → left wrapper → left
    /// column; the hero card is the first child of the left column).
    fn grid_hero_card_height(
        hero_dep: &StatusCardDependency,
        rail_card_height: Option<f32>,
    ) -> f32 {
        let renderer =
            iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)));
        let mut element = build_dashboard_grid(hero_dep, rail_card_height);
        let mut tree = Tree::new(element.as_widget());
        // Maximized window: 1600 x 900 viewport.
        let limits = layout::Limits::new(Size::ZERO, Size::new(1600.0, 900.0));
        let node = element.as_widget_mut().layout(&mut tree, &renderer, &limits);
        // scrollable[0] = canvas; canvas[0] = col wrapper; [0] = col;
        // col[2] = main_content row; row[0] = left wrapper; [0] = left
        // column; left column [0] = hero card.
        let hero = &node.children()[0].children()[0].children()[0].children()[2]
            .children()[0].children()[0].children()[0];
        hero.bounds().height
    }

    #[test]
    fn hero_card_height_is_content_determined_in_dashboard_grid() {
        // CONN-10 (spec §14): the card's vertical size must be determined
        // by its own content, never by the right rail. Replicate the wide
        // dashboard grid with the rail OPEN (three tall cards, each taller
        // than the whole left column) and with the rail CLOSED (empty),
        // and assert the hero card's laid-out height is identical in both
        // — and equal to the standalone content-determined height.
        load_font(include_bytes!("fonts/ArchivoSemiCondensed-Bold.ttf"));
        load_font(include_bytes!("fonts/IBMPlexSans-Regular.ttf"));
        load_font(include_bytes!("fonts/IBMPlexSans-Medium.ttf"));
        load_font(include_bytes!("fonts/IBMPlexSans-SemiBold.ttf"));

        // Maximized window (~1600 → content 1215 → card (1215-24)*2/3 =
        // 794, Full tier).
        let content_width = 1215.0;
        let card_width = crate::design_tokens::status_card_content_width(content_width);
        assert!(
            card_width >= crate::status_card::STATUS_CARD_MEDIUM_CONTENT,
            "precondition: maximized-with-rail card width {card_width} must be Full tier"
        );
        let hero_dep = dep(HomeConnectionVariant::Ready, card_width);

        let standalone = measure_card_height(&hero_dep, card_width);
        let rail_open = grid_hero_card_height(&hero_dep, Some(400.0));
        let rail_closed = grid_hero_card_height(&hero_dep, None);

        assert!(
            (rail_open - standalone).abs() < 0.5,
            "hero card height in the grid with the rail OPEN ({rail_open:.1}px) must equal \
             its content-determined standalone height ({standalone:.1}px) — the rail must \
             not stretch the card (CONN-10 / spec §14)"
        );
        assert!(
            (rail_closed - standalone).abs() < 0.5,
            "hero card height in the grid with the rail CLOSED ({rail_closed:.1}px) must \
             equal its content-determined standalone height ({standalone:.1}px) — opening \
             the rail must not change the card's height (CONN-10 / spec §14)"
        );
    }
}
