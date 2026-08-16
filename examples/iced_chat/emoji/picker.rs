//! Emoji picker panel (BORU-TWEMOJI-04).
//!
//! The picker moved here from `app/chat.rs` (`IcedChat::view_emoji_picker`).
//! External behavior is unchanged: a `iced_aw::Card` hosting the curated
//! emoji grid as text-glyph buttons; selecting an emoji emits
//! [`crate::app::AppMessage::InsertEmoji`] into the composer. The visual
//! swap from system-font glyphs to Twemoji SVG cells happens in
//! BORU-TWEMOJI-10, through the shared renderer in [`super::renderer`].
//!
//! The emoji list is sourced from [`super::catalog::common_emojis`] so the
//! picker and the future message renderer share one metadata source.

use crate::app::{bg_surface, border_muted, text_muted, AppMessage};
use crate::design_tokens::{SPACE_2, SPACE_4, SPACE_8};

/// Render the emoji picker panel (280×160 Card, 40 curated emojis in an
/// 8-per-row grid).
///
/// The original method took `&self` to read the active theme; as a standalone
/// module function it takes the theme explicitly. The chat layer passes
/// `&self.theme()`.
pub fn view_emoji_picker(theme: &iced::Theme) -> iced::Element<'static, AppMessage> {
    use iced::widget::{button, column, row, text};

    let btheme = crate::theme::BoruTheme::for_theme(theme);

    let head = crate::fonts::type_role_text(
        crate::fonts::TypeRole::CardTitle,
        crate::i18n::t("emoji.title"),
    )
    .color(text_muted(theme));

    let mut grid = column![].spacing(SPACE_2);
    for chunk in crate::emoji::catalog::common_emojis().chunks(8) {
        let mut r = row![].spacing(SPACE_2);
        for emoji in chunk {
            // BORU-TWEMOJI-07: replace `chars().next()` with the full Unicode
            // grapheme so variation selectors (❤️/⚠️) survive insertion.
            let c = emoji.unicode.chars().next().unwrap();
            r = r.push(
                button(text(emoji.unicode).size(20.0))
                    .on_press(AppMessage::InsertEmoji(c))
                    .padding([SPACE_2, SPACE_4])
                    .style(|_t, _s| iced::widget::button::Style::default()),
            );
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
