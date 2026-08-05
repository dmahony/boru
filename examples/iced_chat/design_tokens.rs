//! Central visual tokens and reusable style primitives for the Boru desktop UI.
//!
//! Keep product decisions here so screens compose the same palette, rhythm, and
//! interaction states instead of inventing local literals.
//!
//! See `DESIGN_SYSTEM.md` for the full specification.
//!
//! ## Token naming conventions
//!
//! | Category     | Convention                             | Example              |
//! |-------------|----------------------------------------|----------------------|
//! | Backgrounds  | `color_<role>` or `bg_<role>`          | `color_canvas()`     |
//! | Text         | `text_<role>`                          | `text_primary()`     |
//! | Accents      | bare semantic name                     | `primary()`          |
//! | Borders      | `border_<weight>`                      | `border_muted()`     |

use iced::widget::{button, container};
use iced::{Background, Border, Color, Theme};

// ── Color palette (phase 1 — Boru Modern redesign) ───────────────────
//
// All hex values match the visual system spec from the Boru Modern UI
// plan, section 4.  Semantic names chosen over component names so the
// same token can be used across unrelated surfaces without coupling.

const CANVAS: Color = Color::from_rgb(
    0xF7 as f32 / 255.0,
    0xF9 as f32 / 255.0,
    0xF8 as f32 / 255.0,
);
const SIDEBAR: Color = Color::from_rgb(
    0xFC as f32 / 255.0,
    0xFD as f32 / 255.0,
    0xFC as f32 / 255.0,
);
const SURFACE: Color = Color::WHITE;
const SURFACE_SELECTED: Color = Color::from_rgb(
    0xED as f32 / 255.0,
    0xF7 as f32 / 255.0,
    0xF1 as f32 / 255.0,
);
const BORDER_COLOR: Color = Color::from_rgb(
    0xDC as f32 / 255.0,
    0xE5 as f32 / 255.0,
    0xDF as f32 / 255.0,
);
const BORDER_STRONG: Color = Color::from_rgb(
    0xC8 as f32 / 255.0,
    0xD7 as f32 / 255.0,
    0xCE as f32 / 255.0,
);
const TEXT_PRIMARY: Color = Color::from_rgb(
    0x17 as f32 / 255.0,
    0x21 as f32 / 255.0,
    0x1B as f32 / 255.0,
);
const TEXT_SECONDARY: Color = Color::from_rgb(
    0x5F as f32 / 255.0,
    0x6F as f32 / 255.0,
    0x66 as f32 / 255.0,
);
const TEXT_MUTED: Color = Color::from_rgb(
    0x64 as f32 / 255.0,
    0x70 as f32 / 255.0,
    0x6A as f32 / 255.0,
);
const PRIMARY: Color = Color::from_rgb(
    0x18 as f32 / 255.0,
    0x7F as f32 / 255.0,
    0x50 as f32 / 255.0,
);
const PRIMARY_HOVER: Color = Color::from_rgb(
    0x14 as f32 / 255.0,
    0x76 as f32 / 255.0,
    0x43 as f32 / 255.0,
);
const PRIMARY_SOFT: Color = Color::from_rgb(
    0xEA as f32 / 255.0,
    0xF5 as f32 / 255.0,
    0xEE as f32 / 255.0,
);
const SUCCESS: Color = Color::from_rgb(
    0x1A as f32 / 255.0,
    0x7F as f32 / 255.0,
    0x48 as f32 / 255.0,
);
const DANGER: Color = Color::from_rgb(
    0xC8 as f32 / 255.0,
    0x4E as f32 / 255.0,
    0x4E as f32 / 255.0,
);
const FOCUS: Color = Color::from_rgb(
    0x2B as f32 / 255.0,
    0x9B as f32 / 255.0,
    0x67 as f32 / 255.0,
);

// ── Legacy palette constants (kept for gradual migration) ────────────
// These match the pre-redesign palette used extensively in app.rs.
// They will be replaced by the spec values above as screens are touched.

#[deprecated(since = "0.109.0", note = "use `primary()` (spec green #188C50)")]
pub const PRIMARY_LEGACY: Color = Color::from_rgb(
    0x2f as f32 / 255.0,
    0x6b as f32 / 255.0,
    0x4f as f32 / 255.0,
);
#[deprecated(since = "0.109.0", note = "use `primary_hover()` (spec green #147643)")]
pub const PRIMARY_HOVER_LEGACY: Color = Color::from_rgb(
    0x28 as f32 / 255.0,
    0x5b as f32 / 255.0,
    0x44 as f32 / 255.0,
);

// ── Extended palette constants ────────────────────────────────────────
const WARNING: Color = Color::from_rgb(
    0x70 as f32 / 255.0,
    0x45 as f32 / 255.0,
    0x05 as f32 / 255.0,
);
const INPUT_BG: Color = Color::from_rgb(
    0xf0 as f32 / 255.0,
    0xf0 as f32 / 255.0,
    0xf4 as f32 / 255.0,
);

// ── Spacing scale (4 px base unit) ────────────────────────────────────
pub const SPACE_2: f32 = 2.0;
pub const SPACE_4: f32 = 4.0;
pub const SPACE_6: f32 = 6.0;
pub const SPACE_8: f32 = 8.0;
pub const SPACE_10: f32 = 10.0;
pub const SPACE_12: f32 = 12.0;
pub const SPACE_16: f32 = 16.0;
pub const SPACE_18: f32 = 18.0; // group-to-group gap between user message groups (plan §4)
pub const SPACE_20: f32 = 20.0;
pub const SPACE_24: f32 = 24.0;
pub const SPACE_28: f32 = 28.0;
pub const SPACE_32: f32 = 32.0;
pub const SPACE_40: f32 = 40.0;

// ── Control heights ───────────────────────────────────────────────────
pub const CONTROL_HEIGHT: f32 = 40.0;
pub const CONTROL_HEIGHT_COMPACT: f32 = 36.0;

// ── Corner radii ──────────────────────────────────────────────────────
pub const RADIUS_SM: f32 = 8.0; // small controls
pub const RADIUS_MD: f32 = 10.0; // buttons, list selections
pub const RADIUS_LG: f32 = 12.0; // chat bubbles, dialogs
pub const RADIUS_XL: f32 = 16.0; // hero cards, composer
pub const RADIUS_CARD: f32 = 16.0; // card containers (plan: 14-18 px band; same value as RADIUS_XL today)

// ── Borders ───────────────────────────────────────────────────────────
pub const BORDER_WIDTH: f32 = 1.0; // standard 1 px border

// ── Focus ─────────────────────────────────────────────────────────────
pub const FOCUS_WIDTH: f32 = 2.0; // focus ring width

// ── Avatar sizes ──────────────────────────────────────────────────────
pub const AVATAR_SM: f32 = 36.0;
pub const AVATAR_MD: f32 = 48.0;
pub const AVATAR_LG: f32 = 64.0;

// ── Layout dimensions ─────────────────────────────────────────────────
/// Target sidebar width at reference viewport (1280×800). Range: 288–320 px.
pub const SIDEBAR_WIDTH: f32 = 304.0;
pub const SIDEBAR_WIDTH_MIN: f32 = 288.0;
pub const SIDEBAR_WIDTH_MAX: f32 = 320.0;
/// Horizontal inset from sidebar edges to content. Spec: 24 px.
pub const SIDEBAR_INSET: f32 = 24.0;
pub const DETAILS_PANEL_WIDTH: f32 = 280.0;
pub const MESSAGE_MAX_WIDTH: f32 = 480.0;
/// Chat bubble hard maximum width. Spec: 560 px.
pub const CHAT_BUBBLE_MAX_WIDTH: f32 = 560.0;
/// Chat bubble width as a fraction of the timeline width. Spec: 68 %.
/// The effective maximum is `min(CHAT_BUBBLE_MAX_WIDTH, timeline * RATIO)`.
pub const CHAT_BUBBLE_WIDTH_RATIO: f32 = 0.68;
pub const IMAGE_PREVIEW_MAX_WIDTH: f32 = 360.0;
pub const IMAGE_PREVIEW_MAX_HEIGHT: f32 = 400.0;

// ── Responsive thresholds ─────────────────────────────────────────────
/// Reference viewport (primary design target).
pub const VIEWPORT_REF_WIDTH: f32 = 1280.0;
pub const VIEWPORT_REF_HEIGHT: f32 = 800.0;

/// Minimum supported width before layout collapses.
pub const VIEWPORT_MIN_WIDTH: f32 = 1024.0;
pub const VIEWPORT_MIN_HEIGHT: f32 = 720.0;

// ── FS-02 File Sharing dashboard extended tokens ──────────────────────
// Proposed in docs/fs-02-file-sharing-dashboard-spec.md, section 2.2.
// These are semantic additions used by progress bars, table rows, peer
// chips, and the peers panel. Any future screen needing the same primitives
// reuses these constants.

/// Thin progress bar height (4 px) — unobtrusive inline indicator.
pub const PROGRESS_BAR_HEIGHT: f32 = 4.0;
/// Slightly thicker progress bar (6 px) — for download cards where the
/// bar is the primary visual element.
pub const PROGRESS_BAR_HEIGHT_BOLD: f32 = 6.0;
/// Standard file-table row height (56 px) — tall enough for two-line
/// name + MIME + metadata.
pub const TABLE_ROW_HEIGHT: f32 = 56.0;
/// Compact row height (48 px) — for activity-log entries and other
/// single-line rows. Matches `card_shell::CARD_ROW_HEIGHT`.
pub const TABLE_ROW_HEIGHT_COMPACT: f32 = 48.0;
/// Standard peer/status chip height (28 px).
pub const CHIP_HEIGHT: f32 = 28.0;
/// Bounded max height for the Peers panel before vertical scroll (320 px).
pub const PEER_PANEL_MAX_HEIGHT: f32 = 320.0;

/// Comfortable large display.
pub const VIEWPORT_LG_WIDTH: f32 = 1440.0;
pub const VIEWPORT_LG_HEIGHT: f32 = 900.0;

/// Full HD.
pub const VIEWPORT_XL_WIDTH: f32 = 1920.0;
pub const VIEWPORT_XL_HEIGHT: f32 = 1080.0;

/// Maximum content width for prose/chat panels before centering.
pub const CONTENT_MAX_WIDTH: f32 = 720.0;

/// Maximum content width for the home dashboard before centering
/// (UI-HOME-02: plan asks for ~1440–1520 px).
pub const DASHBOARD_MAX_WIDTH: f32 = 1480.0;

/// Returns a sidebar width clamped to the allowed range for a given window width.
/// At the reference viewport (1280 px) this returns the target 304 px.
pub fn sidebar_width_for(window_width: f32) -> f32 {
    let fraction = (window_width - VIEWPORT_MIN_WIDTH) / (VIEWPORT_REF_WIDTH - VIEWPORT_MIN_WIDTH);
    let clamped_fraction = fraction.clamp(0.0, 1.0);
    SIDEBAR_WIDTH_MIN + (SIDEBAR_WIDTH - SIDEBAR_WIDTH_MIN) * clamped_fraction
}

/// Returns true when the window width is at or below the compact threshold.
pub fn is_compact(width: f32) -> bool {
    width <= VIEWPORT_MIN_WIDTH
}

/// Returns true when the window width is between compact and reference thresholds.
pub fn is_medium(width: f32) -> bool {
    width > VIEWPORT_MIN_WIDTH && width < VIEWPORT_REF_WIDTH
}

/// Returns true when the window width is at or above the large threshold.
pub fn is_large(width: f32) -> bool {
    width >= VIEWPORT_LG_WIDTH
}

/// Available content width of the home dashboard (window width minus the
/// sidebar, the 1 px divider, and both horizontal page paddings).
///
/// Home responsive breakpoints (UI-HOME-15) are expressed in this content
/// width rather than the raw window width so the fixed 288–320 px sidebar
/// never starves the dashboard: at 1280 window, sidebar 304 + divider 1 +
/// padding 56 leaves 919 px of dashboard, not 1280.
pub fn home_content_width(window_width: f32) -> f32 {
    let sidebar = sidebar_width_for(window_width);
    let h_padding = if is_large(window_width) { SPACE_32 } else { SPACE_28 };
    (window_width - sidebar - 1.0 - 2.0 * h_padding).max(0.0)
}

// ── Home dashboard content-width breakpoints (UI-HOME-15) ─────────────
// All values are in *content* width (see `home_content_width`), so they are
// robust to the sidebar's responsive 288–320 px width.

/// Below this content width the right rail stacks under the main column
/// (narrow band). Above it the dashboard keeps two columns.
pub const HOME_TWO_COL_CONTENT: f32 = 720.0;

/// Below this content width quick actions collapse to one card per row
/// (minimum supported width). Above it a two-by-two grid is used.
pub const HOME_QUICK_ONE_COL_CONTENT: f32 = 520.0;

/// Above this content width quick actions use four columns (wide band).
pub const HOME_QUICK_FOUR_COL_CONTENT: f32 = 1000.0;

/// Above this content width the hero mesh illustration renders at full
/// size; between this and [`HOME_ILLUSTRATION_HIDE_CONTENT`] it is scaled
/// down; below it is hidden entirely so the connection text keeps room.
pub const HOME_ILLUSTRATION_FULL_CONTENT: f32 = 720.0;

/// Below this content width the hero mesh illustration is hidden.
///
/// Aligned with [`HOME_QUICK_ONE_COL_CONTENT`]: below the minimum supported
/// width (one quick action per row) the illustration would only crowd the
/// connection text, so it is removed entirely.
pub const HOME_ILLUSTRATION_HIDE_CONTENT: f32 = 520.0;

/// Below this content width card headers switch to a two-line compact
/// layout (title line, then badges/action line) and the page header stacks
/// the status pill under the greeting instead of beside it.
pub const HOME_COMPACT_HEADER_CONTENT: f32 = 560.0;

// ── Shadow tokens ─────────────────────────────────────────────────────

/// Subtle card shadow — rgba(0,0,0,0.05) offset(0,1) blur(2).
pub fn shadow_card(theme: &Theme) -> iced::Shadow {
    let _ = theme;
    iced::Shadow {
        color: Color::from_rgba(0.0, 0.0, 0.0, 0.05),
        offset: iced::Vector::new(0.0, 1.0),
        blur_radius: 2.0,
    }
}

/// Elevated shadow — rgba(0,0,0,0.04) offset(0,8) blur(24).
pub fn shadow_elevated(theme: &Theme) -> iced::Shadow {
    let _ = theme;
    iced::Shadow {
        color: Color::from_rgba(0.0, 0.0, 0.0, 0.04),
        offset: iced::Vector::new(0.0, 8.0),
        blur_radius: 24.0,
    }
}

/// Dialog / popover shadow — rgba(0,0,0,0.20) offset(0,4) blur(12).
pub fn shadow_dialog(theme: &Theme) -> iced::Shadow {
    let _ = theme;
    iced::Shadow {
        color: Color::from_rgba(0.0, 0.0, 0.0, 0.20),
        offset: iced::Vector::new(0.0, 4.0),
        blur_radius: 12.0,
    }
}

// ── Theme helpers ─────────────────────────────────────────────────────
fn dark(theme: &Theme) -> bool {
    matches!(theme, Theme::Dark)
}

// ── Colour accessors (theme-aware) ────────────────────────────────────

/// Canvas — main panel background. Spec: #F7F9F8.
pub fn color_canvas(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.10, 0.10, 0.18)
    } else {
        CANVAS
    }
}

/// Sidebar background. Spec: #FCFDFC.
pub fn color_sidebar(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.16, 0.16, 0.24)
    } else {
        SIDEBAR
    }
}

/// Surface — card, dialog, white panel. Spec: #FFFFFF.
pub fn surface(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.16, 0.16, 0.24)
    } else {
        SURFACE
    }
}

/// Selected surface — highlighted row/selection background. Spec: #EDF7F1.
pub fn surface_selected(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.16, 0.23, 0.34)
    } else {
        SURFACE_SELECTED
    }
}

/// Surface hover state. Derived: slightly darker than canvas.
pub fn surface_hover(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.20, 0.20, 0.30)
    } else {
        Color::from_rgb(
            0xEF as f32 / 255.0,
            0xF3 as f32 / 255.0,
            0xF1 as f32 / 255.0,
        )
    }
}

/// Standard border. Spec: #DCE5DF.
pub fn border_muted(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.22, 0.22, 0.32)
    } else {
        BORDER_COLOR
    }
}

/// Stronger border for emphasis. Spec: #C8D7CE.
pub fn border_strong(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.28, 0.28, 0.38)
    } else {
        BORDER_STRONG
    }
}

/// Primary body text. Spec: #17211B.  Contrast ≥ 12.5:1 on canvas.
pub fn text_primary(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.80, 0.80, 0.80)
    } else {
        TEXT_PRIMARY
    }
}

/// Secondary text for supporting labels. Spec: #5F6F66.
pub fn text_secondary(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.60, 0.60, 0.60)
    } else {
        TEXT_SECONDARY
    }
}

/// Muted / tertiary text. Spec: #8A978F (darkened to #64706A for WCAG AA).
pub fn text_muted(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.60, 0.60, 0.60)
    } else {
        TEXT_MUTED
    }
}

/// Primary brand accent (green). Spec: #188C50 (darkened to #187F50 for WCAG AA).
pub fn primary(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.29, 0.62, 1.0)
    } else {
        PRIMARY
    }
}

/// Primary hover state. Spec: #147643.
pub fn primary_hover(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.36, 0.70, 1.0)
    } else {
        PRIMARY_HOVER
    }
}

/// Primary pressed state. Derived: darker than hover.
pub fn primary_pressed(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.24, 0.52, 0.86)
    } else {
        Color::from_rgb(
            0x10 as f32 / 255.0,
            0x5F as f32 / 255.0,
            0x38 as f32 / 255.0,
        )
    }
}

/// Primary soft background for subtle accents. Spec: #EAF5EE.
pub fn primary_soft(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgba(0.15, 0.30, 0.15, 0.40)
    } else {
        PRIMARY_SOFT
    }
}

/// Online / success green. Spec: #20A661 (darkened to #1A7F48 for WCAG AA).
pub fn color_success(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.24, 0.86, 0.52)
    } else {
        SUCCESS
    }
}

/// Destructive / error red. Spec: #C84E4E.
pub fn color_danger(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.90, 0.25, 0.25)
    } else {
        DANGER
    }
}

/// Destructive soft background (8 % opacity on danger red).
/// Used for inline confirmation banners and destructive action previews.
/// Light: rgba(200,78,78,0.08); Dark: rgba(230,64,64,0.12).
pub fn destructive_soft(theme: &Theme) -> Color {
    let d = color_danger(theme);
    let a = if dark(theme) { 0.12 } else { 0.08 };
    Color::from_rgba(d.r, d.g, d.b, a)
}

/// Success soft background (8 % opacity on success green).
/// Used for status badges and positive emphasis surfaces (mirrors
/// `destructive_soft` so the card status palette stays symmetric).
pub fn success_soft(theme: &Theme) -> Color {
    let s = color_success(theme);
    let a = if dark(theme) { 0.12 } else { 0.08 };
    Color::from_rgba(s.r, s.g, s.b, a)
}

/// Warning soft background (8 % opacity on warning amber).
/// Used for status badges and caution surfaces (mirrors
/// `destructive_soft` so the card status palette stays symmetric).
pub fn warning_soft(theme: &Theme) -> Color {
    let w = color_warning(theme);
    let a = if dark(theme) { 0.12 } else { 0.08 };
    Color::from_rgba(w.r, w.g, w.b, a)
}

/// Keyboard focus ring color. Spec: #2B9B67.
pub fn color_focus(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.40, 0.70, 0.40)
    } else {
        FOCUS
    }
}

/// Warning / amber colour for reconnecting states.
pub fn color_warning(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.95, 0.65, 0.15)
    } else {
        WARNING
    }
}

/// Input field background.
pub fn bg_input(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.13, 0.13, 0.22)
    } else {
        INPUT_BG
    }
}

/// Modal dialog backdrop — dims the content behind a centred dialog.
/// Matches the help-panel overlay spec (DESIGN_SYSTEM.md §4.9):
/// light `rgba(0,0,0,0.35)`, dark `rgba(0,0,0,0.55)`.
pub fn dialog_backdrop(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgba(0.0, 0.0, 0.0, 0.55)
    } else {
        Color::from_rgba(0.0, 0.0, 0.0, 0.35)
    }
}

/// Dialog panel style — elevated surface with dialog shadow.
pub fn dialog_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(surface(theme))),
        border: iced::Border {
            color: border_muted(theme),
            width: BORDER_WIDTH,
            radius: RADIUS_LG.into(),
        },
        shadow: shadow_dialog(theme),
        ..Default::default()
    }
}

// ── Deprecated aliases (backward compat during migration) ────────────
// These keep existing app.rs code compiling. New code should use the
// canonical names above.

/// Color for local (self) message label.
pub fn text_local_label(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.2, 0.8, 0.2)
    } else {
        Color::from_rgb(0.0, 0.45, 0.0)
    }
}

/// Color for local message body text.
pub fn text_local_body(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.3, 0.9, 0.3)
    } else {
        Color::from_rgb(0.0, 0.35, 0.0)
    }
}

/// Color for remote message label (nickname).
pub fn text_remote_label(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.4, 0.65, 1.0)
    } else {
        Color::from_rgb(0.0, 0.33, 0.66)
    }
}

/// Color for remote message body text.
pub fn text_remote_body(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.8, 0.8, 0.8)
    } else {
        TEXT_PRIMARY
    }
}

/// Background tint for message bubbles. System messages get no bubble.
///
/// Spec (plan §4): incoming bubbles use the `surface` token (white in light
/// mode); outgoing bubbles use `primary_soft` (#EAF5EE).  The surface colour
/// is what separates an incoming bubble from the canvas, so incoming bubbles
/// also carry a 1 px border via [`bubble_border`].
pub fn bubble_bg(theme: &Theme, is_local: bool, is_system: bool) -> Option<Background> {
    if is_system {
        return None;
    }
    let color = if is_local {
        primary_soft(theme)
    } else {
        surface(theme)
    };
    Some(Background::Color(color))
}

/// Border for message bubbles (1 px, 12 px radius).
///
/// Incoming bubbles get a subtle `border_muted` outline (spec: white bubbles
/// with border/subtle elevation).  Outgoing bubbles rely on the soft-green
/// `primary_soft` surface and carry no border, except a failed outgoing
/// message which gets a `danger` border so the error is never communicated by
/// colour alone (the metadata text and label icon also convey it).
pub fn bubble_border(
    theme: &Theme,
    is_local: bool,
    is_system: bool,
    failed: bool,
) -> Option<Border> {
    if is_system {
        return None;
    }
    if is_local && !failed {
        return None;
    }
    Some(Border {
        color: if failed {
            color_danger(theme)
        } else {
            border_muted(theme)
        },
        width: BORDER_WIDTH,
        radius: RADIUS_LG.into(),
        ..Default::default()
    })
}

// ── Backward-compatible aliases for existing callers ─────────────────
// Code that still calls `app_background(theme)`, `text(theme)`,
// `border(theme)`, `online(theme)`, `destructive(theme)`, `selected_surface(theme)`,
// `surface_secondary(theme)` keeps compiling during the migration.

/// @deprecated use `color_canvas(theme)` instead.
pub fn app_background(theme: &Theme) -> Color {
    color_canvas(theme)
}

/// @deprecated use `text_primary(theme)` instead.
pub fn text(theme: &Theme) -> Color {
    text_primary(theme)
}

/// @deprecated use `border_muted(theme)` instead.
pub fn border(theme: &Theme) -> Color {
    border_muted(theme)
}

/// @deprecated use `color_success(theme)` instead.
pub fn online(theme: &Theme) -> Color {
    color_success(theme)
}

/// @deprecated use `color_danger(theme)` instead.
pub fn destructive(theme: &Theme) -> Color {
    color_danger(theme)
}

/// @deprecated use `surface_selected(theme)` instead.
pub fn selected_surface(theme: &Theme) -> Color {
    surface_selected(theme)
}

/// @deprecated use `surface_hover(theme)` instead (different hue).
pub fn surface_secondary(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.13, 0.13, 0.22)
    } else {
        Color::from_rgb(
            0xEE as f32 / 255.0,
            0xF1 as f32 / 255.0,
            0xEE as f32 / 255.0,
        )
    }
}

// ── Style helpers ─────────────────────────────────────────────────────

/// Focus ring border style.
pub fn focus_border(theme: &Theme) -> iced::Border {
    iced::Border {
        color: color_focus(theme),
        width: FOCUS_WIDTH,
        radius: RADIUS_SM.into(),
    }
}

/// Surface container style — white bg, standard border, rounded corners.
pub fn surface_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(surface(theme))),
        border: iced::Border {
            color: border_muted(theme),
            width: BORDER_WIDTH,
            radius: RADIUS_SM.into(),
        },
        ..Default::default()
    }
}

/// Card container style — surface bg, subtle border, rounded corners,
/// light drop shadow. This is the shared dashboard-card surface: every
/// card on the home screen (hero, mesh, rail, quick actions) derives from
/// it so surfaces, borders, radii and shadows stay consistent.
pub fn card_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(surface(theme))),
        border: iced::Border {
            color: border_muted(theme),
            width: BORDER_WIDTH,
            radius: RADIUS_CARD.into(),
        },
        shadow: shadow_card(theme),
        ..Default::default()
    }
}

/// Elevated container style — for dialogs, popovers, modals.
pub fn elevated_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(surface(theme))),
        border: iced::Border {
            color: border_muted(theme),
            width: BORDER_WIDTH,
            radius: RADIUS_LG.into(),
        },
        shadow: shadow_elevated(theme),
        ..Default::default()
    }
}

/// Icon button style — transparent bg, themed text/hover.
pub fn icon_button(theme: &Theme, status: button::Status) -> button::Style {
    let color = match status {
        button::Status::Hovered => primary(theme),
        button::Status::Pressed => primary_pressed(theme),
        button::Status::Disabled => text_muted(theme),
        _ => text_secondary(theme),
    };
    button::Style {
        background: match status {
            button::Status::Hovered => Some(Background::Color(surface_hover(theme))),
            button::Status::Pressed => Some(Background::Color(surface_selected(theme))),
            _ => None,
        },
        text_color: color,
        border: iced::Border {
            radius: RADIUS_SM.into(),
            color: if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                primary(theme)
            } else {
                iced::Color::TRANSPARENT
            },
            width: if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                1.0
            } else {
                0.0
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Palette verification ──────────────────────────────────────────

    #[test]
    fn spec_palette_matches_hex_values() {
        let theme = Theme::Light;

        // Backgrounds
        assert_eq!(color_canvas(&theme), CANVAS);
        assert_eq!(color_sidebar(&theme), SIDEBAR);
        assert_eq!(surface(&theme), SURFACE);
        assert_eq!(surface_selected(&theme), SURFACE_SELECTED);

        // Text
        assert_eq!(text_primary(&theme), TEXT_PRIMARY);
        assert_eq!(text_secondary(&theme), TEXT_SECONDARY);
        assert_eq!(text_muted(&theme), TEXT_MUTED);

        // Borders
        assert_eq!(border_muted(&theme), BORDER_COLOR);
        assert_eq!(border_strong(&theme), BORDER_STRONG);

        // Accents
        assert_eq!(primary(&theme), PRIMARY);
        assert_eq!(primary_hover(&theme), PRIMARY_HOVER);
        assert_eq!(primary_soft(&theme), PRIMARY_SOFT);
        assert_eq!(color_success(&theme), SUCCESS);
        assert_eq!(color_danger(&theme), DANGER);
        assert_eq!(color_focus(&theme), FOCUS);
    }

    #[test]
    fn contrast_ratios_pass_wcag_aa() {
        // Verify body text contrast ≥ 4.5:1 on canvas (rough check
        // via luminance — actual WCAG calculation is more precise, but this
        // catches regressions in the raw values).
        let canvas = CANVAS;
        let primary = TEXT_PRIMARY;

        // Relative luminance helper
        fn relative_luminance(c: Color) -> f32 {
            let linearize = |ch: f32| -> f32 {
                if ch <= 0.04045 {
                    ch / 12.92
                } else {
                    ((ch + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linearize(c.r) + 0.7152 * linearize(c.g) + 0.0722 * linearize(c.b)
        }

        fn contrast_ratio(a: Color, b: Color) -> f32 {
            let l1 = relative_luminance(a);
            let l2 = relative_luminance(b);
            let (lighter, darker) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
            (lighter + 0.05) / (darker + 0.05)
        }

        // Body text (≥ 4.5:1 AA for normal text)
        let body_on_canvas = contrast_ratio(primary, canvas);
        assert!(
            body_on_canvas >= 4.5,
            "text_primary on canvas: {:.1}:1 (need ≥ 4.5:1)",
            body_on_canvas
        );

        // Secondary text (≥ 4.5:1 AA for normal text)
        let secondary_on_canvas = contrast_ratio(TEXT_SECONDARY, canvas);
        assert!(
            secondary_on_canvas >= 4.5,
            "text_secondary on canvas: {:.1}:1 (need ≥ 4.5:1)",
            secondary_on_canvas
        );

        // Primary on canvas for buttons (≥ 3:1 for large text)
        let primary_on_canvas = contrast_ratio(PRIMARY, canvas);
        assert!(
            primary_on_canvas >= 3.0,
            "primary on canvas: {:.1}:1",
            primary_on_canvas
        );

        // Muted text — spec value #8A978F was below AA; UI-19 darkened the
        // token to #64706A so muted text passes WCAG AA on every light
        // surface (white, canvas, sidebar, soft-green bubble, selected).
        for (name, bg) in [
            ("white", Color::WHITE),
            ("canvas", CANVAS),
            ("sidebar", SIDEBAR),
            ("soft-green bubble", PRIMARY_SOFT),
            ("selected surface", SURFACE_SELECTED),
        ] {
            let muted_on = contrast_ratio(TEXT_MUTED, bg);
            assert!(
                muted_on >= 4.5,
                "text_muted on {name}: {:.1}:1 (need ≥ 4.5:1)",
                muted_on
            );
        }

        // Primary on white for button text / accents (≥ 4.5:1 normal text).
        let primary_on_white = contrast_ratio(PRIMARY, Color::WHITE);
        assert!(
            primary_on_white >= 4.5,
            "primary on white: {:.1}:1 (need ≥ 4.5:1)",
            primary_on_white
        );

        // Success (online green) on white — used for status label text as
        // well as dots, so it must pass normal-text AA.
        let success_on_white = contrast_ratio(SUCCESS, Color::WHITE);
        assert!(
            success_on_white >= 4.5,
            "success on white: {:.1}:1 (need ≥ 4.5:1)",
            success_on_white
        );

        // Focus ring — non-text UI component, WCAG 1.4.11 requires ≥ 3:1
        // against the adjacent surface.
        for (name, bg) in [
            ("white", Color::WHITE),
            ("canvas", CANVAS),
            ("sidebar", SIDEBAR),
            ("soft-green bubble", PRIMARY_SOFT),
            ("selected surface", SURFACE_SELECTED),
        ] {
            let focus_on = contrast_ratio(FOCUS, bg);
            assert!(
                focus_on >= 3.0,
                "focus ring on {name}: {:.1}:1 (need ≥ 3:1)",
                focus_on
            );
        }
    }

    // ── Geometry consistency ───────────────────────────────────────────

    #[test]
    fn spacing_scale_is_monotonic() {
        let scale = [
            SPACE_4, SPACE_8, SPACE_12, SPACE_16, SPACE_20, SPACE_24, SPACE_32, SPACE_40,
        ];
        for w in scale.windows(2) {
            assert!(
                w[1] > w[0],
                "spacing scale not monotonic: {} → {}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn all_spacing_is_positive() {
        for s in [
            SPACE_4, SPACE_8, SPACE_12, SPACE_16, SPACE_20, SPACE_24, SPACE_32, SPACE_40,
        ] {
            assert!(s > 0.0, "spacing {s} must be positive");
        }
    }

    #[test]
    fn radii_are_ordered() {
        assert!(RADIUS_XL >= RADIUS_LG);
        assert!(RADIUS_LG >= RADIUS_MD);
        assert!(RADIUS_MD >= RADIUS_SM);
        assert!(RADIUS_SM > 0.0);
    }

    #[test]
    fn control_heights_are_reasonable() {
        assert!((32.0..=48.0).contains(&CONTROL_HEIGHT));
        assert!((28.0..=44.0).contains(&CONTROL_HEIGHT_COMPACT));
    }

    #[test]
    fn layout_tokens_are_positive() {
        assert!(SIDEBAR_WIDTH > 0.0);
        assert!(DETAILS_PANEL_WIDTH > 0.0);
        assert!(MESSAGE_MAX_WIDTH > 0.0);
        assert!(IMAGE_PREVIEW_MAX_WIDTH > 0.0);
        assert!(IMAGE_PREVIEW_MAX_HEIGHT > 0.0);
        assert!(AVATAR_SM > 0.0);
        assert!(AVATAR_MD > 0.0);
        assert!(AVATAR_LG > 0.0);
    }

    // ── Responsive thresholds ──────────────────────────────────────────

    #[test]
    fn viewport_thresholds_are_monotonic() {
        assert!(VIEWPORT_XL_WIDTH > VIEWPORT_LG_WIDTH);
        assert!(VIEWPORT_LG_WIDTH > VIEWPORT_REF_WIDTH);
        assert!(VIEWPORT_REF_WIDTH > VIEWPORT_MIN_WIDTH);
        assert!(VIEWPORT_XL_HEIGHT > VIEWPORT_LG_HEIGHT);
        assert!(VIEWPORT_LG_HEIGHT > VIEWPORT_REF_HEIGHT);
        assert!(VIEWPORT_REF_HEIGHT > VIEWPORT_MIN_HEIGHT);
        assert!(CONTENT_MAX_WIDTH < VIEWPORT_REF_WIDTH);
        // UI-HOME-02: dashboard max width sits in the plan's 1440–1520 px band.
        assert!((1440.0..=1520.0).contains(&DASHBOARD_MAX_WIDTH));
    }

    #[test]
    fn is_compact_and_is_large_are_mutually_exclusive() {
        assert!(!(is_compact(VIEWPORT_MIN_WIDTH) && is_large(VIEWPORT_MIN_WIDTH)));
        assert!(is_compact(VIEWPORT_MIN_WIDTH));
        assert!(!is_compact(VIEWPORT_REF_WIDTH));
        assert!(is_large(VIEWPORT_LG_WIDTH));
        assert!(!is_large(VIEWPORT_REF_WIDTH));
    }

    #[test]
    fn sidebar_width_for_clamps_to_allowed_range() {
        // At min viewport (1024 px): should be 288 px.
        assert!((sidebar_width_for(VIEWPORT_MIN_WIDTH) - SIDEBAR_WIDTH_MIN).abs() < 0.5);
        // At reference viewport (1280 px): should be 304 px.
        assert!((sidebar_width_for(VIEWPORT_REF_WIDTH) - SIDEBAR_WIDTH).abs() < 0.5);
        // Below min: clamped to 288 px.
        assert!((sidebar_width_for(800.0) - SIDEBAR_WIDTH_MIN).abs() < 0.5);
        // Above max: clamped to 304 px.
        assert!((sidebar_width_for(VIEWPORT_LG_WIDTH) - SIDEBAR_WIDTH).abs() < 0.5);
        assert!((sidebar_width_for(VIEWPORT_XL_WIDTH) - SIDEBAR_WIDTH).abs() < 0.5);
    }

    #[test]
    fn home_content_width_shrinks_with_the_sidebar() {
        // UI-HOME-15: the dashboard's available width is the window width
        // minus the sidebar, the 1 px divider and both horizontal paddings.
        // At the reference window the sidebar is 304 px, so the content is
        // ~919 px, never the full 1280.
        let content = home_content_width(VIEWPORT_REF_WIDTH);
        assert!(
            (content - (VIEWPORT_REF_WIDTH - SIDEBAR_WIDTH - 1.0 - 2.0 * SPACE_28)).abs() < 0.5,
            "content width at 1280 window must subtract sidebar + divider + padding, got {content}"
        );
        assert!(content < VIEWPORT_REF_WIDTH);
        assert!(content > 0.0);
        // A wide window gives more content width.
        assert!(home_content_width(VIEWPORT_LG_WIDTH) > home_content_width(VIEWPORT_REF_WIDTH));
        // A tiny window never goes negative.
        assert_eq!(home_content_width(0.0), 0.0);
        // No regression in the evidence widths: these are the actual content
        // widths the four-width screenshot set must render at.
        assert!((home_content_width(1600.0) - 1231.0).abs() < 2.0);
        assert!((home_content_width(1280.0) - 919.0).abs() < 2.0);
        assert!((home_content_width(1024.0) - 679.0).abs() < 2.0);
        assert!((home_content_width(800.0) - 455.0).abs() < 2.0);
    }

    #[test]
    fn home_content_breakpoints_are_ordered() {
        // UI-HOME-15: the content-width tiers must be monotonic so every
        // supported window maps to exactly one intentional layout. As width
        // shrinks: four quick actions → two columns → scaled illustration →
        // compact headers → one quick action per row + illustration hidden.
        assert!(HOME_QUICK_FOUR_COL_CONTENT > HOME_TWO_COL_CONTENT);
        assert!(HOME_TWO_COL_CONTENT >= HOME_ILLUSTRATION_FULL_CONTENT);
        assert!(HOME_ILLUSTRATION_FULL_CONTENT > HOME_COMPACT_HEADER_CONTENT);
        assert!(HOME_COMPACT_HEADER_CONTENT > HOME_QUICK_ONE_COL_CONTENT);
        // The illustration hides exactly where quick actions collapse to one
        // per row: below the minimum supported width there is no room for it.
        assert!(HOME_QUICK_ONE_COL_CONTENT >= HOME_ILLUSTRATION_HIDE_CONTENT);
        assert!(HOME_ILLUSTRATION_HIDE_CONTENT > 0.0);
    }

    #[test]
    fn home_breakpoints_map_evidence_widths_to_intentional_tiers() {
        // UI-HOME-15 acceptance: wide / medium / narrow / minimum must each
        // land in a distinct, intentional tier at the four evidence widths.
        let wide = home_content_width(1600.0);
        assert!(wide >= HOME_QUICK_FOUR_COL_CONTENT, "1600 should be wide (4 quick actions)");
        let medium = home_content_width(1280.0);
        assert!(medium >= HOME_TWO_COL_CONTENT && medium < HOME_QUICK_FOUR_COL_CONTENT,
            "1280 should be medium (two columns, 2x2 quick actions)");
        let narrow = home_content_width(1024.0);
        assert!(narrow < HOME_TWO_COL_CONTENT && narrow >= HOME_QUICK_ONE_COL_CONTENT,
            "1024 should be narrow (one column, 2x2 quick actions)");
        let minimum = home_content_width(800.0);
        assert!(minimum < HOME_QUICK_ONE_COL_CONTENT,
            "800 should be minimum (one quick action per row)");
    }

    // ── Shadow token invariants ────────────────────────────────────────

    #[test]
    fn shadow_blur_radii_increase_with_elevation() {
        let theme = Theme::Light;
        let card = shadow_card(&theme);
        let elevated = shadow_elevated(&theme);
        let dialog = shadow_dialog(&theme);

        assert!(card.blur_radius > 0.0);
        assert!(elevated.blur_radius > card.blur_radius);
        // Dialog has higher blur than card
        assert!(dialog.blur_radius > card.blur_radius);
    }

    // ── Backward compatibility ────────────────────────────────────────

    #[test]
    fn deprecated_aliases_map_to_new_tokens() {
        let theme = Theme::Light;
        assert_eq!(app_background(&theme), color_canvas(&theme));
        assert_eq!(text(&theme), text_primary(&theme));
        assert_eq!(border(&theme), border_muted(&theme));
        assert_eq!(online(&theme), color_success(&theme));
        assert_eq!(destructive(&theme), color_danger(&theme));
        assert_eq!(selected_surface(&theme), surface_selected(&theme));
    }

    #[test]
    fn text_local_and_remote_have_distinct_colors() {
        let light = Theme::Light;
        let dark = Theme::Dark;
        assert_ne!(text_local_label(&light), text_remote_label(&light));
        assert_ne!(text_local_body(&light), text_remote_body(&light));
        assert_ne!(text_local_label(&dark), text_remote_label(&dark));
        assert_ne!(text_local_body(&dark), text_remote_body(&dark));
    }

    #[test]
    fn bubble_bg_system_returns_none() {
        let theme = Theme::Light;
        assert!(bubble_bg(&theme, true, true).is_none());
        assert!(bubble_bg(&theme, false, true).is_none());
    }

    #[test]
    fn bubble_bg_local_and_remote_are_some() {
        let theme = Theme::Light;
        assert!(bubble_bg(&theme, true, false).is_some());
        assert!(bubble_bg(&theme, false, false).is_some());
    }

    #[test]
    fn bubble_bg_uses_spec_surfaces() {
        // Light mode: incoming = surface white, outgoing = primary_soft green.
        let light = Theme::Light;
        assert_eq!(
            bubble_bg(&light, false, false),
            Some(Background::Color(surface(&light)))
        );
        assert_eq!(
            bubble_bg(&light, true, false),
            Some(Background::Color(primary_soft(&light)))
        );
        // Dark mode stays distinct and non-transparent-white.
        let dark = Theme::Dark;
        assert_ne!(
            bubble_bg(&dark, false, false),
            Some(Background::Color(Color::WHITE))
        );
        assert!(bubble_bg(&dark, true, false).is_some());
    }

    #[test]
    fn bubble_border_follows_spec_rules() {
        let light = Theme::Light;
        // Incoming (remote) bubbles carry a subtle border.
        let remote = bubble_border(&light, false, false, false).expect("remote border");
        assert_eq!(remote.width, BORDER_WIDTH);
        assert_eq!(remote.color, border_muted(&light));
        assert_eq!(remote.radius, RADIUS_LG.into());
        // Outgoing (local) bubbles have no border unless failed.
        assert!(bubble_border(&light, true, false, false).is_none());
        let failed = bubble_border(&light, true, false, true).expect("failed border");
        assert_eq!(failed.color, color_danger(&light));
        // System messages never get a bubble border.
        assert!(bubble_border(&light, false, true, false).is_none());
        assert!(bubble_border(&light, true, true, true).is_none());
    }

    #[test]
    fn dark_theme_is_different_from_light() {
        let light = Theme::Light;
        let dark = Theme::Dark;
        assert_ne!(primary(&light), primary(&dark));
        assert_ne!(color_canvas(&light), color_canvas(&dark));
        assert_ne!(text_primary(&light), text_primary(&dark));
        assert_ne!(surface(&light), surface(&dark));
    }

    // ── Card foundation tokens (UI-HOME-03) ──────────────────────────

    #[test]
    fn card_radius_token_is_within_the_plan_band() {
        // Plan: dashboard cards use a 14-18 px corner radius.
        assert!(
            (14.0..=18.0).contains(&RADIUS_CARD),
            "RADIUS_CARD must be in the 14-18 px plan band"
        );
    }

    #[test]
    fn card_style_uses_the_card_radius_token() {
        let light = Theme::Light;
        let style = card_style(&light);
        assert_eq!(style.border.radius, RADIUS_CARD.into());
        assert_eq!(style.border.width, BORDER_WIDTH);
        assert_eq!(style.border.color, border_muted(&light));
        assert_eq!(style.background, Some(Background::Color(surface(&light))));
        assert!(
            style.shadow.color.a > 0.0 && style.shadow.color.a < 1.0,
            "card style must carry a low-opacity shadow"
        );
    }

    #[test]
    fn status_soft_colors_are_translucent_token_variants() {
        let light = Theme::Light;
        let dark = Theme::Dark;
        for theme in [&light, &dark] {
            for soft in [
                success_soft(theme),
                warning_soft(theme),
                destructive_soft(theme),
            ] {
                assert!(
                    soft.a > 0.0 && soft.a < 1.0,
                    "status soft colors must be translucent tints"
                );
            }
        }
    }
}
