//! Emoji picker panel (BORU-TWEMOJI-04; SVG rendering BORU-TWEMOJI-10;
//! responsive layout BORU-TWEMOJI-11; category navigation BORU-TWEMOJI-12).
//!
//! The picker moved here from `app/chat.rs` (`IcedChat::view_emoji_picker`).
//! Since BORU-TWEMOJI-10 the grid renders Twemoji SVG artwork through the
//! shared renderer/cache ([`super::renderer`]) instead of system-font
//! glyphs, so supported entries never show missing-glyph/tofu boxes.
//! Selecting an emoji still emits [`crate::app::AppMessage::InsertEmoji`]
//! carrying the full Unicode grapheme string — never an asset key or SVG
//! path — so composer insertion is unchanged.
//!
//! Entries whose grapheme does not resolve to a bundled asset (or whose
//! vendored SVG cannot be loaded) fall back to their original Unicode text
//! per BORU-TWEMOJI-20: unsupported emoji are never hidden or replaced.
//!
//! The emoji list is sourced from [`super::catalog`] so the picker and the
//! future message renderer share one metadata source.
//!
//! # Category navigation (BORU-TWEMOJI-12)
//!
//! The picker no longer shows one undifferentiated list: a tab row above
//! the grid lets the user switch between the 8 content categories
//! ([`EmojiCategory::ALL`]). Each tab is a compact 32 px button carrying the
//! category's Twemoji artwork ([`super::catalog::category_icon`]); the
//! active tab uses Boru's selection styling (`surface_selected` background
//! + primary border). The grid shows exactly [`super::catalog::emojis_for_category`]
//! for the active category, so switching categories rebuilds the grid from
//! the filtered catalog — no stale items can survive a switch. The tab row
//! scrolls horizontally when the card is too narrow to show all 8 tabs.
//! `Recent` stays a reserved pseudo-category ([`EmojiCategory::Recent`],
//! BORU-TWEMOJI-14); [`super::catalog::category_icon`] returns `None` for
//! it, which is the extension point for a later recents tab.
//!
//! # Responsive layout (BORU-TWEMOJI-11)
//!
//! The card is wrapped in [`iced::widget::Responsive`] so the grid column
//! count adapts to the available width instead of a fixed 8-per-row layout:
//! wide windows show up to [`EMOJI_MAX_COLUMNS`] columns (card ≈ 374 px,
//! within the 340–400 px target), narrow windows show fewer columns and the
//! card never exceeds the available width — cells stay a fixed
//! [`EMOJI_CELL_SIZE`] and are never stretched to fill. The scroll region
//! height also adapts: it grows with the grid content up to
//! [`PICKER_MAX_SCROLL`] when the window is tall, and shrinks in short
//! windows so the card never clips. iced 0.14 has no `wrap` widget and
//! `Grid::fluid` computes column counts with `ceil`, which would shrink the
//! fixed-size cells; manual rows inside `Responsive` keep cells exactly
//! 36 px (see [`picker_columns`] / [`picker_card_width`] tests).
//!
//! The card width is at least wide enough for the category tab row
//! ([`category_row_natural_width`] + chrome) when space permits
//! ([`picker_card_width_with_category_row`]), so tabs are never cut off on
//! wide windows; on narrow windows the card caps at the available width and
//! the tab row scrolls instead.

use crate::app::{bg_surface, border_muted, text_muted, AppMessage};
use crate::design_tokens::{SPACE_4, SPACE_6, SPACE_8};
use crate::emoji::catalog::EmojiCategory;
use iced::widget::Responsive;
use iced::{Length, Size};

/// Visual emoji artwork size (PDF Task 10: ~24 px).
const EMOJI_ART_SIZE: f32 = 24.0;
/// Button hit area (PDF Task 10: ~36 x 36 px).
const EMOJI_CELL_SIZE: f32 = 36.0;
/// Gap between grid cells (matches the row/column `spacing`).
const EMOJI_CELL_GAP: f32 = SPACE_4;
/// Maximum columns on a wide window. Card width for N columns is
/// `N*36 + (N-1)*4 + chrome(18)`; N=9 → 374 px (within the 340–400 target),
/// N=10 would be 414 px (over), so 9 is the cap.
const EMOJI_MAX_COLUMNS: usize = 9;
/// Horizontal chrome of the picker Card: 1 px borders each side + 8 px body
/// padding each side.
const PICKER_CHROME_X: f32 = 2.0 + 2.0 * SPACE_8;
/// Vertical chrome of the picker Card: head (18 px title + 8 px padding each
/// side) + 8 px body padding each side + 1 px borders each side.
const PICKER_CHROME_Y: f32 = 58.0;
/// Maximum scroll-region height (card ≈ 58 + 340 = 398 px ≤ 400 target).
const PICKER_MAX_SCROLL: f32 = 340.0;
/// Category tab hit area (Task 12: compact icon tabs, ~32 px).
const CATEGORY_TAB_SIZE: f32 = 32.0;
/// Gap between category tabs (matches the grid spacing).
const CATEGORY_TAB_GAP: f32 = SPACE_4;
/// Category icon artwork size inside the tab (20 px in the 32 px hit area).
const CATEGORY_ICON_SIZE: f32 = 20.0;
/// Vertical chrome the tab row adds to the card body: the 32 px row plus the
/// 8 px body-column spacing below it.
const CATEGORY_ROW_CHROME: f32 = CATEGORY_TAB_SIZE + SPACE_8;

/// Render the emoji picker panel (a Card hosting the category tab row and
/// the active category's emoji grid as SVG buttons; selecting an emoji
/// inserts its Unicode into the composer).
///
/// `active` selects the content category shown in the grid (BORU-TWEMOJI-12).
///
/// The card is wrapped in [`Responsive`] (with `Shrink` width/height) so the
/// column count and scroll height adapt to the space the overlay actually
/// provides. `Responsive` fills its parent by default; `Shrink` makes the
/// node hug the card while the closure still receives the full available
/// [`Size`], so the existing bottom-right overlay alignment is preserved.
pub fn view_emoji_picker(
    theme: &iced::Theme,
    active: EmojiCategory,
) -> iced::Element<'static, AppMessage> {
    let btheme = crate::theme::BoruTheme::for_theme(theme);
    // ChatTheme is Copy; captured by value so the Responsive closure is
    // 'static (no borrow of the caller's theme).
    let chat = btheme.chat;
    let head_color = text_muted(theme);

    Responsive::new(move |size: Size| {
        use iced::widget::column;

        let columns = picker_columns(size.width);
        let card_width = picker_card_width_with_category_row(columns, size.width);

        let renderer = crate::emoji::renderer::TwemojiRenderer;
        // The grid is rebuilt from the filtered catalog on every frame, so a
        // category switch (a new `active`) immediately replaces the visible
        // entries — stale items from the previous category cannot survive.
        let entries: Vec<&crate::emoji::catalog::Emoji> =
            crate::emoji::catalog::emojis_for_category(active).collect();
        let scroll_height = picker_scroll_height(
            columns,
            size.height,
            chat.emoji_picker_scroll_height,
            entries.len(),
        );

        let head = crate::fonts::type_role_text(
            crate::fonts::TypeRole::CardTitle,
            crate::i18n::t("emoji.title"),
        )
        .color(head_color);

        let mut grid = column![].spacing(SPACE_4);
        for chunk in entries.chunks(columns) {
            let mut r = iced::widget::row![].spacing(SPACE_4);
            for emoji in chunk {
                r = r.push(emoji_cell(&renderer, emoji));
            }
            grid = grid.push(r);
        }

        let scroll =
            crate::ui_components::gutter_scrollable(grid).height(Length::Fixed(scroll_height));

        // Tab row stays fixed above the scrolling grid.
        let body = column![category_tab_row(&renderer, active), scroll].spacing(SPACE_8);

        iced_aw::Card::new(head, body)
            .width(Length::Fixed(card_width))
            .padding_head(iced::Padding::new(SPACE_8))
            .padding_body(iced::Padding::new(SPACE_8))
            .on_close(AppMessage::ToggleEmojiPicker)
            .style(move |t, _status| {
                let b = crate::theme::BoruTheme::for_theme(t);
                iced_aw::style::card::Style {
                    background: iced::Background::Color(bg_surface(t)),
                    border_radius: b.radii.sm,
                    border_width: b.borders.hairline,
                    border_color: border_muted(t),
                    head_background: iced::Background::Color(bg_surface(t)),
                    head_text_color: text_muted(t),
                    body_background: iced::Background::Color(iced::Color::TRANSPARENT),
                    body_text_color: text_muted(t),
                    foot_background: iced::Background::Color(iced::Color::TRANSPARENT),
                    foot_text_color: text_muted(t),
                    close_color: text_muted(t),
                }
            })
            .into()
    })
    .width(Length::Shrink)
    .height(Length::Shrink)
    .into()
}

/// Number of grid columns that fit the available width without stretching
/// cells or overflowing the card (BORU-TWEMOJI-11).
///
/// Each cell is [`EMOJI_CELL_SIZE`] + [`EMOJI_CELL_GAP`] of pitch; N cells
/// need `N*36 + (N-1)*4` px, plus [`PICKER_CHROME_X`] of card chrome. The
/// floor division guarantees the natural card width (see
/// [`picker_card_width`]) never exceeds the available width, so narrow
/// windows get fewer columns and nothing clips.
fn picker_columns(available_width: f32) -> usize {
    let grid_w = (available_width - PICKER_CHROME_X).max(0.0);
    let n = ((grid_w + EMOJI_CELL_GAP) / (EMOJI_CELL_SIZE + EMOJI_CELL_GAP)).floor() as usize;
    n.clamp(1, EMOJI_MAX_COLUMNS)
}

/// Natural card width for a column count, capped at the available width so
/// the card never overflows the overlay (no horizontal scroll / clipping).
fn picker_card_width(columns: usize, available_width: f32) -> f32 {
    let natural = columns as f32 * EMOJI_CELL_SIZE
        + (columns as f32 - 1.0) * EMOJI_CELL_GAP
        + PICKER_CHROME_X;
    natural.min(available_width)
}

/// Natural width of the 8-tab category row (BORU-TWEMOJI-12).
///
/// `8 * 32 + 7 * 4 = 284` px. The card is at least this wide plus chrome
/// when space permits ([`picker_card_width_with_category_row`]), so all
/// tabs are visible on a typical picker width.
fn category_row_natural_width() -> f32 {
    EmojiCategory::ALL.len() as f32 * CATEGORY_TAB_SIZE
        + (EmojiCategory::ALL.len() as f32 - 1.0) * CATEGORY_TAB_GAP
}

/// Card width that also fits the category tab row (BORU-TWEMOJI-12).
///
/// The grid's natural width and the tab row's natural width (plus chrome)
/// both matter: the card is at least wide enough for the whole tab row when
/// space permits, but never wider than the available width — a narrow
/// window caps the card and the tab row scrolls horizontally instead of
/// clipping.
fn picker_card_width_with_category_row(columns: usize, available_width: f32) -> f32 {
    let grid = picker_card_width(columns, available_width);
    let row = (category_row_natural_width() + PICKER_CHROME_X).min(available_width);
    grid.max(row)
}

/// Scroll-region height for the chosen column count, visible entry count
/// and available height (BORU-TWEMOJI-11, category-aware since
/// BORU-TWEMOJI-12).
///
/// The region is tall enough for the full grid content (all rows of the
/// active category at this column count), at least the theme token's scroll
/// height, never taller than [`PICKER_MAX_SCROLL`] (keeps the card ≤ ~400 px
/// when space permits), and never taller than what actually fits the window
/// (short windows shrink the region instead of clipping the card). The
/// category tab row sits above the region, so `CATEGORY_ROW_CHROME` is
/// subtracted from the available height.
fn picker_scroll_height(columns: usize, available_height: f32, token: f32, entries: usize) -> f32 {
    let rows = entries.div_ceil(columns);
    let content_h = rows as f32 * EMOJI_CELL_SIZE + (rows as f32 - 1.0) * EMOJI_CELL_GAP;
    let fits = (available_height - PICKER_CHROME_Y - CATEGORY_ROW_CHROME).max(0.0);
    content_h.max(token).min(PICKER_MAX_SCROLL).min(fits)
}

/// The category tab row: one compact icon button per content category,
/// scrollable horizontally when the card is too narrow to show all 8
/// (BORU-TWEMOJI-12).
///
/// The row's own width is its natural width ([`category_row_natural_width`])
/// so the horizontal scrollable only kicks in when the card body is
/// narrower than that (narrow picker widths) — wide cards show all tabs
/// with no scrollbar.
fn category_tab_row(
    renderer: &impl crate::emoji::renderer::EmojiRenderer,
    active: EmojiCategory,
) -> iced::Element<'static, AppMessage> {
    use iced::widget::{row, scrollable};

    let mut tabs = row![].spacing(CATEGORY_TAB_GAP).width(Length::Shrink);
    for category in EmojiCategory::ALL {
        tabs = tabs.push(category_tab(renderer, category, active));
    }

    scrollable(tabs)
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::default().spacing(SPACE_4),
        ))
        .height(Length::Fixed(CATEGORY_TAB_SIZE))
        .into()
}

/// One category tab: a 32x32 button whose content is the category's Twemoji
/// artwork (20 px) when the shared renderer resolves the icon, otherwise the
/// icon's Unicode text (BORU-TWEMOJI-20 fallback). The active category uses
/// Boru's selection styling — `surface_selected` background + primary
/// border — so the current tab is unmistakable.
fn category_tab(
    renderer: &impl crate::emoji::renderer::EmojiRenderer,
    category: EmojiCategory,
    active: EmojiCategory,
) -> iced::Element<'static, AppMessage> {
    use iced::widget::{button, svg, text};
    use iced::{ContentFit, Length};

    let artwork: iced::Element<'static, AppMessage> =
        match crate::emoji::catalog::category_icon(category) {
            Some(icon) => match cell_artwork(renderer, icon) {
                CellArtwork::Svg(handle) => svg(handle)
                    .width(Length::Fixed(CATEGORY_ICON_SIZE))
                    .height(Length::Fixed(CATEGORY_ICON_SIZE))
                    .content_fit(ContentFit::Contain)
                    .into(),
                CellArtwork::Text => text(icon.unicode).size(16.0).into(),
            },
            // Extension point for BORU-TWEMOJI-14: a pseudo-category
            // without artwork (Recent) falls back to a themed glyph.
            None => text("🕐").size(16.0).into(),
        };

    button(artwork)
        .on_press(AppMessage::SelectEmojiCategory(category))
        .width(Length::Fixed(CATEGORY_TAB_SIZE))
        .height(Length::Fixed(CATEGORY_TAB_SIZE))
        // 6 px padding centres the 20 px artwork inside the 32 px hit area.
        .padding(SPACE_6)
        .style(move |t, status| category_tab_style(t, status, category == active))
        .into()
}

/// Boru-consistent tab styling: the active tab gets the selection
/// background (`surface_selected`) and a primary border; inactive tabs are
/// transparent with a hover/press tint. Mirrors the selection styling used
/// elsewhere in Boru (e.g. `TabStrip::tab_button_style`).
fn category_tab_style(
    theme: &iced::Theme,
    status: iced::widget::button::Status,
    active: bool,
) -> iced::widget::button::Style {
    use iced::widget::button::{Status, Style};
    let background = if active {
        Some(iced::Background::Color(
            crate::design_tokens::surface_selected(theme),
        ))
    } else {
        match status {
            Status::Hovered => Some(iced::Background::Color(
                crate::design_tokens::surface_hover(theme),
            )),
            Status::Pressed => Some(iced::Background::Color(
                crate::design_tokens::surface_selected(theme),
            )),
            _ => None,
        }
    };
    let border = if active {
        iced::Border {
            color: crate::design_tokens::primary(theme),
            radius: SPACE_4.into(),
            width: 1.0,
        }
    } else {
        iced::Border {
            radius: SPACE_4.into(),
            ..Default::default()
        }
    };
    Style {
        background,
        text_color: crate::design_tokens::text_primary(theme),
        border,
        ..Default::default()
    }
}

/// Decide the artwork for a picker cell: the Twemoji SVG handle when the
/// shared renderer resolves the grapheme to a bundled asset that can be
/// loaded, else `Text` (original Unicode fallback, BORU-TWEMOJI-20).
enum CellArtwork {
    Svg(iced::widget::svg::Handle),
    Text,
}

/// Resolve the artwork decision for a catalog emoji through the shared
/// renderer/cache. Kept separate from the widget construction so the
/// supported/fallback behaviour is unit-testable.
fn cell_artwork(
    renderer: &impl crate::emoji::renderer::EmojiRenderer,
    emoji: &crate::emoji::catalog::Emoji,
) -> CellArtwork {
    match renderer
        .resolve(emoji.unicode)
        .and_then(|a| renderer.svg_handle(&a))
    {
        Some(handle) => CellArtwork::Svg(handle),
        None => CellArtwork::Text,
    }
}

/// Build the `InsertEmoji` message for a catalog emoji. Always carries the
/// full Unicode grapheme string — never the asset key or an SVG path — so
/// composer insertion and message content stay plain Unicode.
fn insert_message(emoji: &crate::emoji::catalog::Emoji) -> AppMessage {
    AppMessage::InsertEmoji(emoji.unicode.to_string())
}

/// One picker cell: a 36x36 button whose content is the 24px Twemoji SVG
/// artwork when the shared renderer resolves the grapheme, otherwise the
/// original Unicode text (BORU-TWEMOJI-20 fallback — never suppress).
fn emoji_cell(
    renderer: &impl crate::emoji::renderer::EmojiRenderer,
    emoji: &crate::emoji::catalog::Emoji,
) -> iced::Element<'static, AppMessage> {
    use iced::widget::{button, svg, text};
    use iced::{ContentFit, Length};

    let artwork: iced::Element<'static, AppMessage> = match cell_artwork(renderer, emoji) {
        CellArtwork::Svg(handle) => svg(handle)
            .width(Length::Fixed(EMOJI_ART_SIZE))
            .height(Length::Fixed(EMOJI_ART_SIZE))
            .content_fit(ContentFit::Contain)
            .into(),
        CellArtwork::Text => text(emoji.unicode).size(20.0).into(),
    };

    button(artwork)
        .on_press(insert_message(emoji))
        .width(Length::Fixed(EMOJI_CELL_SIZE))
        .height(Length::Fixed(EMOJI_CELL_SIZE))
        // 6 px padding centres the 24 px artwork inside the 36 px hit area.
        .padding(SPACE_6)
        .style(emoji_cell_style)
        .into()
}

/// Boru-consistent hover/pressed feedback for emoji cells: a subtle
/// `surface_hover` tint on hover, darker `surface_pressed` on press,
/// rounded corners. Mirrors `BUTTON_GHOST_BG`, but keeps the text colour
/// at the primary text colour so a fallback Unicode emoji stays readable
/// in both light and dark themes (SVG artwork ignores the text colour).
fn emoji_cell_style(
    theme: &iced::Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    use iced::widget::button::{Status, Style};
    let background = match status {
        Status::Hovered => Some(iced::Background::Color(
            crate::design_tokens::surface_hover(theme),
        )),
        Status::Pressed => Some(iced::Background::Color(
            crate::design_tokens::surface_pressed(theme),
        )),
        _ => None,
    };
    Style {
        background,
        text_color: crate::design_tokens::text_primary(theme),
        border: iced::Border {
            radius: SPACE_4.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emoji::catalog::{common_emojis, Emoji, EmojiCategory};
    use crate::emoji::renderer::{EmojiRenderer, TwemojiRenderer};

    /// Every curated common emoji must render as Twemoji SVG — none may
    /// fall back to a system-font glyph (the missing-glyph/tofu problem
    /// this task removes).
    #[test]
    fn every_common_emoji_renders_svg() {
        let renderer = TwemojiRenderer;
        for emoji in common_emojis() {
            let artwork = cell_artwork(&renderer, emoji);
            assert!(
                matches!(artwork, CellArtwork::Svg(_)),
                "{} (asset {}) must render as SVG, not fall back",
                emoji.unicode,
                emoji.asset
            );
        }
    }

    /// Unsupported/newer emoji (not vendored) fall back to their original
    /// Unicode text — never an empty or broken cell (BORU-TWEMOJI-20).
    #[test]
    fn unsupported_emoji_falls_back_to_text() {
        let renderer = TwemojiRenderer;
        // 🫩 face with bags under eyes — Unicode 16.0, not vendored.
        let unknown = Emoji {
            unicode: "🫩",
            name: "face with bags under eyes",
            category: EmojiCategory::SmileysAndPeople,
            keywords: &[],
            asset: "zzzz-not-vendored",
        };
        assert!(matches!(
            cell_artwork(&renderer, &unknown),
            CellArtwork::Text
        ));
    }

    /// A missing vendored SVG file for a resolvable key also falls back to
    /// text instead of a broken image.
    #[test]
    fn unreadable_asset_falls_back_to_text() {
        // A well-formed key whose vendored file does not exist: `svg_handle`
        // returns None and the cell must fall back. The key is unique to
        // this test so it cannot poison the process-global cache entry for a
        // real asset (other tests run in parallel).
        let broken = crate::emoji::renderer::EmojiAsset {
            key: "zzzz-missing-file-test",
            path: std::path::PathBuf::from("assets/emoji/twemoji/svg/definitely-missing.svg"),
        };
        // EmojiRenderer::svg_handle for a missing file returns None.
        let renderer = TwemojiRenderer;
        assert!(renderer.svg_handle(&broken).is_none());
    }

    /// Insertion always carries the full Unicode grapheme string, never the
    /// asset key or an SVG path — the composer must keep receiving plain
    /// Unicode (PDF guardrail: presentation layer only).
    #[test]
    fn insert_message_carries_unicode_never_asset() {
        for emoji in common_emojis() {
            match insert_message(emoji) {
                AppMessage::InsertEmoji(s) => {
                    assert_eq!(s, emoji.unicode, "must insert the Unicode grapheme");
                    assert_ne!(s, emoji.asset, "must never insert the asset key");
                    assert!(
                        !s.contains(".svg") && !s.contains('/'),
                        "must never insert an SVG path: {s:?}"
                    );
                }
                other => panic!("unexpected message: {other:?}"),
            }
        }
    }

    /// Multi-codepoint graphemes survive insertion as a single string (a
    /// single `char` would split ❤️/⚠️/👍🏽 into their first code point).
    #[test]
    fn insert_message_keeps_multicodepoint_graphemes() {
        let heart = Emoji {
            unicode: "❤️", // U+2764 U+FE0F
            name: "red heart",
            category: EmojiCategory::Symbols,
            keywords: &[],
            asset: "2764",
        };
        match insert_message(&heart) {
            AppMessage::InsertEmoji(s) => assert_eq!(s, "❤️"),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    // ── Responsive geometry (BORU-TWEMOJI-11) ───────────────────────────

    /// The reference token width (336 px) shows the reference 8-column grid.
    #[test]
    fn reference_width_shows_reference_columns() {
        assert_eq!(picker_columns(336.0), 8);
    }

    /// Wide windows get more columns, capped at 9 (card ≈ 374 px ≤ 400).
    #[test]
    fn wide_windows_show_more_columns_capped() {
        assert_eq!(picker_columns(400.0), 9);
        assert_eq!(picker_columns(500.0), 9);
        assert_eq!(picker_columns(1920.0), 9);
        assert_eq!(picker_card_width(9, 1920.0), 374.0);
        assert!(picker_card_width(9, 1920.0) <= 400.0);
    }

    /// Narrow windows show fewer columns and the card never exceeds the
    /// available width (no clipping).
    #[test]
    fn narrow_windows_show_fewer_columns_without_clipping() {
        assert_eq!(picker_columns(250.0), 5);
        assert_eq!(picker_columns(200.0), 4);
        assert_eq!(picker_columns(120.0), 2);
        assert_eq!(picker_columns(60.0), 1);

        for width in [
            60.0_f32, 100.0, 150.0, 200.0, 250.0, 336.0, 400.0, 800.0, 1920.0,
        ] {
            let columns = picker_columns(width);
            let card = picker_card_width(columns, width);
            assert!(
                card <= width,
                "card {card} must never exceed available {width} at {columns} cols"
            );
        }
    }

    /// Cells are never stretched: the card width is exactly the natural grid
    /// width plus chrome, for every supported column count.
    #[test]
    fn card_width_is_natural_grid_width_never_stretched() {
        for columns in 1..=EMOJI_MAX_COLUMNS {
            let natural = columns as f32 * EMOJI_CELL_SIZE
                + (columns as f32 - 1.0) * EMOJI_CELL_GAP
                + PICKER_CHROME_X;
            // With plenty of room the card must equal the natural width.
            assert_eq!(picker_card_width(columns, 1000.0), natural);
            // The cell artwork size is never scaled by the layout.
            assert_eq!(EMOJI_ART_SIZE, 24.0);
            assert_eq!(EMOJI_CELL_SIZE, 36.0);
        }
    }

    /// Scroll height: grows with the active category's grid content up to
    /// the cap, respects the token as a floor, and never exceeds what fits
    /// the window (after the category tab row chrome).
    #[test]
    fn scroll_height_grows_with_content_and_respects_window() {
        // 9 columns → Smileys (23 entries) → 3 rows → 116 px content;
        // token floor 200.
        assert_eq!(picker_scroll_height(9, 800.0, 200.0, 23), 200.0);
        // 4 columns → all 61 entries → 16 rows → 636 px content, capped at
        // 340 (card ≤ ~400).
        assert_eq!(picker_scroll_height(4, 800.0, 200.0, 61), 340.0);
        // Short window: shrink to fit, never clip (tab row chrome included).
        let fits = picker_scroll_height(9, 240.0, 200.0, 23);
        assert!(fits <= 240.0 - PICKER_CHROME_Y - CATEGORY_ROW_CHROME);
        // Token floor applies when the window is tall enough.
        assert_eq!(picker_scroll_height(9, 1000.0, 200.0, 23), 200.0);
    }

    /// Invariant sweep: for any plausible available size the card width is
    /// within the available width and the card height (chrome + tab row +
    /// scroll) within the available height — nothing clips at any window
    /// size, narrow or wide.
    #[test]
    fn responsive_invariants_hold_across_window_sizes() {
        let all = crate::emoji::catalog::common_emojis().len();
        for width in (60..=1920).step_by(40) {
            let columns = picker_columns(width as f32);
            let card = picker_card_width_with_category_row(columns, width as f32);
            assert!(card <= width as f32, "width overflow at {width}");
            assert!(columns >= 1 && columns <= EMOJI_MAX_COLUMNS);
        }
        for height in (120..=1200).step_by(60) {
            for columns in 1..=EMOJI_MAX_COLUMNS {
                let scroll = picker_scroll_height(columns, height as f32, 200.0, all);
                assert!(
                    scroll + PICKER_CHROME_Y + CATEGORY_ROW_CHROME <= height as f32 + 0.001,
                    "height overflow at {height} cols {columns}: {scroll}"
                );
                assert!(scroll <= PICKER_MAX_SCROLL + 0.001);
            }
        }
    }

    // ── Category navigation (BORU-TWEMOJI-12) ─────────────────────

    /// Every category icon renders as Twemoji SVG through the shared
    /// renderer — the tab row never shows a system-font glyph.
    #[test]
    fn every_category_icon_renders_svg() {
        let renderer = TwemojiRenderer;
        for category in EmojiCategory::ALL {
            let icon = crate::emoji::catalog::category_icon(category)
                .unwrap_or_else(|| panic!("category {category:?} needs an icon"));
            assert!(
                matches!(cell_artwork(&renderer, icon), CellArtwork::Svg(_)),
                "category {category:?} icon must render as SVG"
            );
        }
    }

    /// The tab row's natural width fits inside a wide card (all 8 tabs
    /// visible) and exceeds a narrow card's usable width (row scrolls
    /// instead of clipping).
    #[test]
    fn category_row_fits_wide_and_scrolls_narrow() {
        assert_eq!(EmojiCategory::ALL.len(), 8);
        assert_eq!(category_row_natural_width(), 8.0 * 32.0 + 7.0 * 4.0);
        // Wide card (9 columns → 374 px): usable body width 356 ≥ 284.
        let wide = picker_card_width_with_category_row(9, 1920.0);
        assert_eq!(wide, 374.0);
        assert!(wide - PICKER_CHROME_X >= category_row_natural_width());
        // Narrow card (60 px window → card 60 px): usable width 42 < 284,
        // so the row must scroll.
        let narrow = picker_card_width_with_category_row(1, 60.0);
        assert_eq!(narrow, 60.0);
        assert!(narrow - PICKER_CHROME_X < category_row_natural_width());
    }

    /// The card is at least wide enough for the whole tab row plus chrome
    /// when space permits, and never exceeds the available width.
    #[test]
    fn card_width_accommodates_category_row() {
        for width in [250.0_f32, 336.0, 400.0, 800.0, 1920.0] {
            let columns = picker_columns(width);
            let card = picker_card_width_with_category_row(columns, width);
            assert!(card <= width, "card {card} exceeds available {width}");
            if width >= category_row_natural_width() + PICKER_CHROME_X {
                assert!(
                    card >= category_row_natural_width() + PICKER_CHROME_X,
                    "card {card} must fit the full tab row at width {width}"
                );
            }
        }
    }

    /// Acceptance: switching categories changes the grid's entry set and
    /// never leaves stale items — the two sets are disjoint, and the union
    /// equals the full catalog.
    #[test]
    fn grid_entries_change_with_category_selection_without_stale_items() {
        let smileys: Vec<_> =
            crate::emoji::catalog::emojis_for_category(EmojiCategory::SmileysAndPeople).collect();
        let flags: Vec<_> =
            crate::emoji::catalog::emojis_for_category(EmojiCategory::Flags).collect();
        assert!(!smileys.is_empty() && !flags.is_empty());
        // Different selections show different entries.
        assert!(
            smileys
                .iter()
                .any(|e| !flags.iter().any(|f| f.unicode == e.unicode)),
            "smileys and flags must differ"
        );
        // No overlap: a switch can never keep a stale item on screen.
        for s in &smileys {
            assert!(
                !flags.iter().any(|f| f.unicode == s.unicode),
                "stale item {} leaked across categories",
                s.unicode
            );
        }
        // Every catalog entry appears in exactly one category grid.
        let all: Vec<_> = crate::emoji::catalog::common_emojis().iter().collect();
        let union: Vec<_> = EmojiCategory::ALL
            .iter()
            .flat_map(|c| crate::emoji::catalog::emojis_for_category(*c))
            .collect();
        assert_eq!(union.len(), all.len());
    }
}
