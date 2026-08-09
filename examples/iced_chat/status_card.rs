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
//! - a native `canvas` peer-to-peer mesh on the right — an irregular,
//!   decentralised node graph with two slightly larger hubs, thin
//!   low-opacity lines, and a very slow (≈6 s cycle) subtle node
//!   brighten/fade when the mesh is Ready and the OS does not prefer
//!   reduced motion.
//!
//! The card is deliberately **theme-independent**: it is a dark panel in
//! both light and dark app themes, so every colour used here is a
//! `design_tokens::STATUS_*` constant rather than a theme accessor.
//!
//! All connection-state inputs come from the caller's snapshot
//! ([`StatusCardDependency`]); this module never reads live networking
//! state — the truthfulness mapping stays in `app.rs`
//! (`home_connection_variant`).

use iced::widget::canvas;
use iced::widget::canvas::{Frame, Path, Stroke};
use iced::widget::{button, container, Column, Row, Space};
use iced::{Alignment, Background, Border, Color, Length, Radians, Rectangle};

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
pub(crate) const STATUS_CARD_MIN_CONTENT_HEIGHT: f32 = 150.0;

/// Content width at which the card switches from the full three-region
/// row layout to the reduced medium layout.
pub(crate) const STATUS_CARD_MEDIUM_CONTENT: f32 = 760.0;
/// Content width below which the card switches to the stacked narrow
/// layout (icon + heading on one row, then divider/description/pill, then
/// the network graphic below).
pub(crate) const STATUS_CARD_NARROW_CONTENT: f32 = 520.0;

/// Minimum width the text (heading/description) column is guaranteed in the
/// Full tier. Mirrors the spec's `minmax(260px, 1fr)` text column — the
/// heading must never be squeezed below ~260px while a horizontal layout is
/// active (spec sections 2, 3, 18).
pub(crate) const STATUS_CARD_TEXT_MIN_WIDTH: f32 = 260.0;
/// Same guarantee for the Medium tier. The spec's MODE B keeps the text
/// area at roughly 220-260px; 240 is the mid-band floor so the mesh still
/// has room to stay visible at the bottom of the band.
const STATUS_CARD_TEXT_MIN_WIDTH_MEDIUM: f32 = 240.0;
/// Upper bound for the decorative mesh in the horizontal tiers (spec MODE A
/// graph 170-190px). The mesh must NEVER consume width the heading needs:
/// it only gets the leftover after the text minimum is satisfied.
/// CONN-05 tunes the exact graph size; this card only enforces the bound.
pub(crate) const STATUS_CARD_MESH_MAX_WIDTH: f32 = 190.0;

/// Amber accent for connecting / degraded states on the dark panel.
const STATUS_WARNING: Color = Color::from_rgb(
    0xE8 as f32 / 255.0,
    0xA3 as f32 / 255.0,
    0x3D as f32 / 255.0,
);

/// Red accent for the offline state on the dark panel.
const STATUS_DANGER: Color = Color::from_rgb(
    0xE5 as f32 / 255.0,
    0x5B as f32 / 255.0,
    0x5B as f32 / 255.0,
);

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
    /// Mesh-pulse phase (0..STATUS_CARD_PULSE_PHASES), bumped once per
    /// second by the app's ActivityTick.
    pub(crate) pulse_frame: u32,
    /// True when the mesh nodes may pulse (Ready and OS does not prefer
    /// reduced motion).
    pub(crate) animate_mesh: bool,
    /// True for non-Ready states — the mesh renders quieter so it never
    /// competes with the amber/red state signal.
    pub(crate) dimmed_mesh: bool,
    /// Home menu background opacity setting (0..=1); the card background
    /// participates like every other home card.
    pub(crate) home_menu_opacity: f32,
}

/// Render the full connection status card.
pub(crate) fn view_status_card(
    dep: &StatusCardDependency,
) -> iced::Element<'static, AppMessage> {
    let accent = variant_accent(dep.variant);
    let indicator = status_indicator(dep.variant);
    let tier = layout_tier(dep.content_width);

    let (heading_size, support_size) = match tier {
        Tier::Full => (30.0, 17.0),
        Tier::Medium => (28.0, 16.0),
        Tier::Narrow => (26.0, 16.0),
    };

    let heading = status_heading(dep, heading_size);
    let divider = status_divider(accent);
    let supporting = fonts::type_role_text_lh(TypeRole::Body, "Private communication, peer to peer.", 1.5)
        .size(support_size)
        .color(design_tokens::STATUS_SECONDARY_TEXT);

    let footer = if dep.show_retry || dep.show_details {
        actions_row(dep.show_retry, dep.show_details)
    } else if matches!(dep.variant, HomeConnectionVariant::Ready) {
        security_pill()
    } else {
        Space::new().height(Length::Fixed(0.0)).into()
    };

    // The decorative mesh yields before the text column: in the horizontal
    // tiers its width is the leftover AFTER the text minimum is satisfied
    // (bounded to [0, STATUS_CARD_MESH_MAX_WIDTH]); the narrow stacked tier
    // keeps the fixed per-tier size.
    let network = match tier {
        Tier::Full | Tier::Medium => {
            network_mesh(dep, tier, horizontal_mesh_width(dep.content_width, tier))
        }
        Tier::Narrow => network_mesh(dep, tier, network_size(tier).0),
    };

    let body: iced::Element<'static, AppMessage> = match tier {
        Tier::Full | Tier::Medium => {
            // [status icon] [status information] [network]
            // Vertical rhythm of the info column (hd/divider/dd/description/
            // dp/footer) trimmed for the compact height band (CONN-04).
            // Horizontal gaps (icon-text, text-graph) stay — CONN-05 owns
            // their tuning.
            let (icon_text_gap, text_graph_gap, hd_gap, dd_gap, dp_gap) = match tier {
                Tier::Full => (32.0, 40.0, 16.0, 12.0, 20.0),
                _ => (28.0, 32.0, 12.0, 10.0, 16.0),
            };
            let info = Column::new()
                .push(heading)
                .push(Space::new().height(Length::Fixed(hd_gap)))
                .push(divider)
                .push(Space::new().height(Length::Fixed(dd_gap)))
                .push(supporting)
                .push(Space::new().height(Length::Fixed(dp_gap)))
                .push(footer)
                .spacing(0)
                .width(Length::Fill);
            Row::new()
                .push(
                    Space::new()
                        .width(Length::Fixed(0.0))
                        .height(Length::Fixed(STATUS_CARD_MIN_CONTENT_HEIGHT)),
                )
                .push(indicator)
                .push(Space::new().width(Length::Fixed(icon_text_gap)))
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
            Column::new()
                .push(header_row)
                .push(Space::new().height(Length::Fixed(18.0)))
                .push(divider)
                .push(Space::new().height(Length::Fixed(14.0)))
                .push(supporting)
                .push(Space::new().height(Length::Fixed(22.0)))
                .push(footer)
                .push(Space::new().height(Length::Fixed(28.0)))
                .push(network)
                .spacing(0)
                .width(Length::Fill)
                .into()
        }
    };

    let opacity = dep.home_menu_opacity.clamp(0.0, 1.0);

    // Vertical padding tightened to SPACE_24 for the compact height band
    // (CONN-04; the spec's 24-28px padding target — CONN-05 owns the full
    // pass). Horizontal stays SPACE_32 so the width math in
    // `horizontal_mesh_width` (which subtracts 2 × SPACE_32) is unchanged.
    container(body)
        .padding([design_tokens::SPACE_24, design_tokens::SPACE_32])
        .width(Length::Fill)
        .style(move |_t| {
            container::Style {
                background: Some(Background::Gradient(iced::Gradient::Linear(
                    iced::gradient::Linear::new(Radians(std::f32::consts::FRAC_PI_4))
                        .add_stop(0.0, with_alpha(design_tokens::STATUS_CARD_BG_TOP, opacity))
                        .add_stop(0.5, with_alpha(design_tokens::STATUS_CARD_BG_MID, opacity))
                        .add_stop(1.0, with_alpha(design_tokens::STATUS_CARD_BG_BOTTOM, opacity)),
                ))),
                border: Border {
                    color: design_tokens::STATUS_CARD_BORDER,
                    width: 1.0,
                    radius: design_tokens::STATUS_CARD_RADIUS.into(),
                },
                shadow: design_tokens::status_card_shadow(),
                ..Default::default()
            }
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

fn layout_tier(content_width: f32) -> Tier {
    if content_width >= STATUS_CARD_MEDIUM_CONTENT {
        Tier::Full
    } else if content_width >= STATUS_CARD_NARROW_CONTENT {
        Tier::Medium
    } else {
        Tier::Narrow
    }
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

/// Outlined status indicator: a large circular outline, an inner ring with
/// a faint internal glow, and the state glyph (white check when Ready).
fn status_indicator(variant: HomeConnectionVariant) -> iced::Element<'static, AppMessage> {
    let accent = variant_accent(variant);
    let (glyph, glyph_color) = match variant {
        HomeConnectionVariant::Ready => (crate::app::ICON_CHECK, Color::WHITE),
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

    let inner = container(
        crate::app::icon_svg(glyph, glyph_size).style(move |_t, _| iced::widget::svg::Style {
            color: Some(glyph_color),
        }),
    )
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

/// Two-tone heading: `Boru` in the accent green, the rest near-white; any
/// other variant renders its truthful headline in the variant accent.
fn status_heading(
    dep: &StatusCardDependency,
    size: f32,
) -> iced::Element<'static, AppMessage> {
    const HEADING_LH: f32 = 1.15;
    if matches!(dep.variant, HomeConnectionVariant::Ready) {
        Row::new()
            .push(
                fonts::type_role_text_lh(TypeRole::DisplayHeading, "Boru ", HEADING_LH)
                    .size(size)
                    .color(design_tokens::STATUS_CONNECTED),
            )
            .push(
                fonts::type_role_text_lh(
                    TypeRole::DisplayHeading,
                    "is connected and ready.",
                    HEADING_LH,
                )
                .size(size)
                .color(design_tokens::STATUS_PRIMARY_TEXT)
                .width(Length::Fill)
                .wrapping(iced::widget::text::Wrapping::Word),
            )
            .spacing(0)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into()
    } else {
        fonts::type_role_text_lh(
            TypeRole::DisplayHeading,
            dep.headline.clone(),
            HEADING_LH,
        )
        .size(size)
        .color(variant_accent(dep.variant))
        .width(Length::Fill)
        .wrapping(iced::widget::text::Wrapping::Word)
        .into()
    }
}

/// Short accent divider under the heading (a small rounded green bar).
fn status_divider(accent: Color) -> iced::Element<'static, AppMessage> {
    container(Space::new().width(Length::Fill).height(Length::Fill))
        .width(Length::Fixed(32.0))
        .height(Length::Fixed(3.0))
        .style(move |_t| container::Style {
            background: Some(Background::Color(with_alpha(accent, 0.55))),
            border: Border {
                radius: 1.5.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// Compact `Secure • Decentralized • Private` status pill with a lock
/// glyph. Purely informational — no hover/click behaviour.
fn security_pill() -> iced::Element<'static, AppMessage> {
    container(
        Row::new()
            .push(
                crate::app::icon_svg(crate::app::ICON_LOCK, 14.0)
                    .style(move |_t, _| iced::widget::svg::Style {
                        color: Some(design_tokens::STATUS_CONNECTED),
                    }),
            )
            .push(Space::new().width(Length::Fixed(design_tokens::SPACE_8)))
            .push(
                fonts::type_role_text(
                    TypeRole::SupportingText,
                    "Secure  \u{2022}  Decentralized  \u{2022}  Private",
                )
                .color(design_tokens::STATUS_CONNECTED),
            )
            .spacing(0)
            .align_y(Alignment::Center),
    )
    .padding([design_tokens::SPACE_8, 14.0])
    .style(move |_t| container::Style {
        background: Some(Background::Color(with_alpha(
            design_tokens::STATUS_CONNECTED,
            0.10,
        ))),
        border: Border {
            color: with_alpha(design_tokens::STATUS_CONNECTED, 0.25),
            width: 1.0,
            radius: 14.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// Retry / Details actions for the Offline / Degraded states.
fn actions_row(show_retry: bool, show_details: bool) -> iced::Element<'static, AppMessage> {
    let mut row = Row::new().spacing(design_tokens::SPACE_8);
    if show_retry {
        row = row.push(
            button(fonts::type_role_text(TypeRole::ButtonLabel, "Retry"))
                .on_press(AppMessage::RetryConnection)
                .padding([design_tokens::SPACE_6, design_tokens::SPACE_12])
                .style(crate::app::BUTTON_PRIMARY),
        );
    }
    if show_details {
        row = row.push(
            button(fonts::type_role_text(TypeRole::ButtonLabel, "Details"))
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
/// size. The narrow stacked tier keeps its own fixed size.
fn network_size(tier: Tier) -> (f32, f32) {
    match tier {
        Tier::Full => (STATUS_CARD_MESH_MAX_WIDTH, 170.0),
        Tier::Medium => (STATUS_CARD_MESH_MAX_WIDTH, 136.0),
        Tier::Narrow => (190.0, 130.0),
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
fn horizontal_mesh_width(content_width: f32, tier: Tier) -> f32 {
    let (text_min, icon_text_gap, text_graph_gap) = match tier {
        Tier::Full => (STATUS_CARD_TEXT_MIN_WIDTH, 32.0, 40.0),
        _ => (STATUS_CARD_TEXT_MIN_WIDTH_MEDIUM, 28.0, 32.0),
    };
    // Card inner width = content width minus the card's 32px padding.
    let inner = (content_width - 2.0 * design_tokens::SPACE_32).max(0.0);
    let fixed =
        design_tokens::STATUS_INDICATOR_SIZE + icon_text_gap + text_graph_gap;
    let space = (inner - fixed).max(0.0);
    (space - text_min).clamp(0.0, STATUS_CARD_MESH_MAX_WIDTH)
}

/// Build the native canvas peer-to-peer mesh at the given width (the
/// height comes from the tier's nominal size).
fn network_mesh(
    dep: &StatusCardDependency,
    tier: Tier,
    width: f32,
) -> iced::Element<'static, AppMessage> {
    let (_, h) = network_size(tier);
    canvas(NetworkMesh {
        pulse: dep.pulse_frame % STATUS_CARD_PULSE_PHASES,
        animate: dep.animate_mesh,
        dimmed: dep.dimmed_mesh,
    })
    .width(Length::Fixed(width))
    .height(Length::Fixed(h))
    .into()
}

// ── Network mesh canvas ───────────────────────────────────────────────

/// One node of the decorative mesh, in normalized (0..1) coordinates so
/// the drawing scales crisply at every widget size.
#[derive(Debug, Clone, Copy)]
struct MeshNode {
    x: f32,
    y: f32,
    r: f32,
    /// Slightly larger, softly glowing nodes — still just peers in the
    /// mesh, never a central server.
    hub: bool,
}

/// Seven nodes in an irregular peer-to-peer arrangement.
const MESH_NODES: [MeshNode; 7] = [
    MeshNode { x: 0.10, y: 0.66, r: 4.5, hub: false },
    MeshNode { x: 0.30, y: 0.24, r: 7.0, hub: true },
    MeshNode { x: 0.50, y: 0.64, r: 5.0, hub: false },
    MeshNode { x: 0.72, y: 0.18, r: 6.0, hub: true },
    MeshNode { x: 0.90, y: 0.50, r: 4.0, hub: false },
    MeshNode { x: 0.22, y: 0.90, r: 3.5, hub: false },
    MeshNode { x: 0.82, y: 0.88, r: 4.0, hub: false },
];

/// Irregular mesh edges — deliberately NOT a star/server diagram.
const MESH_EDGES: [(usize, usize); 11] = [
    (0, 1),
    (1, 2),
    (2, 3),
    (3, 4),
    (0, 5),
    (5, 2),
    (2, 4),
    (6, 4),
    (6, 3),
    (5, 6),
    (1, 6),
];

/// Index of the node that softly brightens during the pulse.
const PULSE_NODE_A: usize = 1;
/// Index of the node that fades slightly (inverse phase).
const PULSE_NODE_B: usize = 3;

/// Native canvas program drawing the decentralised mesh. Geometry is
/// rebuilt whenever the card re-renders, so the slow per-second pulse
/// costs nothing extra — the home screen already rebuilds once per second
/// for the rail-card relative timestamps.
struct NetworkMesh {
    pulse: u32,
    animate: bool,
    dimmed: bool,
}

impl canvas::Program<AppMessage> for NetworkMesh {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry<iced::Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width.max(1.0), bounds.height.max(1.0));

        // Pulse phase in radians; a full cycle is STATUS_CARD_PULSE_PHASES
        // seconds (one phase per ActivityTick).
        let (sin_t, cos_t) = if self.animate {
            let t = self.pulse as f32 / STATUS_CARD_PULSE_PHASES as f32
                * std::f32::consts::TAU;
            (t.sin(), t.cos())
        } else {
            (0.0, 0.0)
        };

        let (node_a, node_b, other, line_a) = if self.dimmed {
            (0.35, 0.35, 0.30, 0.12)
        } else if self.animate {
            (
                0.55 + 0.25 * (0.5 + 0.5 * sin_t),
                0.55 + 0.20 * (0.5 + 0.5 * cos_t),
                0.42 + 0.08 * (0.5 + 0.5 * sin_t),
                0.18 + 0.07 * (0.5 + 0.5 * cos_t),
            )
        } else {
            (0.65, 0.65, 0.50, 0.22)
        };

        let pos = |i: usize| {
            let n = MESH_NODES[i];
            iced::Point::new(n.x * w, n.y * h)
        };

        // Thin low-opacity connection lines.
        for (a, b) in MESH_EDGES {
            frame.stroke(
                &Path::line(pos(a), pos(b)),
                Stroke::default()
                    .with_color(with_alpha(design_tokens::STATUS_NETWORK_LINE, line_a))
                    .with_width(1.0),
            );
        }

        // Soft glow behind the hub nodes (drawn under the node).
        for (i, n) in MESH_NODES.iter().enumerate() {
            if !n.hub {
                continue;
            }
            let alpha = match i {
                PULSE_NODE_A => node_a,
                PULSE_NODE_B => node_b,
                // Any future extra hub quietly joins node B's calmer phase.
                _ => node_b,
            };
            let center = iced::Point::new(n.x * w, n.y * h);
            frame.fill(
                &Path::circle(center, n.r * 2.8),
                with_alpha(design_tokens::STATUS_NETWORK_NODE, alpha * 0.16),
            );
        }

        // Nodes.
        for (i, n) in MESH_NODES.iter().enumerate() {
            let alpha = if n.hub {
                match i {
                    PULSE_NODE_A => node_a,
                    PULSE_NODE_B => node_b,
                    // Any future extra hub quietly joins node B's calmer phase.
                    _ => node_b,
                }
            } else {
                other
            };
            let center = iced::Point::new(n.x * w, n.y * h);
            frame.fill(
                &Path::circle(center, n.r),
                with_alpha(design_tokens::STATUS_NETWORK_NODE, alpha),
            );
        }

        vec![frame.into_geometry()]
    }
}

pub(crate) fn network_mesh_for_debug(
    pulse: u32,
    animate: bool,
    dimmed: bool,
) -> iced::Element<'static, AppMessage> {
    canvas(NetworkMesh {
        pulse: pulse % STATUS_CARD_PULSE_PHASES,
        animate,
        dimmed,
    })
    .width(Length::Fixed(200.0))
    .height(Length::Fixed(136.0))
    .into()
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_tiers_are_ordered_and_consistent() {
        assert!(
            STATUS_CARD_MEDIUM_CONTENT > STATUS_CARD_NARROW_CONTENT,
            "medium tier must sit above the narrow tier"
        );
        assert_eq!(layout_tier(STATUS_CARD_MEDIUM_CONTENT), Tier::Full);
        assert_eq!(layout_tier(STATUS_CARD_MEDIUM_CONTENT - 1.0), Tier::Medium);
        assert_eq!(layout_tier(STATUS_CARD_NARROW_CONTENT), Tier::Medium);
        assert_eq!(layout_tier(STATUS_CARD_NARROW_CONTENT - 1.0), Tier::Narrow);
        assert_eq!(layout_tier(0.0), Tier::Narrow);
        // The minimum supported window width (1024) must land in the
        // medium tier, where the three regions stay visible. CONN-02: the
        // tier input is the card's REAL width — at 1024 the rail stacks, so
        // the card spans the full content width (679 px) — never the raw
        // window-derived dashboard width.
        assert_eq!(
            layout_tier(crate::design_tokens::status_card_content_width(
                crate::design_tokens::home_content_width(1024.0)
            )),
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
        assert_eq!(layout_tier(card_width), Tier::Medium);
    }

    #[test]
    fn mesh_is_decentralized_not_star_shaped() {
        // Every node must have at least two connections (a mesh), and no
        // node may connect to every other node (no central server).
        let n = MESH_NODES.len();
        let mut degree = vec![0usize; n];
        for (a, b) in MESH_EDGES {
            degree[a] += 1;
            degree[b] += 1;
        }
        for (i, d) in degree.iter().enumerate() {
            assert!(
                *d >= 2,
                "node {i} has degree {d} — a mesh node needs ≥ 2 links"
            );
            assert!(
                *d < n - 1,
                "node {i} connects to every other node — that is a central server, not a mesh"
            );
        }
        // Exactly the two intended hubs, both larger than ordinary nodes.
        let hubs: Vec<&MeshNode> = MESH_NODES.iter().filter(|n| n.hub).collect();
        assert_eq!(hubs.len(), 2, "the mesh should have two slightly larger nodes");
        let min_regular = MESH_NODES
            .iter()
            .filter(|n| !n.hub)
            .map(|n| n.r)
            .fold(f32::INFINITY, f32::min);
        for hub in hubs {
            assert!(
                hub.r > min_regular,
                "hub radius must be larger than ordinary node radii"
            );
        }
    }

    #[test]
    fn min_height_and_network_sizes_are_positive() {
        assert!(STATUS_CARD_MIN_CONTENT_HEIGHT > 0.0);
        for tier in [Tier::Full, Tier::Medium, Tier::Narrow] {
            let (w, h) = network_size(tier);
            assert!(w > 0.0 && h > 0.0, "{tier:?} network size must be positive");
        }
        // CONN-03: the horizontal tiers' nominal mesh width must respect
        // the 190px bound (spec MODE A graph 170-190px; the exact size is
        // CONN-05's job — this card never exceeds the bound).
        for tier in [Tier::Full, Tier::Medium] {
            let (w, _) = network_size(tier);
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
        // 400/450/500/550/600/700/800/900+). For every width that selects
        // a horizontal tier, the mesh must yield enough space that the
        // text column (icon + gaps + mesh removed from the card inner
        // width) stays at or above its tier minimum.
        let widths = [
            400.0, 450.0, 500.0, 520.0, 550.0, 560.0, 600.0, 650.0, 700.0,
            759.0, 760.0, 800.0, 900.0, 1024.0, 1215.0,
        ];
        for width in widths {
            let tier = layout_tier(width);
            if tier == Tier::Narrow {
                // Stacked layout — no horizontal text minimum applies.
                continue;
            }
            let (text_min, icon_gap, graph_gap) = match tier {
                Tier::Full => (STATUS_CARD_TEXT_MIN_WIDTH, 32.0, 40.0),
                _ => (STATUS_CARD_TEXT_MIN_WIDTH_MEDIUM, 28.0, 32.0),
            };
            let mesh_w = horizontal_mesh_width(width, tier);
            let inner = width - 2.0 * design_tokens::SPACE_32;
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
        // At the bottom of the Medium band (520px) the decorative mesh
        // must shrink well below its cap so the text keeps its minimum —
        // the spec's priority order (heading > description > pill > graph).
        let tight = horizontal_mesh_width(520.0, Tier::Medium);
        assert!(
            tight > 0.0 && tight < STATUS_CARD_MESH_MAX_WIDTH,
            "at 520px the mesh should shrink ({tight}px) but stay visible"
        );
        let text_at_tight = 520.0
            - 2.0 * design_tokens::SPACE_32
            - design_tokens::STATUS_INDICATOR_SIZE
            - 28.0
            - 32.0
            - tight;
        assert!(
            text_at_tight + 0.01 >= STATUS_CARD_TEXT_MIN_WIDTH_MEDIUM,
            "text {text_at_tight}px must keep the Medium minimum"
        );
        // At a wide Full tier the mesh hits its cap and the text takes all
        // remaining space.
        let wide = horizontal_mesh_width(1215.0, Tier::Full);
        assert!(
            (wide - STATUS_CARD_MESH_MAX_WIDTH).abs() < 0.01,
            "at 1215px the mesh should be capped at {STATUS_CARD_MESH_MAX_WIDTH}px, got {wide}px"
        );
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
