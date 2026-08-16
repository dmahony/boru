//! Emoji picker panel (BORU-TWEMOJI-04; SVG rendering BORU-TWEMOJI-10).
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
//! The emoji list is sourced from [`super::catalog::common_emojis`] so the
//! picker and the future message renderer share one metadata source.

use crate::app::{bg_surface, border_muted, text_muted, AppMessage};
use crate::design_tokens::{SPACE_4, SPACE_6, SPACE_8};

/// Visual emoji artwork size (PDF Task 10: ~24 px).
const EMOJI_ART_SIZE: f32 = 24.0;
/// Button hit area (PDF Task 10: ~36 x 36 px).
const EMOJI_CELL_SIZE: f32 = 36.0;
/// Columns in the picker grid (matches the pre-SVG 8-per-row layout).
const EMOJI_COLUMNS: usize = 8;

/// Render the emoji picker panel (a Card hosting the curated emoji grid as
/// SVG buttons; selecting an emoji inserts its Unicode into the composer).
pub fn view_emoji_picker(theme: &iced::Theme) -> iced::Element<'static, AppMessage> {
    use iced::widget::{column, row};

    let btheme = crate::theme::BoruTheme::for_theme(theme);

    let head = crate::fonts::type_role_text(
        crate::fonts::TypeRole::CardTitle,
        crate::i18n::t("emoji.title"),
    )
    .color(text_muted(theme));

    let renderer = crate::emoji::renderer::TwemojiRenderer;

    let mut grid = column![].spacing(SPACE_4);
    for chunk in crate::emoji::catalog::common_emojis().chunks(EMOJI_COLUMNS) {
        let mut r = row![].spacing(SPACE_4);
        for emoji in chunk {
            r = r.push(emoji_cell(&renderer, emoji, theme));
        }
        grid = grid.push(r);
    }

    let scroll = crate::ui_components::gutter_scrollable(grid)
        .height(iced::Length::Fixed(btheme.chat.emoji_picker_scroll_height));

    let chat_theme = btheme.chat;
    iced_aw::Card::new(head, scroll)
        .width(chat_theme.emoji_picker_width)
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
    theme: &iced::Theme,
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
}
