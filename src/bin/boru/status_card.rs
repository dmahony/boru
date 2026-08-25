//! Redesigned home connection status card (dark privacy panel).
//!
//! Replaces the old pale-green hero card on the Boru home screen with the
//! approved dark status panel:
//!
//! - very dark green / near-black gradient background (`#10201C → #091714 →
//!   #06100E`) with a thin low-contrast green border, 22 px radius, subtle
//!   outer shadow;
//! - outlined status indicator (circular outline + internal glow + glyph)
//!   on the left;
//! - two-tone heading (`Boru` in the accent green, the rest near-white),
//!   a short accent divider, secondary copy, and a
//!   `Secure • Decentralized • Private` pill;
//! - a transparent monochrome world map on the right, kept subordinate to
//!   the status content and contain-fitted at every responsive tier.
//!
//! The card is deliberately **theme-independent**: it is a dark panel in
//! both light and dark app themes, so every colour used here is a
//! `design_tokens::STATUS_*` constant rather than a theme accessor.
//!
//! All connection-state inputs come from the caller's snapshot
//! ([`StatusCardDependency`]); this module never reads live networking
//! state — the truthfulness mapping stays in `app.rs`
//! (`home_connection_variant`).

use crate::theme::ColorTokens;
use iced::widget::{button, container, image, svg, Column, Row, Space};
use iced::{Alignment, Background, Border, Color, Length, Radians};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::app::{AppMessage, HomeConnectionVariant};
use crate::design_tokens;
use crate::fonts::{self, TypeRole};

/// Length of one node-pulse animation cycle, in seconds. Each phase is one
/// second of wall clock (driven by the app's existing per-second
/// `ActivityTick`), so the full cycle is slow and unobtrusive.
pub(crate) const STATUS_CARD_PULSE_PHASES: u32 = 6;

/// Lower bound for the status card's content height, before padding.
/// Kept compact (CONN-04): with the card's 24 px vertical padding the
/// Ready card lands ~218-235 px tall — inside or within a few px of the
/// spec's 200-230 px band — and grows beyond this floor only when its
/// content requires it (a wrapped two-line heading at Medium widths, or
/// wrapped degraded/offline reasons). Implemented as a zero-width spacer
/// so the card never clips content; the mesh (170 px Full / 136 px
/// Medium) and a wrapped two-line heading usually exceed this floor
/// anyway.
pub(crate) const STATUS_CARD_MIN_CONTENT_HEIGHT: f32 = 110.0;

/// Content width at which the card switches from the full three-region
/// row layout (MODE A) to the reduced compact horizontal layout
/// (MODE B). Matches the spec's `@container (min-width: 760px)` MODE A
/// boundary (spec §11).
pub(crate) const STATUS_CARD_MEDIUM_CONTENT: f32 = 760.0;
/// Content width below which the card switches from the compact
/// horizontal layout (MODE B) to the stacked narrow layout (MODE C).
/// Matches the spec's `@container (max-width: 559px)` MODE C boundary
/// (spec §12).
pub(crate) const STATUS_CARD_NARROW_CONTENT: f32 = 560.0;
/// Content width below which the decorative network graphic is hidden
/// entirely — the `@container (max-width: 520px) { .network-graphic {
/// display: none; } }` rule from spec §13. The card must communicate
/// status perfectly without the decoration.
pub(crate) const STATUS_CARD_MESH_HIDE_CONTENT: f32 = 520.0;

/// Minimum width the text (heading/description) column is guaranteed in the
/// Full tier. Mirrors the spec's `minmax(260px, 1fr)` text column — the
/// heading must never be squeezed below ~260px while a horizontal layout is
/// active (spec sections 2, 3, 18).
pub(crate) const STATUS_CARD_TEXT_MIN_WIDTH: f32 = 260.0;
/// Same guarantee for the Medium (MODE B) tier. Spec §11: the graph stays
/// on the right only while the text area retains at least ~260px — the
/// mesh must yield before heading/description/pill. The Full-tier minimum
/// is already 260, so MODE B keeps the same floor and the mesh simply
/// shrinks (and can reach 0) as width drops toward the MODE C boundary.
const STATUS_CARD_TEXT_MIN_WIDTH_MEDIUM: f32 = 260.0;
/// Upper bound for the decorative mesh in the horizontal tiers (spec §5
/// graph 280–320 px; tuned to use the available right side of the status
/// card without starving the connection text. The mesh must
/// NEVER consume width the heading needs: it only gets the leftover after
/// the text minimum is satisfied.
pub(crate) const STATUS_CARD_MESH_MAX_WIDTH: f32 = 320.0;

/// Horizontal padding of the card (px), spec §5 band 24–28 px. CONN-05:
/// reduced from 32 (CONN-04 had already trimmed the vertical padding to
/// 24) so the content row gets more of the card's width. Shared with the
/// mesh-width math below so the two can never drift apart.
pub(crate) const STATUS_CARD_PADDING_X: f32 = design_tokens::SPACE_24;

/// Horizontal gap between the status indicator and the text column (px).
/// Full tier ≈ 24 px (spec §5); the Medium tier is slightly tighter.
const STATUS_ICON_TEXT_GAP_FULL: f32 = 24.0;
const STATUS_ICON_TEXT_GAP_MEDIUM: f32 = 20.0;
/// Horizontal gap between the text column and the network graph (px),
/// spec §5 band 24–32 px (CONN-05: down from 40 Full / 32 Medium).
const STATUS_TEXT_GRAPH_GAP_FULL: f32 = 24.0;
const STATUS_TEXT_GRAPH_GAP_MEDIUM: f32 = 24.0;

/// Amber accent for connecting / degraded states on the dark panel.
/// Mirrors the `ColorTokens::status_warning` semantic token.
const STATUS_WARNING: Color = ColorTokens::light().status_warning;

/// Red accent for the offline state on the dark panel.
/// Mirrors the `ColorTokens::status_danger` semantic token.
const STATUS_DANGER: Color = ColorTokens::light().status_danger;

/// All inputs the status card needs to render. Built by `app.rs` from the
/// same live selectors the old hero card consumed — nothing here invents
/// connection state.
pub(crate) struct StatusCardDependency {
    /// Truthful connection variant (Starting / Connecting / Ready /
    /// Degraded / Offline).
    pub(crate) variant: HomeConnectionVariant,
    /// Available dashboard content width (window minus sidebar/divider/
    /// padding); drives the responsive tiers.
    pub(crate) content_width: f32,
    /// Pre-formatted headline for the current variant (app.rs owns the
    /// per-variant copy, incl. the Starting braille-dot animation).
    pub(crate) headline: String,
    /// Show the Retry action (Offline).
    pub(crate) show_retry: bool,
    /// Show the Details action (Offline / Degraded).
    pub(crate) show_details: bool,
    /// Retained for the shared home-card update cadence. The map itself is
    /// static so it cannot suggest live network topology.
    pub(crate) pulse_frame: u32,
    /// Retained for compatibility with the home status snapshot.
    pub(crate) animate_mesh: bool,
    /// True for non-Ready states. The map uses fixed neutral artwork and
    /// never adopts semantic status colors.
    pub(crate) dimmed_mesh: bool,
    /// Home menu background opacity setting (0..=1); the card background
    /// participates like every other home card.
    pub(crate) home_menu_opacity: f32,
    /// Card container radius (px) from the LIVE theme
    /// (`BoruTheme::radii.card`, BORU-UI-03). The status card is
    /// colour-theme-independent by design, but its corner radius follows
    /// the same "Card" token the other home cards use so the inspector's
    /// Card radius slider changes it immediately.
    pub(crate) card_radius: f32,
    /// BORU-LAYOUT-03: card sizing constraints (min content height, tier
    /// breakpoints, mesh hide/text-min widths, padding, icon-text and
    /// text-graph gaps, divider geometry) from the layout model
    /// (`home.card_sizing`). Defaults reproduce the module constants.
    pub(crate) sizing: crate::layout::HomeCardSizing,
    pub(crate) network_map_points: Vec<crate::app::NetworkMapPointSnapshot>,
    pub(crate) network_nodes_online: usize,
    pub(crate) network_countries: usize,
    pub(crate) network_networks: usize,
    pub(crate) accent_color: Color,
    pub(crate) dark_mode: bool,
}

/// Render the full connection status card.
pub(crate) fn view_status_card(dep: &StatusCardDependency) -> iced::Element<'static, AppMessage> {
    let accent = variant_accent(dep.variant);
    let indicator = status_indicator(dep.variant, dep.dark_mode);
    let tier = layout_tier(dep.content_width, dep.sizing);

    let (heading_size, support_size) = heading_sizes(tier);

    let heading = status_heading(dep, heading_size);
    let divider = status_divider(accent, dep.sizing);
    let supporting =
        fonts::type_role_text_lh(TypeRole::Body, crate::i18n::t("status.peer_to_peer"), 1.4)
            .size(support_size)
            .color(status_secondary_text(dep.dark_mode));

    let footer = if dep.show_retry || dep.show_details {
        Column::new()
            .push(actions_row(dep.show_retry, dep.show_details))
            .push(Space::new().height(Length::Fixed(10.0)))
            .push(network_stats_footer(dep))
            .spacing(0)
            .width(Length::Fill)
            .into()
    } else if matches!(dep.variant, HomeConnectionVariant::Ready) {
        Column::new()
            .push(security_pill())
            .push(Space::new().height(Length::Fixed(10.0)))
            .push(network_stats_footer(dep))
            .spacing(0)
            .width(Length::Fill)
            .into()
    } else {
        network_stats_footer(dep)
    };

    // The decorative mesh yields before the text column: in the horizontal
    // tiers its width is the leftover AFTER the text minimum is satisfied
    // (bounded to [0, STATUS_CARD_MESH_MAX_WIDTH]); the narrow stacked tier
    // keeps the fixed per-tier size. Below STATUS_CARD_MESH_HIDE_CONTENT
    // the mesh is not rendered at all (spec §13 — the card must still
    // communicate status perfectly without the decoration).
    let network: iced::Element<'static, AppMessage> =
        if mesh_rendered(dep.content_width, dep.sizing) {
            match tier {
                Tier::Full | Tier::Medium => network_map(
                    dep,
                    tier,
                    horizontal_mesh_width(dep.content_width, tier, dep.sizing),
                ),
                Tier::Narrow => network_map(dep, tier, network_size(tier, dep.sizing).0),
            }
        } else {
            Space::new()
                .width(Length::Fixed(0.0))
                .height(Length::Fixed(0.0))
                .into()
        };

    let body: iced::Element<'static, AppMessage> = match tier {
        Tier::Full | Tier::Medium => {
            // [status icon] [status information] [network]
            //
            // CONN-11 (spec §15): the check indicator belongs to the
            // HEADING ROW — `[✓] Boru is connected and ready.` reads as
            // one composed unit, then the divider / description / pill
            // flow below in the text column (the visual anchor). Before
            // this pass the icon floated beside the WHOLE text column
            // (its vertical centre sat between the description and the
            // pill), which is the "scattered" look the spec calls out.
            // The icon/heading row is exactly the MODE C pattern, applied
            // to the horizontal tiers too.
            //
            // Vertical rhythm of the info column (hd/divider/dd/
            // description/dp/footer) trimmed for the compact height band
            // (CONN-04): the icon in the heading row adds ~44px vs the
            // bare heading, so the sub-heading gaps are tightened to keep
            // the card in the spec's 200-230px band. Horizontal gaps
            // (icon-text, text-graph) tuned by CONN-05 to the spec §5
            // bands (Full ≈24px icon-text, 24–32px text-graph).
            let (icon_text_gap, text_graph_gap, dd_gap, dp_gap) = match tier {
                Tier::Full => (
                    dep.sizing.status_icon_text_gap_full,
                    dep.sizing.status_text_graph_gap_full,
                    6.0,
                    10.0,
                ),
                _ => (
                    dep.sizing.status_icon_text_gap_medium,
                    dep.sizing.status_text_graph_gap_medium,
                    6.0,
                    10.0,
                ),
            };
            let header_row = Row::new()
                .push(indicator)
                .push(Space::new().width(Length::Fixed(icon_text_gap)))
                .push(heading)
                .spacing(0)
                .align_y(Alignment::Center)
                .width(Length::Fill);
            // The divider / description / pill share the HEADING's left
            // edge (spec §15 sketch: `---` and the description and the
            // pill all start under "Boru is connected and ready.") — the
            // text block is the visual anchor, the icon sits off to its
            // left. The indent is exactly the icon + icon-text gap, so
            // the sub-heading content reads as one column with the
            // heading.
            let content = Column::new()
                .push(divider)
                .push(Space::new().height(Length::Fixed(dd_gap)))
                .push(supporting)
                .push(Space::new().height(Length::Fixed(dp_gap)))
                .push(footer)
                .spacing(0)
                .width(Length::Fill);
            let info = Column::new()
                .push(header_row)
                .push(
                    Row::new()
                        .push(Space::new().width(Length::Fixed(
                            design_tokens::STATUS_INDICATOR_SIZE + icon_text_gap,
                        )))
                        .push(content)
                        .spacing(0)
                        .width(Length::Fill),
                )
                .spacing(0)
                .width(Length::Fill);
            Row::new()
                .push(
                    Space::new()
                        .width(Length::Fixed(0.0))
                        .height(Length::Fixed(dep.sizing.status_card_min_content_height)),
                )
                .push(info)
                .push(Space::new().width(Length::Fixed(text_graph_gap)))
                .push(network)
                .spacing(0)
                .align_y(Alignment::Center)
                .width(Length::Fill)
                .into()
        }
        Tier::Narrow => {
            // icon + heading on one row, then description / pill / mesh
            let header_row = Row::new()
                .push(indicator)
                .push(Space::new().width(Length::Fixed(16.0)))
                .push(heading)
                .spacing(0)
                .align_y(Alignment::Center)
                .width(Length::Fill);
            let mut column = Column::new()
                .push(header_row)
                .push(Space::new().height(Length::Fixed(18.0)))
                .push(divider)
                .push(Space::new().height(Length::Fixed(14.0)))
                .push(supporting)
                .push(Space::new().height(Length::Fixed(22.0)))
                .push(footer);
            // The decorative mesh is optional below MODE B and hidden
            // entirely below STATUS_CARD_MESH_HIDE_CONTENT (spec §12/13);
            // only add its gap when it is actually rendered.
            if mesh_rendered(dep.content_width, dep.sizing) {
                column = column
                    .push(Space::new().height(Length::Fixed(28.0)))
                    .push(network);
            }
            column.spacing(0).width(Length::Fill).into()
        }
    };

    let opacity = dep.home_menu_opacity.clamp(0.0, 1.0);
    let card_radius = dep.card_radius;

    // Outer padding: vertical and horizontal both SPACE_24 (24 px), inside
    // the spec §5 24–28 px band (CONN-04 trimmed the vertical, CONN-05
    // matched the horizontal so the padding is uniform and the content row
    // gets more width). The mesh-width math below shares
    // STATUS_CARD_PADDING_X so it always stays in sync.
    // CONN-10 (spec §14 — no parent layout stretching): the card's vertical
    // size is explicitly content-determined (`height: fit-content` /
    // `align-self: start` in CSS terms). Without this explicit Shrink the
    // container's height would be inferred from the content's size hint
    // (`Container::new` uses `size.height.fluid()`), so any future child
    // with a Fill-height hint — or a parent chain that forces Fill — could
    // stretch the card taller than its content. Pinning Shrink makes the
    // card's height always equal to its own content, never a taller
    // sibling, the right rail, or the outer Fill chain.
    let dark_mode = dep.dark_mode;
    container(body)
        .padding([design_tokens::SPACE_8, dep.sizing.status_card_padding_x])
        .width(Length::Fill)
        .height(Length::Shrink)
        .style(move |_t| container::Style {
            background: Some(Background::Gradient(iced::Gradient::Linear(
                iced::gradient::Linear::new(Radians(std::f32::consts::FRAC_PI_4))
                    .add_stop(0.0, with_alpha(status_card_bg_top(dark_mode), opacity))
                    .add_stop(0.5, with_alpha(status_card_bg_mid(dark_mode), opacity))
                    .add_stop(
                        1.0,
                        with_alpha(status_card_bg_bottom(dark_mode), opacity),
                    ),
            ))),
            border: Border {
                color: status_card_border(dark_mode),
                width: 1.0,
                radius: card_radius.into(),
            },
            shadow: design_tokens::status_card_shadow(),
            ..Default::default()
        })
        .into()
}

/// Responsive layout tier of the status card (content-width based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    Full,
    Medium,
    Narrow,
}

fn layout_tier(content_width: f32, sizing: crate::layout::HomeCardSizing) -> Tier {
    if content_width >= sizing.status_card_medium_content {
        Tier::Full
    } else if content_width >= sizing.status_card_narrow_content {
        Tier::Medium
    } else {
        Tier::Narrow
    }
}

/// Whether the decorative network mesh is rendered at the given card
/// width. Spec §13: below ~520px the mesh is hidden entirely (the
/// `@container (max-width: 520px) { .network-graphic { display: none; } }`
/// equivalent) — the card must still communicate status without it.
/// BORU-LAYOUT-03: the hide threshold comes from the layout model.
fn mesh_rendered(content_width: f32, sizing: crate::layout::HomeCardSizing) -> bool {
    content_width >= sizing.status_card_mesh_hide_content
}

/// Accent colour for the current variant: green when connected, amber
/// while connecting/degraded, red when offline.
fn variant_accent(variant: HomeConnectionVariant) -> Color {
    match variant {
        HomeConnectionVariant::Ready => design_tokens::STATUS_CONNECTED,
        HomeConnectionVariant::Starting
        | HomeConnectionVariant::Connecting
        | HomeConnectionVariant::Degraded => STATUS_WARNING,
        HomeConnectionVariant::Offline => STATUS_DANGER,
    }
}

/// Build a `Color` with the given alpha from an opaque `Color`.
fn with_alpha(c: Color, a: f32) -> Color {
    Color::from_rgba(c.r, c.g, c.b, a.clamp(0.0, 1.0))
}

fn status_card_bg_top(dark_mode: bool) -> Color {
    if dark_mode {
        design_tokens::STATUS_CARD_BG_TOP
    } else {
        Color::from_rgb(0.96, 0.98, 0.97)
    }
}

fn status_card_bg_mid(dark_mode: bool) -> Color {
    if dark_mode {
        design_tokens::STATUS_CARD_BG_MID
    } else {
        Color::from_rgb(0.93, 0.97, 0.95)
    }
}

fn status_card_bg_bottom(dark_mode: bool) -> Color {
    if dark_mode {
        design_tokens::STATUS_CARD_BG_BOTTOM
    } else {
        Color::from_rgb(0.90, 0.95, 0.93)
    }
}

fn status_card_border(dark_mode: bool) -> Color {
    if dark_mode {
        design_tokens::STATUS_CARD_BORDER
    } else {
        Color::from_rgba(0.25, 0.48, 0.39, 0.24)
    }
}

fn status_primary_text(dark_mode: bool) -> Color {
    if dark_mode {
        design_tokens::STATUS_PRIMARY_TEXT
    } else {
        Color::from_rgb(0.10, 0.16, 0.14)
    }
}

fn status_secondary_text(dark_mode: bool) -> Color {
    if dark_mode {
        design_tokens::STATUS_SECONDARY_TEXT
    } else {
        Color::from_rgb(0.28, 0.37, 0.34)
    }
}

/// Outlined status indicator: a large circular outline, an inner ring with
/// a faint internal glow, and the state glyph (white check when Ready).
fn status_indicator(variant: HomeConnectionVariant, dark_mode: bool) -> iced::Element<'static, AppMessage> {
    let accent = variant_accent(variant);
    let (glyph, glyph_color) = match variant {
        HomeConnectionVariant::Ready => (
            crate::app::ICON_CHECK,
            if dark_mode {
                Color::WHITE
            } else {
                Color::from_rgb(0.10, 0.22, 0.18)
            },
        ),
        HomeConnectionVariant::Starting | HomeConnectionVariant::Connecting => {
            (crate::app::ICON_RETRY, accent)
        }
        HomeConnectionVariant::Degraded => (crate::app::ICON_MESH, accent),
        HomeConnectionVariant::Offline => (crate::app::ICON_OFFLINE, accent),
    };

    let size = design_tokens::STATUS_INDICATOR_SIZE;
    let ring = design_tokens::STATUS_INDICATOR_RING;
    let glyph_size = design_tokens::STATUS_INDICATOR_GLYPH;
    let glow_alpha = if matches!(variant, HomeConnectionVariant::Ready) {
        0.10
    } else {
        0.07
    };

    let inner = container(crate::app::icon_svg(glyph, glyph_size).style(move |_t, _| {
        iced::widget::svg::Style {
            color: Some(glyph_color),
        }
    }))
    .width(Length::Fixed(ring))
    .height(Length::Fixed(ring))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(move |_t| container::Style {
        background: Some(Background::Color(with_alpha(accent, glow_alpha))),
        border: Border {
            color: accent,
            width: 2.0,
            radius: (ring / 2.0).into(),
        },
        ..Default::default()
    });

    container(inner)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_t| container::Style {
            background: None,
            border: Border {
                color: with_alpha(accent, 0.18),
                width: 1.0,
                radius: (size / 2.0).into(),
            },
            ..Default::default()
        })
        .into()
}

/// Heading + supporting-text pixel sizes per responsive tier.
///
/// CONN-06: the heading's Full tier lands in the spec's 24-27 px band —
/// the real application context (the compact ~200-230 px card from
/// CONN-04) is the authority, and 30 px read oversized there. Medium and
/// Narrow scale down proportionally with 24 px as the floor. The
/// supporting text ("Private communication, peer to peer.") keeps its
/// own compact scale unchanged.
fn heading_sizes(tier: Tier) -> (f32, f32) {
    match tier {
        Tier::Full => (26.0, 17.0),
        Tier::Medium => (25.0, 16.0),
        Tier::Narrow => (24.0, 16.0),
    }
}

/// Two-tone heading: `Boru` in the accent green, the rest near-white; any
/// other variant renders its truthful headline in the variant accent.
fn status_heading(dep: &StatusCardDependency, size: f32) -> iced::Element<'static, AppMessage> {
    const HEADING_LH: f32 = 1.15;
    if matches!(dep.variant, HomeConnectionVariant::Ready) {
        // CONN-06: "Boru is connected and ready." is ONE rich-text
        // paragraph — two differently styled runs (green "Boru", the
        // remainder near-white) in a single layout flow. A Row of
        // separate Text widgets could break the spans apart (the "is
        // connected Boru and ready" failure mode); a shared paragraph
        // cannot. A non-breaking space glues the brand to the sentence so
        // "Boru" can never wrap onto a line of its own either. Weight is
        // Archivo SemiCondensed Bold (700) via TypeRole::DisplayHeading —
        // already in the spec's 650-700 band; line height stays 1.15.
        use iced::widget::text::{LineHeight, Rich, Span, Wrapping};
        // The spans carry no links, so the `Link` generic is explicitly
        // `()` — without a turbofish the compiler cannot infer it.
        Rich::with_spans([
            Span::<()>::new("Boru\u{00A0}")
                .font(TypeRole::DisplayHeading.font())
                .color(design_tokens::STATUS_CONNECTED),
            Span::<()>::new(crate::i18n::t("status.connected_tail"))
                .font(TypeRole::DisplayHeading.font())
                .color(status_primary_text(dep.dark_mode)),
        ])
        .size(size)
        .line_height(LineHeight::Relative(HEADING_LH))
        .width(Length::Fill)
        .wrapping(Wrapping::Word)
        .into()
    } else {
        fonts::type_role_text_lh(TypeRole::DisplayHeading, dep.headline.clone(), HEADING_LH)
            .size(size)
            .color(variant_accent(dep.variant))
            .width(Length::Fill)
            .wrapping(iced::widget::text::Wrapping::Word)
            .into()
    }
}

/// Short accent divider under the heading (a small rounded green bar).
/// CONN-11: widened to ~44px and slightly desaturated so it reads as a
/// deliberate accent aligned under the heading text (it shares the text
/// column's left edge), while staying subordinate to the status message
/// (spec §17 — decorative elements must not compete). BORU-LAYOUT-03:
/// the divider geometry comes from the layout model
/// (`home.card_sizing.status_divider_*`), defaulting to the same values
/// `HomeTheme::status_divider_*` supplied before.
fn status_divider(
    accent: Color,
    sizing: crate::layout::HomeCardSizing,
) -> iced::Element<'static, AppMessage> {
    let status = crate::theme::BoruTheme::default().home;
    container(Space::new().width(Length::Fill).height(Length::Fill))
        .width(Length::Fixed(sizing.status_divider_width))
        .height(Length::Fixed(sizing.status_divider_height))
        .style(move |_t| container::Style {
            background: Some(Background::Color(with_alpha(accent, 0.45))),
            border: Border {
                radius: status.status_divider_radius.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// Compact `Secure • Decentralized • Private` status pill with a lock
/// glyph. Purely informational — no hover/click behaviour.
///
/// CONN-07 (spec §9): the pill is ONE compact inline row —
/// `white-space: nowrap` + `width: fit-content` — so the icon and the
/// text always share a single line. The text element uses
/// `Wrapping::None` (the iced equivalent of nowrap) and both the inner
/// row and the container use Shrink width so the pill hugs its content
/// and can never be compressed into a vertical column. When the card is
/// genuinely narrow the stacked layout (CONN-09) takes over instead of
/// squeezing this pill.
pub(crate) fn security_pill() -> iced::Element<'static, AppMessage> {
    container(
        Row::new()
            .push(
                crate::app::icon_svg(crate::app::ICON_LOCK, 14.0).style(move |_t, _| {
                    iced::widget::svg::Style {
                        color: Some(design_tokens::STATUS_CONNECTED),
                    }
                }),
            )
            .push(Space::new().width(Length::Fixed(design_tokens::SPACE_8)))
            .push(
                fonts::type_role_text(
                    TypeRole::SupportingText,
                    crate::i18n::t("status.security_pill"),
                )
                .color(design_tokens::STATUS_CONNECTED)
                // Nowrap: the pill text must never wrap onto a second
                // line or break into a vertical stack (spec §9).
                .wrapping(iced::widget::text::Wrapping::None),
            )
            .spacing(0)
            .align_y(Alignment::Center)
            // fit-content — hug the icon + gap + text.
            .width(Length::Shrink),
    )
    .padding([design_tokens::SPACE_6, design_tokens::SPACE_10])
    .width(Length::Shrink)
    .style(move |_t| container::Style {
        background: Some(Background::Color(with_alpha(
            design_tokens::STATUS_CONNECTED,
            0.10,
        ))),
        border: Border {
            color: with_alpha(design_tokens::STATUS_CONNECTED, 0.25),
            width: 1.0,
            // BORU-UI-03: pill radius from `HomeTheme::security_pill_radius`.
            radius: crate::theme::BoruTheme::default()
                .home
                .security_pill_radius
                .into(),
        },
        ..Default::default()
    })
    .into()
}

/// Compact live Network Status summary. The counts are already derived by
/// [`NetworkMapState`]; this widget only formats and lays them out.
fn network_stats_footer(dep: &StatusCardDependency) -> iced::Element<'static, AppMessage> {
    let stat = |count: usize, singular: &'static str, plural: &'static str| {
        fonts::type_role_text(
            TypeRole::SupportingText,
            network_stat_text(count, singular, plural),
        )
        .color(status_secondary_text(dep.dark_mode))
        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
    };

    Row::new()
        .push(stat(
            dep.network_nodes_online,
            "status.node_online",
            "status.nodes_online",
        ))
        .push(stat(
            dep.network_countries,
            "status.country",
            "status.countries",
        ))
        .push(stat(
            dep.network_networks,
            "status.network",
            "status.networks",
        ))
        .spacing(design_tokens::SPACE_16)
        .width(Length::Fill)
        .wrap()
        .into()
}

fn network_stat_text(count: usize, singular: &str, plural: &str) -> String {
    let key = network_stat_key(count, singular, plural);
    let count = count.to_string();
    crate::i18n::t_args(key, &[("count", &count)])
}

fn network_stat_key<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

/// Retry / Details actions for the Offline / Degraded states.
fn actions_row(show_retry: bool, show_details: bool) -> iced::Element<'static, AppMessage> {
    let mut row = Row::new().spacing(design_tokens::SPACE_8);
    if show_retry {
        row = row.push(
            button(fonts::type_role_text(
                TypeRole::ButtonLabel,
                crate::i18n::t("home.retry"),
            ))
            .on_press(AppMessage::RetryConnection)
            .padding([design_tokens::SPACE_6, design_tokens::SPACE_12])
            .style(crate::app::BUTTON_PRIMARY),
        );
    }
    if show_details {
        row = row.push(
            button(fonts::type_role_text(
                TypeRole::ButtonLabel,
                crate::i18n::t("home.details"),
            ))
            .on_press(AppMessage::OpenConnectionDetails)
            .padding([design_tokens::SPACE_6, design_tokens::SPACE_12])
            .style(crate::app::BUTTON_OUTLINE),
        );
    }
    row.into()
}

/// Size of the network mesh per layout tier (width, height). The
/// horizontal tiers cap the width at [`STATUS_CARD_MESH_MAX_WIDTH`] — the
/// exact value used in the row comes from [`horizontal_mesh_width`] (the
/// mesh yields space to the text column); this only bounds the nominal
/// size. The narrow stacked tier keeps its own fixed size. NOTE: the
/// mesh is only *rendered* when [`mesh_rendered`] is true — below
/// [`STATUS_CARD_MESH_HIDE_CONTENT`] the graphic is dropped entirely, so
/// this nominal size is never laid out at those widths.
fn network_size(tier: Tier, sizing: crate::layout::HomeCardSizing) -> (f32, f32) {
    match tier {
        Tier::Full => (sizing.status_card_mesh_max_width, 160.0),
        Tier::Medium => (sizing.status_card_mesh_max_width, 130.0),
        Tier::Narrow => (220.0, 110.0),
    }
}

/// Width of the decorative mesh in the horizontal (Full/Medium) tiers.
///
/// Implements the spec's `auto minmax(260px, 1fr) minmax(150px, 190px)`
/// grid for the card's row: the icon keeps its fixed size, the text
/// column is flexible but never below its guaranteed minimum, and the
/// mesh gets only the REMAINDER after that minimum is satisfied — bounded
/// to `[0, STATUS_CARD_MESH_MAX_WIDTH]`. When space is tight the mesh
/// shrinks (and can reach 0) instead of starving the heading (spec
/// section 11 priority order: heading > description > pill > graph).
fn horizontal_mesh_width(
    content_width: f32,
    tier: Tier,
    sizing: crate::layout::HomeCardSizing,
) -> f32 {
    let (text_min, icon_text_gap, text_graph_gap) = match tier {
        Tier::Full => (
            sizing.status_card_text_min_width,
            sizing.status_icon_text_gap_full,
            sizing.status_text_graph_gap_full,
        ),
        _ => (
            sizing.status_card_text_min_width_medium,
            sizing.status_icon_text_gap_medium,
            sizing.status_text_graph_gap_medium,
        ),
    };
    // Card inner width = content width minus the card's horizontal padding.
    let inner = (content_width - 2.0 * sizing.status_card_padding_x).max(0.0);
    let fixed = design_tokens::STATUS_INDICATOR_SIZE + icon_text_gap + text_graph_gap;
    let space = (inner - fixed).max(0.0);
    (space - text_min).clamp(0.0, sizing.status_card_mesh_max_width)
}

/// Build the detailed transparent world map at the given width.
///
/// Keep the artwork as a direct PNG handle rather than embedding a large
/// data-URI SVG. The latter overflows the default Windows thread stack while
/// iced's SVG parser recursively walks the embedded image payload.
fn network_map(
    dep: &StatusCardDependency,
    tier: Tier,
    width: f32,
) -> iced::Element<'static, AppMessage> {
    let (_, h) = network_size(tier, dep.sizing);
    // The home dependency is intentionally rebuilt by the per-second
    // ActivityTick so relative timestamps stay current. Keep the embedded
    // image handle stable across those renders; constructing a new handle
    // each time makes Iced repeatedly discard/redecode the map and causes a
    // visible flash while the card is refreshed.
    image(world_map_handle())
        .width(Length::Fixed(width))
        .height(Length::Fixed(h))
        .into()
}

fn world_map_handle() -> image::Handle {
    static HANDLE: OnceLock<image::Handle> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            image::Handle::from_bytes(
                include_bytes!("../../../assets/status/world-map.png").to_vec(),
            )
        })
        .clone()
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct MapCacheKey {
    width_bits: u32,
    height_bits: u32,
    points: Vec<(iroh_base::PublicKey, u64, u64)>,
    accent: [u32; 3],
}

fn network_map_cache() -> &'static Mutex<HashMap<MapCacheKey, svg::Handle>> {
    static CACHE: OnceLock<Mutex<HashMap<MapCacheKey, svg::Handle>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ProjectedMapPoint {
    node_id: iroh_base::PublicKey,
    x: f64,
    y: f64,
}

const OVERLAP_DISTANCE: f64 = 16.0;
const MAX_OVERLAP_OFFSET: f64 = 5.5;

/// Spread only points whose original screen positions are close enough to
/// obscure one another. The seed is derived solely from the node identity, so
/// the result is stationary across renders and independent of input order.
fn spread_overlapping_points(points: &[ProjectedMapPoint]) -> Vec<ProjectedMapPoint> {
    points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let overlaps = points.iter().enumerate().any(|(other_index, other)| {
                index != other_index
                    && (point.x - other.x).hypot(point.y - other.y) <= OVERLAP_DISTANCE
            });
            if !overlaps {
                return *point;
            }
            let seed = stable_node_seed(point.node_id);
            let angle = (seed as f64 / u64::MAX as f64) * std::f64::consts::TAU;
            let radius = 3.5 + ((seed >> 32) as f64 / u32::MAX as f64) * 2.0;
            ProjectedMapPoint {
                x: point.x + radius.min(MAX_OVERLAP_OFFSET) * angle.cos(),
                y: point.y + radius.min(MAX_OVERLAP_OFFSET) * angle.sin(),
                ..*point
            }
        })
        .collect()
}

fn stable_node_seed(node_id: iroh_base::PublicKey) -> u64 {
    // FNV-1a is small, dependency-free, and deliberately stable across runs.
    node_id
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
}

/// Project WGS84 coordinates into the shared 1000×500 equirectangular map
/// viewport. Invalid values are omitted and valid edge values are clamped.
fn project_point(latitude: f64, longitude: f64, width: f64, height: f64) -> Option<(f64, f64)> {
    if !latitude.is_finite()
        || !longitude.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || !(-90.0..=90.0).contains(&latitude)
        || !(-180.0..=180.0).contains(&longitude)
    {
        return None;
    }
    let x = ((longitude + 180.0) / 360.0 * width).clamp(0.0, width);
    let y = ((90.0 - latitude) / 180.0 * height).clamp(0.0, height);
    Some((x, y))
}

/// Debug harness entry point retained for offscreen status-card captures.
pub(crate) fn network_mesh_for_debug(
    _pulse: u32,
    _animate: bool,
    _dimmed: bool,
) -> iced::Element<'static, AppMessage> {
    static HANDLE: OnceLock<svg::Handle> = OnceLock::new();
    svg(HANDLE
        .get_or_init(|| {
            svg::Handle::from_memory(include_bytes!("../../../assets/status/world-map.svg"))
        })
        .clone())
    .width(Length::Fixed(200.0))
    .height(Length::Fixed(136.0))
    .into()
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_stats_use_singular_only_for_one() {
        assert_eq!(network_stat_key(1, "one", "many"), "one");
        assert_eq!(network_stat_key(0, "one", "many"), "many");
        assert_eq!(network_stat_key(2, "one", "many"), "many");
    }

    #[test]
    fn layout_tiers_are_ordered_and_consistent() {
        // BORU-LAYOUT-03: tier selection reads the layout model's defaults,
        // which are pinned to the module constants by layout.rs tests.
        let s = crate::layout::HomeCardSizing::default();
        assert!(
            STATUS_CARD_MEDIUM_CONTENT > STATUS_CARD_NARROW_CONTENT,
            "MODE A boundary must sit above the MODE B/C boundary"
        );
        assert!(
            STATUS_CARD_NARROW_CONTENT > STATUS_CARD_MESH_HIDE_CONTENT,
            "the MODE B/C boundary (560) must sit above the mesh-hide width (520)"
        );
        assert_eq!(layout_tier(STATUS_CARD_MEDIUM_CONTENT, s), Tier::Full);
        assert_eq!(
            layout_tier(STATUS_CARD_MEDIUM_CONTENT - 1.0, s),
            Tier::Medium
        );
        assert_eq!(layout_tier(STATUS_CARD_NARROW_CONTENT, s), Tier::Medium);
        assert_eq!(
            layout_tier(STATUS_CARD_NARROW_CONTENT - 1.0, s),
            Tier::Narrow
        );
        // The mesh-hide width lives INSIDE the Narrow (MODE C) band — the
        // mesh disappears before the stacked layout ever gives up.
        assert_eq!(layout_tier(STATUS_CARD_MESH_HIDE_CONTENT, s), Tier::Narrow);
        assert_eq!(layout_tier(0.0, s), Tier::Narrow);
        // The minimum supported window width (1024) must land in the
        // medium tier, where the three regions stay visible. CONN-02: the
        // tier input is the card's REAL width — at 1024 the rail stacks, so
        // the card spans the full content width (679 px) — never the raw
        // window-derived dashboard width.
        assert_eq!(
            layout_tier(
                crate::design_tokens::status_card_content_width(
                    crate::design_tokens::home_content_width(1024.0)
                ),
                s
            ),
            Tier::Medium
        );
    }

    #[test]
    fn card_tier_uses_card_width_not_window_width() {
        // CONN-02 regression: at the 1280 reference window the window-derived
        // dashboard width is 919 px, which alone would select Tier::Full —
        // but with the right rail open the card's real container is only
        // (919−24)×2/3 = 596.7 px, which must select Tier::Medium. The tier
        // must track the card's actual width, not the window.
        let window_content = crate::design_tokens::home_content_width(1280.0);
        assert!(
            window_content >= STATUS_CARD_MEDIUM_CONTENT,
            "precondition: old (window-derived) input would pick Full"
        );
        assert!(
            window_content >= crate::design_tokens::HOME_TWO_COL_CONTENT,
            "precondition: rail open at 1280"
        );
        let card_width = crate::design_tokens::status_card_content_width(window_content);
        assert!(
            card_width < STATUS_CARD_MEDIUM_CONTENT,
            "card real width {card_width} must sit below the Full tier"
        );
        assert!(
            card_width >= STATUS_CARD_NARROW_CONTENT,
            "card real width {card_width} must stay in the readable Medium band"
        );
        assert_eq!(
            layout_tier(card_width, crate::layout::HomeCardSizing::default()),
            Tier::Medium
        );
    }

    #[test]
    fn world_map_asset_is_neutral_and_transparent() {
        // The production renderer uses the PNG handle below. Keep this test
        // tied to that asset rather than the legacy SVG debug fixture.
        let asset = include_bytes!("../../../assets/status/world-map.png");
        let map = ::image::load_from_memory(asset)
            .expect("world map must be a decodable PNG")
            .to_rgba8();
        assert_eq!(map.dimensions(), (1448, 1086));
        let mut has_transparent = false;
        let mut has_visible_pixels = false;
        for pixel in map.pixels() {
            has_transparent |= pixel[3] < 255;
            has_visible_pixels |= pixel[3] > 0;
        }
        assert!(has_transparent, "world map must retain transparent regions");
        assert!(has_visible_pixels, "world map must contain visible artwork");
    }

    #[test]
    fn world_map_handle_is_reused_across_renders() {
        assert_eq!(world_map_handle().id(), world_map_handle().id());
    }

    #[test]
    fn projection_uses_equirectangular_edges() {
        assert_eq!(project_point(90.0, -180.0, 1000.0, 500.0), Some((0.0, 0.0)));
        assert_eq!(
            project_point(-90.0, 180.0, 1000.0, 500.0),
            Some((1000.0, 500.0))
        );
        assert_eq!(project_point(0.0, 0.0, 1000.0, 500.0), Some((500.0, 250.0)));
    }

    #[test]
    fn overlapping_points_are_spread_deterministically_by_node_id() {
        let base = |byte| ProjectedMapPoint {
            node_id: iroh_base::SecretKey::from_bytes(&[byte; 32]).public(),
            x: 500.0,
            y: 250.0,
        };
        let points = (1..=8).map(base).collect::<Vec<_>>();
        let first = spread_overlapping_points(&points);
        let second = spread_overlapping_points(&points);
        assert_eq!(first, second);
        assert_eq!(first.len(), 8);
        assert!(first
            .iter()
            .all(|point| { (point.x - 500.0).hypot(point.y - 250.0) <= MAX_OVERLAP_OFFSET }));
        assert!(first
            .iter()
            .any(|point| point.x != 500.0 || point.y != 250.0));
        assert!(first.iter().enumerate().any(|(index, point)| {
            first[index + 1..]
                .iter()
                .any(|other| point.x != other.x || point.y != other.y)
        }));
    }

    #[test]
    fn overlap_spreading_preserves_all_same_city_nodes() {
        let points = (20..=22)
            .map(|byte| ProjectedMapPoint {
                node_id: iroh_base::SecretKey::from_bytes(&[byte; 32]).public(),
                x: 500.0,
                y: 250.0,
            })
            .collect::<Vec<_>>();

        let spread = spread_overlapping_points(&points);
        assert_eq!(spread.len(), points.len());
        assert_eq!(
            spread.iter().map(|point| point.node_id).collect::<Vec<_>>(),
            points.iter().map(|point| point.node_id).collect::<Vec<_>>()
        );
        assert!(spread
            .iter()
            .all(|point| { (point.x - 500.0).hypot(point.y - 250.0) <= MAX_OVERLAP_OFFSET }));
        assert!(spread
            .windows(2)
            .any(|pair| pair[0].x != pair[1].x || pair[0].y != pair[1].y));
    }

    #[test]
    fn non_overlapping_points_are_unchanged() {
        let point = ProjectedMapPoint {
            node_id: iroh_base::SecretKey::from_bytes(&[1; 32]).public(),
            x: 100.0,
            y: 200.0,
        };
        assert_eq!(spread_overlapping_points(&[point]), vec![point]);
        let other = ProjectedMapPoint { x: 300.0, ..point };
        assert_eq!(
            spread_overlapping_points(&[point, other]),
            vec![point, other]
        );
    }

    #[test]
    fn same_node_id_gets_the_same_offset_when_overlapping() {
        let point = ProjectedMapPoint {
            node_id: iroh_base::SecretKey::from_bytes(&[7; 32]).public(),
            x: 100.0,
            y: 200.0,
        };
        let first = spread_overlapping_points(&[point, ProjectedMapPoint { x: 100.0, ..point }]);
        let second = spread_overlapping_points(&[point, ProjectedMapPoint { x: 100.0, ..point }]);
        assert_eq!(first[0], second[0]);
    }

    #[test]
    fn projection_rejects_invalid_coordinates() {
        for (latitude, longitude) in [
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
            (-90.1, 0.0),
            (0.0, 180.1),
        ] {
            assert_eq!(project_point(latitude, longitude, 1000.0, 500.0), None);
        }
    }

    #[test]
    fn min_height_and_network_sizes_are_positive() {
        let s = crate::layout::HomeCardSizing::default();
        assert!(STATUS_CARD_MIN_CONTENT_HEIGHT > 0.0);
        for tier in [Tier::Full, Tier::Medium, Tier::Narrow] {
            let (w, h) = network_size(tier, s);
            assert!(w > 0.0 && h > 0.0, "{tier:?} network size must be positive");
        }
        // CONN-05: the horizontal tiers' nominal mesh width must respect
        // the 320px bound used by the current dashboard card
        // exact size; this card never exceeds the bound).
        for tier in [Tier::Full, Tier::Medium] {
            let (w, _) = network_size(tier, s);
            assert!(
                w <= STATUS_CARD_MESH_MAX_WIDTH + f32::EPSILON,
                "{tier:?} mesh width {w} exceeds the {STATUS_CARD_MESH_MAX_WIDTH}px bound"
            );
        }
    }

    #[test]
    fn text_column_keeps_minimum_width_in_horizontal_tiers() {
        // CONN-03 spec sections 2/3/18: while ANY horizontal layout is
        // active the heading must never be squeezed below ~220-260px.
        // Sweep the tier boundary bands (the spec's manual test widths:
        // 400/450/500/550/600/700/800/900+, plus the new MODE B/C
        // boundary 560 and MODE A boundary 760). For every width that
        // selects a horizontal tier (Full ≥ 760, Medium 560-759), the
        // mesh must yield enough space that the text column (icon + gaps
        // + mesh removed from the card inner width) stays at or above its
        // tier minimum. Narrow (< 560) is the stacked MODE C — no
        // horizontal text minimum applies.
        let widths = [
            400.0, 450.0, 500.0, 520.0, 550.0, 559.0, 560.0, 600.0, 650.0, 700.0, 759.0, 760.0,
            800.0, 900.0, 1024.0, 1215.0,
        ];
        for width in widths {
            let s = crate::layout::HomeCardSizing::default();
            let tier = layout_tier(width, s);
            if tier == Tier::Narrow {
                // Stacked layout — no horizontal text minimum applies.
                continue;
            }
            let (text_min, icon_gap, graph_gap) = match tier {
                Tier::Full => (
                    STATUS_CARD_TEXT_MIN_WIDTH,
                    STATUS_ICON_TEXT_GAP_FULL,
                    STATUS_TEXT_GRAPH_GAP_FULL,
                ),
                _ => (
                    STATUS_CARD_TEXT_MIN_WIDTH_MEDIUM,
                    STATUS_ICON_TEXT_GAP_MEDIUM,
                    STATUS_TEXT_GRAPH_GAP_MEDIUM,
                ),
            };
            let mesh_w = horizontal_mesh_width(width, tier, s);
            let inner = width - 2.0 * STATUS_CARD_PADDING_X;
            let text_w =
                inner - design_tokens::STATUS_INDICATOR_SIZE - icon_gap - graph_gap - mesh_w;
            assert!(
                text_w + 0.01 >= text_min,
                "at card width {width}px ({tier:?}) the text column is {text_w}px \
                 but must stay >= {text_min}px (mesh took {mesh_w}px)"
            );
            assert!(
                mesh_w <= STATUS_CARD_MESH_MAX_WIDTH + 0.01,
                "at card width {width}px ({tier:?}) the mesh ({mesh_w}px) exceeds the \
                 {STATUS_CARD_MESH_MAX_WIDTH}px bound"
            );
        }
    }

    #[test]
    fn mesh_yields_before_text_when_space_is_tight() {
        // At the bottom of the MODE B (Medium) band (560px) the decorative
        // mesh must shrink well below its cap so the text keeps its
        // minimum — the spec's priority order (heading > description >
        // pill > graph).
        let s = crate::layout::HomeCardSizing::default();
        let tight = horizontal_mesh_width(560.0, Tier::Medium, s);
        assert!(
            tight > 0.0 && tight < STATUS_CARD_MESH_MAX_WIDTH,
            "at 560px the mesh should shrink ({tight}px) but stay visible"
        );
        let text_at_tight = 560.0
            - 2.0 * STATUS_CARD_PADDING_X
            - design_tokens::STATUS_INDICATOR_SIZE
            - STATUS_ICON_TEXT_GAP_MEDIUM
            - STATUS_TEXT_GRAPH_GAP_MEDIUM
            - tight;
        assert!(
            text_at_tight + 0.01 >= STATUS_CARD_TEXT_MIN_WIDTH_MEDIUM,
            "text {text_at_tight}px must keep the Medium minimum"
        );
        // At a wide Full tier the mesh hits its cap and the text takes all
        // remaining space.
        let wide = horizontal_mesh_width(1215.0, Tier::Full, s);
        assert!(
            (wide - STATUS_CARD_MESH_MAX_WIDTH).abs() < 0.01,
            "at 1215px the mesh should be capped at {STATUS_CARD_MESH_MAX_WIDTH}px, got {wide}px"
        );
    }

    #[test]
    fn mesh_is_hidden_below_mesh_hide_width() {
        // Spec §13: below ~520px card width the decorative network graphic
        // is NOT rendered (the `@container (max-width: 520px)` rule). The
        // card must still communicate status perfectly without the mesh.
        assert!(
            STATUS_CARD_MESH_HIDE_CONTENT < STATUS_CARD_NARROW_CONTENT,
            "the mesh-hide width must sit inside the MODE C (narrow) band"
        );
        // The mesh is rendered from the hide boundary upward...
        let s = crate::layout::HomeCardSizing::default();
        assert!(mesh_rendered(STATUS_CARD_MESH_HIDE_CONTENT, s));
        assert!(mesh_rendered(STATUS_CARD_NARROW_CONTENT, s));
        assert!(mesh_rendered(STATUS_CARD_MEDIUM_CONTENT, s));
        // ...and hidden below it (400/450/500/519 are all sub-520).
        assert!(!mesh_rendered(STATUS_CARD_MESH_HIDE_CONTENT - 1.0, s));
        assert!(!mesh_rendered(500.0, s));
        assert!(!mesh_rendered(400.0, s));
        assert!(!mesh_rendered(0.0, s));
        // The hide width is a MODE C width: stacked layout, no horizontal
        // text minimum, no mesh.
        assert_eq!(layout_tier(STATUS_CARD_MESH_HIDE_CONTENT, s), Tier::Narrow);
    }

    #[test]
    fn heading_sizes_land_in_the_conn06_band() {
        // CONN-06 acceptance (spec §§7/8/16 — the real application
        // context is the authority): the heading must read at the compact
        // card's scale. Full tier in the 24-27 px band, Medium/Narrow
        // scaled down proportionally with 24 px as the floor.
        let (full, _) = heading_sizes(Tier::Full);
        let (medium, _) = heading_sizes(Tier::Medium);
        let (narrow, _) = heading_sizes(Tier::Narrow);
        assert!(
            (24.0..=27.0).contains(&full),
            "Full-tier heading {full}px must be in the spec's 24-27 px band"
        );
        assert!(
            (24.0..=full).contains(&medium) && medium >= 24.0,
            "Medium-tier heading {medium}px must scale down from Full ({full}px) to the 24px floor"
        );
        assert_eq!(
            narrow, 24.0,
            "Narrow-tier heading must sit on the 24px floor"
        );
        // Supporting text ("Private communication, peer to peer.") keeps
        // its own compact scale — always positive.
        let (_, support_full) = heading_sizes(Tier::Full);
        assert!(support_full > 0.0);
    }

    #[test]
    fn variant_accent_covers_every_state() {
        use HomeConnectionVariant::*;
        for variant in [Starting, Connecting, Ready, Degraded, Offline] {
            let accent = variant_accent(variant);
            assert!(accent.a > 0.9, "{variant:?} accent must be opaque");
        }
        assert_eq!(
            variant_accent(Ready),
            design_tokens::STATUS_CONNECTED,
            "connected state must use the status green"
        );
    }
}
